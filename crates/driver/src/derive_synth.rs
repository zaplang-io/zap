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
//! First cut (#1291): a **non-generic** struct and a library trait whose
//! `interface` declares one method — the Dump / Hash / Serialize shape — of any
//! arity (`MethodSig` carries no type parameters of its own). One stub is
//! synthesized per interface method, each forwarding to the trait's registered
//! generator; a multi-method trait therefore forwards every method to that one
//! generator, so distinct per-method generators are a follow-up. The `@derive`
//! is stripped of the library trait once synthesized so the lowering backstop no
//! longer reports it as deferred (built-in traits and any un-synthesized name are
//! kept). A hand-written method of the same name wins (no stub synthesized). A
//! generic struct is left for bound propagation (E3c #1292); enum deriving and
//! derive-diagnostics wiring (E3d #1293) are later slices.

use std::collections::HashMap;

use zapc_diag::ErrorCode;
use zapc_sema::TypeError;
use zapc_syntax::ast::{Arg, ArgSpread, Block, Expr, Function, Item, MethodSig, Module, Stmt};
use zapc_syntax::span::Span;

/// Synthesize forwarding stubs for every `@derive(<library trait>)` on a type,
/// in place. Returns diagnostics (a library trait with no interface, etc.).
pub fn expand_library_derives(module: &mut Module) -> Vec<TypeError> {
    let mut diags = Vec::new();

    // Pass 1: collect the library-registered derivable traits (`@derivable(T)`
    // on a free function) and the interfaces that supply their method sets. Both
    // are scanned from the AST directly — sema has not run yet.
    let mut generators: HashMap<String, String> = HashMap::new(); // trait -> generator fn
    let mut interfaces: HashMap<String, Vec<MethodSig>> = HashMap::new();
    for item in &module.items {
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
        return diags; // no library derivables -> nothing to do
    }

    // Pass 2: for each struct, synthesize stubs for its library-trait derives and
    // strip those traits from its `@derive(...)`.
    for item in &mut module.items {
        let Item::Struct(s) = item else { continue };
        // Which library traits does this struct derive?
        let derived: Vec<String> = s
            .decorators
            .iter()
            .filter(|d| d.name == "derive")
            .flat_map(|d| d.args.iter())
            .filter_map(|a| match &a.value {
                Expr::Name(n, _) if generators.contains_key(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        if derived.is_empty() {
            continue;
        }
        // A generic struct needs bound propagation (E3c, #1292); defer it by
        // leaving the derive in place (the lowering backstop reports it).
        if !s.generics.is_empty() {
            continue;
        }

        let sp = s.span;
        let mut synthesized: Vec<String> = Vec::new();
        for trait_name in &derived {
            let generator = &generators[trait_name];
            let Some(methods) = interfaces.get(trait_name) else {
                diags.push(err(
                    format!(
                        "cannot derive `{trait_name}` for `{}`: `{trait_name}` is registered with \
                         @derivable but has no `interface {trait_name}` declaring the method(s) to \
                         synthesize (E3b, #1291)",
                        s.name
                    ),
                    sp,
                ));
                continue;
            };
            for m in methods {
                // A hand-written method of the same name wins (HQ7): skip.
                if s.methods.iter().any(|existing| existing.name == m.name) {
                    continue;
                }
                let stub = forwarding_stub(m, generator, &s.name, sp);
                s.methods.push(stub);
            }
            synthesized.push(trait_name.clone());
        }

        // Strip the synthesized library traits from `@derive(...)` so the
        // lowering backstop no longer reports them as deferred; drop a `@derive`
        // left empty. Built-in traits (and any not synthesized) are kept.
        strip_derived(s, &synthesized);
    }

    diags
}

/// Build the thin forwarding stub for one interface method `m`, forwarding every
/// parameter (including `self`) positionally to `generator` specialized at
/// `type_name` — i.e. `generator[Type](self, …)`. The type argument is explicit
/// (I#161) rather than inferred: synthesized nodes share the enclosing type's
/// span, so sema's span-keyed type map cannot distinguish the `self` argument's
/// type for monomorphization to pin `T` from, and the concrete type is known
/// here anyway.
fn forwarding_stub(m: &MethodSig, generator: &str, type_name: &str, sp: Span) -> Function {
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
    // `generator[type_name]` — an explicit type-argument subscript on the callee
    // (mono's `f[args](...)` form, read back through `expr_to_type`).
    let callee = Expr::Index {
        base: Box::new(Expr::Name(generator.to_string(), sp)),
        index: Box::new(Expr::Name(type_name.to_string(), sp)),
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

/// Remove `synthesized` trait names from the struct's `@derive(...)` decorators,
/// dropping any `@derive` left with no arguments.
fn strip_derived(s: &mut zapc_syntax::ast::StructDef, synthesized: &[String]) {
    for d in &mut s.decorators {
        if d.name == "derive" {
            d.args.retain(|a| match &a.value {
                Expr::Name(n, _) => !synthesized.contains(n),
                _ => true,
            });
        }
    }
    s.decorators
        .retain(|d| d.name != "derive" || !d.args.is_empty());
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
        let diags = expand_library_derives(&mut m);
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
    fn synthesized_program_type_checks_clean() {
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Point:\n    int64 x\n    int64 y\ndef main():\n    auto p = Point(1, 2)\n    p.dump()\n"
        ));
        let diags = expand_library_derives(&mut m);
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
        let diags = expand_library_derives(&mut m);
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
        let diags = expand_library_derives(&mut m);
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
        let diags = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        let point = get_struct(&m, "Point");
        assert!(method(point, "dump").is_some(), "library stub synthesized");
        // Only the library trait is stripped; `Eq` survives for the built-in path.
        assert_eq!(derive_args(point), vec!["Eq".to_string()]);
    }

    #[test]
    fn a_generic_struct_is_deferred_not_synthesized() {
        // Bound propagation for a generic struct is E3c (#1292): leave the derive
        // in place so the lowering backstop reports it, and synthesize nothing.
        let mut m = module(&format!(
            "{LIB}@derive(Dump)\nstruct Box[T]:\n    T value\n"
        ));
        let diags = expand_library_derives(&mut m);
        assert!(diags.is_empty(), "no diagnostics: {diags:?}");

        let boxed = get_struct(&m, "Box");
        assert!(
            method(boxed, "dump").is_none(),
            "no stub on a generic struct"
        );
        assert_eq!(
            derive_args(boxed),
            vec!["Dump".to_string()],
            "derive left for backstop"
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
        let diags = expand_library_derives(&mut m);
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
        let diags = expand_library_derives(&mut m);
        assert!(diags.is_empty());
        assert_eq!(m.items.len(), before, "no items added or removed");
        assert!(get_struct(&m, "Point").methods.is_empty());
    }
}
