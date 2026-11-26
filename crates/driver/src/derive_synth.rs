//! `@derive(X)` forwarding-impl synthesis for library-registered derivable
//! traits — E3b (#1291, epic #1289).
//!
//! A library marks a generic generator with `@derivable(Trait)` (E3a, #1290) and
//! declares a structural `interface Trait` with the method(s) the trait
//! requires. `@derive(Trait)` on a type then makes the type conform by gaining,
//! for each interface method, a **thin forwarding stub** that calls the
//! registered generator generic specialized at the type (HQ7: all logic lives in
//! the library generic; the stub only forwards). The generator's
//! `comptime for … reflect.type_info(T) …` body unrolls per instantiation via
//! monomorphization's type-parameter resolution (the E3b-part-1 mechanism).
//!
//! This is a **pre-sema** AST desugar (run from the driver front end alongside
//! the other decorator desugars), so the synthesized method is type-checked,
//! conformance-checked, monomorphized and lowered like any hand-written method.
//! It scans `@derivable`/`interface` declarations itself (sema's registry is not
//! built yet at this point).
//!
//! One stub is synthesized per interface method, each forwarding to the trait's
//! registered generator; a multi-method trait therefore forwards every method to
//! that one generator, so distinct per-method generators are a follow-up. The
//! `@derive` is stripped of the library trait once synthesized so the lowering
//! backstop no longer reports it as deferred (built-in traits and any
//! un-synthesized name are kept). A hand-written method of the same name wins (no
//! stub synthesized).
//!
//! ## Generic types (E3c, #1292 — inferred bounds, structural)
//!
//! A **generic** type derives too: the stub forwards to the generator specialized
//! at the type applied to its own parameters — `generator[Pair[A, B]](self)` on
//! `struct Pair[A, B]`. Conformance and recursion termination are **structural**,
//! per instantiation: monomorphization specializes the generator at each concrete
//! `Pair[int64, str]` and its per-field recursion is checked by ordinary
//! structural resolution, while the `SpecializationCache` makes a recursive type
//! (`struct Node[T]: T val; Optional[Node[T]] next`) terminate — the merged-in
//! instance stands in for the in-progress goal ("assume-the-goal").
//!
//! The spec's `impl[T_i: X]` framing does not map onto Zap: there is no `impl`
//! construct, and a method's own type parameter *shadows* the enclosing struct's
//! as a fresh **unbounded** variable (sema's `method_own_generic_shadows_enclosing_bound`),
//! so a literal `[A: X]` cannot be attached to the synthesized method without
//! breaking `self`'s type. The inferred bounds are instead realized structurally
//! (above) and surfaced as data: [`expand_library_derives`] returns, per generic
//! derive, the set of parameters each non-skipped field mentions together with
//! those fields' spans ([`GenericDeriveBounds`]), for E3d (#1293) to render an
//! unsatisfied bound at the offending field. A `@skip` field attribute does not
//! exist yet, so every field counts for now (its exclusion is a follow-up).
//!
//! **Enums (#1373, epic #1371):** an `enum` carrying a library `@derive(X)` is
//! handled identically to a struct — the same thin forwarding stub, trait
//! stripping, and inferred bounds — with the bound fields taken from the
//! **variant payload fields** (a type parameter used in any variant's payload
//! counts). The generator *body* for an enum (runtime variant discrimination via
//! the Option-C `reflect.match` surface, decision #1372) is the codegen slice
//! #1374; this module only wires the stub so an enum `@derive` is recognized and
//! forwarded. The diagnostics rendering itself is E3d (#1293).

use std::collections::HashMap;

use zapc_diag::ErrorCode;
use zapc_sema::TypeError;
use zapc_syntax::ast::{
    Arg, ArgSpread, Block, Decorator, Expr, Function, Item, MethodSig, Module, Param, Stmt, Type,
    TypeParam,
};
use zapc_syntax::span::Span;

/// One inferred serde-style bound for a generic `@derive` (E3c, #1292): the type
/// parameter `param` is used by one or more non-skipped fields, so a concrete
/// instantiation conforms only if the type bound to `param` conforms to the
/// derived trait. `field_spans` are those fields' spans, so E3d (#1293) can land
/// an unsatisfied-bound diagnostic at the offending field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferredBound {
    pub param: String,
    pub field_spans: Vec<Span>,
}

/// The inferred bounds of one generic `@derive(Trait)` on one type (E3c, #1292).
/// `params` holds exactly the type parameters that appear in a non-skipped field,
/// in declaration order; a parameter used in no field (or only behind a concrete
/// container) is absent. Conformance itself is checked structurally per
/// instantiation — this is the data E3d renders, not a bound the synthesis emits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericDeriveBounds {
    /// The derived type's name — a struct or (since #1373) an enum.
    pub type_name: String,
    pub trait_name: String,
    pub params: Vec<InferredBound>,
}

/// Synthesize forwarding stubs for every `@derive(<library trait>)` on a type,
/// in place. Returns `(diagnostics, inferred-bounds)`: diagnostics for a library
/// trait with no interface, and — for each generic derive — the inferred serde
/// bounds (E3c, #1292) for E3d to consume (the current pipeline ignores them).
pub fn expand_library_derives(module: &mut Module) -> (Vec<TypeError>, Vec<GenericDeriveBounds>) {
    // The built-in prelude may itself register `@derivable` traits (#1596, the
    // seam the Json retire #1558 uses): a prelude-provided generator + interface
    // routes `@derive(Trait)` in any module without the user declaring them. The
    // prelude is injected into sema/lowering separately (`with_prelude`), after
    // this pre-sema pass, so its declarations are scanned here explicitly.
    let mut prelude = zapc_syntax::parser::prelude_items();
    prelude.extend(zapc_syntax::parser::json_derive_items());
    expand_library_derives_with(module, &prelude)
}

/// [`expand_library_derives`] with the prelude's declarations supplied
/// explicitly (`extra`), so the prelude-awareness (#1596) is unit-testable with a
/// synthetic prelude. A `@derivable`/`interface` in `extra` is registered just
/// like one in `module`; a declaration in `module` shadows a prelude one of the
/// same trait name (the user's own generator wins).
pub fn expand_library_derives_with(
    module: &mut Module,
    extra: &[Item],
) -> (Vec<TypeError>, Vec<GenericDeriveBounds>) {
    let mut diags = Vec::new();
    let mut bounds: Vec<GenericDeriveBounds> = Vec::new();

    // Pass 1: collect the library-registered derivable traits (`@derivable(T)`
    // on a free function) and the interfaces that supply their method sets. Both
    // are scanned from the AST directly — sema has not run yet. Prelude
    // declarations (`extra`) are scanned first so a same-named declaration in the
    // user module overrides them.
    let mut generators: HashMap<String, String> = HashMap::new(); // trait -> generator fn
    let mut interfaces: HashMap<String, Vec<MethodSig>> = HashMap::new();
    for item in extra.iter().chain(module.items.iter()) {
        match item {
            Item::Function(f) => {
                if let Some(trait_name) = derivable_trait(f) {
                    generators.insert(trait_name, f.name.clone());
                }
            }
            Item::Interface(i) => {
                interfaces.insert(i.name.clone(), i.methods.clone());
            }
            _ => {}
        }
    }
    if generators.is_empty() {
        return (diags, bounds); // no library derivables -> nothing to do
    }

    // Pass 2: for each struct AND enum (#1373), synthesize stubs for its
    // library-trait derives and strip those traits from its `@derive(...)`.
    for item in &mut module.items {
        match item {
            Item::Struct(s) => {
                let bound_fields: Vec<(&Type, Span)> =
                    s.fields.iter().map(|f| (&f.ty, f.span)).collect();
                synthesize_for_type(
                    &s.name,
                    &s.generics,
                    &mut s.decorators,
                    &mut s.methods,
                    &bound_fields,
                    s.span,
                    &generators,
                    &interfaces,
                    &mut diags,
                    &mut bounds,
                );
            }
            Item::Enum(e) => {
                // An enum's bound fields are its variant payload fields (#1373):
                // a type parameter used in any variant's payload contributes a
                // bound, just as a struct field does.
                let bound_fields: Vec<(&Type, Span)> = e
                    .variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|vf| (&vf.ty, vf.span)))
                    .collect();
                synthesize_for_type(
                    &e.name,
                    &e.generics,
                    &mut e.decorators,
                    &mut e.methods,
                    &bound_fields,
                    e.span,
                    &generators,
                    &interfaces,
                    &mut diags,
                    &mut bounds,
                );
            }
            _ => {}
        }
    }

    (diags, bounds)
}

/// Synthesize the forwarding stubs for one type's library-trait derives, strip
/// those traits, and record its inferred bounds — the shared body for a struct
/// and an enum (#1373). `bound_fields` are the `(type, span)` pairs scanned for
/// a generic parameter mention (struct fields, or an enum's variant payload
/// fields). `decorators`/`methods` are the type's own, mutated in place.
#[allow(clippy::too_many_arguments)]
fn synthesize_for_type(
    name: &str,
    generics: &[TypeParam],
    decorators: &mut Vec<Decorator>,
    methods: &mut Vec<Function>,
    bound_fields: &[(&Type, Span)],
    sp: Span,
    generators: &HashMap<String, String>,
    interfaces: &HashMap<String, Vec<MethodSig>>,
    diags: &mut Vec<TypeError>,
    bounds: &mut Vec<GenericDeriveBounds>,
) {
    // `@derive(Json)` is sugar for the two library codec traits (#1558):
    // `JsonEncode` (`to_json`) + `JsonDecode` (`from_json`), each a single-method
    // trait with its own prelude generator. Rewrite it before collecting, so the
    // retired built-in `Json` is served entirely by the prelude library
    // generators through the ordinary library-derive path.
    expand_json_derive_sugar(decorators);

    // Which library traits does this type derive?
    let derived: Vec<String> = decorators
        .iter()
        .filter(|d| d.name == "derive")
        .flat_map(|d| d.args.iter())
        .filter_map(|a| match &a.value {
            Expr::Name(n, _) if generators.contains_key(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    if derived.is_empty() {
        return;
    }

    // The type the generator is specialized at: the bare name for a non-generic
    // type, or the type applied to its own parameters for a generic one
    // (`Pair[A, B]`), so each concrete instantiation specializes the generator
    // structurally (E3c, #1292).
    let type_arg = self_type_arg_expr(name, generics, sp);
    let mut synthesized: Vec<String> = Vec::new();
    for trait_name in &derived {
        let generator = &generators[trait_name];
        // The method set to synthesize comes from a same-named `interface Trait`
        // the library declares (E3b). For a **ported built-in** trait (E4, #1433)
        // the name collides with a generic, reference-gated operator-overloading
        // interface (`Eq[R]`/`Ord[R]`), so there is no plain `interface Eq` to
        // read; the synthesis then supplies the dunder signature internally from
        // [`builtin_derive_method_sigs`]. This fallback is inert until a built-in
        // gains a `@derivable` generator (its port slice) — the current built-ins
        // are never in `generators`, so `derived` never names them here.
        let builtin_sigs = if interfaces.contains_key(trait_name) {
            None
        } else {
            builtin_derive_method_sigs(trait_name, sp)
        };
        let sigs: &[MethodSig] = match (interfaces.get(trait_name), &builtin_sigs) {
            (Some(s), _) => s,
            (None, Some(s)) => s,
            (None, None) => {
                diags.push(err(
                    format!(
                        "cannot derive `{trait_name}` for `{name}`: `{trait_name}` is registered \
                         with @derivable but has no `interface {trait_name}` declaring the \
                         method(s) to synthesize (E3b, #1291)"
                    ),
                    sp,
                ));
                continue;
            }
        };
        for m in sigs {
            // A hand-written method of the same name wins (HQ7): skip.
            if methods.iter().any(|existing| existing.name == m.name) {
                continue;
            }
            methods.push(forwarding_stub(m, generator, &type_arg, sp));
        }
        synthesized.push(trait_name.clone());
        // Record the inferred serde bounds for a generic derive (E3c, #1292):
        // which of the type's parameters appear in a bound field, and where. A
        // non-generic type has none.
        if !generics.is_empty() {
            let params = inferred_bound_params(generics, bound_fields);
            if !params.is_empty() {
                bounds.push(GenericDeriveBounds {
                    type_name: name.to_string(),
                    trait_name: trait_name.clone(),
                    params,
                });
            }
        }
    }

    // Strip the synthesized library traits from `@derive(...)` so the lowering
    // backstop no longer reports them as deferred; drop a `@derive` left empty.
    // Built-in traits (and any not synthesized) are kept.
    strip_derived(decorators, &synthesized);
}

/// The type the generator is specialized at for `s`: `Expr::Name(s.name)` for a
/// non-generic type, or `s.name[p0, p1, …]` (an index/subscript expression that
/// `expr_to_type` reads back as `Named { name, args }`) for a generic one — the
/// same shape the parser produces for `generator[Pair[A, B]](…)`.
/// Rewrite `@derive(Json)` to `@derive(JsonEncode, JsonDecode)` in place (#1558):
/// the built-in `Json` is now sugar for the two prelude library codec traits.
/// Each `Json` argument of every `@derive(...)` on the type is replaced by the
/// two trait-name arguments in order (`@derive(Eq, Json)` →
/// `@derive(Eq, JsonEncode, JsonDecode)`); any other argument is untouched. A
/// literal `JsonEncode`/`JsonDecode` is unaffected (idempotent).
fn expand_json_derive_sugar(decorators: &mut [Decorator]) {
    for d in decorators.iter_mut() {
        if d.name != "derive"
            || !d
                .args
                .iter()
                .any(|a| matches!(&a.value, Expr::Name(n, _) if n == "Json"))
        {
            continue;
        }
        let mut new_args: Vec<Arg> = Vec::with_capacity(d.args.len() + 1);
        for a in d.args.drain(..) {
            match &a.value {
                Expr::Name(n, sp) if n == "Json" => {
                    let sp = *sp;
                    for t in ["__JsonDeriveEncode", "__JsonDeriveDecode"] {
                        new_args.push(Arg {
                            name: None,
                            value: Expr::Name(t.to_string(), sp),
                            spread: ArgSpread::None,
                            span: sp,
                        });
                    }
                }
                _ => new_args.push(a),
            }
        }
        d.args = new_args;
    }
}

fn self_type_arg_expr(name: &str, generics: &[TypeParam], sp: Span) -> Expr {
    if generics.is_empty() {
        return Expr::Name(name.to_string(), sp);
    }
    let params: Vec<Expr> = generics
        .iter()
        .map(|g| Expr::Name(g.name.clone(), sp))
        .collect();
    // One parameter is a bare subscript (`Node[T]`); several form a tuple
    // (`Pair[A, B]`), matching `type_args_from_index` / `expr_to_type`.
    let index = if params.len() == 1 {
        params.into_iter().next().unwrap()
    } else {
        Expr::Tuple(params, sp)
    };
    Expr::Index {
        base: Box::new(Expr::Name(name.to_string(), sp)),
        index: Box::new(index),
        span: sp,
    }
}

/// The inferred serde bounds (E3c, #1292) of a generic type: each type parameter
/// that appears in a bound field's type, with that/those fields' spans, in
/// declaration order. `bound_fields` are the `(type, span)` pairs to scan — a
/// struct's fields, or (since #1373) an enum's variant payload fields. A
/// parameter used in no field (or only behind a concrete container such as
/// `list[int64]`) is omitted; a parameter mentioned by several fields collects
/// every such field's span. `@skip` does not exist yet, so every field counts.
fn inferred_bound_params(
    generics: &[TypeParam],
    bound_fields: &[(&Type, Span)],
) -> Vec<InferredBound> {
    let param_names: Vec<&str> = generics.iter().map(|g| g.name.as_str()).collect();
    generics
        .iter()
        .filter_map(|g| {
            let field_spans: Vec<Span> = bound_fields
                .iter()
                .filter(|(ty, _)| type_mentions_param(ty, &g.name, &param_names))
                .map(|(_, span)| *span)
                .collect();
            (!field_spans.is_empty()).then(|| InferredBound {
                param: g.name.clone(),
                field_spans,
            })
        })
        .collect()
}

/// Whether `ty` mentions the type parameter `param`. A `Named` whose own name is
/// `param` counts only when it is one of the type's parameters (`param_names`),
/// i.e. a bare `A`, not a same-named concrete type; its arguments and the members
/// of tuple/function types are searched recursively (so `list[A]`, `dict[A, A]`,
/// `(A, int64)`, `fn(A) -> B` are all caught).
fn type_mentions_param(ty: &Type, param: &str, param_names: &[&str]) -> bool {
    match ty {
        Type::Named { name, args, .. } => {
            (name == param && param_names.contains(&name.as_str()))
                || args
                    .iter()
                    .any(|a| type_mentions_param(a, param, param_names))
        }
        Type::Tuple { elems, .. } => elems
            .iter()
            .any(|e| type_mentions_param(e, param, param_names)),
        Type::Func { params, ret, .. } => {
            params
                .iter()
                .any(|p| type_mentions_param(p, param, param_names))
                || ret
                    .as_deref()
                    .is_some_and(|r| type_mentions_param(r, param, param_names))
        }
    }
}

/// The dunder method signature(s) a **ported built-in** derivable trait (E4,
/// #1429/#1433) synthesizes, supplied internally so the derive synthesis does
/// not need a user-authored `interface Trait` — whose name would collide with
/// the generic, reference-gated operator-overloading interfaces (`Eq[R]` /
/// `Ord[R]`) the operator prelude already declares. Returns `None` for any other
/// trait, so a library `@derivable(X)` with no `interface X` still gets the clear
/// "no interface" error. The signatures mirror the built-in codegen's
/// (`DerivedTrait::signatures`, D-0047): `Eq`→`__eq__(self, Self) -> bool`,
/// `Ord`→`__cmp__(self, Self) -> int64`, `Hash`→`__hash__(self) -> int64`,
/// `Repr`→`__repr__(self) -> str`. The codec traits (`Json`/`Csv`/`Yaml`) carry
/// associated functions and are supplied by their own port slices.
fn builtin_derive_method_sigs(trait_name: &str, sp: Span) -> Option<Vec<MethodSig>> {
    let named = |n: &str| Type::Named {
        name: n.to_string(),
        args: Vec::new(),
        span: sp,
    };
    let param = |name: &str, ty: Option<Type>| Param {
        decorators: Vec::new(),
        ty,
        name: name.to_string(),
        default: None,
        variadic: false,
        kwargs: false,
        keyword_only: false,
        positional_only: false,
        span: sp,
    };
    // `self` is the untyped receiver; `other` is the same type (`Self`), matching
    // the built-in `__eq__`/`__cmp__` two-argument shape.
    let self_param = || param("self", None);
    let other_param = || param("other", Some(named("Self")));
    let sig = |name: &str, params: Vec<Param>, ret: Type| MethodSig {
        name: name.to_string(),
        params,
        ret: Some(ret),
        span: sp,
    };
    let sigs = match trait_name {
        "Eq" => vec![sig(
            "__eq__",
            vec![self_param(), other_param()],
            named("bool"),
        )],
        "Ord" => vec![sig(
            "__cmp__",
            vec![self_param(), other_param()],
            named("int64"),
        )],
        "Hash" => vec![sig("__hash__", vec![self_param()], named("int64"))],
        "Repr" => vec![sig("__repr__", vec![self_param()], named("str"))],
        _ => return None,
    };
    Some(sigs)
}

/// Build the thin forwarding stub for one interface method `m`, forwarding every
/// parameter (including `self`) positionally to `generator` specialized at
/// `type_arg` — i.e. `generator[Type](self, …)`, where `type_arg` is the type as
/// an expression (`Point`, or `Pair[A, B]` for a generic type). The type argument
/// is explicit (I#161) rather than inferred: synthesized nodes share the
/// enclosing type's span, so sema's span-keyed type map cannot distinguish the
/// `self` argument's type for monomorphization to pin `T` from, and the type is
/// known here anyway.
fn forwarding_stub(m: &MethodSig, generator: &str, type_arg: &Expr, sp: Span) -> Function {
    let args: Vec<Arg> = m
        .params
        .iter()
        .map(|p| Arg {
            name: None,
            value: Expr::Name(p.name.clone(), sp),
            spread: ArgSpread::None,
            span: sp,
        })
        .collect();
    // `generator[type_arg]` — an explicit type-argument subscript on the callee
    // (mono's `f[args](...)` form, read back through `expr_to_type`).
    let callee = Expr::Index {
        base: Box::new(Expr::Name(generator.to_string(), sp)),
        index: Box::new(type_arg.clone()),
        span: sp,
    };
    let call = Expr::Call {
        callee: Box::new(callee),
        args,
        span: sp,
    };
    let stmts = if m.ret.is_some() {
        vec![Stmt::Return {
            value: Some(call),
            span: sp,
        }]
    } else {
        vec![Stmt::Expr(call)]
    };
    Function {
        decorators: Vec::new(),
        name: m.name.clone(),
        generics: Vec::new(),
        params: m.params.clone(),
        ret: m.ret.clone(),
        body: Block { stmts, span: sp },
        span: sp,
    }
}

/// The trait a free function registers via `@derivable(Trait)` (one positional
/// identifier), or `None`.
fn derivable_trait(f: &Function) -> Option<String> {
    let d = f.decorators.iter().find(|d| d.name == "derivable")?;
    match d.args.first().map(|a| &a.value) {
        Some(Expr::Name(t, _)) if d.args.len() == 1 => Some(t.clone()),
        _ => None,
    }
}

/// Remove `synthesized` trait names from a type's `@derive(...)` decorators,
/// dropping any `@derive` left with no arguments. Operates on the decorator list
/// directly, so it serves both a struct and an enum (#1373).
fn strip_derived(decorators: &mut Vec<Decorator>, synthesized: &[String]) {
    for d in decorators.iter_mut() {
        if d.name == "derive" {
            d.args.retain(|a| match &a.value {
                Expr::Name(n, _) => !synthesized.contains(n),
                _ => true,
            });
        }
    }
    decorators.retain(|d| d.name != "derive" || !d.args.is_empty());
}

fn err(message: String, span: Span) -> TypeError {
    TypeError {
        code: ErrorCode::ZAP5035,
        message,
        span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zapc_syntax::ast::{Item, StructDef};
    use zapc_syntax::parser::parse_str;

    fn module(src: &str) -> Module {
        let (m, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        m
    }

    fn get_struct<'a>(m: &'a Module, name: &str) -> &'a StructDef {
        m.items
            .iter()
            .find_map(|it| match it {
                Item::Struct(s) if s.name == name => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no struct `{name}`"))
    }

    fn method<'a>(s: &'a StructDef, name: &str) -> Option<&'a Function> {
        s.methods.iter().find(|f| f.name == name)
    }

    /// The names a struct still carries in its `@derive(...)` after the pass.
    fn derive_args(s: &StructDef) -> Vec<String> {
        s.decorators
            .iter()
            .filter(|d| d.name == "derive")
            .flat_map(|d| d.args.iter())
            .filter_map(|a| match &a.value {
                Expr::Name(n, _) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    // A minimal library: a `@derivable(Dump)` generic generator + the structural
    // `interface Dump` declaring the single method the trait requires.
    const LIB: &str = "\
@derivable(Dump)
def _derive_dump[T](T self):
    pass
interface Dump:
    def dump(self)
";

    #[test]
    fn synthesizes_forwarding_stub_and_strips_the_library_trait() {
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Point:\n    int64 x\n    int64 y\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        let point = get_struct(&m, "Point");
        // The stub exists, is non-generic, carries no decorators, and has the
        // interface method's signature (untyped `self`, no return).
        let dump = method(point, "dump").expect("synthesized `dump` stub");
        assert!(dump.generics.is_empty());
        assert!(dump.decorators.is_empty());
        assert_eq!(dump.params.len(), 1, "self only");
        assert_eq!(dump.params[0].name, "self");
        assert!(dump.params[0].ty.is_none(), "untyped self");
        assert!(dump.ret.is_none());

        // Body forwards every param positionally to the registered generator.
        assert_eq!(dump.body.stmts.len(), 1);
        let Stmt::Expr(Expr::Call { callee, args, .. }) = &dump.body.stmts[0] else {
            panic!("expected a forwarding call, got {:?}", dump.body.stmts[0]);
        };
        // `_derive_dump[Point](self)`: explicit type argument on the callee.
        let Expr::Index { base, index, .. } = &**callee else {
            panic!("expected `gen[Type](...)`, got {callee:?}");
        };
        assert!(matches!(&**base, Expr::Name(n, _) if n == "_derive_dump"));
        assert!(matches!(&**index, Expr::Name(n, _) if n == "Point"));
        assert_eq!(args.len(), 1);
        assert!(matches!(&args[0].value, Expr::Name(n, _) if n == "self"));
        assert!(matches!(args[0].spread, ArgSpread::None));

        // The library trait is stripped; the now-empty `@derive` is removed.
        assert!(
            derive_args(point).is_empty(),
            "library trait stripped: {:?}",
            derive_args(point)
        );
        assert!(
            !point.decorators.iter().any(|d| d.name == "derive"),
            "empty @derive removed"
        );
    }

    #[test]
    fn a_prelude_provided_derivable_routes_a_derive_with_no_in_file_declaration() {
        // #1596: the `@derivable(Dump)` + `interface Dump` live in the prelude
        // (here the synthetic `extra`), NOT the user module — the shape the Json
        // retire (#1558) needs. `@derive(Dump)` must still synthesize the
        // forwarding stub and strip the trait, exactly as when they are in-file.
        let extra = module(LIB).items;
        let mut m = module("@derive(Dump)\nstruct Point:\n    int64 x\n");
        let (diags, _bounds) = expand_library_derives_with(&mut m, &extra);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        let point = get_struct(&m, "Point");
        let dump = method(point, "dump").expect("stub synthesized from a prelude derivable");
        // Forwards to the prelude-registered generator by name.
        let Stmt::Expr(Expr::Call { callee, .. }) = &dump.body.stmts[0] else {
            panic!("expected a forwarding call, got {:?}", dump.body.stmts[0]);
        };
        let Expr::Index { base, .. } = &**callee else {
            panic!("expected `gen[Type](...)`, got {callee:?}");
        };
        assert!(matches!(&**base, Expr::Name(n, _) if n == "_derive_dump"));
        assert!(
            derive_args(point).is_empty(),
            "the prelude-registered library trait is stripped"
        );
    }

    #[test]
    fn a_user_declaration_overrides_a_same_named_prelude_derivable() {
        // A user `@derivable(Dump)` of the same name as a prelude one wins (the
        // registry scans the prelude first, then the user module). Both name the
        // same generator here, so routing still succeeds; the point is that a
        // same-name user declaration does not break registration.
        let extra = module(LIB).items;
        let mut m = module(&format!("{LIB}@derive(Dump)\nstruct P:\n    int64 x\n"));
        let (diags, _bounds) = expand_library_derives_with(&mut m, &extra);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let p = get_struct(&m, "P");
        assert!(
            method(p, "dump").is_some(),
            "routed with both declarations present"
        );
        assert!(derive_args(p).is_empty(), "trait stripped");
    }

    #[test]
    fn synthesized_program_type_checks_clean() {
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Point:\n    int64 x\n    int64 y\ndef main():\n    auto p = Point(1, 2)\n    p.dump()\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "synth diagnostics: {diags:?}");
        // The synthesized stub, its generic forwarding target and the `p.dump()`
        // call site all type-check like hand-written code.
        let (errs, _warns, _types) = zapc_sema::analyze_types(&m);
        assert!(
            errs.is_empty(),
            "sema errors on synthesized program: {errs:?}"
        );
    }

    #[test]
    fn a_hand_written_method_of_the_same_name_wins() {
        // HQ7: the user's own `dump` is kept; no forwarding stub is synthesized
        // over it, and the trait is still stripped (it is satisfied by the user
        // method).
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Point:\n    int64 x\n    def dump(self):\n        print(\"custom\")\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        let point = get_struct(&m, "Point");
        let dumps: Vec<_> = point.methods.iter().filter(|f| f.name == "dump").collect();
        assert_eq!(dumps.len(), 1, "exactly one `dump` (the user's)");
        // It is the user's: a `print(...)` body, not a forwarding call.
        assert!(matches!(
            &dumps[0].body.stmts[0],
            Stmt::Expr(Expr::Call { callee, .. }) if matches!(&**callee, Expr::Name(n, _) if n == "print")
        ));
        assert!(derive_args(point).is_empty(), "trait still stripped");
    }

    #[test]
    fn a_library_trait_without_an_interface_is_an_error() {
        // Registered via @derivable but no `interface Dump` to supply methods.
        let mut m = module(
            "@derivable(Dump)\ndef _derive_dump[T](T self):\n    pass\n@derive(Dump)\nstruct Point:\n    int64 x\n",
        );
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert_eq!(diags.len(), 1, "one diagnostic");
        assert_eq!(diags[0].code, ErrorCode::ZAP5035);
        assert!(
            diags[0].message.contains("interface Dump"),
            "message names the missing interface: {}",
            diags[0].message
        );
    }

    #[test]
    fn a_builtin_derive_is_left_untouched() {
        // `Eq` is not a library `@derivable`, so the pass neither synthesizes a
        // stub nor strips it — the existing built-in derive path still owns it.
        let mut m = module(&format!(
            "{LIB}@derive(Dump, Eq)\nstruct Point:\n    int64 x\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        let point = get_struct(&m, "Point");
        assert!(method(point, "dump").is_some(), "library stub synthesized");
        // Only the library trait is stripped; `Eq` survives for the built-in path.
        assert_eq!(derive_args(point), vec!["Eq".to_string()]);
    }

    // --- E3c (#1292): generic types derive, with inferred structural bounds ---

    #[test]
    fn a_generic_struct_is_synthesized_with_a_self_applied_type_arg() {
        // A generic type derives: the stub forwards to `_derive_dump[Pair[A, B]]`
        // (the type applied to its own parameters), and the trait is stripped.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Pair[A, B]:\n    A first\n    B second\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        let pair = get_struct(&m, "Pair");
        let dump = method(pair, "dump").expect("stub synthesized on a generic struct");
        let Stmt::Expr(Expr::Call { callee, .. }) = &dump.body.stmts[0] else {
            panic!("expected a forwarding call, got {:?}", dump.body.stmts[0]);
        };
        // callee is `_derive_dump[Pair[A, B]]` — the type arg is `Pair` indexed by
        // the two parameters (a tuple).
        let Expr::Index { base, index, .. } = &**callee else {
            panic!("expected `gen[Type]`, got {callee:?}");
        };
        assert!(matches!(&**base, Expr::Name(n, _) if n == "_derive_dump"));
        let Expr::Index {
            base: ta_base,
            index: ta_index,
            ..
        } = &**index
        else {
            panic!("expected `Pair[...]` type arg, got {index:?}");
        };
        assert!(matches!(&**ta_base, Expr::Name(n, _) if n == "Pair"));
        let Expr::Tuple(elems, _) = &**ta_index else {
            panic!("expected a tuple of params, got {ta_index:?}");
        };
        assert!(matches!(&elems[0], Expr::Name(n, _) if n == "A"));
        assert!(matches!(&elems[1], Expr::Name(n, _) if n == "B"));
        assert!(derive_args(pair).is_empty(), "trait stripped");
    }

    #[test]
    fn a_single_param_generic_uses_a_bare_subscript_type_arg() {
        // One parameter → `Node[T]` (bare subscript, not a one-tuple).
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Box[T]:\n    T value\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let dump = method(get_struct(&m, "Box"), "dump").expect("stub on generic struct");
        let Stmt::Expr(Expr::Call { callee, .. }) = &dump.body.stmts[0] else {
            panic!("expected a call");
        };
        let Expr::Index { index, .. } = &**callee else {
            panic!("expected gen[Type]");
        };
        let Expr::Index {
            index: ta_index, ..
        } = &**index
        else {
            panic!("expected Box[...]");
        };
        assert!(
            matches!(&**ta_index, Expr::Name(n, _) if n == "T"),
            "single param is a bare subscript, not a tuple: {ta_index:?}"
        );
    }

    #[test]
    fn inferred_bounds_cover_exactly_the_params_used_in_fields() {
        // `Pair[A, B]` — both params appear in a field → both bounded, in order.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Pair[A, B]:\n    A first\n    B second\n"
        ));
        let (_diags, bounds) = expand_library_derives(&mut m);
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].type_name, "Pair");
        assert_eq!(bounds[0].trait_name, "Dump");
        let params: Vec<&str> = bounds[0].params.iter().map(|b| b.param.as_str()).collect();
        assert_eq!(params, vec!["A", "B"]);
        assert!(
            bounds[0].params.iter().all(|b| b.field_spans.len() == 1),
            "each param used by exactly one field here"
        );
    }

    #[test]
    fn a_param_used_in_no_field_contributes_no_bound() {
        // `B` appears in no field → absent from the inferred bounds (only `A`).
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Phantom[A, B]:\n    A used\n    int64 tag\n"
        ));
        let (_diags, bounds) = expand_library_derives(&mut m);
        let params: Vec<&str> = bounds[0].params.iter().map(|b| b.param.as_str()).collect();
        assert_eq!(
            params,
            vec!["A"],
            "only the param used in a field is bounded"
        );
    }

    #[test]
    fn a_param_mentioned_twice_in_one_field_yields_one_bound() {
        // `dict[A, A]` mentions `A` twice but yields a single `A` bound with that
        // field's span once.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Keyed[A]:\n    dict[A, A] m\n"
        ));
        let (_diags, bounds) = expand_library_derives(&mut m);
        assert_eq!(bounds[0].params.len(), 1);
        assert_eq!(bounds[0].params[0].param, "A");
        assert_eq!(
            bounds[0].params[0].field_spans.len(),
            1,
            "one field → one span, not one per mention"
        );
    }

    #[test]
    fn a_param_only_behind_a_concrete_container_contributes_no_bound() {
        // `list[int64]` mentions no parameter → no spurious bound; the struct has
        // a generic param `T` used nowhere, so the inferred set is empty and no
        // record is emitted.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Concrete[T]:\n    list[int64] xs\n"
        ));
        let (_diags, bounds) = expand_library_derives(&mut m);
        assert!(
            bounds.is_empty(),
            "no param appears in a field → no bounds record: {bounds:?}"
        );
    }

    #[test]
    fn a_param_used_by_several_fields_collects_each_span() {
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct TwoA[A]:\n    A first\n    A second\n"
        ));
        let (_diags, bounds) = expand_library_derives(&mut m);
        assert_eq!(bounds[0].params.len(), 1);
        assert_eq!(
            bounds[0].params[0].field_spans.len(),
            2,
            "both fields mention A"
        );
    }

    #[test]
    fn a_recursive_generic_type_is_synthesized_and_its_param_is_bounded() {
        // `Node[T]` with a `T val` and a self-referential `Optional[Node[T]] next`:
        // the stub forwards to `_derive_dump[Node[T]]`, and `T` is inferred (it is
        // mentioned by `val` directly and by `next` nested inside
        // `Optional[Node[T]]`). Termination of a *recursing* generator is mono's
        // SpecializationCache job (and needs the kind-switch #1253); this covers
        // the synthesis + field-scan shape.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Node[T]:\n    T val\n    Optional[Node[T]] next\n"
        ));
        let (diags, bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        assert!(
            method(get_struct(&m, "Node"), "dump").is_some(),
            "stub on a recursive generic struct"
        );
        assert_eq!(bounds[0].params.len(), 1);
        assert_eq!(bounds[0].params[0].param, "T");
        assert_eq!(
            bounds[0].params[0].field_spans.len(),
            2,
            "T is mentioned by `val` and (nested) by `next`"
        );
    }

    #[test]
    fn a_nongeneric_derive_produces_no_bounds_record() {
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Point:\n    int64 x\n    int64 y\n"
        ));
        let (diags, bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty());
        assert!(
            bounds.is_empty(),
            "non-generic derive has no inferred bounds"
        );
    }

    #[test]
    fn a_stub_carries_the_interface_methods_parameters_and_return() {
        // Beyond the parameterless `self` case: the stub mirrors the interface
        // method's full signature and, for a method with a return type, forwards
        // through a `return` (so the generator's result propagates).
        let lib = "\
@derivable(Show)
def _show[T](T self, int64 width) -> str:
    return \"\"
interface Show:
    def show(self, int64 width) -> str
";
        let mut m = module(&format!("{lib}@derive(Show)\nstruct Point:\n    int64 x\n"));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        let show = method(get_struct(&m, "Point"), "show").expect("show stub");
        assert_eq!(show.params.len(), 2, "self + width");
        assert_eq!(show.params[1].name, "width");
        assert!(show.ret.is_some(), "return type carried over");
        // A returning method forwards via `return gen(self, width)`.
        let Stmt::Return {
            value: Some(Expr::Call { args, .. }),
            ..
        } = &show.body.stmts[0]
        else {
            panic!("expected `return gen(...)`, got {:?}", show.body.stmts[0]);
        };
        assert_eq!(args.len(), 2, "self and width forwarded");
        assert!(matches!(&args[0].value, Expr::Name(n, _) if n == "self"));
        assert!(matches!(&args[1].value, Expr::Name(n, _) if n == "width"));
    }

    #[test]
    fn a_program_with_no_library_derivable_is_unchanged() {
        let mut m = module("struct Point:\n    int64 x\ndef main():\n    auto p = Point(1)\n");
        let before = m.items.len();
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty());
        assert_eq!(m.items.len(), before, "no items added or removed");
        assert!(get_struct(&m, "Point").methods.is_empty());
    }

    // --- #1373 (epic #1371): enums derive the same way structs do -----------

    fn get_enum<'a>(m: &'a Module, name: &str) -> &'a zapc_syntax::ast::EnumDef {
        m.items
            .iter()
            .find_map(|it| match it {
                Item::Enum(e) if e.name == name => Some(e),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no enum `{name}`"))
    }

    fn enum_method<'a>(e: &'a zapc_syntax::ast::EnumDef, name: &str) -> Option<&'a Function> {
        e.methods.iter().find(|f| f.name == name)
    }

    fn enum_derive_args(e: &zapc_syntax::ast::EnumDef) -> Vec<String> {
        e.decorators
            .iter()
            .filter(|d| d.name == "derive")
            .flat_map(|d| d.args.iter())
            .filter_map(|a| match &a.value {
                Expr::Name(n, _) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn synthesizes_a_forwarding_stub_for_a_payload_enum_and_strips_the_trait() {
        // A tier-3 payload enum derives the library trait: the forwarding stub is
        // spliced onto the enum's methods and the trait is stripped, exactly as
        // for a struct.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nenum Shape:\n    Circle(float64 r)\n    Rect(float64 w, float64 h)\n    Unit\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        let shape = get_enum(&m, "Shape");
        let dump = enum_method(shape, "dump").expect("synthesized `dump` stub on the enum");
        assert!(dump.generics.is_empty() && dump.decorators.is_empty());
        assert_eq!(dump.params.len(), 1, "self only");
        // Body forwards `_derive_dump[Shape](self)`.
        let Stmt::Expr(Expr::Call { callee, args, .. }) = &dump.body.stmts[0] else {
            panic!("expected a forwarding call, got {:?}", dump.body.stmts[0]);
        };
        let Expr::Index { base, index, .. } = &**callee else {
            panic!("expected `gen[Type](...)`, got {callee:?}");
        };
        assert!(matches!(&**base, Expr::Name(n, _) if n == "_derive_dump"));
        assert!(matches!(&**index, Expr::Name(n, _) if n == "Shape"));
        assert_eq!(args.len(), 1);
        assert!(matches!(&args[0].value, Expr::Name(n, _) if n == "self"));
        assert!(enum_derive_args(shape).is_empty(), "library trait stripped");
        assert!(
            !shape.decorators.iter().any(|d| d.name == "derive"),
            "empty @derive removed"
        );
    }

    #[test]
    fn a_tier1_constant_enum_synthesizes_a_stub() {
        // A no-payload (tier-1) enum still gets a forwarding stub.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nenum Color:\n    RED\n    GREEN\n    BLUE\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        assert!(
            enum_method(get_enum(&m, "Color"), "dump").is_some(),
            "stub synthesized for a constant enum"
        );
        assert!(enum_derive_args(get_enum(&m, "Color")).is_empty());
    }

    #[test]
    fn a_hand_written_enum_method_of_the_same_name_wins() {
        // HQ7: the user's own `dump` on the enum is kept; no stub over it; the
        // trait is still stripped.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nenum Color:\n    RED\n    GREEN\n    def dump(self):\n        print(\"custom\")\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let color = get_enum(&m, "Color");
        let dumps: Vec<_> = color.methods.iter().filter(|f| f.name == "dump").collect();
        assert_eq!(dumps.len(), 1, "exactly one `dump` (the user's)");
        assert!(matches!(
            &dumps[0].body.stmts[0],
            Stmt::Expr(Expr::Call { callee, .. }) if matches!(&**callee, Expr::Name(n, _) if n == "print")
        ));
        assert!(enum_derive_args(color).is_empty(), "trait still stripped");
    }

    #[test]
    fn a_builtin_derive_on_an_enum_is_left_untouched() {
        let mut m = module(&format!(
            "{LIB}@derive(Dump, Eq)\nenum Color:\n    RED\n    GREEN\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        let color = get_enum(&m, "Color");
        assert!(
            enum_method(color, "dump").is_some(),
            "library stub synthesized"
        );
        assert_eq!(
            enum_derive_args(color),
            vec!["Eq".to_string()],
            "only the library trait is stripped; the built-in survives"
        );
    }

    #[test]
    fn a_library_trait_without_an_interface_is_an_error_for_an_enum() {
        let mut m = module(
            "@derivable(Dump)\ndef _derive_dump[T](T self):\n    pass\n@derive(Dump)\nenum Color:\n    RED\n",
        );
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert_eq!(diags.len(), 1, "one diagnostic");
        assert_eq!(diags[0].code, ErrorCode::ZAP5035);
        assert!(
            diags[0].message.contains("`Color`") && diags[0].message.contains("interface Dump"),
            "names the enum and the missing interface: {}",
            diags[0].message
        );
    }

    #[test]
    fn a_generic_enum_infers_bounds_from_variant_payload_fields() {
        // `Tree[T]`: `T` is used in the `Leaf(T value)` payload, so it is bounded;
        // the self-applied type arg is `Tree[T]`.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nenum Tree[T]:\n    Leaf(T value)\n    Empty\n"
        ));
        let (diags, bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        // Self-applied type arg on the stub: `_derive_dump[Tree[T]](self)`.
        let dump = enum_method(get_enum(&m, "Tree"), "dump").expect("stub");
        let Stmt::Expr(Expr::Call { callee, .. }) = &dump.body.stmts[0] else {
            panic!("forwarding call");
        };
        let Expr::Index { index, .. } = &**callee else {
            panic!("gen[Type]");
        };
        // `Tree[T]` is a single-param subscript: `Tree` indexed by bare `T`.
        let Expr::Index {
            base, index: inner, ..
        } = &**index
        else {
            panic!("expected `Tree[T]` subscript, got {index:?}");
        };
        assert!(matches!(&**base, Expr::Name(n, _) if n == "Tree"));
        assert!(matches!(&**inner, Expr::Name(n, _) if n == "T"));

        // Exactly one inferred bound: `T`, from the `Leaf` payload.
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].type_name, "Tree");
        assert_eq!(bounds[0].trait_name, "Dump");
        assert_eq!(bounds[0].params.len(), 1);
        assert_eq!(bounds[0].params[0].param, "T");
        assert_eq!(
            bounds[0].params[0].field_spans.len(),
            1,
            "one payload field mentions T"
        );
    }

    #[test]
    fn a_generic_enum_param_in_no_payload_contributes_no_bound() {
        // A phantom param (used in no variant payload) yields no bounds record.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nenum Phantom[T]:\n    A\n    B\n"
        ));
        let (diags, bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        assert!(
            enum_method(get_enum(&m, "Phantom"), "dump").is_some(),
            "stub still synthesized"
        );
        assert!(
            bounds.is_empty(),
            "a param used in no payload contributes no bound: {bounds:?}"
        );
    }

    #[test]
    fn a_generic_enum_param_behind_a_concrete_container_contributes_no_bound() {
        // `T` appears only as `list[int64]`-style concrete payload: here the
        // payload mentions a *different* concrete type, so `T` is unbounded.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nenum Holder[T]:\n    Ints(list[int64] xs)\n    None\n"
        ));
        let (diags, bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        assert!(
            bounds.is_empty(),
            "no param mentioned in a payload: {bounds:?}"
        );
    }

    #[test]
    fn a_param_used_by_several_variant_payloads_collects_each_span() {
        // `T` in two variants' payloads → one bound with two field spans.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nenum Two[T]:\n    First(T a)\n    Second(T b)\n"
        ));
        let (diags, bounds) = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].params.len(), 1);
        assert_eq!(bounds[0].params[0].param, "T");
        assert_eq!(
            bounds[0].params[0].field_spans.len(),
            2,
            "both payload fields' spans collected"
        );
    }

    // ---- E4 enabler (#1433): built-in trait method sigs without an interface ----

    // A ported built-in's generator with NO same-named `interface` (the name
    // would collide with the operator-overloading `Eq[R]`): the synthesis must
    // supply the dunder signature internally.
    const EQ_LIB_NO_IFACE: &str = "\
@derivable(Eq)
def _derive_eq[T](T a, T b) -> bool:
    return True
";

    #[test]
    fn a_built_in_trait_sig_is_supplied_without_an_interface() {
        // #1433: `@derive(Eq)` with a `@derivable(Eq)` generator but no
        // `interface Eq` synthesizes the `__eq__` stub from the built-in sig
        // table — the enabler for porting the built-in `Eq` to a library
        // generator (#1431), since the name `Eq` is taken by the generic
        // operator interface `Eq[R]`.
        let mut m = module(&format!(
            "{EQ_LIB_NO_IFACE}@derive(Eq)\nstruct Point:\n    int64 x\n    int64 y\n"
        ));
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert!(
            diags.is_empty(),
            "the built-in sig supplies the method, so no 'no interface' error: {diags:?}"
        );
        let point = get_struct(&m, "Point");
        let eq = method(point, "__eq__").expect("synthesized `__eq__` stub");
        // Two-argument shape: untyped `self` + `Self other`, returning `bool`.
        assert_eq!(eq.params.len(), 2, "self + other");
        assert_eq!(eq.params[0].name, "self");
        assert!(eq.params[0].ty.is_none(), "untyped self");
        assert_eq!(eq.params[1].name, "other");
        assert!(
            matches!(&eq.params[1].ty, Some(Type::Named { name, .. }) if name == "Self"),
            "other is typed `Self`: {:?}",
            eq.params[1].ty
        );
        assert!(
            matches!(&eq.ret, Some(Type::Named { name, .. }) if name == "bool"),
            "returns bool: {:?}",
            eq.ret
        );
        // Forwards to `_derive_eq[Point](self, other)`.
        let Stmt::Return {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        } = &eq.body.stmts[0]
        else {
            panic!(
                "expected a forwarding `return` call, got {:?}",
                eq.body.stmts[0]
            );
        };
        let Expr::Index { base, index, .. } = &**callee else {
            panic!("expected `gen[Type](...)`, got {callee:?}");
        };
        assert!(matches!(&**base, Expr::Name(n, _) if n == "_derive_eq"));
        assert!(matches!(&**index, Expr::Name(n, _) if n == "Point"));
        assert_eq!(args.len(), 2, "self + other forwarded");
        assert!(matches!(&args[0].value, Expr::Name(n, _) if n == "self"));
        assert!(matches!(&args[1].value, Expr::Name(n, _) if n == "other"));
        // The trait is stripped; the now-empty `@derive` is removed.
        assert!(derive_args(point).is_empty(), "library trait stripped");
    }

    #[test]
    fn the_built_in_sig_table_covers_the_four_method_traits_only() {
        let sp = Span::default();
        for (trait_name, dunder, nparams, ret) in [
            ("Eq", "__eq__", 2usize, "bool"),
            ("Ord", "__cmp__", 2, "int64"),
            ("Hash", "__hash__", 1, "int64"),
            ("Repr", "__repr__", 1, "str"),
        ] {
            let sigs = builtin_derive_method_sigs(trait_name, sp)
                .unwrap_or_else(|| panic!("`{trait_name}` has a built-in sig"));
            assert_eq!(sigs.len(), 1, "{trait_name}: one method");
            assert_eq!(sigs[0].name, dunder, "{trait_name} dunder");
            assert_eq!(sigs[0].params.len(), nparams, "{trait_name} arity");
            assert_eq!(sigs[0].params[0].name, "self");
            assert!(
                matches!(&sigs[0].ret, Some(Type::Named { name, .. }) if name == ret),
                "{trait_name} return type"
            );
        }
        // The codec traits carry associated functions and are out of scope for
        // this enabler, and a non-built-in trait has no built-in sig.
        assert!(builtin_derive_method_sigs("Json", sp).is_none());
        assert!(builtin_derive_method_sigs("Csv", sp).is_none());
        assert!(builtin_derive_method_sigs("Yaml", sp).is_none());
        assert!(builtin_derive_method_sigs("Dump", sp).is_none());
    }

    #[test]
    fn a_non_built_in_derivable_without_an_interface_still_errors() {
        // The fallback is scoped to the built-in trait names only: a library
        // `@derivable(Dump)` with no `interface Dump` keeps the clear
        // "no interface" diagnostic (no regression).
        let mut m = module(
            "@derivable(Dump)\ndef _derive_dump[T](T self):\n    pass\n\
             @derive(Dump)\nstruct Point:\n    int64 x\n",
        );
        let (diags, _bounds) = expand_library_derives(&mut m);
        assert_eq!(diags.len(), 1, "one diagnostic for the missing interface");
        assert!(
            format!("{:?}", diags[0]).contains("no `interface Dump`"),
            "names the missing interface: {diags:?}"
        );
    }
}
