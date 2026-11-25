//! Monomorphization (I#154, D-0017 / D-0064): the ZIR-stage pass between sema
//! and codegen that turns user-defined generics into ordinary concrete code.
//!
//! It runs on the AST at the very top of `zapc_ir`'s `lower_module` and
//! **erases generics**: for every used `(generic, concrete-type-args)`
//! combination it emits a specialized `struct`/`enum` declaration with a mangled
//! name and empty generic list, substitutes the type parameters, and rewrites
//! every reference — type annotations, call-site type arguments, generic-enum
//! variant constructions/patterns, and the `None` sink — to the specialized
//! name. The result is a generics-free module that the existing lowering and
//! codegen handle unchanged.
//!
//! The central consequence (the epic's crux) is that a monomorphized instance is
//! a plain concrete `StructLayout`, so codegen's existing per-struct
//! `ZapTypeDesc` emission (`emit_struct_desc`: pointer-field offsets from the
//! real LLVM layout) produces a correct precise-GC pointer map per instance with
//! **no new mechanism**. Generic enums ride the tier-3 tagged-struct path. GC
//! correctness for generics is therefore a theorem, not new code.
//!
//! Scope: generic *types* (`struct`/`enum`) with concrete methods, `Optional[T]`,
//! generic *functions*/methods with their own type parameters (I#161), and
//! interface *bounds* on type parameters (`[T: Iface]`, I#164). A bound is
//! enforced at each concrete instantiation by the I#84 structural-conformance
//! rule (re-derived here at the AST level, since ir does not depend on the sema
//! *checker*); the bound's methods then resolve statically per instance through
//! ordinary type substitution — no fat pointer / vtable is emitted (design/0012
//! slice 5).
//!
//! Type-argument inference (I#1169, ADR decisions/lang/1169): a generic
//! constructor / call pins its type parameters by typing each argument
//! expression (`rewrite_expr`) and unifying. That inferer is best-effort and
//! re-derives types at the AST level; where it comes up `Unknown`, it backfills
//! from the type **sema already inferred**, read from a span-keyed `TypeMap`
//! (`zapc_sema_types`, bridged via `Ty::to_ast_type`). This is a **data-only**
//! dependency on the shared type model — not on the sema checker — so ir still
//! owns its lowering/codegen logic (the GC-correctness-as-a-theorem property is
//! unaffected: layout is independent of how a type argument was inferred). The
//! `TypeMap` is supplied by the driver (`zapc_sema::analyze_types`); callers
//! that pass none (the REPL/JIT, most tests) get the re-derivation alone.

use std::collections::{HashMap, HashSet};

use zapc_syntax::ast::{
    self, Arg, ArgSpread, BinOp, Block, CompClause, DictItem, EnumDef, Expr, Function,
    InterfaceDef, Item, MethodSig, Module, Param, PatArg, Pattern, Stmt, StructDef, Type,
    TypeParam, UnOp,
};
use zapc_syntax::span::Span;

// The type model shared with sema (I#1169): `TypeMap` (sema's inferred types,
// keyed by expression span) and `Ty` (with its `to_ast_type` bridge). Re-exported
// so `zapc_ir` — which depends on `zapc_mono`, not on the sema checker — reaches
// them through monomorphization. Threaded into the inference path in a later
// slice (#1167); re-exported here so the carrier crosses the sema->ir boundary.
pub use zapc_sema_types::{Ty as SemaTy, TypeMap};

mod comptime_for;
pub use comptime_for::{
    reflect_field_access, type_registry, unroll_enum_variants, unroll_struct_fields,
    unroll_struct_fields_with_access,
};
use zapc_comptime::TypeRegistry;

/// A lowering error: a human-readable message. Produced by monomorphization and
/// by the main AST->ZIR lowering pass, which share this type (monomorphization
/// is the first lowering stage). `zapc_ir` re-exports it as `zapc_ir::LowerError`
/// and `zapc_ir::lower::LowerError`, so downstream code is unchanged (#129).
#[derive(Clone, Debug, PartialEq)]
pub struct LowerError {
    /// Stable diagnostic identifier. Lowering is a back-end phase, so these are
    /// `ZAP6xxx` (Experimental, per `docs/STABILITY.md`).
    pub code: zapc_diag::ErrorCode,
    pub message: String,
    /// Present when this error is a comptime `@derive` failure (E3d, #1293): the
    /// data the driver needs to render it through the #1256 renderers (the
    /// `ZAP7003` consumer header at the user's field, the `required because of …`
    /// backtrace, and the root cause). `None` for an ordinary lowering error,
    /// which renders as the location-free `ZAP6xxx` header exactly as before.
    /// Boxed so an ordinary `LowerError` stays small.
    pub derive: Option<Box<DeriveErrorInfo>>,
}

/// The data a comptime `@derive` failure carries up to the driver, where source
/// text is available to render it (E3d, #1293). Plain data (no `zapc_comptime`
/// types) so it crosses the mono → driver boundary cleanly; the driver
/// reconstructs the `CtError`/`DeriveBacktrace` and calls the #1256 renderers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeriveErrorInfo {
    /// The derived trait and the type it is derived for, when the failure is
    /// inside a registered `@derivable` generator's descent (the consumer
    /// `ZAP7003` "cannot derive `trait` for `type`" form). `None` when the
    /// comptime failure is not under a derive (rendered as a plain comptime
    /// error at its own span).
    pub trait_and_type: Option<(String, String)>,
    /// The root cause's message (the library's own words for a `comptime.error`,
    /// or the engine's for a fault) and the span it points at (the user's field
    /// / element, via span fidelity through #1251/#1252).
    pub cause_message: String,
    pub cause_span: zapc_syntax::span::Span,
    /// `true` for a library-authored `comptime.error` abort (`ZAP7002`), `false`
    /// for an engine fault (`ZAP7001`); selects the root-cause lead-in and, for a
    /// fault, the dual-span generator-fault form.
    pub cause_is_user: bool,
    /// The span to cite as "instantiated here" for an engine fault inside a
    /// generator (the derived type's declaration), when under a derive. `None`
    /// for a comptime failure outside a derive. Drives the dual-span
    /// generator-fault diagnostic (E3d, #1293): a *bug* in a library generator is
    /// reported at the generator line **and** at the user's derive site, never a
    /// panic.
    pub instantiation_span: Option<zapc_syntax::span::Span>,
    /// Backtrace frames (outermost first): `(label, span)` per context the
    /// descent unwound through, e.g. `("field `b` of `Outer`", <field span>)`.
    pub frames: Vec<(String, zapc_syntax::span::Span)>,
}

/// Monomorphize a module: produce an equivalent AST with all user generics
/// instantiated to concrete declarations and every generic reference rewritten.
pub fn monomorphize(module: &Module) -> Result<Module, LowerError> {
    monomorphize_with_types(module, &TypeMap::default())
}

/// Monomorphize `module`, consulting sema's inferred types (#1167): when the
/// pass's own best-effort inferer cannot type a generic constructor / call
/// argument, it falls back to `types.type_at(arg_span)` — the type sema already
/// computed — bridged to an `ast::Type`. This closes the inference gaps the
/// hand-rolled inferer leaves (built-in call returns #1132, module constants
/// #1162, generators, …) without changing any argument it already types.
/// `types` comes from the driver (via `zapc_sema::analyze_types`); `zapc_ir`
/// cannot build it, since it does not depend on the sema checker.
pub fn monomorphize_with_types(module: &Module, types: &TypeMap) -> Result<Module, LowerError> {
    Mono::new(module, types).run()
}

/// A concrete (fully type-param-free) type, the result of the pass's focused
/// inferer. `Ty` wraps an ordinary concrete `ast::Type`; `NoneLit` is the bare
/// `None` literal awaiting a sink; `Unknown` is anything the inferer cannot type
/// (non-generic code, which is then left untouched — always safe).
#[derive(Clone, Debug)]
enum CTy {
    Ty(Type),
    NoneLit,
    Unknown,
}

/// The numeric family of a best-effort scalar type, for inferring the result
/// type of an arithmetic binary expression (#1128). It mirrors the distinctions
/// `sema_types::num_join` draws, whose promotion order is
/// `complex > float > unsigned > bigint > signed`. Re-derived here (by scalar
/// name) because `mono`/`ir` do not depend on the `sema` checker — they re-derive types (see
/// the module docs).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Num {
    Signed,
    Unsigned,
    Bigint,
    Float,
    Complex,
}

/// The numeric family of a best-effort type, or `None` when it is not a scalar
/// number the inferer models (`Unknown`, a sequence/collection, a tuple, a
/// function type, or a user type). Only a bare `Named` scalar qualifies.
fn num_kind(c: &CTy) -> Option<Num> {
    let CTy::Ty(Type::Named { name, args, .. }) = c else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    Some(match name.as_str() {
        // `int` is the untyped-int literal's name; it joins like `int64`.
        "int" | "int32" | "int64" => Num::Signed,
        "uint8" | "uint16" | "uint32" | "uint64" | "usize" => Num::Unsigned,
        "bigint" => Num::Bigint,
        // `float` is the untyped-float literal's name; it joins like `float64`.
        "float" | "float32" | "float64" => Num::Float,
        "complex" => Num::Complex,
        _ => return None,
    })
}

/// Whether a type is a fixed-width/`usize`/untyped integer eligible for the
/// bitwise and shift operators — and, if so, whether it is unsigned. `bigint`
/// is excluded (sema rejects bitwise ops on it); floats and complex are not
/// integers.
fn int_unsigned(c: &CTy) -> Option<bool> {
    match num_kind(c) {
        Some(Num::Signed) => Some(false),
        Some(Num::Unsigned) => Some(true),
        _ => None,
    }
}

/// `+` on two identical sequence types concatenates to that type, mirroring
/// sema: `str + str` → `str`, `bytes + bytes` → `bytes`, and
/// `list[T] + list[U]` → the left list's type. Anything else is not a
/// concatenation the inferer models, so it stays `Unknown`.
fn same_sequence(l: &CTy, r: &CTy) -> CTy {
    if let (CTy::Ty(Type::Named { name: ln, .. }), CTy::Ty(Type::Named { name: rn, .. })) = (l, r) {
        if matches!(
            (ln.as_str(), rn.as_str()),
            ("str", "str") | ("bytes", "bytes") | ("list", "list")
        ) {
            return l.clone();
        }
    }
    CTy::Unknown
}

/// The source span of an expression, for looking its inferred type up in the
/// sema `TypeMap` (#1167). Mirrors `zapc_sema`'s own `expr_span` over the shared
/// `ast::Expr`.
fn expr_span(e: &Expr) -> Span {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Imaginary(_, s)
        | Expr::Bool(_, s)
        | Expr::None(s)
        | Expr::Name(_, s)
        | Expr::Tuple(_, s)
        | Expr::List(_, s)
        | Expr::Dict(_, s)
        | Expr::Set(_, s) => *s,
        Expr::Str { span, .. }
        | Expr::Starred { span, .. }
        | Expr::ListComp { span, .. }
        | Expr::DictComp { span, .. }
        | Expr::SetComp { span, .. }
        | Expr::GenExp { span, .. }
        | Expr::Comptime { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Compare { span, .. }
        | Expr::Call { span, .. }
        | Expr::Index { span, .. }
        | Expr::Slice { span, .. }
        | Expr::SliceSpec { span, .. }
        | Expr::Attr { span, .. }
        | Expr::Lambda { span, .. }
        | Expr::RegexMatch { span, .. }
        | Expr::RegexSubst { span, .. }
        | Expr::ChanRecv { span, .. }
        | Expr::Yield { span, .. }
        | Expr::IfElse { span, .. }
        | Expr::Named { span, .. } => *span,
    }
}

struct Mono<'a> {
    /// Sema's inferred type for every expression, keyed by span (#1167). When
    /// the pass's own best-effort inferer leaves an argument `Unknown`,
    /// `rewrite_expr` falls back to this (bridged to an `ast::Type`). Empty on
    /// the paths that pass no types (most tests, `lower_module` without a map),
    /// in which case behaviour is exactly as before.
    types: &'a TypeMap,
    /// Generic type-parameter name lists, keyed by type name.
    params: HashMap<String, Vec<String>>,
    /// Generic struct declarations by name.
    gstructs: HashMap<String, &'a StructDef>,
    /// Generic enum declarations by name.
    genums: HashMap<String, &'a EnumDef>,
    /// Generic free-function declarations by name (I#161, slice 4).
    gfuncs: HashMap<String, &'a Function>,
    /// Every struct declaration (generic or not), for field/method lookups.
    structs: HashMap<String, &'a StructDef>,
    /// Every enum declaration (generic or not), for variant/method lookups.
    enums: HashMap<String, &'a EnumDef>,
    /// Every interface declaration, for static type-parameter bound satisfaction
    /// checks (I#164, slice 5): a bound `[T: Iface]` is enforced by the same
    /// structural-conformance rule as I#84, re-derived here at the AST level
    /// (ir does not depend on the sema checker; it re-derives types — see module docs).
    interfaces: HashMap<String, &'a InterfaceDef>,
    /// Free-function return types, for inference of `f(...)` results.
    func_ret: HashMap<String, Option<Type>>,
    /// Every free-function declaration (generic or not), by name. Used to read a
    /// callee's parameter types so a call argument can be rewritten with the
    /// parameter's declared type as its expected-type sink (#1342), the way a
    /// `return` already passes the function's return type.
    funcs: HashMap<String, &'a Function>,
    /// Library-registered derivable generators (E3a, #1290): generator function
    /// name -> the trait it derives (from `@derivable(Trait)`). Used to recover
    /// the `@derive` context (trait + type) when a `comptime.error` or engine
    /// fault is raised inside a generator's descent, so the failure renders as a
    /// consumer `ZAP7003` (E3d, #1293).
    derivable_generators: HashMap<String, String>,
    /// The derive context of the generic function body currently being
    /// specialized, when it is a derivable generator: `(trait, concrete type)`.
    /// Set around the body rewrite in `emit_func_instance`; read when a comptime
    /// failure surfaces during that body's `comptime for` unroll (E3d, #1293).
    cur_derive: Option<(String, String)>,
    /// The comptime [`TypeRegistry`] for this module (built once), attached to the
    /// evaluator that unrolls a `comptime for` so a generator's `f.ty.kind` /
    /// container recursion resolves user types (#1253, #1330). Without it a
    /// `.kind`-based guard in a derive generator cannot evaluate, so a guarded
    /// `comptime.error` would mis-fire.
    comptime_registry: TypeRegistry,
    /// Module-level global bindings (epic #661 / D-0112): a top-level
    /// `TYPE name = expr` (`const` or mutable) contributes its declared type
    /// here. These are seeded into every function body's type environment (as an
    /// outermost scope, so locals and params still shadow them) so that a method
    /// call whose receiver is a bare module-global `Expr::Name` resolves the
    /// receiver's concrete type exactly as it does for a local — which is what
    /// drives receiver-type specialization (e.g. `dict.get`, generic methods).
    /// Without this, a global's receiver is opaque and the specialization is
    /// silently skipped (issue #671). Only globals with an explicit type
    /// annotation are recorded; an `auto` global's type is inferred by sema but
    /// not re-derived here, so it is left out (and falls back to the prior
    /// opaque-receiver behavior).
    globals: HashMap<String, Type>,
    /// Transparent type aliases (#606, D-0105): alias name -> (generic parameter
    /// names, target type). An alias is expanded to its target — with the
    /// parameters substituted — wherever a type is rewritten, so no alias head
    /// survives into lowering/codegen. Expansion is purely structural; it emits
    /// no declaration (an alias is not a nominal type).
    aliases: HashMap<String, (Vec<String>, Type)>,
    /// Alias names on a definition cycle (`type A = B; type B = A`). Left
    /// unexpanded so expansion cannot loop; the cycle is reported by sema.
    cyclic_aliases: HashSet<String>,

    /// Specialized declarations to emit, in request order, with their mangled
    /// names recorded in `requested` for dedup.
    spec_structs: Vec<StructDef>,
    spec_enums: Vec<EnumDef>,
    requested: HashSet<String>,
    /// Pending instantiations: `(generic name, concrete args)`.
    worklist: Vec<(String, Vec<Type>)>,
    /// Specialized generic free functions to emit (I#161), deduped by mangled
    /// name in `requested_funcs`; drained via `func_worklist`.
    spec_funcs: Vec<Function>,
    requested_funcs: HashSet<String>,
    func_worklist: Vec<(String, Vec<Type>)>,
    /// Specialized generic methods to attach to their owner's concrete
    /// declaration (I#161): owner concrete type name -> methods. Merged into the
    /// emitted declarations after the worklists drain. Deduped by
    /// `owner::method$args` in `requested_methods`; drained via `method_worklist`.
    spec_methods: HashMap<String, Vec<Function>>,
    requested_methods: HashSet<String>,
    method_worklist: Vec<MethodInst>,
    /// The module's items (snapshot) and span, so `run` can iterate without
    /// re-borrowing the source module while mutating `self`.
    src_items: Vec<Item>,
    span: Span,

    // --- per-declaration rewrite context (set while rewriting one fn/method) ---
    /// Type-parameter substitution for the declaration currently being rewritten
    /// (empty for non-generic top-level code).
    cur_subst: HashMap<String, Type>,
    /// Associated-type projections for the declaration currently being rewritten
    /// (#1207): `"T.Out" -> <concrete binding>` for each bounded type parameter
    /// whose generic-interface bound declares an associated type, resolved to the
    /// concrete argument's producing-method return. Consulted by [`apply_subst`]
    /// so a `T.Out` annotation in a specialized signature becomes the concrete
    /// type before codegen. Empty when no bound contributes a projection.
    cur_proj: HashMap<String, Type>,
    /// The enclosing generic instance (its name + concrete args), used to resolve
    /// a bare `Self.Variant` (`Optional.Some`/`Optional.None`) inside a generic
    /// method body to the current instantiation.
    cur_self: Option<(String, Vec<Type>)>,
    /// The current function's concrete declared return type, for the `return`
    /// expected-type sink.
    cur_ret: Option<Type>,
}

/// A pending generic-method instantiation (I#161): the owner generic type (with
/// its concrete args) and the method's own name + concrete type arguments.
#[derive(Clone, Debug)]
struct MethodInst {
    owner: String,
    owner_args: Vec<Type>,
    method: String,
    targs: Vec<Type>,
}

/// A lexical environment mapping value names to their inferred concrete types,
/// used to resolve generic constructions/patterns and the `None` sink.
#[derive(Default)]
struct Env {
    scopes: Vec<HashMap<String, CTy>>,
}

impl Env {
    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop(&mut self) {
        self.scopes.pop();
    }
    fn declare(&mut self, name: &str, ty: CTy) {
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name.to_string(), ty);
        }
    }
    fn lookup(&self, name: &str) -> Option<CTy> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }
}

fn err(m: impl Into<String>) -> LowerError {
    LowerError {
        code: zapc_diag::ErrorCode::ZAP6001,
        message: m.into(),
        derive: None,
    }
}

/// Whether `outer` fully encloses `inner` (half-open byte ranges). Used to
/// attribute a derive diagnostic whose cause is a nested type's use-site span
/// (#1346) to the enclosing field's declaration.
fn span_contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// An enum's variant payload fields, flattened to [`ast::Field`]s (#1375, E3d):
/// so a derive failure a generator points at inside a variant payload
/// (`comptime.error(f.span, …)`) is attributed to that field in the
/// `required because of …` chain, exactly as a struct field is. Carries each
/// payload field's type, name, and span (decorators/bitfield/default are not
/// part of a variant payload).
fn variant_payload_fields(def: &EnumDef) -> Vec<ast::Field> {
    def.variants
        .iter()
        .flat_map(|v| {
            v.fields.iter().map(|vf| ast::Field {
                decorators: Vec::new(),
                ty: vf.ty.clone(),
                name: vf.name.clone(),
                bits: None,
                default: None,
                span: vf.span,
            })
        })
        .collect()
}

/// Render a declared [`Type`] to a canonical label string (`list[Widget]`,
/// `dict[str, list[Widget]]`, `(int64, str)`) for a `required because of …`
/// backtrace frame. Mono has no source text, so the label is reconstructed from
/// the AST (#1293).
fn render_type(t: &Type) -> String {
    match t {
        Type::Named { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = args.iter().map(render_type).collect();
                format!("{name}[{}]", inner.join(", "))
            }
        }
        Type::Tuple { elems, .. } => {
            let inner: Vec<String> = elems.iter().map(render_type).collect();
            format!("({})", inner.join(", "))
        }
        Type::Func { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(render_type).collect();
            match ret {
                Some(r) => format!("fn({}) -> {}", ps.join(", "), render_type(r)),
                None => format!("fn({})", ps.join(", ")),
            }
        }
    }
}

/// One step of the descent toward `cause` inside a container type: the
/// `required because of …` label for the level crossed and the child type to
/// continue into. `None` when `cur` is not a container, or no immediate child's
/// declared span contains `cause` (so the walk stops cleanly) (#1293).
fn descend_toward(cur: &Type, cause: Span) -> Option<(String, &Type)> {
    match cur {
        Type::Named { name, args, .. } => match (name.as_str(), args.as_slice()) {
            ("list" | "List", [e]) if span_contains(e.span(), cause) => {
                Some((format!("the element type of `{}`", render_type(cur)), e))
            }
            ("dict" | "Dict", [k, v]) => {
                if span_contains(k.span(), cause) {
                    Some((format!("the key type of `{}`", render_type(cur)), k))
                } else if span_contains(v.span(), cause) {
                    Some((format!("the value type of `{}`", render_type(cur)), v))
                } else {
                    None
                }
            }
            ("Optional", [e]) if span_contains(e.span(), cause) => {
                Some((format!("the element type of `{}`", render_type(cur)), e))
            }
            _ => None,
        },
        Type::Tuple { elems, .. } => elems
            .iter()
            .position(|e| span_contains(e.span(), cause))
            .map(|i| {
                (
                    format!("element {} of `{}`", i + 1, render_type(cur)),
                    &elems[i],
                )
            }),
        Type::Func { .. } => None,
    }
}

/// Walk a field's declared type from the outside in toward the cause span,
/// pushing one `required because of …` frame per container level crossed (list
/// element, dict key/value, optional element, tuple element). Stops at the node
/// the generator pointed at (`span == cause`) or when no child contains the
/// cause. Frames are appended outermost-first — the renderer prints them
/// inner-first (nearest the failure, climbing to the field) (#1293). The depth
/// is bounded defensively; a declared type is finite, so the bound is never hit
/// in practice.
fn push_container_frames(frames: &mut Vec<(String, Span)>, field_ty: &Type, cause: Span) {
    let mut cur = field_ty;
    for _ in 0..64 {
        if cur.span() == cause {
            return;
        }
        let Some((label, next)) = descend_toward(cur, cause) else {
            return;
        };
        frames.push((label, cur.span()));
        cur = next;
    }
}

/// The trait a free function registers via `@derivable(Trait)` (one positional
/// identifier), or `None` (E3a, #1290). Mirrors the driver's synthesis scan.
fn derivable_trait(f: &Function) -> Option<String> {
    let d = f.decorators.iter().find(|d| d.name == "derivable")?;
    match d.args.first().map(|a| &a.value) {
        Some(Expr::Name(t, _)) if d.args.len() == 1 => Some(t.clone()),
        _ => None,
    }
}

/// Substitute type-parameter names with concrete argument types in a type
/// expression (#606, transparent-alias expansion). A bare `Named` whose name is
/// a key in `subst` becomes the mapped type; every other node is rebuilt with
/// its children substituted.
fn subst_type(t: &Type, subst: &HashMap<&str, &Type>) -> Type {
    match t {
        Type::Named { name, args, span } => {
            if args.is_empty() {
                if let Some(replacement) = subst.get(name.as_str()) {
                    return (*replacement).clone();
                }
            }
            Type::Named {
                name: name.clone(),
                args: args.iter().map(|a| subst_type(a, subst)).collect(),
                span: *span,
            }
        }
        Type::Tuple { elems, span } => Type::Tuple {
            elems: elems.iter().map(|e| subst_type(e, subst)).collect(),
            span: *span,
        },
        Type::Func { params, ret, span } => Type::Func {
            params: params.iter().map(|p| subst_type(p, subst)).collect(),
            ret: ret.as_ref().map(|r| Box::new(subst_type(r, subst))),
            span: *span,
        },
    }
}

/// Substitute generic type-parameter names in an interface method signature
/// (#1205, generic-interface conformance): each parameter type and the return
/// type has `subst_type` applied, so `def get(self) -> T` in `interface
/// Container[T]` becomes `def get(self) -> int64` when matched against a
/// conformer at `Container[int64]`. Parameter names/flags are preserved.
fn subst_method_sig(m: &MethodSig, subst: &HashMap<&str, &Type>) -> MethodSig {
    MethodSig {
        name: m.name.clone(),
        params: m
            .params
            .iter()
            .map(|p| Param {
                ty: p.ty.as_ref().map(|t| subst_type(t, subst)),
                ..p.clone()
            })
            .collect(),
        ret: m.ret.as_ref().map(|t| subst_type(t, subst)),
        span: m.span,
    }
}

/// A zero-value literal expression for a field type, for `reflect.construct`
/// (#1454): the throwaway initial value a field-set builder overwrites. v1
/// covers the scalar / `str` / `bytes` field kinds a codec record uses; a
/// generic/container/nested type yields `None` (a clean "unsupported yet" error
/// at the call site), mirroring the derive codegen's field-type restrictions.
fn zero_expr(ty: &Type, span: Span) -> Option<Expr> {
    let Type::Named { name, args, .. } = ty else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let empty_str = |bytes: bool| Expr::Str {
        value: String::new(),
        raw: false,
        bytes,
        fstring: false,
        span,
        fstring_fields: Vec::new(),
    };
    Some(match name.as_str() {
        "int" | "int64" | "i64" | "int32" | "i32" | "int16" | "i16" | "int8" | "i8" | "uint8"
        | "u8" | "uint16" | "u16" | "uint32" | "u32" | "uint64" | "u64" | "usize" => {
            Expr::Int("0".to_string(), span)
        }
        "float" | "float64" | "f64" | "float32" | "f32" => Expr::Float("0.0".to_string(), span),
        "bool" => Expr::Bool(false, span),
        "str" => empty_str(false),
        "bytes" => empty_str(true),
        _ => return None,
    })
}

/// Extract the type name from a `reflect.type_info(<TypeName>)` call expression
/// (#1298) — the base of a `comptime for … in reflect.type_info(T).<member>`
/// iterable. Returns `None` for any other shape, or a non-bare-name type
/// argument (generic/container spellings in that position are deferred).
fn reflect_type_info_arg(base: &Expr) -> Option<String> {
    let Expr::Call { callee, args, .. } = base else {
        return None;
    };
    let Expr::Attr {
        base: recv, name, ..
    } = &**callee
    else {
        return None;
    };
    if name != "type_info" || !matches!(&**recv, Expr::Name(m, _) if m == "reflect") {
        return None;
    }
    match args.as_slice() {
        [a] if a.name.is_none() => match &a.value {
            Expr::Name(t, _) => Some(t.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Collect the names of aliases referenced anywhere in `t` (#606 cycle
/// detection); only names present in `aliases` are recorded.
fn collect_alias_refs(
    t: &Type,
    aliases: &HashMap<String, (Vec<String>, Type)>,
    out: &mut Vec<String>,
) {
    match t {
        Type::Named { name, args, .. } => {
            if aliases.contains_key(name) {
                out.push(name.clone());
            }
            for a in args {
                collect_alias_refs(a, aliases, out);
            }
        }
        Type::Tuple { elems, .. } => {
            for e in elems {
                collect_alias_refs(e, aliases, out);
            }
        }
        Type::Func { params, ret, .. } => {
            for p in params {
                collect_alias_refs(p, aliases, out);
            }
            if let Some(r) = ret {
                collect_alias_refs(r, aliases, out);
            }
        }
    }
}

/// Canonicalize a primitive type-name alias so `int64`/`i64` (etc.) mangle to
/// one instance.
fn canon_prim(name: &str) -> &str {
    match name {
        "i64" => "int64",
        "i32" => "int32",
        "f64" => "float64",
        "f32" => "float32",
        "u8" => "uint8",
        _ => name,
    }
}

/// The human-readable name of a concrete type, for bound diagnostics (I#164).
fn type_name(t: &Type) -> String {
    match t {
        Type::Named { name, args, .. } => {
            if args.is_empty() {
                canon_prim(name).to_string()
            } else {
                let inner: Vec<String> = args.iter().map(type_name).collect();
                format!("{}[{}]", canon_prim(name), inner.join(", "))
            }
        }
        Type::Tuple { elems, .. } => {
            let inner: Vec<String> = elems.iter().map(type_name).collect();
            format!("({})", inner.join(", "))
        }
        Type::Func { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(type_name).collect();
            match ret {
                Some(r) => format!("fn({}) -> {}", ps.join(", "), type_name(r)),
                None => format!("fn({})", ps.join(", ")),
            }
        }
    }
}

/// The value type `V` of a `dict[K, V]` receiver type, else `None` (ticket #281,
/// used to resolve `dict.get`'s `Optional[V]` result instance).
fn dict_value_ty(cty: &CTy) -> Option<Type> {
    if let CTy::Ty(Type::Named { name, args, .. }) = cty {
        if name == "dict" && args.len() == 2 {
            return Some(args[1].clone());
        }
    }
    None
}

/// The payload type `V` of an expected `Optional[V]` type, else `None` (ticket
/// #281): the binding's declared-type sink that fixes `dict.get`'s result when
/// the receiver's dict type is opaque to this inferer.
fn optional_payload_ty(expected: Option<&Type>) -> Option<Type> {
    if let Some(Type::Named { name, args, .. }) = expected {
        if name == "Optional" && args.len() == 1 {
            return Some(args[0].clone());
        }
    }
    None
}

/// Does a concrete method `have` match an interface method `want` for structural
/// conformance (D-0016/I#84, re-derived for monomorphization, I#164)? The
/// receiver (`self`, parameter 0) is ignored — it is the differing
/// concrete/interface type — so the remaining parameter types are compared
/// pairwise and the return type is compared, by structural type identity
/// (primitive aliases canonicalized). The interface's associated types are
/// treated as **free variables** the conformer binds (#1207): a `want` position
/// typed as a bare associated-type name (listed in `assoc`) matches any concrete
/// `have` type, and repeated occurrences must bind consistently. With `assoc`
/// empty this is plain structural equality.
fn method_conforms(have: &Function, want: &MethodSig, assoc: &[&str]) -> bool {
    let hp = have.params.get(1..).unwrap_or(&[]);
    let wp = want.params.get(1..).unwrap_or(&[]);
    if hp.len() != wp.len() {
        return false;
    }
    let mut binds: HashMap<&str, &Type> = HashMap::new();
    for (a, b) in hp.iter().zip(wp.iter()) {
        if !ty_opt_conforms(a.ty.as_ref(), b.ty.as_ref(), assoc, &mut binds) {
            return false;
        }
    }
    ty_opt_conforms(have.ret.as_ref(), want.ret.as_ref(), assoc, &mut binds)
}

/// Structural match of a concrete type `have` against a `want` that may name an
/// associated type (#1207). A bare `want` named in `assoc` binds to `have`
/// (consistency-checked across occurrences); everything else is `type_eq`.
fn ty_opt_conforms<'t>(
    have: Option<&'t Type>,
    want: Option<&'t Type>,
    assoc: &[&str],
    binds: &mut HashMap<&'t str, &'t Type>,
) -> bool {
    match (have, want) {
        (None, None) => true,
        (Some(h), Some(w)) => {
            if let Type::Named { name, args, .. } = w {
                if args.is_empty() && assoc.contains(&name.as_str()) {
                    return match binds.get(name.as_str()) {
                        Some(prev) => type_eq(prev, h),
                        None => {
                            binds.insert(name.as_str(), h);
                            true
                        }
                    };
                }
            }
            type_eq(h, w)
        }
        _ => false,
    }
}

/// A compiler-known **capability interface** (D-0139): the adjectival vocabulary
/// (`Equatable`/`Hashable`/`Comparable`) a container's element/key/value position
/// is bounded on. Unlike a user `interface`, a capability is satisfied
/// *intrinsically* by the shape of a concrete type — a scalar has a total order
/// and a hash; a tier-1 `enum` hashes/orders by its backing value; a `value
/// struct` auto-derives equality and hashing from its fields (#808); a tier-3
/// `enum` is a tagged value aggregate hashable iff its payloads are (#809) —
/// rather than by a user-written method set. A capability bound on a user generic
/// container (`struct BST[K: Comparable, V]`, I#811) is therefore checked
/// structurally against the concrete argument here, exactly as a built-in
/// collection's element position is (#807/#808/#809), which is what makes the
/// capability model uniform across built-in and user containers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Capability {
    Equatable,
    Hashable,
    Comparable,
}

impl Capability {
    fn name(self) -> &'static str {
        match self {
            Capability::Equatable => "Equatable",
            Capability::Hashable => "Hashable",
            Capability::Comparable => "Comparable",
        }
    }

    /// Resolve an interface name to the capability it denotes, if any. The three
    /// capability interfaces are compiler-known and never appear as AST
    /// `interface` items, so a bound that names one is routed to the intrinsic
    /// check instead of the structural (method-set) one.
    fn from_iface(iface: &str) -> Option<Capability> {
        match iface {
            "Equatable" => Some(Capability::Equatable),
            "Hashable" => Some(Capability::Hashable),
            "Comparable" => Some(Capability::Comparable),
            _ => None,
        }
    }
}

/// Does `@derive(...)` on `decorators` list the trait `want` (e.g. `Eq`, `Hash`,
/// `Ord`)? Mirrors the sema reader in `struct_info_of`.
fn derives_trait(decorators: &[ast::Decorator], want: &str) -> bool {
    decorators.iter().any(|d| {
        d.name == "derive"
            && d.args
                .iter()
                .any(|a| matches!(&a.value, Expr::Name(n, _) if n == want))
    })
}

/// Does a type with these `methods` / `decorators` provide `dunder` either by a
/// hand-written method or by the `@derive(derive_trait)` that generates it?
fn provides_dunder(
    methods: &[Function],
    decorators: &[ast::Decorator],
    dunder: &str,
    derive_trait: &str,
) -> bool {
    methods.iter().any(|m| m.name == dunder) || derives_trait(decorators, derive_trait)
}

/// The forward and reflected operator dunders for a binary operator (D-0047,
/// #1180), or `None` for an operator that is not dispatched to a dunder
/// (logical / comparison / membership / identity / channel send). The names
/// mirror `reflected_op_dunder` and the forward-dunder table in the IR crate's
/// operator lowering (`crates/ir/src/lower/operators.rs`); they are duplicated
/// here because the two crates do not share a map, and the IR table is the
/// authority — any new overloadable operator must be added to both.
fn op_dunders(op: BinOp) -> Option<(&'static str, &'static str)> {
    use BinOp::*;
    Some(match op {
        Add => ("__add__", "__radd__"),
        Sub => ("__sub__", "__rsub__"),
        Mul => ("__mul__", "__rmul__"),
        Div => ("__truediv__", "__rtruediv__"),
        FloorDiv => ("__floordiv__", "__rfloordiv__"),
        Mod => ("__mod__", "__rmod__"),
        Pow => ("__pow__", "__rpow__"),
        MatMul => ("__matmul__", "__rmatmul__"),
        BitAnd => ("__and__", "__rand__"),
        BitOr => ("__or__", "__ror__"),
        BitXor => ("__xor__", "__rxor__"),
        Shl => ("__lshift__", "__rlshift__"),
        Shr => ("__rshift__", "__rrshift__"),
        _ => return None,
    })
}

/// Structural equality of two optional types, treating `None` (no annotation /
/// no return) as the unit type so an absent return matches an absent return.
fn opt_type_eq(a: Option<&Type>, b: Option<&Type>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => type_eq(x, y),
        _ => false,
    }
}

/// Structural equality of two concrete types (primitive aliases canonicalized).
fn type_eq(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (
            Type::Named {
                name: n1, args: a1, ..
            },
            Type::Named {
                name: n2, args: a2, ..
            },
        ) => {
            canon_prim(n1) == canon_prim(n2)
                && a1.len() == a2.len()
                && a1.iter().zip(a2.iter()).all(|(x, y)| type_eq(x, y))
        }
        (Type::Tuple { elems: e1, .. }, Type::Tuple { elems: e2, .. }) => {
            e1.len() == e2.len() && e1.iter().zip(e2.iter()).all(|(x, y)| type_eq(x, y))
        }
        (
            Type::Func {
                params: p1,
                ret: r1,
                ..
            },
            Type::Func {
                params: p2,
                ret: r2,
                ..
            },
        ) => {
            p1.len() == p2.len()
                && p1.iter().zip(p2.iter()).all(|(x, y)| type_eq(x, y))
                && opt_type_eq(r1.as_deref(), r2.as_deref())
        }
        _ => false,
    }
}

impl<'a> Mono<'a> {
    fn new(module: &'a Module, types: &'a TypeMap) -> Mono<'a> {
        let mut m = Mono {
            types,
            params: HashMap::new(),
            gstructs: HashMap::new(),
            genums: HashMap::new(),
            gfuncs: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            interfaces: HashMap::new(),
            func_ret: HashMap::new(),
            funcs: HashMap::new(),
            derivable_generators: HashMap::new(),
            cur_derive: None,
            comptime_registry: type_registry(module),
            globals: HashMap::new(),
            aliases: HashMap::new(),
            cyclic_aliases: HashSet::new(),
            spec_structs: Vec::new(),
            spec_enums: Vec::new(),
            requested: HashSet::new(),
            worklist: Vec::new(),
            spec_funcs: Vec::new(),
            requested_funcs: HashSet::new(),
            func_worklist: Vec::new(),
            spec_methods: HashMap::new(),
            requested_methods: HashSet::new(),
            method_worklist: Vec::new(),
            src_items: module.items.clone(),
            span: module.span,
            cur_subst: HashMap::new(),
            cur_proj: HashMap::new(),
            cur_self: None,
            cur_ret: None,
        };
        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    m.structs.insert(s.name.clone(), s);
                    if !s.generics.is_empty() {
                        m.params.insert(
                            s.name.clone(),
                            s.generics.iter().map(|p| p.name.clone()).collect(),
                        );
                        m.gstructs.insert(s.name.clone(), s);
                    }
                }
                Item::Enum(e) => {
                    m.enums.insert(e.name.clone(), e);
                    if !e.generics.is_empty() {
                        m.params.insert(
                            e.name.clone(),
                            e.generics.iter().map(|p| p.name.clone()).collect(),
                        );
                        m.genums.insert(e.name.clone(), e);
                    }
                }
                Item::Function(f) => {
                    m.func_ret.insert(f.name.clone(), f.ret.clone());
                    m.funcs.insert(f.name.clone(), f);
                    if !f.generics.is_empty() {
                        m.gfuncs.insert(f.name.clone(), f);
                    }
                    // `@derivable(Trait)` registers this generator for `Trait`
                    // (E3a, #1290); record name -> trait so a comptime failure in
                    // its descent can be rendered as a derive error (E3d, #1293).
                    if let Some(tr) = derivable_trait(f) {
                        m.derivable_generators.insert(f.name.clone(), tr);
                    }
                }
                Item::Interface(i) => {
                    m.interfaces.insert(i.name.clone(), i);
                }
                Item::TypeAlias(a) => {
                    m.aliases.insert(
                        a.name.clone(),
                        (
                            a.generics.iter().map(|p| p.name.clone()).collect(),
                            a.target.clone(),
                        ),
                    );
                }
                _ => {}
            }
        }
        m.compute_alias_cycles();
        // Record module-level global bindings (epic #661 / D-0112) with their
        // declared type (alias-expanded). A global is a single-name top-level
        // `Stmt::Let`, `const` or mutable; tuple-destructuring at module scope is
        // rejected earlier (ZAP3018). Only explicitly-typed globals are recorded
        // (see the `globals` field doc). Collected first, then inserted, to keep
        // the immutable `apply_subst` borrow off the mutated map.
        let global_tys: Vec<(String, Type)> = module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Stmt(Stmt::Let {
                    ty: Some(t), name, ..
                }) => Some((name.clone(), m.apply_subst(t))),
                _ => None,
            })
            .collect();
        for (name, ty) in global_tys {
            m.globals.insert(name, ty);
        }
        m
    }

    /// Mark aliases that lie on a definition cycle (#606) so expansion never
    /// loops. An alias is cyclic iff it can reach itself through alias
    /// references in its (and other aliases') targets.
    fn compute_alias_cycles(&mut self) {
        let mut deps: HashMap<String, Vec<String>> = HashMap::new();
        for (name, (_params, target)) in &self.aliases {
            let mut refs = Vec::new();
            collect_alias_refs(target, &self.aliases, &mut refs);
            deps.insert(name.clone(), refs);
        }
        let names: Vec<String> = self.aliases.keys().cloned().collect();
        for start in &names {
            let mut stack = deps.get(start).cloned().unwrap_or_default();
            let mut seen: HashSet<String> = HashSet::new();
            while let Some(n) = stack.pop() {
                if n == *start {
                    self.cyclic_aliases.insert(start.clone());
                    break;
                }
                if seen.insert(n.clone()) {
                    if let Some(next) = deps.get(&n) {
                        stack.extend(next.iter().cloned());
                    }
                }
            }
        }
    }

    /// One-step expansion of a (possibly generic) alias head: substitute the
    /// alias's parameters with `args` in its target. `None` if `name` is not a
    /// (non-cyclic) alias. The caller recurses to expand nested alias heads.
    fn alias_target(&self, name: &str, args: &[Type]) -> Option<Type> {
        if self.cyclic_aliases.contains(name) {
            return None;
        }
        let (params, target) = self.aliases.get(name)?;
        let mut subst: HashMap<&str, &Type> = HashMap::new();
        for (p, a) in params.iter().zip(args.iter()) {
            subst.insert(p.as_str(), a);
        }
        Some(subst_type(target, &subst))
    }

    fn run(mut self) -> Result<Module, LowerError> {
        // First rewrite all non-generic top-level items (dropping the generic
        // declarations themselves), which seeds the instantiation worklist.
        let mut out: Vec<Item> = Vec::new();
        let src = std::mem::take(&mut self.src_items);
        for item in &src {
            match item {
                // Transparent aliases (#606) are expanded in place at every type
                // reference and emit no declaration of their own.
                Item::TypeAlias(_) => {}
                Item::Struct(s) if !s.generics.is_empty() => {}
                Item::Enum(e) if !e.generics.is_empty() => {}
                Item::Function(f) if !f.generics.is_empty() => {
                    // Generic free functions (I#161) are emitted on demand from
                    // call sites; one that is never instantiated is dropped.
                }
                Item::Struct(s) => out.push(Item::Struct(self.rewrite_struct_nongeneric(s)?)),
                Item::Enum(e) => out.push(Item::Enum(self.rewrite_enum_nongeneric(e)?)),
                Item::Function(f) => {
                    self.cur_subst.clear();
                    self.cur_self = None;
                    out.push(Item::Function(self.rewrite_fn(f, None)?))
                }
                // A module-level global binding (epic #661 / D-0112) is a
                // top-level `Stmt::Let`. Like a local `Stmt::Let` inside a
                // function body, its declared type must be rewritten — which
                // *requests* a user-generic instance (`Box[int64]` → `Box$int64`,
                // #690) — and its initializer expression monomorphized (so a
                // generic constructor `Box(7)` resolves to that instance). Before
                // #690 these items passed through unchanged, so a generic-typed
                // global was never instantiated and never resolved at lowering.
                // Rewritten with cleared ambient substitution (a global is never
                // under a type-parameter scope) and a fresh env seeded with the
                // module globals (so one global's initializer may name another),
                // mirroring `rewrite_fn_inner`.
                Item::Stmt(st) => {
                    self.cur_subst.clear();
                    self.cur_self = None;
                    let mut env = Env::default();
                    env.push();
                    for (name, ty) in &self.globals {
                        env.declare(name, CTy::Ty(ty.clone()));
                    }
                    out.push(Item::Stmt(self.rewrite_stmt(st, &mut env)?));
                }
                // An interface's method signatures carry type annotations that
                // must be rewritten exactly like a struct field or a function
                // signature (#1309): a return/parameter that names a generic
                // instance (`Optional[int64]`) is renamed to its mangled concrete
                // declaration (`Optional$int64`) and that instance is *requested*
                // from the worklist. Without this the interface passed through
                // verbatim, so IR lowering resolved the un-mangled `Optional[…]`
                // head (ignoring its type arguments) and the interface call's
                // result lost its `Optional` struct type — a later method call on
                // it (`reader.end_time().is_some()`) failed to lower with ZAP6001
                // "method call on a non-struct value". A plain interface (no
                // generic-instance types in its signatures) rewrites to itself.
                Item::Interface(i) => {
                    self.cur_subst.clear();
                    self.cur_self = None;
                    out.push(Item::Interface(self.rewrite_interface_nongeneric(i)));
                }
                other => out.push(other.clone()),
            }
        }
        // Drain the instantiation worklists to a fixpoint (instantiating one
        // generic may reference another generic type, function, or method),
        // emitting specialized declarations. Types are drained first so a
        // method's owner-type instance exists before its methods are attached.
        loop {
            if let Some((name, args)) = self.worklist.pop() {
                self.emit_instance(&name, &args)?;
            } else if let Some((name, args)) = self.func_worklist.pop() {
                self.emit_func_instance(&name, &args)?;
            } else if let Some(mi) = self.method_worklist.pop() {
                self.emit_method_instance(&mi)?;
            } else {
                break;
            }
        }
        // Append specialized declarations after the user's items so that any
        // non-generic type they reference is already declared earlier; the
        // lowering pass is order-tolerant (it pre-collects all type ids) so this
        // is for readability only.
        for s in std::mem::take(&mut self.spec_structs) {
            out.push(Item::Struct(s));
        }
        for e in std::mem::take(&mut self.spec_enums) {
            out.push(Item::Enum(e));
        }
        for f in std::mem::take(&mut self.spec_funcs) {
            out.push(Item::Function(f));
        }
        // Attach specialized generic methods (I#161) to their owner's concrete
        // declaration, now that every owner instance (generic or not) is emitted.
        let spec_methods = std::mem::take(&mut self.spec_methods);
        for (owner, methods) in spec_methods {
            let mut attached = false;
            for item in &mut out {
                match item {
                    Item::Struct(s) if s.name == owner => {
                        s.methods.extend(methods.iter().cloned());
                        attached = true;
                        break;
                    }
                    Item::Enum(e) if e.name == owner => {
                        e.methods.extend(methods.iter().cloned());
                        attached = true;
                        break;
                    }
                    _ => {}
                }
            }
            if !attached {
                return Err(err(format!(
                    "internal error: no concrete declaration '{owner}' to attach \
                     its specialized generic methods to (I#161)"
                )));
            }
        }
        Ok(Module {
            items: out,
            span: self.span,
        })
    }
}

// ---------------------------------------------------------------------------
// Mangling, substitution, and instantiation requests
// ---------------------------------------------------------------------------

impl Mono<'_> {
    /// Is `name` a user generic type (struct or enum)?
    fn is_generic(&self, name: &str) -> bool {
        self.gstructs.contains_key(name) || self.genums.contains_key(name)
    }

    /// Canonical mangled string for a concrete type (used both as an
    /// instantiation key component and to name specialized declarations). The
    /// scheme is `base$arg1$arg2…` (RUNTIME-ABI name-mangling, finalized for
    /// generics here); arguments mangle recursively. Primitive aliases are
    /// canonicalized so `int64`/`i64` collapse to one instance.
    fn mangle_type(&self, t: &Type) -> String {
        match t {
            Type::Named { name, args, .. } => {
                if args.is_empty() {
                    canon_prim(name).to_string()
                } else {
                    let inner: Vec<String> = args.iter().map(|a| self.mangle_type(a)).collect();
                    format!("{}${}", canon_prim(name), inner.join("$"))
                }
            }
            Type::Tuple { elems, .. } => {
                let inner: Vec<String> = elems.iter().map(|e| self.mangle_type(e)).collect();
                format!("tuple${}", inner.join("$"))
            }
            Type::Func { params, ret, .. } => {
                let ps: Vec<String> = params.iter().map(|p| self.mangle_type(p)).collect();
                let r = ret
                    .as_ref()
                    .map(|t| self.mangle_type(t))
                    .unwrap_or_else(|| "void".to_string());
                format!("fn${}$to${}", ps.join("$"), r)
            }
        }
    }

    /// The specialized declaration name for `generic[args]`.
    fn mangle_name(&self, generic: &str, args: &[Type]) -> String {
        let inner: Vec<String> = args.iter().map(|a| self.mangle_type(a)).collect();
        format!("{}${}", generic, inner.join("$"))
    }

    /// Apply the current type-parameter substitution to a type, producing a
    /// concrete *generic-form* type (type parameters replaced; generics not yet
    /// renamed to their mangled names). Used for inference.
    fn apply_subst(&self, t: &Type) -> Type {
        match t {
            Type::Named { name, args, span } => {
                // A module-qualified seam generic `concurrent.Queue[T]` is the
                // bare prelude `Queue[T]` (I#996, D-0202): normalize the dotted
                // head so inference (unifying a bare `Queue[T]` function return
                // against this annotated sink) and the later generic-instance
                // renaming both key on the bare prelude name.
                let name = zapc_syntax::seam::seam_generic_bare_type(name).unwrap_or(name.as_str());
                if args.is_empty() {
                    // An associated-type projection `T.Out` (#1207) resolves to
                    // the concrete binding computed for this instance, before the
                    // plain type-parameter substitution (so `T` alone still maps).
                    if let Some(bound) = self.cur_proj.get(name) {
                        return bound.clone();
                    }
                    if let Some(concrete) = self.cur_subst.get(name) {
                        return concrete.clone();
                    }
                }
                let rargs: Vec<Type> = args.iter().map(|a| self.apply_subst(a)).collect();
                // Expand a transparent alias to its target (#606), recursing so
                // nested alias heads and any type parameters also resolve.
                if let Some(expanded) = self.alias_target(name, &rargs) {
                    return self.apply_subst(&expanded);
                }
                Type::Named {
                    name: name.to_string(),
                    args: rargs,
                    span: *span,
                }
            }
            Type::Tuple { elems, span } => Type::Tuple {
                elems: elems.iter().map(|e| self.apply_subst(e)).collect(),
                span: *span,
            },
            Type::Func { params, ret, span } => Type::Func {
                params: params.iter().map(|p| self.apply_subst(p)).collect(),
                ret: ret.as_ref().map(|r| Box::new(self.apply_subst(r))),
                span: *span,
            },
        }
    }

    /// Record an instantiation request for `generic[args]` and return its mangled
    /// name. The args must already be concrete (substituted).
    fn request(&mut self, generic: &str, args: &[Type]) -> String {
        let mangled = self.mangle_name(generic, args);
        if self.requested.insert(mangled.clone()) {
            self.worklist.push((generic.to_string(), args.to_vec()));
        }
        mangled
    }

    /// The concrete declaration name for a type `generic[args]`: the plain name
    /// for a non-generic type (empty `args`), else the mangled instance name.
    fn instance_name(&self, generic: &str, args: &[Type]) -> String {
        if args.is_empty() {
            generic.to_string()
        } else {
            self.mangle_name(generic, args)
        }
    }

    /// Record a generic free-function instantiation `f[args]` (I#161) and return
    /// its specialized, mangled symbol. The args must already be concrete.
    fn request_func(&mut self, func: &str, args: &[Type]) -> String {
        let mangled = self.mangle_name(func, args);
        if self.requested_funcs.insert(mangled.clone()) {
            self.func_worklist.push((func.to_string(), args.to_vec()));
        }
        mangled
    }

    /// Record a generic-method instantiation `owner[owner_args].method[targs]`
    /// (I#161) and return the specialized *method* symbol (`method$arg…`) used at
    /// the call site. The owner type instance is requested too, so the concrete
    /// declaration the method attaches to is emitted. All args must be concrete.
    fn request_method(
        &mut self,
        owner: &str,
        owner_args: &[Type],
        method: &str,
        targs: &[Type],
    ) -> String {
        // Ensure the owner's concrete type instance exists (a generic owner is
        // instantiated; a non-generic owner is already emitted in pass 1).
        if !owner_args.is_empty() {
            self.request(owner, owner_args);
        }
        let owner_mangled = self.instance_name(owner, owner_args);
        let method_spec = self.mangle_name(method, targs);
        let key = format!("{owner_mangled}::{method_spec}");
        if self.requested_methods.insert(key) {
            self.method_worklist.push(MethodInst {
                owner: owner.to_string(),
                owner_args: owner_args.to_vec(),
                method: method.to_string(),
                targs: targs.to_vec(),
            });
        }
        method_spec
    }

    /// Rewrite a type annotation: apply the current substitution, then rename any
    /// user generic instantiation `G[args]` to its mangled concrete declaration
    /// name (requesting that instance), recursing into arguments. Built-in
    /// generics (`list`/`dict`/…) keep their head and only have their arguments
    /// rewritten (they stay special-cased, Fork X).
    fn rewrite_type(&mut self, t: &Type) -> Type {
        let t = self.apply_subst(t);
        self.rename_type(&t)
    }

    fn rename_type(&mut self, t: &Type) -> Type {
        match t {
            Type::Named { name, args, span } => {
                // Rename arguments first: a nested generic instance becomes its
                // mangled zero-arg name, whose `mangle_type` equals the concrete
                // form's, so the outer mangle key is stable either way.
                let rargs: Vec<Type> = args.iter().map(|a| self.rename_type(a)).collect();
                // A transparent alias is expanded before any generic-instance
                // renaming (#606): the expansion may itself name a generic type,
                // which then gets requested/mangled by recursing.
                if let Some(expanded) = self.alias_target(name, &rargs) {
                    return self.rename_type(&expanded);
                }
                if self.is_generic(name) && !args.is_empty() {
                    let mangled = self.request(name, &rargs);
                    Type::Named {
                        name: mangled,
                        args: Vec::new(),
                        span: *span,
                    }
                } else {
                    Type::Named {
                        name: name.clone(),
                        args: rargs,
                        span: *span,
                    }
                }
            }
            Type::Tuple { elems, span } => Type::Tuple {
                elems: elems.iter().map(|e| self.rename_type(e)).collect(),
                span: *span,
            },
            Type::Func { params, ret, span } => Type::Func {
                params: params.iter().map(|p| self.rename_type(p)).collect(),
                ret: ret.as_ref().map(|r| Box::new(self.rename_type(r))),
                span: *span,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Instantiation + declaration rewriting
// ---------------------------------------------------------------------------

impl Mono<'_> {
    /// Build the substitution map `param -> concrete arg` for a generic.
    fn subst_map(&self, generic: &str, args: &[Type]) -> HashMap<String, Type> {
        let mut map = HashMap::new();
        if let Some(ps) = self.params.get(generic) {
            for (p, a) in ps.iter().zip(args.iter()) {
                map.insert(p.clone(), a.clone());
            }
        }
        map
    }

    // -----------------------------------------------------------------------
    // Interface-bound satisfaction (I#164, slice 5)
    // -----------------------------------------------------------------------

    /// Enforce the interface bounds declared on a generic's type parameters at
    /// one concrete instantiation (I#164). `generics` are the declaration's type
    /// parameters (each optionally `T: Iface`); `args` are the concrete types
    /// substituted for them, in order. Each bounded parameter's concrete argument
    /// must structurally conform to the bound interface (the I#84 rule). The
    /// bound's methods then resolve statically per instance through the ordinary
    /// type substitution applied when the body is rewritten — no fat pointer /
    /// vtable is emitted (design/0012 slice 5). A single bound per parameter is
    /// supported (the grammar carries at most one; `T: A + B` is a follow-up).
    fn check_bounds(
        &self,
        what: &str,
        generics: &[TypeParam],
        args: &[Type],
    ) -> Result<(), LowerError> {
        // The instantiation's own substitution `param -> concrete arg` (#1206):
        // a parametrized bound's arguments may reference the generic's own type
        // parameters (`[T: RMul[f64, T]]`), so they are substituted to concrete
        // types before the bound is checked — `RMul[f64, T]` with `T := Matrix`
        // becomes `RMul[f64, Matrix]`.
        let self_subst: HashMap<&str, &Type> = generics
            .iter()
            .map(|p| p.name.as_str())
            .zip(args.iter())
            .collect();
        for (p, arg) in generics.iter().zip(args.iter()) {
            let Some(bound) = &p.bound else { continue };
            let Type::Named { name: iface, .. } = bound else {
                return Err(err(format!(
                    "type parameter '{}' of {what} has an invalid bound: a bound must name \
                     an interface",
                    p.name
                )));
            };
            // Substitute the generic's own parameters into the bound (its type
            // arguments and, for the diagnostic, the whole bound) before the
            // conformance check.
            let bound = subst_type(bound, &self_subst);
            let iface_args: &[Type] = match &bound {
                Type::Named { args, .. } => args,
                _ => &[],
            };
            if let Err(reasons) = self.satisfies_bound(arg, iface, iface_args) {
                // Name the bound with its (substituted) type arguments
                // (`Container[int64]` / `RMul[f64, Matrix]`) so the diagnostic
                // shows exactly what was required (#1205/#1206).
                let bound_name = type_name(&bound);
                return Err(err(format!(
                    "type '{}' does not satisfy the bound '{}' on type parameter '{}' of \
                     {what}: {}",
                    type_name(arg),
                    bound_name,
                    p.name,
                    reasons.join("; ")
                )));
            }
        }
        Ok(())
    }

    /// Does the concrete type `arg` satisfy the bound named `iface`
    /// (I#164/I#811)? Two kinds of bound are enforced here:
    ///
    ///   * A **capability interface** (`Equatable`/`Hashable`/`Comparable`,
    ///     D-0139) is satisfied *intrinsically* by the shape of `arg`, exactly as
    ///     a built-in container's element position is — a scalar, `str`/`bytes`,
    ///     `float`, tier-1 `enum`, auto-derived `value struct`, hashable tier-3
    ///     `enum`, and so on. This is what lets a user `struct BST[K: Comparable,
    ///     V]` accept `int`/`str`/enum/value-struct keys and reject the rest with
    ///     a capability-named reason (I#811).
    ///   * A **user `interface`** is satisfied structurally (D-0016/I#84): every
    ///     method of the interface *and of each refined supertype* (D-0139/#806)
    ///     must be present on the concrete type with a matching signature. Only
    ///     user structs and enums carry methods, so a primitive cannot satisfy a
    ///     user method-bearing interface; a refined *capability* supertype is
    ///     still checked intrinsically.
    ///
    /// Returns the human-readable reasons on failure (each a clause the
    /// [`Self::check_bounds`] caller joins after "does not satisfy the bound").
    fn satisfies_bound(
        &self,
        arg: &Type,
        iface: &str,
        iface_args: &[Type],
    ) -> Result<(), Vec<String>> {
        if let Some(cap) = Capability::from_iface(iface) {
            let mut visited = HashSet::new();
            return self.type_has_capability(arg, cap, &mut visited);
        }
        let Some(idef) = self.interfaces.get(iface) else {
            return Err(vec![format!("'{iface}' is not an interface")]);
        };
        // Generic-interface conformance (#1205): the interface's own type
        // parameters are substituted (`T := iface_args[i]`) in each of its
        // declared method signatures before the structural match, so a concrete
        // type conforms to `Container[int64]` iff it provides `Container`'s
        // methods with `T` replaced by `int64`. A non-generic bound carries no
        // arguments and this map stays empty (the pre-#1205 behavior). An arity
        // mismatch is reported here, at the instantiation that supplied them.
        if !iface_args.is_empty() && iface_args.len() != idef.generics.len() {
            return Err(vec![format!(
                "interface '{iface}' expects {} type argument(s), but the bound supplies {}",
                idef.generics.len(),
                iface_args.len()
            )]);
        }
        let iface_subst: HashMap<&str, &Type> = idef
            .generics
            .iter()
            .map(|g| g.name.as_str())
            .zip(iface_args.iter())
            .collect();
        let mut reasons = Vec::new();
        // The interface itself plus every refined supertype, transitively
        // (D-0139/#806): a `[T: C]` bound where `interface C: A` requires A's
        // methods too, and a refined *capability* supertype is satisfied
        // intrinsically rather than by method lookup.
        for origin in self.iface_closure(iface) {
            let attr = if origin == iface {
                String::new()
            } else {
                format!(" (required by supertype '{origin}')")
            };
            if let Some(cap) = Capability::from_iface(&origin) {
                let mut visited = HashSet::new();
                if let Err(rs) = self.type_has_capability(arg, cap, &mut visited) {
                    reasons.extend(rs.into_iter().map(|r| format!("{r}{attr}")));
                }
                continue;
            }
            let Some(idef) = self.interfaces.get(origin.as_str()) else {
                continue;
            };
            let methods: Option<&[Function]> = match arg {
                Type::Named { name, .. } => {
                    if let Some(s) = self.structs.get(name.as_str()) {
                        Some(&s.methods)
                    } else {
                        self.enums.get(name.as_str()).map(|e| e.methods.as_slice())
                    }
                }
                _ => None,
            };
            let Some(methods) = methods else {
                reasons.push(format!(
                    "'{}' is not a struct or enum and cannot implement interface methods{attr}",
                    type_name(arg)
                ));
                continue;
            };
            // Only the direct interface's method signatures are instantiated at
            // `iface_args` (#1205); a refined supertype carries its own, so its
            // signatures are matched verbatim (generic supertypes are deferred).
            let subst_here = origin == iface && !iface_subst.is_empty();
            // The direct interface's associated types are free variables the
            // conformer *binds* structurally (#1207): a method position typed as
            // one matches any concrete type (consistently). A supertype carries
            // none in the MVP.
            let assoc: Vec<&str> = if origin == iface {
                idef.assoc_types.iter().map(|a| a.name.as_str()).collect()
            } else {
                Vec::new()
            };
            for im in &idef.methods {
                let want = if subst_here {
                    subst_method_sig(im, &iface_subst)
                } else {
                    im.clone()
                };
                let im = &want;
                match methods.iter().find(|m| m.name == im.name) {
                    None => reasons.push(format!("missing method '{}'{attr}", im.name)),
                    Some(cm) if !method_conforms(cm, im, &assoc) => reasons.push(format!(
                        "method '{}' has a mismatched signature{attr}",
                        im.name
                    )),
                    Some(_) => {}
                }
            }
        }
        if reasons.is_empty() {
            Ok(())
        } else {
            Err(reasons)
        }
    }

    /// An interface name followed by its transitively refined supertype names, in
    /// a stable breadth-first order with duplicates removed (D-0139/#806).
    /// Capability supertypes (which have no AST `interface` item) are included by
    /// name so [`Self::satisfies_bound`] can route them to the intrinsic check.
    fn iface_closure(&self, iface: &str) -> Vec<String> {
        let mut out = vec![iface.to_string()];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i].clone();
            i += 1;
            if let Some(idef) = self.interfaces.get(cur.as_str()) {
                for sup in &idef.supertypes {
                    if let Type::Named { name, .. } = sup {
                        if !out.iter().any(|n| n == name) {
                            out.push(name.clone());
                        }
                    }
                }
            }
        }
        out
    }

    /// Does the concrete type `arg` satisfy the capability `cap` intrinsically
    /// (D-0139, I#811)? The rule set mirrors the element/key capability model the
    /// built-in containers already enforce (#807/#808/#809) so a user generic
    /// container admits exactly the element types a built-in one does. `visited`
    /// guards the (value-aggregate) recursion against cycles. On failure the
    /// returned clauses name the capability and the blocking reason — for a value
    /// aggregate, the field that is not conforming.
    fn type_has_capability(
        &self,
        arg: &Type,
        cap: Capability,
        visited: &mut HashSet<String>,
    ) -> Result<(), Vec<String>> {
        match arg {
            Type::Func { .. } => Err(vec![format!(
                "a function type does not implement '{}'",
                cap.name()
            )]),
            Type::Tuple { elems, .. } => {
                // A tuple is `Equatable`/`Hashable` iff every element is (CPython
                // parity, #680). It has no derived *ordering* yet, so a tuple does
                // not satisfy `Comparable`.
                if cap == Capability::Comparable {
                    return Err(vec![
                        "a tuple has no derived ordering, so it does not implement \
                         'Comparable' (a Comparable bound needs a scalar, a tier-1 enum, \
                         or a type providing '__cmp__' / '@derive(Ord)')"
                            .to_string(),
                    ]);
                }
                for e in elems {
                    if let Err(rs) = self.type_has_capability(e, cap, visited) {
                        return Err(rs
                            .into_iter()
                            .map(|r| format!("tuple element of type '{}': {r}", type_name(e)))
                            .collect());
                    }
                }
                Ok(())
            }
            Type::Named { name, args, .. } => self.named_has_capability(name, args, cap, visited),
        }
    }

    /// The [`Self::type_has_capability`] arm for a named type (scalar, built-in
    /// container, `Optional[T]`, or a user struct/enum).
    fn named_has_capability(
        &self,
        name: &str,
        args: &[Type],
        cap: Capability,
        visited: &mut HashSet<String>,
    ) -> Result<(), Vec<String>> {
        // `Optional[T]` (#704): `None` and a present value are both hashable and
        // equatable iff `T` is; there is no derived ordering yet (as for tuples).
        if name == "Optional" && args.len() == 1 {
            if cap == Capability::Comparable {
                return Err(vec![
                    "'Optional' has no derived ordering, so it does not implement 'Comparable'"
                        .to_string(),
                ]);
            }
            return self
                .type_has_capability(&args[0], cap, visited)
                .map_err(|rs| {
                    rs.into_iter()
                        .map(|r| format!("Optional payload: {r}"))
                        .collect()
                });
        }
        match canon_prim(name) {
            // Scalars have equality, a hash, and a total order intrinsically
            // (`float`: `-0.0` canonicalized, `NaN` handled per #803).
            "int64" | "int32" | "uint8" | "uint16" | "uint32" | "uint64" | "usize" | "bool"
            | "char" | "str" | "bytes" | "float64" | "float32" => Ok(()),
            // Representation gaps: no stable hash/order slot yet (the built-in
            // collections reject these too, ZAP4024).
            "bigint" | "complex" | "pyobject" => Err(vec![format!(
                "'{}' does not implement '{}' (no representation for it as a hashed/ordered \
                 element yet)",
                canon_prim(name),
                cap.name()
            )]),
            // Mutable collections have no stable identity, hash, or order.
            "list" | "dict" | "set" | "heap" | "channel" | "array" | "deque" | "oset" => {
                Err(vec![format!(
                    "a mutable collection ('{name}') does not implement '{}' (it has no stable \
                     hash or ordering)",
                    cap.name()
                )])
            }
            other => {
                if let Some(e) = self.enums.get(other) {
                    self.enum_has_capability(other, e, cap, visited)
                } else if let Some(s) = self.structs.get(other) {
                    self.struct_has_capability(other, s, cap, visited)
                } else {
                    Err(vec![format!(
                        "'{other}' is not known to implement '{}'",
                        cap.name()
                    )])
                }
            }
        }
    }

    /// Capability satisfaction for a user `enum` (D-0139/#809). A **tier-1**
    /// (constant) enum hashes/orders/equals by its backing integer intrinsically.
    /// A **tier-3** (data-carrying) enum is a tagged value aggregate: it is
    /// `Equatable`/`Hashable` iff it declares the matching dunder(s) (hand-written
    /// or `@derive`d) or — absent any such declaration — every payload field has
    /// the capability; ordering (`Comparable`) is never auto and requires an
    /// explicit `__cmp__` / `@derive(Ord)`.
    fn enum_has_capability(
        &self,
        name: &str,
        e: &EnumDef,
        cap: Capability,
        visited: &mut HashSet<String>,
    ) -> Result<(), Vec<String>> {
        let is_tier3 = e.variants.iter().any(|v| !v.fields.is_empty());
        if !is_tier3 {
            return Ok(()); // tier-1: intrinsic Eq/Hash/Comparable (D-0018/#809)
        }
        if cap == Capability::Comparable {
            return if provides_dunder(&e.methods, &e.decorators, "__cmp__", "Ord") {
                Ok(())
            } else {
                Err(vec![format!(
                    "tier-3 enum '{name}' does not implement 'Comparable': it has no '__cmp__' \
                     method and does not '@derive(Ord)' (ordering is never auto-derived)"
                )])
            };
        }
        let has_eq = provides_dunder(&e.methods, &e.decorators, "__eq__", "Eq");
        let has_hash = provides_dunder(&e.methods, &e.decorators, "__hash__", "Hash");
        self.aggregate_eq_hash(
            &format!("tier-3 enum '{name}'"),
            cap,
            has_eq,
            has_hash,
            e.variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| (f.name.as_str(), &f.ty))),
            visited,
        )
    }

    /// Capability satisfaction for a user `struct` (D-0139/#808). A struct that
    /// declares the matching dunder(s) (hand-written or `@derive`d) satisfies the
    /// capability. Otherwise a **value struct** auto-derives `Equatable`/`Hashable`
    /// iff every field has the capability; a **reference struct** never
    /// auto-derives (identity is its default, D-0023). Ordering requires an
    /// explicit `__cmp__` / `@derive(Ord)` for both.
    fn struct_has_capability(
        &self,
        name: &str,
        s: &StructDef,
        cap: Capability,
        visited: &mut HashSet<String>,
    ) -> Result<(), Vec<String>> {
        let kind = if s.is_value { "value struct" } else { "struct" };
        if cap == Capability::Comparable {
            return if provides_dunder(&s.methods, &s.decorators, "__cmp__", "Ord") {
                Ok(())
            } else {
                Err(vec![format!(
                    "{kind} '{name}' does not implement 'Comparable': it has no '__cmp__' method \
                     and does not '@derive(Ord)' (ordering is never auto-derived — it is a choice)"
                )])
            };
        }
        let has_eq = provides_dunder(&s.methods, &s.decorators, "__eq__", "Eq");
        let has_hash = provides_dunder(&s.methods, &s.decorators, "__hash__", "Hash");
        if !s.is_value {
            // A reference struct satisfies Eq/Hash only by declaring the dunder(s)
            // itself — no auto-derive (D-0023).
            let ok = match cap {
                Capability::Equatable => has_eq,
                Capability::Hashable => has_eq && has_hash,
                Capability::Comparable => unreachable!(),
            };
            return if ok {
                Ok(())
            } else {
                Err(vec![format!(
                    "{kind} '{name}' does not implement '{}': a reference struct does not \
                     auto-derive it — add the dunder(s) or '@derive(...)' (identity is the \
                     default, D-0023)",
                    cap.name()
                )])
            };
        }
        self.aggregate_eq_hash(
            &format!("{kind} '{name}'"),
            cap,
            has_eq,
            has_hash,
            s.fields.iter().map(|f| (f.name.as_str(), &f.ty)),
            visited,
        )
    }

    /// Shared `Equatable`/`Hashable` rule for a value aggregate (a `value struct`
    /// or a tier-3 `enum`, D-0139/#808/#809): a declared matching dunder set wins;
    /// otherwise, with neither `__eq__` nor `__hash__` declared, the aggregate
    /// auto-derives the capability iff every field/payload has it. A *partial*
    /// override (exactly one of `__eq__`/`__hash__`) blocks auto-derive and does
    /// not satisfy `Hashable`. `visited` guards against reference cycles.
    fn aggregate_eq_hash<'f>(
        &self,
        who: &str,
        cap: Capability,
        has_eq: bool,
        has_hash: bool,
        fields: impl Iterator<Item = (&'f str, &'f Type)>,
        visited: &mut HashSet<String>,
    ) -> Result<(), Vec<String>> {
        // A declared dunder set is authoritative.
        match cap {
            Capability::Equatable if has_eq => return Ok(()),
            Capability::Hashable if has_eq && has_hash => return Ok(()),
            _ => {}
        }
        // A partial override opts out of auto-derive.
        if has_eq || has_hash {
            return Err(vec![format!(
                "{who} does not implement '{}': it declares one of '__eq__'/'__hash__' but not \
                 both, which opts out of auto-derive (declare both, or neither)",
                cap.name()
            )]);
        }
        if !visited.insert(who.to_string()) {
            return Ok(()); // cycle guard (value aggregates cannot truly cycle)
        }
        let mut reasons = Vec::new();
        for (fname, fty) in fields {
            if let Err(rs) = self.type_has_capability(fty, cap, visited) {
                let detail = rs.into_iter().next().unwrap_or_default();
                reasons.push(format!(
                    "{who} does not implement '{}': field '{fname}' of type '{}' does not ({detail})",
                    cap.name(),
                    type_name(fty)
                ));
            }
        }
        visited.remove(who);
        if reasons.is_empty() {
            Ok(())
        } else {
            Err(reasons)
        }
    }

    /// Emit the specialized declaration for `generic[args]`.
    fn emit_instance(&mut self, generic: &str, args: &[Type]) -> Result<(), LowerError> {
        // Enforce any interface bounds on the type's parameters (I#164).
        if let Some(s) = self.gstructs.get(generic).copied() {
            self.check_bounds(&format!("type '{generic}'"), &s.generics, args)?;
        } else if let Some(e) = self.genums.get(generic).copied() {
            self.check_bounds(&format!("type '{generic}'"), &e.generics, args)?;
        }
        let mangled = self.mangle_name(generic, args);
        let subst = self.subst_map(generic, args);
        // Associated-type projections from the type's own bounded parameters
        // (#1207), so a `T.Out` in a field/method signature resolves.
        let type_generics: &[TypeParam] = if let Some(s) = self.gstructs.get(generic) {
            &s.generics
        } else if let Some(e) = self.genums.get(generic) {
            &e.generics
        } else {
            &[]
        };
        let proj = self.build_proj(type_generics, args);
        let saved_proj = std::mem::replace(&mut self.cur_proj, proj);
        // The receiver type used for inference is the *generic form*
        // (`Optional[int64]`), so method/variant/pattern lookups resolve against
        // the generic declaration and re-derive the instance; the specialized
        // struct name is applied during rename.
        let self_ty = Type::Named {
            name: generic.to_string(),
            args: args.to_vec(),
            span: self.span,
        };
        let saved_subst = std::mem::replace(&mut self.cur_subst, subst);
        let saved_self = self.cur_self.replace((generic.to_string(), args.to_vec()));

        if let Some(s) = self.gstructs.get(generic).copied() {
            let mut fields = Vec::with_capacity(s.fields.len());
            for f in &s.fields {
                fields.push(self.rewrite_field(f)?);
            }
            let mut methods = Vec::with_capacity(s.methods.len());
            for m in &s.methods {
                // Generic methods (their own type parameters) are specialized on
                // demand per call site (I#161), not emitted eagerly here.
                if m.generics.is_empty() {
                    methods.push(self.rewrite_fn(m, Some(self_ty.clone()))?);
                }
            }
            let spec = StructDef {
                decorators: s.decorators.clone(),
                name: mangled,
                generics: Vec::new(),
                fields,
                methods,
                is_value: s.is_value,
                span: s.span,
            };
            self.spec_structs.push(spec);
        } else if let Some(e) = self.genums.get(generic).copied() {
            let mut variants = Vec::with_capacity(e.variants.len());
            for v in &e.variants {
                let mut nv = v.clone();
                nv.fields = v
                    .fields
                    .iter()
                    .map(|vf| {
                        let mut nvf = vf.clone();
                        nvf.ty = self.rewrite_type(&vf.ty);
                        nvf
                    })
                    .collect();
                variants.push(nv);
            }
            let mut methods = Vec::with_capacity(e.methods.len());
            for m in &e.methods {
                // Generic enum methods are specialized on demand (I#161).
                if m.generics.is_empty() {
                    methods.push(self.rewrite_fn(m, Some(self_ty.clone()))?);
                }
            }
            let spec = EnumDef {
                decorators: e.decorators.clone(),
                name: mangled,
                generics: Vec::new(),
                variants,
                methods,
                span: e.span,
            };
            self.spec_enums.push(spec);
        } else {
            return Err(err(format!("'{generic}' is not a generic type")));
        }

        self.cur_subst = saved_subst;
        self.cur_self = saved_self;
        self.cur_proj = saved_proj;
        Ok(())
    }

    /// Rewrite a struct field under the ambient type-parameter substitution
    /// (`cur_subst`/`cur_self`): rename any generic type it references, and —
    /// crucially — monomorphize its per-instance **default initializer** (#1152).
    ///
    /// A field default (D-0038) is a value expression evaluated fresh in the
    /// constructor, so it must be specialized exactly like a function body or a
    /// parameter default: a generic constructor in the default
    /// (`Atomic[int64] state = Atomic(0)`) is rewritten to its mangled instance
    /// (`Atomic$int64(0)`), driven by the field's declared instance type as the
    /// expected-type sink (so an arg-less or inference-ambiguous generic
    /// constructor still picks the right instance, mirroring the `Stmt::Let`
    /// rule). Rewriting only the field *type* — as the field loops did before
    /// #1152 — left the bare generic name in the default, which then reached IR
    /// lowering and failed as "call to unknown function 'Atomic'". A computed
    /// default supplied via `@default(expr)` carries the same kind of
    /// initializer and is rewritten too.
    ///
    /// The default is evaluated with no locals in scope; the environment is
    /// seeded only with module globals (epic #661 / D-0112), matching the
    /// parameter-default rewrite, so a default may reference a `const` global.
    fn rewrite_field(&mut self, f: &ast::Field) -> Result<ast::Field, LowerError> {
        let mut nf = f.clone();
        nf.ty = self.rewrite_type(&f.ty);
        // Expected-type sink for the default: the field's declared type under
        // the current substitution (concrete for a non-generic owner; the
        // instantiated type for a generic owner).
        let expected = self.apply_subst(&f.ty);
        if let Some(d) = &f.default {
            let mut env = self.field_default_env();
            nf.default = Some(self.rewrite_expr(d, &mut env, Some(&expected))?.0);
        }
        for dec in &mut nf.decorators {
            if dec.name == "default" {
                let mut env = self.field_default_env();
                for a in &mut dec.args {
                    let rewritten = self.rewrite_expr(&a.value, &mut env, Some(&expected))?.0;
                    a.value = rewritten;
                }
            }
        }
        Ok(nf)
    }

    /// A fresh rewrite environment for a field-default initializer: one scope
    /// seeded with the module globals (epic #661 / D-0112), so a default may
    /// reference a `const` global, mirroring the parameter-default rewrite in
    /// [`Self::rewrite_fn_inner`]. No `self`, parameters, or locals are in scope
    /// for a field default.
    fn field_default_env(&self) -> Env {
        let mut env = Env::default();
        env.push();
        for (name, ty) in &self.globals {
            env.declare(name, CTy::Ty(ty.clone()));
        }
        env
    }

    /// Rewrite a non-generic struct declaration: rename any generic types its
    /// fields reference, and rewrite its method bodies.
    fn rewrite_struct_nongeneric(&mut self, s: &StructDef) -> Result<StructDef, LowerError> {
        let self_ty = Type::Named {
            name: s.name.clone(),
            args: Vec::new(),
            span: s.span,
        };
        self.cur_subst.clear();
        self.cur_self = Some((s.name.clone(), Vec::new()));
        let mut fields = Vec::with_capacity(s.fields.len());
        for f in &s.fields {
            fields.push(self.rewrite_field(f)?);
        }
        let mut methods = Vec::with_capacity(s.methods.len());
        for m in &s.methods {
            // A generic method on a non-generic type is specialized on demand
            // and attached later (I#161).
            if m.generics.is_empty() {
                methods.push(self.rewrite_fn(m, Some(self_ty.clone()))?);
            }
        }
        self.cur_self = None;
        Ok(StructDef {
            decorators: s.decorators.clone(),
            name: s.name.clone(),
            generics: Vec::new(),
            fields,
            methods,
            is_value: s.is_value,
            span: s.span,
        })
    }

    /// Rewrite a non-generic enum declaration (rename generic references in
    /// variant field types; rewrite method bodies).
    fn rewrite_enum_nongeneric(&mut self, e: &EnumDef) -> Result<EnumDef, LowerError> {
        let self_ty = Type::Named {
            name: e.name.clone(),
            args: Vec::new(),
            span: e.span,
        };
        self.cur_subst.clear();
        self.cur_self = Some((e.name.clone(), Vec::new()));
        let mut variants = Vec::with_capacity(e.variants.len());
        for v in &e.variants {
            let mut nv = v.clone();
            nv.fields = v
                .fields
                .iter()
                .map(|vf| {
                    let mut nvf = vf.clone();
                    nvf.ty = self.rewrite_type(&vf.ty);
                    nvf
                })
                .collect();
            variants.push(nv);
        }
        let mut methods = Vec::with_capacity(e.methods.len());
        for m in &e.methods {
            // A generic method on a non-generic enum is specialized on demand (I#161).
            if m.generics.is_empty() {
                methods.push(self.rewrite_fn(m, Some(self_ty.clone()))?);
            }
        }
        self.cur_self = None;
        Ok(EnumDef {
            decorators: e.decorators.clone(),
            name: e.name.clone(),
            generics: Vec::new(),
            variants,
            methods,
            span: e.span,
        })
    }

    /// Rewrite a *non-generic* function/method (its own type-parameter list is
    /// empty). `self_ty` is the concrete (generic-form) type of the `self`
    /// receiver for a method, `None` for a free function. Generic functions and
    /// methods are instead specialized on demand through [`Self::rewrite_fn_inner`]
    /// (I#161).
    /// Rewrite a non-generic interface's method signatures (#1309).
    ///
    /// An interface declaration carries type annotations in each method's
    /// parameters and return, exactly like a struct's fields or a function's
    /// signature — and they must be rewritten the same way, so a generic
    /// instance named there (`Optional[int64]`) is renamed to its mangled
    /// concrete declaration (`Optional$int64`) and that instance is requested
    /// from the worklist. Before #1309 the interface was emitted verbatim, so IR
    /// lowering resolved the un-mangled `Optional[…]` head while ignoring its
    /// type arguments: the interface call's result lost its concrete `Optional`
    /// struct type and a method call on it failed to lower (`ZAP6001`,
    /// "method call on a non-struct value").
    ///
    /// Only a **non-generic** interface is rewritten. A generic interface's
    /// method signatures mention the interface's own type parameters (`T`), which
    /// `rewrite_type` cannot distinguish from a nullary generic to instantiate;
    /// rewriting one would wrongly request a bogus `Optional$T`-style instance.
    /// Generic interfaces are therefore passed through unchanged, preserving the
    /// prior behavior (the caller clears the ambient substitution, so this never
    /// runs under a type-parameter scope). The `self` parameter and the
    /// supertype / associated-type lists need no renaming (an interface name is
    /// never a generic instance, and an associated type carries no annotation).
    fn rewrite_interface_nongeneric(&mut self, i: &InterfaceDef) -> InterfaceDef {
        if !i.generics.is_empty() {
            return i.clone();
        }
        let methods = i
            .methods
            .iter()
            .map(|m| {
                let params = m
                    .params
                    .iter()
                    .map(|p| {
                        let mut np = p.clone();
                        if p.name != "self" {
                            np.ty = p.ty.as_ref().map(|t| self.rewrite_type(t));
                        }
                        np
                    })
                    .collect();
                MethodSig {
                    name: m.name.clone(),
                    params,
                    ret: m.ret.as_ref().map(|t| self.rewrite_type(t)),
                    span: m.span,
                }
            })
            .collect();
        InterfaceDef {
            name: i.name.clone(),
            generics: i.generics.clone(),
            assoc_types: i.assoc_types.clone(),
            supertypes: i.supertypes.clone(),
            methods,
            span: i.span,
        }
    }

    fn rewrite_fn(&mut self, f: &Function, self_ty: Option<Type>) -> Result<Function, LowerError> {
        self.rewrite_fn_inner(f, self_ty, None)
    }

    /// Rewrite a function/method body under the ambient type-parameter
    /// substitution (`cur_subst`). When specializing a generic function/method
    /// (I#161), `cur_subst` already maps the function's own type parameters (and,
    /// for a method, the owner's) to concrete types and `out_name` gives the
    /// specialized, mangled symbol; the emitted declaration always has an empty
    /// generic list. For a non-generic declaration `out_name` is `None` and the
    /// original name is kept.
    fn rewrite_fn_inner(
        &mut self,
        f: &Function,
        self_ty: Option<Type>,
        out_name: Option<String>,
    ) -> Result<Function, LowerError> {
        let saved_ret = self.cur_ret.take();
        self.cur_ret = f.ret.as_ref().map(|t| self.apply_subst(t));

        let mut env = Env::default();
        // Outermost scope: module-level globals (epic #661 / D-0112). Seeding
        // them here — below the function's own scope — lets a receiver that is a
        // bare module-global name resolve its declared type for receiver-type
        // specialization (issue #671), while a local or parameter of the same
        // name still shadows the global (inner scopes are searched first).
        env.push();
        for (name, ty) in &self.globals {
            env.declare(name, CTy::Ty(ty.clone()));
        }
        env.push();
        if let Some(st) = &self_ty {
            env.declare("self", CTy::Ty(st.clone()));
        }
        let mut params = Vec::with_capacity(f.params.len());
        for p in &f.params {
            let mut np = p.clone();
            // A *method* `self` receiver is bound above via `self_ty` and its type
            // is implicit, so it is left untouched. But a **free function** may
            // have an ordinary parameter literally named `self` (`self_ty` is
            // `None`): it must be type-substituted and declared like any other
            // parameter, or a generic `def g[T](T self)` keeps `self: T`
            // unsubstituted and `reflect.field(self, …)` lowers against a
            // non-struct value (#1324).
            let is_method_receiver = p.name == "self" && self_ty.is_some();
            if !is_method_receiver {
                if let Some(t) = &p.ty {
                    env.declare(&p.name, CTy::Ty(self.apply_subst(t)));
                    np.ty = Some(self.rewrite_type(t));
                }
            }
            if let Some(d) = &p.default {
                np.default = Some(self.rw(d, &mut env)?);
            }
            params.push(np);
        }
        let ret = f.ret.as_ref().map(|t| self.rewrite_type(t));
        let body = self.rewrite_block(&f.body, &mut env)?;
        env.pop();
        env.pop();

        self.cur_ret = saved_ret;
        Ok(Function {
            decorators: f.decorators.clone(),
            name: out_name.unwrap_or_else(|| f.name.clone()),
            generics: Vec::new(),
            params,
            ret,
            body,
            span: f.span,
        })
    }
}

// ---------------------------------------------------------------------------
// Generic function / method specialization (I#161)
// ---------------------------------------------------------------------------

impl<'a> Mono<'a> {
    /// The associated-type projections contributed by `generics`/`args` at an
    /// instantiation (#1207): for each type parameter bounded on a generic
    /// interface that declares associated types, map `"<param>.<assoc>"` to the
    /// concrete argument's binding. Empty when no bound contributes.
    fn build_proj(&self, generics: &[TypeParam], args: &[Type]) -> HashMap<String, Type> {
        let mut proj = HashMap::new();
        for (p, arg) in generics.iter().zip(args.iter()) {
            let Some(Type::Named { name: iface, .. }) = &p.bound else {
                continue;
            };
            let Some(idef) = self.interfaces.get(iface.as_str()) else {
                continue;
            };
            for at in &idef.assoc_types {
                if let Some(binding) = self.assoc_binding(idef, &at.name, arg) {
                    proj.insert(format!("{}.{}", p.name, at.name), binding);
                }
            }
        }
        proj
    }

    /// The concrete binding of associated type `assoc` of interface `idef` on the
    /// concrete type `arg` (#1207): find the interface's *producer* method — the
    /// one whose declared return type is exactly `assoc` — and read the concrete
    /// type's method of that name's return type. `None` when the interface has no
    /// such producer method or the concrete type does not provide it (the
    /// non-conformance is reported separately by the bound check).
    fn assoc_binding(&self, idef: &InterfaceDef, assoc: &str, arg: &Type) -> Option<Type> {
        let producer = idef.methods.iter().find(|m| {
            matches!(&m.ret, Some(Type::Named { name, args, .. }) if name == assoc && args.is_empty())
        })?;
        let Type::Named { name: cname, .. } = arg else {
            return None;
        };
        self.find_method(cname, &producer.name)?.ret.clone()
    }

    /// Find a method named `name` declared on the type `owner` (generic or not).
    fn find_method(&self, owner: &str, name: &str) -> Option<&'a Function> {
        if let Some(s) = self.structs.get(owner) {
            if let Some(m) = s.methods.iter().find(|m| m.name == name) {
                return Some(m);
            }
        }
        if let Some(e) = self.enums.get(owner) {
            if let Some(m) = e.methods.iter().find(|m| m.name == name) {
                return Some(m);
            }
        }
        None
    }

    /// Emit the specialized body of a generic free function `func[args]` (I#161):
    /// substitute its type parameters, rewrite the body, and record it under its
    /// mangled symbol.
    fn emit_func_instance(&mut self, func: &str, args: &[Type]) -> Result<(), LowerError> {
        let f = self
            .gfuncs
            .get(func)
            .copied()
            .ok_or_else(|| err(format!("'{func}' is not a generic function")))?;
        // Enforce any interface bounds on the function's type parameters (I#164).
        self.check_bounds(&format!("function '{func}'"), &f.generics, args)?;
        let mangled = self.mangle_name(func, args);
        let mut subst = HashMap::new();
        for (p, a) in f.generics.iter().zip(args.iter()) {
            subst.insert(p.name.clone(), a.clone());
        }
        let proj = self.build_proj(&f.generics, args);
        let saved_subst = std::mem::replace(&mut self.cur_subst, subst);
        let saved_proj = std::mem::replace(&mut self.cur_proj, proj);
        let saved_self = self.cur_self.take();
        // If this is a `@derivable` generator being specialized at a concrete
        // type, record the derive context (trait + type) so a comptime failure in
        // its `comptime for` descent renders as a consumer `ZAP7003` (E3d, #1293).
        let derive_ctx = self
            .derivable_generators
            .get(func)
            .and_then(|tr| args.first().map(|ty| (tr.clone(), type_name(ty))));
        let saved_derive = std::mem::replace(&mut self.cur_derive, derive_ctx);
        let spec = self.rewrite_fn_inner(f, None, Some(mangled));
        self.cur_subst = saved_subst;
        self.cur_proj = saved_proj;
        self.cur_self = saved_self;
        self.cur_derive = saved_derive;
        self.spec_funcs.push(spec?);
        Ok(())
    }

    /// Emit the specialized body of a generic method and queue it for attachment
    /// to its owner's concrete declaration (I#161). The substitution maps the
    /// owner's type parameters (for a generic owner) and the method's own type
    /// parameters to concrete types; `self` keeps the generic-form receiver type
    /// so the body's `Self.Variant` / field / method references re-derive the
    /// owner instance exactly as a non-generic method body does.
    fn emit_method_instance(&mut self, mi: &MethodInst) -> Result<(), LowerError> {
        let method = self.find_method(&mi.owner, &mi.method).ok_or_else(|| {
            err(format!(
                "type '{}' has no method '{}' (I#161)",
                mi.owner, mi.method
            ))
        })?;
        // Enforce any interface bounds on the method's own type parameters
        // (I#164). The owner type's bounds are enforced when its instance is
        // emitted (requested from the call site via `request_method`).
        self.check_bounds(
            &format!("method '{}.{}'", mi.owner, mi.method),
            &method.generics,
            &mi.targs,
        )?;
        let method_spec = self.mangle_name(&mi.method, &mi.targs);
        let mut subst = self.subst_map(&mi.owner, &mi.owner_args);
        for (p, a) in method.generics.iter().zip(mi.targs.iter()) {
            subst.insert(p.name.clone(), a.clone());
        }
        let self_ty = Type::Named {
            name: mi.owner.clone(),
            args: mi.owner_args.clone(),
            span: self.span,
        };
        // Associated-type projections from the owner's bounded parameters and the
        // method's own (#1207).
        let owner_generics: &[TypeParam] = if let Some(s) = self.gstructs.get(mi.owner.as_str()) {
            &s.generics
        } else if let Some(e) = self.genums.get(mi.owner.as_str()) {
            &e.generics
        } else {
            &[]
        };
        let mut proj = self.build_proj(owner_generics, &mi.owner_args);
        proj.extend(self.build_proj(&method.generics, &mi.targs));
        let saved_subst = std::mem::replace(&mut self.cur_subst, subst);
        let saved_proj = std::mem::replace(&mut self.cur_proj, proj);
        let saved_self = self
            .cur_self
            .replace((mi.owner.clone(), mi.owner_args.clone()));
        let spec = self.rewrite_fn_inner(method, Some(self_ty), Some(method_spec));
        self.cur_subst = saved_subst;
        self.cur_proj = saved_proj;
        self.cur_self = saved_self;
        let owner_mangled = self.instance_name(&mi.owner, &mi.owner_args);
        self.spec_methods
            .entry(owner_mangled)
            .or_default()
            .push(spec?);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Statement / expression / pattern rewriting (with the focused inferer)
// ---------------------------------------------------------------------------

impl Mono<'_> {
    fn rewrite_block(&mut self, b: &Block, env: &mut Env) -> Result<Block, LowerError> {
        let mut stmts = Vec::with_capacity(b.stmts.len());
        for st in &b.stmts {
            // `comptime for` (E2/E3 surface, #1298): unroll it in place, splicing
            // the specialized per-field/variant statements (a block-level rewrite,
            // since unrolling yields many statements from one). If the iterable's
            // type cannot be resolved to a concrete declaration here (e.g. a
            // still-generic `reflect.type_info(T)`), fall through to the ordinary
            // rewrite, which leaves the node for the lowering backstop.
            if let Stmt::ComptimeFor {
                target, iter, body, ..
            } = st
            {
                if let Some(unrolled) = self.try_unroll_comptime_for(target, iter, body)? {
                    for u in &unrolled {
                        stmts.push(self.rewrite_stmt(u, env)?);
                    }
                    continue;
                }
            }
            // #1394: inside a specialized generic (a derive generator), an `if`
            // whose condition chain is a comptime constant — a
            // `reflect.type_info(T).kind` kind dispatch — is pruned to its taken
            // branch, so one generator can dispatch on `T`'s kind at its top
            // level and the dead-kind branch (e.g. an enum comptime `match` when
            // `T` is a struct) never reaches lowering. Gated on an active
            // substitution so ordinary (non-generic) code is untouched; a
            // non-comptime condition stays a runtime `if`.
            if !self.cur_subst.is_empty() {
                if let Stmt::If {
                    branches,
                    else_block,
                    ..
                } = st
                {
                    let type_subst = self.type_subst_names();
                    if let Some(taken) = comptime_for::select_comptime_if(
                        branches,
                        else_block,
                        &self.comptime_registry,
                        type_subst,
                    )? {
                        // Rewrite the taken branch through the block path so any
                        // nested `comptime for` / `comptime if` inside it is
                        // unrolled/pruned too, then splice its statements in.
                        let taken_block = Block {
                            stmts: taken.to_vec(),
                            span: b.span,
                        };
                        let rewritten = self.rewrite_block(&taken_block, env)?;
                        stmts.extend(rewritten.stmts);
                        continue;
                    }
                }
            }
            stmts.push(self.rewrite_stmt(st, env)?);
        }
        Ok(Block {
            stmts,
            span: b.span,
        })
    }

    /// Try to unroll a `comptime for <loop_var> in reflect.type_info(T).<member>:`
    /// (#1298). Returns the specialized statements when `T` names a concrete
    /// `struct`/`enum` in the (post-substitution) module and `member` is
    /// `fields`/`variants`; `None` when the shape or the type is not resolvable
    /// here (left for a later pass / the lowering backstop). Drives the E2
    /// engine through the `comptime_for` bridge (unroll #1251 + `reflect.field`
    /// desugar #1252).
    /// The active type-parameter substitution as parameter-name → concrete
    /// type-name pairs (#1394), for the comptime evaluator to resolve
    /// `reflect.type_info(T)` when pruning a generator's kind-dispatch `if`. Only
    /// `Named` concrete types are carried (a container/tuple instantiation is not
    /// a nominal kind-dispatch target).
    fn type_subst_names(&self) -> HashMap<String, String> {
        self.cur_subst
            .iter()
            .filter_map(|(k, v)| match v {
                Type::Named { name, .. } => Some((k.clone(), name.clone())),
                _ => None,
            })
            .collect()
    }

    fn try_unroll_comptime_for(
        &self,
        target: &Expr,
        iter: &Expr,
        body: &Block,
    ) -> Result<Option<Vec<Stmt>>, LowerError> {
        // `target` must be a single loop-variable name.
        let Expr::Name(loop_var, _) = target else {
            return Ok(None);
        };
        // `iter` must be `reflect.type_info(<TypeName>).<member>`.
        let Expr::Attr {
            base, name: member, ..
        } = iter
        else {
            return Ok(None);
        };
        let Some(type_name) = reflect_type_info_arg(base) else {
            return Ok(None);
        };
        // Resolve a generic type parameter to its concrete type (#1291, E3b):
        // inside a specialized generic generator — e.g. `def ser[T](...)` with a
        // `comptime for f in reflect.type_info(T).fields:` body — `T` is a type
        // parameter that the active substitution (`cur_subst`) maps to the
        // concrete type of this instantiation. A bare concrete type name is left
        // as-is. This is what makes a real generic `@derive` generator unroll
        // per instantiation rather than only a hand-written concrete type.
        let type_name = match self.cur_subst.get(&type_name) {
            Some(Type::Named { name, .. }) => name.clone(),
            _ => type_name,
        };
        match member.as_str() {
            "fields" => match self.structs.get(type_name.as_str()) {
                Some(def) => {
                    let out = comptime_for::unroll_struct_fields_with_access(
                        def,
                        loop_var,
                        body,
                        &[],
                        Some(&self.comptime_registry),
                    )
                    .map_err(|e| self.enrich_derive_error(e, &def.fields, def.span))?;
                    Ok(Some(out))
                }
                None => Ok(None),
            },
            "variants" => match self.enums.get(type_name.as_str()) {
                Some(def) => {
                    // Flatten the variant payload fields so a derive failure the
                    // generator points at (`comptime.error(f.span, …)` for a
                    // variant payload) is attributed to that field in the
                    // `required because of …` chain (#1375, E3d).
                    let payload_fields = variant_payload_fields(def);
                    let out = comptime_for::unroll_enum_variants(
                        def,
                        loop_var,
                        body,
                        &[],
                        Some(&self.comptime_registry),
                    )
                    .map_err(|e| self.enrich_derive_error(e, &payload_fields, def.span))?;
                    Ok(Some(out))
                }
                None => Ok(None),
            },
            // `.fields`/`.variants` are the two comptime sequences a `comptime
            // for` iterates; anything else is left for a later pass.
            _ => Ok(None),
        }
    }

    /// Expand a `case comptime <v> in reflect.type_info(T).variants [as payload]:`
    /// arm into one ordinary variant-pattern arm per variant of the concrete enum
    /// (#1374, epic #1371). Each generated arm binds the variant's payload fields
    /// positionally to the canonical captures the comptime-specialized body
    /// references (`reflect.field(payload, i)` → the i-th capture), so the whole
    /// lowers through the existing enum-match machinery — the runtime
    /// discrimination is reused, not re-invented. Exhaustive by construction (one
    /// arm per variant).
    #[allow(clippy::too_many_arguments)]
    fn expand_comptime_variant_arms(
        &mut self,
        loop_var: &str,
        iter: &Expr,
        payload: Option<&str>,
        body: &Block,
        sty: Option<&Type>,
        cv_span: Span,
        env: &mut Env,
    ) -> Result<Vec<ast::MatchArm>, LowerError> {
        // The iterator must be `reflect.type_info(T).variants`; resolve `T` to the
        // concrete enum via the active substitution (as the `comptime for` path
        // does for a generic generator).
        let Expr::Attr {
            base, name: member, ..
        } = iter
        else {
            return Err(err(
                "`case comptime <v> in …` expects `reflect.type_info(T).variants`",
            ));
        };
        if member != "variants" {
            return Err(err(format!(
                "`case comptime <v> in …` iterates `.variants`, found `.{member}`"
            )));
        }
        let Some(type_name) = reflect_type_info_arg(base) else {
            return Err(err(
                "`case comptime <v> in reflect.type_info(T).variants` expects a \
                 `reflect.type_info(T)` iterator",
            ));
        };
        let type_name = match self.cur_subst.get(&type_name) {
            Some(Type::Named { name, .. }) => name.clone(),
            _ => type_name,
        };
        let Some(def) = self.enums.get(type_name.as_str()).copied() else {
            return Err(err(format!(
                "a comptime-variant `match` requires `{type_name}` to be an enum in scope \
                 (`reflect.type_info({type_name}).variants`)"
            )));
        };
        // Comptime-specialize the shared arm body once per variant (binding the
        // loop variable to each variant record; the payload handle lets
        // `reflect.field` resolve to the per-field captures).
        let bodies = comptime_for::specialize_variant_arms(
            def,
            loop_var,
            payload,
            body,
            &[],
            Some(&self.comptime_registry),
        )?;
        let mut arms = Vec::with_capacity(def.variants.len());
        for (variant, sbody) in def.variants.iter().zip(bodies) {
            // One variant pattern binding the payload fields positionally to the
            // canonical captures the specialized body references; a unit variant
            // carries no parentheses.
            let pattern = if variant.fields.is_empty() {
                Pattern::Struct {
                    path: vec![type_name.clone(), variant.name.clone()],
                    args: None,
                    span: cv_span,
                }
            } else {
                let pargs = (0..variant.fields.len())
                    .map(|i| PatArg {
                        name: None,
                        pattern: Pattern::Capture {
                            name: zapc_comptime::payload_capture_name(i),
                            span: cv_span,
                        },
                        span: cv_span,
                    })
                    .collect();
                Pattern::Struct {
                    path: vec![type_name.clone(), variant.name.clone()],
                    args: Some(pargs),
                    span: cv_span,
                }
            };
            env.push();
            let rpat = self.rewrite_pattern(&pattern, sty, env)?;
            let rbody = self.rewrite_block(&sbody, env)?;
            env.pop();
            arms.push(ast::MatchArm {
                pattern: rpat,
                guard: None,
                body: rbody,
                span: cv_span,
            });
        }
        Ok(arms)
    }

    /// Attach the active `@derive` context (trait + type) to a comptime failure
    /// raised during a generator's `comptime for` descent, so the driver renders
    /// it as a consumer `ZAP7003` "cannot derive `trait` for `type`" (E3d, #1293).
    /// A no-op when there is no derive context (a plain `comptime for` outside a
    /// derive) or the error carries no derive payload. `fields` are the enclosing
    /// type's fields, used to label the backtrace frame at the offending field
    /// (matched by the cause span).
    fn enrich_derive_error(
        &self,
        mut le: LowerError,
        fields: &[ast::Field],
        instantiation_span: Span,
    ) -> LowerError {
        if let (Some(di), Some((tr, ty))) = (le.derive.as_mut(), self.cur_derive.as_ref()) {
            di.trait_and_type = Some((tr.clone(), ty.clone()));
            // The derive site (the type's declaration) is where a generator fault
            // is "instantiated" — cited as the second span of the dual-span
            // generator-fault diagnostic (E3d, #1293).
            di.instantiation_span = Some(instantiation_span);
            le.code = zapc_diag::ErrorCode::ZAP7003;
            if di.frames.is_empty() {
                // Build the `required because of …` chain for the field the
                // generator pointed at. The cause span is either the field
                // itself (`comptime.error(f.span, …)`) or — now that nested
                // types carry use-site spans (#1346) — a type *inside* the
                // field's declared type (the `Widget` element of
                // `list[Widget] items`, via `comptime.error(f.ty.elem.span, …)`).
                // Match the field whose declaration contains the cause span,
                // then walk its declared type from the field inward, emitting one
                // frame per container level crossed — the multi-level chain
                // (#1293). Frames are outermost-first; the renderer prints them
                // inner-first.
                if let Some(f) = fields
                    .iter()
                    .find(|f| f.span == di.cause_span || span_contains(f.ty.span(), di.cause_span))
                {
                    di.frames
                        .push((format!("field `{}` of `{}`", f.name, ty), f.span));
                    push_container_frames(&mut di.frames, &f.ty, di.cause_span);
                }
            }
        }
        le
    }

    /// Rewrite an expression, returning the rewritten node only.
    fn rw(&mut self, e: &Expr, env: &mut Env) -> Result<Expr, LowerError> {
        Ok(self.rewrite_expr(e, env, None)?.0)
    }

    fn rewrite_stmt(&mut self, st: &Stmt, env: &mut Env) -> Result<Stmt, LowerError> {
        Ok(match st {
            // `when target.*` blocks are cfg-resolved away before
            // monomorphization (the driver and sema both call
            // `zapc_syntax::cfg::resolve_cfg`), so one appearing here is an
            // internal invariant violation.
            Stmt::Cfg(_) => {
                return Err(err(
                    "internal error: unresolved `when target.*` block reached \
                     monomorphization (expected cfg resolution first)",
                ));
            }
            Stmt::Let {
                is_const,
                ty,
                name,
                value,
                span,
            } => {
                // The declared type is the expected-type sink (Fork I): it lets a
                // bare `None` or an arg-less generic constructor pick its instance.
                let expected = ty.as_ref().map(|t| self.apply_subst(t));
                let (value, vcty) = self.rewrite_expr(value, env, expected.as_ref())?;
                let decl_cty = match &ty {
                    Some(t) => CTy::Ty(self.apply_subst(t)),
                    None => vcty,
                };
                env.declare(name, decl_cty);
                Stmt::Let {
                    is_const: *is_const,
                    ty: ty.as_ref().map(|t| self.rewrite_type(t)),
                    name: name.clone(),
                    value,
                    span: *span,
                }
            }
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                let expected = cty_type(self.infer(target, env));
                let (value, _) = self.rewrite_expr(value, env, expected.as_ref())?;
                Stmt::Assign {
                    target: self.rw(target, env)?,
                    value,
                    span: *span,
                }
            }
            Stmt::AssignChain {
                targets,
                value,
                span,
            } => {
                // Each target drives the expected type of the one evaluated RHS;
                // rewrite the value against the first target's type (the targets
                // are mutually assignable or sema already erred) and rewrite the
                // targets themselves.
                let expected = targets.first().and_then(|t| cty_type(self.infer(t, env)));
                let (value, _) = self.rewrite_expr(value, env, expected.as_ref())?;
                let mut new_targets = Vec::with_capacity(targets.len());
                for t in targets {
                    new_targets.push(self.rw(t, env)?);
                }
                Stmt::AssignChain {
                    targets: new_targets,
                    value,
                    span: *span,
                }
            }
            Stmt::Del { target, span } => Stmt::Del {
                target: self.rw(target, env)?,
                span: *span,
            },
            Stmt::AugAssign {
                target,
                op,
                value,
                span,
            } => Stmt::AugAssign {
                target: self.rw(target, env)?,
                op: *op,
                value: self.rw(value, env)?,
                span: *span,
            },
            Stmt::Expr(e) => Stmt::Expr(self.rw(e, env)?),
            Stmt::If {
                branches,
                else_block,
                span,
            } => {
                let mut nb = Vec::with_capacity(branches.len());
                for (cond, body) in branches {
                    let c = self.rw(cond, env)?;
                    let b = self.rewrite_block(body, env)?;
                    nb.push((c, b));
                }
                let eb = match else_block {
                    Some(b) => Some(self.rewrite_block(b, env)?),
                    None => None,
                };
                Stmt::If {
                    branches: nb,
                    else_block: eb,
                    span: *span,
                }
            }
            Stmt::While { cond, body, span } => Stmt::While {
                cond: self.rw(cond, env)?,
                body: self.rewrite_block(body, env)?,
                span: *span,
            },
            Stmt::DoWhile { body, cond, span } => Stmt::DoWhile {
                body: self.rewrite_block(body, env)?,
                cond: self.rw(cond, env)?,
                span: *span,
            },
            Stmt::For {
                target,
                iter,
                body,
                span,
            } => {
                // Rewrite the iterable keeping its best-effort concrete type, so
                // the loop variable can be declared with the iterable's element
                // type (#1329; see [`Mono::iter_elem`]) instead of `Unknown` —
                // the same binding a comprehension clause gets (#1131,
                // [`Mono::rw_comp_clauses`]). Without it, a generic call on the
                // loop variable (e.g. `item.collect() -> Optional[T]`) could not
                // be inferred, and a `match` on that result failed to pin the
                // generic enum instance (ZAP6001). A non-iterable or opaque
                // iterable leaves the variable `Unknown`, exactly as before.
                let (iter, iter_cty) = self.rewrite_expr(iter, env, None)?;
                env.push();
                if let Expr::Name(n, _) = target {
                    env.declare(n, self.iter_elem(&iter_cty));
                }
                let target = self.rw(target, env)?;
                let body = self.rewrite_block(body, env)?;
                env.pop();
                Stmt::For {
                    target,
                    iter,
                    body,
                    span: *span,
                }
            }
            // `comptime for` (E2/E3 surface, #1297): type-substitute like a
            // runtime `for` so a specialized declaration is well-formed. Its
            // compile-time unrolling (the engine wiring, #1251/#1298) is a
            // separate pass; here mono only rewrites references/types.
            Stmt::ComptimeFor {
                target,
                iter,
                body,
                span,
            } => {
                let iter = self.rw(iter, env)?;
                env.push();
                if let Expr::Name(n, _) = target {
                    env.declare(n, CTy::Unknown);
                }
                let target = self.rw(target, env)?;
                let body = self.rewrite_block(body, env)?;
                env.pop();
                Stmt::ComptimeFor {
                    target,
                    iter,
                    body,
                    span: *span,
                }
            }
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                let (sexpr, scty) = self.rewrite_expr(scrutinee, env, None)?;
                let sty = cty_type(scty);
                let mut narms = Vec::with_capacity(arms.len());
                for arm in arms {
                    // #1374: a `case comptime <v> in …variants [as payload]:` arm
                    // expands into one ordinary variant-pattern arm per variant.
                    if let Pattern::ComptimeVariants {
                        loop_var,
                        iter,
                        bind,
                        span: cv_span,
                    } = &arm.pattern
                    {
                        let expanded = self.expand_comptime_variant_arms(
                            loop_var,
                            iter,
                            bind.as_deref(),
                            &arm.body,
                            sty.as_ref(),
                            *cv_span,
                            env,
                        )?;
                        narms.extend(expanded);
                        continue;
                    }
                    env.push();
                    let pattern = self.rewrite_pattern(&arm.pattern, sty.as_ref(), env)?;
                    let guard = match &arm.guard {
                        Some(g) => Some(self.rw(g, env)?),
                        None => None,
                    };
                    let body = self.rewrite_block(&arm.body, env)?;
                    env.pop();
                    narms.push(ast::MatchArm {
                        pattern,
                        guard,
                        body,
                        span: arm.span,
                    });
                }
                Stmt::Match {
                    scrutinee: sexpr,
                    arms: narms,
                    span: *span,
                }
            }
            Stmt::Select { cases, span } => {
                let mut ncases = Vec::with_capacity(cases.len());
                for case in cases {
                    env.push();
                    let op = match &case.op {
                        ast::SelectOp::Recv { value, ok, chan } => ast::SelectOp::Recv {
                            value: value.clone(),
                            ok: ok.clone(),
                            chan: self.rw(chan, env)?,
                        },
                        ast::SelectOp::Send { chan, value } => ast::SelectOp::Send {
                            chan: self.rw(chan, env)?,
                            value: self.rw(value, env)?,
                        },
                        ast::SelectOp::Default => ast::SelectOp::Default,
                    };
                    let body = self.rewrite_block(&case.body, env)?;
                    env.pop();
                    ncases.push(ast::SelectCase {
                        op,
                        body,
                        span: case.span,
                    });
                }
                Stmt::Select {
                    cases: ncases,
                    span: *span,
                }
            }
            Stmt::Return { value, span } => {
                let expected = self.cur_ret.clone();
                let value = match value {
                    Some(v) => Some(self.rewrite_expr(v, env, expected.as_ref())?.0),
                    None => None,
                };
                Stmt::Return { value, span: *span }
            }
            Stmt::Raise { value, cause, span } => Stmt::Raise {
                value: match value {
                    Some(v) => Some(self.rw(v, env)?),
                    None => None,
                },
                cause: match cause {
                    Some(c) => Some(self.rw(c, env)?),
                    None => None,
                },
                span: *span,
            },
            Stmt::Assert {
                cond,
                msg,
                cond_src,
                span,
            } => Stmt::Assert {
                cond: self.rw(cond, env)?,
                msg: match msg {
                    Some(m) => Some(self.rw(m, env)?),
                    None => None,
                },
                cond_src: cond_src.clone(),
                span: *span,
            },
            Stmt::Try {
                body,
                excepts,
                finally,
                span,
            } => {
                let body = self.rewrite_block(body, env)?;
                let mut nex = Vec::with_capacity(excepts.len());
                for ec in excepts {
                    nex.push(ast::ExceptClause {
                        ty: ec.ty.as_ref().map(|t| self.rewrite_type(t)),
                        binding: ec.binding.clone(),
                        body: self.rewrite_block(&ec.body, env)?,
                        is_star: ec.is_star,
                        span: ec.span,
                    });
                }
                let finally = match finally {
                    Some(b) => Some(self.rewrite_block(b, env)?),
                    None => None,
                };
                Stmt::Try {
                    body,
                    excepts: nex,
                    finally,
                    span: *span,
                }
            }
            Stmt::With { items, body, span } => {
                let mut nitems = Vec::with_capacity(items.len());
                for it in items {
                    nitems.push(ast::WithItem {
                        expr: self.rw(&it.expr, env)?,
                        alias: it.alias.clone(),
                        span: it.span,
                    });
                }
                Stmt::With {
                    items: nitems,
                    body: self.rewrite_block(body, env)?,
                    span: *span,
                }
            }
            Stmt::Spawn {
                call,
                detached,
                span,
            } => Stmt::Spawn {
                call: self.rw(call, env)?,
                detached: *detached,
                span: *span,
            },
            Stmt::Scope { opts, body, span } => {
                let mut nopts = Vec::with_capacity(opts.len());
                for a in opts {
                    nopts.push(self.rewrite_arg(a, env)?);
                }
                Stmt::Scope {
                    opts: nopts,
                    body: self.rewrite_block(body, env)?,
                    span: *span,
                }
            }
            Stmt::Unsafe { body, span } => Stmt::Unsafe {
                body: self.rewrite_block(body, env)?,
                span: *span,
            },
            Stmt::LetTuple {
                is_const,
                ty,
                target,
                value,
                span,
            } => Stmt::LetTuple {
                is_const: *is_const,
                ty: ty.as_ref().map(|t| self.rewrite_type(t)),
                target: target.clone(),
                value: self.rw(value, env)?,
                span: *span,
            },
            Stmt::AssignTuple {
                target,
                value,
                span,
            } => Stmt::AssignTuple {
                target: target.clone(),
                value: self.rw(value, env)?,
                span: *span,
            },
            Stmt::Break(s) => Stmt::Break(*s),
            Stmt::Continue(s) => Stmt::Continue(*s),
            Stmt::Pass(s) => Stmt::Pass(*s),
        })
    }

    fn rewrite_arg(&mut self, a: &Arg, env: &mut Env) -> Result<Arg, LowerError> {
        Ok(Arg {
            name: a.name.clone(),
            value: self.rw(&a.value, env)?,
            spread: a.spread,
            span: a.span,
        })
    }
}

/// Extract a concrete `ast::Type` from the inferred `CTy`, if any.
fn cty_type(c: CTy) -> Option<Type> {
    match c {
        CTy::Ty(t) => Some(t),
        _ => None,
    }
}

impl Mono<'_> {
    fn prim(&self, name: &str) -> CTy {
        CTy::Ty(Type::Named {
            name: name.to_string(),
            args: Vec::new(),
            span: self.span,
        })
    }

    /// The element type produced by *iterating* a value of best-effort type `c`
    /// (a comprehension's `for v in ITER`), mirroring sema's `iterable_elem`
    /// (#1131): `range` → `int64`; `list`/`set`/`heap`/`deque`/`oset` → their
    /// element; `dict` → its key (iterating a dict yields keys, #299); `str` →
    /// `char`. Anything else — a user struct's iterator protocol, an opaque or
    /// untyped base — stays `Unknown`, so the loop variable's type is simply
    /// unknown and inference falls back exactly as before.
    fn iter_elem(&self, c: &CTy) -> CTy {
        let CTy::Ty(Type::Named { name, args, .. }) = c else {
            return CTy::Unknown;
        };
        match name.as_str() {
            "range" => self.prim("int64"),
            "str" => self.prim("char"),
            // Iterating `bytes` yields unsigned byte values as `int64` (#1349),
            // consistent with `bytes` subscripting in `index_elem` below.
            "bytes" => self.prim("int64"),
            // Iterating a dict yields its keys (`args[0]`).
            "list" | "set" | "heap" | "deque" | "oset" | "dict" => {
                args.first().map_or(CTy::Unknown, |t| CTy::Ty(t.clone()))
            }
            _ => CTy::Unknown,
        }
    }

    /// The element/value type produced by *subscripting* a value of best-effort
    /// type `base` (`base[i]`), mirroring sema's `Expr::Index` rules (#1131):
    /// `list[T]`/`deque[T]` → `T`; `dict[K, V]` → `V`; `str` → `char`;
    /// `bytes`/`range` → `int64`. A tuple needs a compile-time-constant index to
    /// know the element statically, and a user `__getitem__` is not modelled
    /// here, so both stay `Unknown` (fail-closed) — as does an opaque base.
    fn index_elem(&self, base: &CTy) -> CTy {
        let CTy::Ty(Type::Named { name, args, .. }) = base else {
            return CTy::Unknown;
        };
        match name.as_str() {
            "list" | "deque" => args.first().map_or(CTy::Unknown, |t| CTy::Ty(t.clone())),
            "dict" => args.get(1).map_or(CTy::Unknown, |t| CTy::Ty(t.clone())),
            "str" => self.prim("char"),
            "bytes" | "range" => self.prim("int64"),
            _ => CTy::Unknown,
        }
    }

    /// The type produced by *slicing* a value of best-effort type `base`
    /// (`base[lo:hi]`), mirroring sema's `Expr::Slice` rules (#1131): a
    /// `list[T]` slice is a `list[T]` copy, a `str` slice is `str`, and a
    /// `bytes` slice is `bytes` — the base's own type passes through. Any other
    /// base (an opaque/loose array, a user type) stays `Unknown`.
    fn slice_result(&self, base: &CTy) -> CTy {
        if let CTy::Ty(Type::Named { name, .. }) = base {
            if matches!(name.as_str(), "list" | "str" | "bytes") {
                return base.clone();
            }
        }
        CTy::Unknown
    }

    /// Join two numeric families to the type their arithmetic yields, mirroring
    /// `sema_types::num_join`'s promotion order
    /// (`complex > float > unsigned > bigint > signed`). Signed/unsigned mixing
    /// is a sema error (ZAP4015) caught before this matters, so the join is only
    /// ever consulted for a program that also type-checks.
    fn num_join(&self, l: Num, r: Num) -> CTy {
        let name = if l == Num::Complex || r == Num::Complex {
            "complex"
        } else if l == Num::Float || r == Num::Float {
            "float64"
        } else if l == Num::Unsigned || r == Num::Unsigned {
            "uint64"
        } else if l == Num::Bigint || r == Num::Bigint {
            "bigint"
        } else {
            "int64"
        };
        self.prim(name)
    }

    /// The best-effort result type of a binary expression `l OP r`, mirroring
    /// sema's binary result-type rules (`arith_result`). The inferer uses this
    /// so a binary expression carries a type into generic-argument unification
    /// (#1128). An operand it cannot type leaves the result `Unknown`, so no
    /// previously-inferring site changes behaviour and a genuinely unresolvable
    /// parameter is still rejected.
    fn binary_result(&self, op: BinOp, l: &CTy, r: &CTy) -> CTy {
        use BinOp::*;
        match op {
            // Logical, membership and identity operators are always `bool`,
            // whatever the operands (sema: `And|Or`, `In|NotIn`, `Is|IsNot`).
            And | Or | In | NotIn | Is | IsNot => self.prim("bool"),
            // A comparison is `bool` once both operands are typed; a loose
            // (array-mask, #510) operand keeps it `Unknown`, matching sema.
            // Comparison *chains* parse as `Expr::Compare`; this arm covers a
            // `BinOp` comparison and keeps the match exhaustive.
            Eq | Ne | Lt | Le | Gt | Ge => {
                if matches!(l, CTy::Ty(_)) && matches!(r, CTy::Ty(_)) {
                    self.prim("bool")
                } else {
                    CTy::Unknown
                }
            }
            // True division (D-0024) always yields a float — never the integer
            // operand type — and `complex` if either operand is complex.
            Div => match (num_kind(l), num_kind(r)) {
                (Some(a), Some(b)) => {
                    if a == Num::Complex || b == Num::Complex {
                        self.prim("complex")
                    } else {
                        self.prim("float64")
                    }
                }
                _ => CTy::Unknown,
            },
            // `+` promotes two numbers, or concatenates two equal sequence
            // types (`str`/`bytes`/`list`).
            Add => match (num_kind(l), num_kind(r)) {
                (Some(a), Some(b)) => self.num_join(a, b),
                _ => same_sequence(l, r),
            },
            // The remaining arithmetic operators require numbers; `//` and `%`
            // additionally reject complex (sema).
            Sub | Mul | Pow => match (num_kind(l), num_kind(r)) {
                (Some(a), Some(b)) => self.num_join(a, b),
                _ => CTy::Unknown,
            },
            FloorDiv | Mod => match (num_kind(l), num_kind(r)) {
                (Some(a), Some(b)) if a != Num::Complex && b != Num::Complex => self.num_join(a, b),
                _ => CTy::Unknown,
            },
            // Bitwise/shift need (non-bigint) integer operands and yield
            // `int64`, or `uint64` if either operand is unsigned (#552).
            BitAnd | BitOr | BitXor | Shl | Shr => match (int_unsigned(l), int_unsigned(r)) {
                (Some(lu), Some(ru)) => self.prim(if lu || ru { "uint64" } else { "int64" }),
                _ => CTy::Unknown,
            },
            // `@` (matrix multiply, arrays/loose) and `<-` (channel send, unit)
            // carry no useful inference type.
            MatMul | Send => CTy::Unknown,
        }
    }

    /// A read-only best-effort type for an lvalue/name (no rewriting).
    fn infer(&self, e: &Expr, env: &Env) -> CTy {
        match e {
            Expr::Name(n, _) => env.lookup(n).unwrap_or(CTy::Unknown),
            Expr::Attr { base, name, .. } => {
                if let CTy::Ty(bt) = self.infer(base, env) {
                    return self.field_ty(&bt, name);
                }
                CTy::Unknown
            }
            _ => CTy::Unknown,
        }
    }

    /// The type of field `name` on a (possibly generic-instance) struct type.
    fn field_ty(&self, base: &Type, name: &str) -> CTy {
        let Type::Named { name: tn, args, .. } = base else {
            return CTy::Unknown;
        };
        let Some(s) = self.structs.get(tn.as_str()).copied() else {
            return CTy::Unknown;
        };
        let Some(f) = s.fields.iter().find(|f| f.name == name) else {
            return CTy::Unknown;
        };
        let map = self.subst_map(tn, args);
        CTy::Ty(subst_with(&f.ty, &map))
    }

    /// The declared return type of method `m` on a (possibly generic-instance)
    /// struct/enum type, with the receiver's type arguments substituted in.
    fn method_ret(&self, base: &Type, m: &str) -> CTy {
        let Type::Named { name: tn, args, .. } = base else {
            return CTy::Unknown;
        };
        let methods: Option<&[Function]> = if let Some(s) = self.structs.get(tn.as_str()) {
            Some(&s.methods)
        } else {
            self.enums.get(tn.as_str()).map(|e| e.methods.as_slice())
        };
        let Some(methods) = methods else {
            return CTy::Unknown;
        };
        let Some(f) = methods.iter().find(|f| f.name == m) else {
            return CTy::Unknown;
        };
        match &f.ret {
            Some(rt) => {
                let map = self.subst_map(tn, args);
                CTy::Ty(subst_with(rt, &map))
            }
            None => CTy::Unknown,
        }
    }

    /// Rewrite `e` and infer its best-effort concrete type. Thin wrapper over
    /// [`Mono::rewrite_expr_inner`] (the per-form inferer) that backfills an
    /// `Unknown` result from sema's inferred types (#1167): when the pass's own
    /// arms cannot type an expression, `self.types.type_at(span)` — the type
    /// sema already computed — is bridged to an `ast::Type` and used instead.
    ///
    /// This only *augments*: a type the arms already produce is kept untouched,
    /// so no previously-inferring site changes, and `NoneLit` (the bare-`None`
    /// sink) is preserved. A sema type that cannot be bridged, or a span sema
    /// never typed, leaves the result `Unknown` — fail-closed, exactly as
    /// before. The map is empty on paths that pass no types, making this a
    /// no-op there.
    fn rewrite_expr(
        &mut self,
        e: &Expr,
        env: &mut Env,
        expected: Option<&Type>,
    ) -> Result<(Expr, CTy), LowerError> {
        let (node, cty) = self.rewrite_expr_inner(e, env, expected)?;
        let cty = match cty {
            CTy::Unknown => self
                .types
                .type_at(expr_span(e))
                .and_then(|ty| ty.to_ast_type())
                .map_or(CTy::Unknown, CTy::Ty),
            other => other,
        };
        Ok((node, cty))
    }

    fn rewrite_expr_inner(
        &mut self,
        e: &Expr,
        env: &mut Env,
        expected: Option<&Type>,
    ) -> Result<(Expr, CTy), LowerError> {
        match e {
            Expr::Int(..) => Ok((e.clone(), self.prim("int64"))),
            Expr::Float(..) => Ok((e.clone(), self.prim("float64"))),
            Expr::Imaginary(..) => Ok((e.clone(), self.prim("complex"))),
            Expr::Bool(..) => Ok((e.clone(), self.prim("bool"))),
            Expr::Str {
                value,
                raw,
                bytes,
                fstring,
                span,
                ..
            } => {
                let cty = self.prim(if *bytes { "bytes" } else { "str" });
                // An f-string's replacement fields are re-parsed at lowering
                // time and so are invisible to this pass; monomorphize each one
                // here (with the current environment) and record the rewritten
                // expressions for lowering, so inline generics / `dict.get` in a
                // field are specialized (#498). On any split/rewrite trouble we
                // fall back to the untouched literal (lowering re-parses it).
                if *fstring {
                    let base = span.start.saturating_add(2);
                    let (parts, errs) = zapc_syntax::fstring::split_fstring(value, base);
                    if errs.is_empty() {
                        let mut fields = Vec::new();
                        let mut ok = true;
                        for part in &parts {
                            if let zapc_syntax::fstring::FStringPart::Field { expr, .. } = part {
                                match self.rewrite_expr(expr, env, None) {
                                    Ok((ne, _)) => fields.push(ne),
                                    Err(_) => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                        }
                        if ok {
                            return Ok((
                                Expr::Str {
                                    value: value.clone(),
                                    raw: *raw,
                                    bytes: *bytes,
                                    fstring: *fstring,
                                    span: *span,
                                    fstring_fields: fields,
                                },
                                cty,
                            ));
                        }
                    }
                }
                Ok((e.clone(), cty))
            }
            Expr::None(span) => {
                // The `None` sink (Fork I/O): when the expected type is a generic
                // enum with a `None` variant, `None` constructs that instance's
                // `None`. Otherwise it stays the null literal.
                if let Some(Type::Named { name, args, .. }) = expected {
                    if self.genums.contains_key(name)
                        && self
                            .genums
                            .get(name)
                            .is_some_and(|e| e.variants.iter().any(|v| v.name == "None"))
                    {
                        let mangled = self.request(&name.clone(), &args.clone());
                        let node = Expr::Attr {
                            base: Box::new(Expr::Name(mangled, *span)),
                            name: "None".to_string(),
                            span: *span,
                        };
                        return Ok((
                            node,
                            CTy::Ty(Type::Named {
                                name: name.clone(),
                                args: args.clone(),
                                span: *span,
                            }),
                        ));
                    }
                }
                Ok((e.clone(), CTy::NoneLit))
            }
            Expr::Name(n, _) => {
                let cty = env.lookup(n).unwrap_or(CTy::Unknown);
                Ok((e.clone(), cty))
            }
            Expr::Call { callee, args, span } => {
                self.rewrite_call(callee, args, *span, env, expected)
            }
            Expr::Attr { base, name, span } => self.rewrite_attr(base, name, *span, env, expected),
            Expr::Index { base, index, span } => {
                // `G[args]` as a value is only meaningful as a construction/variant
                // base, handled in call/attr. A standalone generic-type subscript
                // becomes the mangled name; otherwise it is an ordinary subscript.
                if let Some(mangled) = self.try_type_instance(base, index) {
                    return Ok((
                        Expr::Name(mangled.0, *span),
                        CTy::Ty(Type::Named {
                            name: mangled.1,
                            args: mangled.2,
                            span: *span,
                        }),
                    ));
                }
                // An ordinary subscript carries the base container's
                // element/value type into generic inference (#1131): without it
                // `xs[0]` on a `list[int64]` stayed `Unknown`, so
                // `Optional.Some(xs[0])` could not pin `T` (and an untyped
                // `auto e = xs[0]` poisoned every later use). An opaque base, a
                // tuple (needs a static index), or a user `__getitem__` keeps
                // the result `Unknown`.
                let (base_e, base_cty) = self.rewrite_expr(base, env, None)?;
                let index = self.rw(index, env)?;
                let cty = self.index_elem(&base_cty);
                Ok((
                    Expr::Index {
                        base: Box::new(base_e),
                        index: Box::new(index),
                        span: *span,
                    },
                    cty,
                ))
            }
            Expr::Unary { op, expr, span } => {
                // Infer the operand so a unary expression carries a type into
                // generic argument unification (#1117). Without this a negative
                // literal `-4` — which parses as `Neg` over `Int(4)` — stayed
                // `Unknown` while the bare `4` was `int64`, so `Optional.Some(-4)`
                // could not pin `T` though `Optional.Some(4)` could. The operand
                // keeps the same expected-type context (`None`) the old
                // `self.rw(expr, env)` call used, so the rewritten expression is
                // byte-for-byte unchanged; only the inferred `CTy` is added.
                let (inner, inner_cty) = self.rewrite_expr(expr, env, None)?;
                let cty = match op {
                    // `not x` is boolean regardless of the operand (Python
                    // truthiness semantics).
                    UnOp::Not => self.prim("bool"),
                    // Arithmetic negation / unary plus / bitwise complement
                    // preserve the operand's (numeric) type, so a negated,
                    // unary-plus, or complemented literal infers exactly the type
                    // its bare-literal counterpart does. An `Unknown` operand
                    // stays `Unknown`, so no previously-inferring site changes.
                    UnOp::Neg | UnOp::Pos | UnOp::BitNot => inner_cty,
                };
                Ok((
                    Expr::Unary {
                        op: *op,
                        expr: Box::new(inner),
                        span: *span,
                    },
                    cty,
                ))
            }
            // `comptime <expr>` (E2/E3 surface, #1298): the comptime operands a
            // generator uses (`comptime f.index`, `comptime f.name`) are folded by
            // the loop unroller before mono sees the body, so by here the wrapper
            // is transparent — unwrap to the rewritten inner so the `comptime`
            // marker never reaches lowering. (A standalone comptime binding that
            // must be *folded* at mono — e.g. `comptime len(info.fields)` outside a
            // loop — is a follow-up; v1 unwraps.)
            Expr::Comptime { expr, .. } => self.rewrite_expr(expr, env, expected),
            Expr::Binary { op, lhs, rhs, span } => {
                // Infer the operands so a binary expression carries a type into
                // generic-argument unification (#1128), mirroring sema's binary
                // result-type rules (`arith_result`). Without this an integer
                // subtraction `2 - 5` stayed `Unknown` while the bare `2` was
                // `int64`, so `Optional.Some(2 - 5)` could not pin `T`; an
                // untyped `auto count = 2 - 5` binding then carried that
                // `Unknown` into every later use. The operands keep the same
                // expected-type context (`None`) the old `self.rw(lhs, env)` /
                // `self.rw(rhs, env)` calls used, so the rewritten expression is
                // byte-for-byte unchanged; only the inferred `CTy` is added.
                let (lhs_e, lcty) = self.rewrite_expr(lhs, env, None)?;
                let (rhs_e, rcty) = self.rewrite_expr(rhs, env, None)?;
                // A *generic* operator dunder (`def __add__[T]` / `def __radd__[T]`,
                // #1227) cannot be dispatched by IR lowering, which only knows the
                // un-instantiated mangled name (`Delta____radd__`) and so reports
                // ZAP6001. Detect the D-0047 / #1180 dispatch here — the left
                // operand's forward dunder wins when its type declares one, else the
                // right operand's reflected dunder — and, when the resolved method is
                // generic, rewrite the operator into the specialized method call the
                // explicit `x.__radd__[T](y)` form already lowers. A non-generic
                // dunder is left as an operator: IR lowering dispatches it unchanged
                // (#1180/#1201), so the operator surface is untouched for every type
                // that worked before.
                if let Some((reflected, owner, owner_args, dunder)) =
                    self.operator_dunder_dispatch(*op, &lcty, &rcty)
                {
                    let (recv_e, arg_e, arg_cty) = if reflected {
                        (rhs_e, lhs_e, lcty)
                    } else {
                        (lhs_e, rhs_e, rcty)
                    };
                    return self.rewrite_operator_dunder(
                        &owner,
                        &owner_args,
                        dunder,
                        recv_e,
                        arg_e,
                        arg_cty,
                        expected,
                        *span,
                    );
                }
                let cty = self.binary_result(*op, &lcty, &rcty);
                Ok((
                    Expr::Binary {
                        op: *op,
                        lhs: Box::new(lhs_e),
                        rhs: Box::new(rhs_e),
                        span: *span,
                    },
                    cty,
                ))
            }
            Expr::Compare { first, rest, span } => {
                let first = Box::new(self.rw(first, env)?);
                let mut new_rest = Vec::with_capacity(rest.len());
                for (op, operand) in rest {
                    new_rest.push((*op, self.rw(operand, env)?));
                }
                Ok((
                    Expr::Compare {
                        first,
                        rest: new_rest,
                        span: *span,
                    },
                    self.prim("bool"),
                ))
            }
            Expr::Tuple(xs, span) => {
                // Infer a `tuple[...]` type from the elements (#1131), so a
                // `tuple` argument can pin a type parameter. Mirror the
                // list/set/dict literal arms, but a tuple is heterogeneous, so
                // every element must be typed — any untyped element drops the
                // whole tuple to `Unknown` (the empty tuple `()` too).
                let mut out = Vec::with_capacity(xs.len());
                let mut elems: Option<Vec<Type>> = Some(Vec::with_capacity(xs.len()));
                for x in xs {
                    let (v, cty) = self.rewrite_expr(x, env, None)?;
                    match (&mut elems, cty) {
                        (Some(acc), CTy::Ty(t)) => acc.push(t),
                        (slot, _) => *slot = None,
                    }
                    out.push(v);
                }
                let cty = match elems {
                    Some(elems) if !elems.is_empty() => CTy::Ty(Type::Tuple { elems, span: *span }),
                    _ => CTy::Unknown,
                };
                Ok((Expr::Tuple(out, *span), cty))
            }
            Expr::List(xs, span) => {
                // Infer the element type from the first element (best-effort), so
                // a `list[T]` parameter can be inferred from a list literal
                // argument (I#161); heterogeneous or untyped elements leave the
                // list `Unknown`.
                let mut out = Vec::with_capacity(xs.len());
                let mut elem: Option<Type> = None;
                for (i, x) in xs.iter().enumerate() {
                    let (v, cty) = self.rewrite_expr(x, env, None)?;
                    if i == 0 {
                        if let CTy::Ty(t) = cty {
                            elem = Some(t);
                        }
                    }
                    out.push(v);
                }
                let cty = match elem {
                    Some(t) => CTy::Ty(Type::Named {
                        name: "list".to_string(),
                        args: vec![t],
                        span: *span,
                    }),
                    None => CTy::Unknown,
                };
                Ok((Expr::List(out, *span), cty))
            }
            Expr::Set(xs, span) => {
                // Mirror the list-literal case (I#170): infer the element type
                // from the first element (best-effort) so a `set[T]` parameter
                // can be inferred from a set literal argument; heterogeneous or
                // untyped elements leave the set `Unknown`.
                let mut out = Vec::with_capacity(xs.len());
                let mut elem: Option<Type> = None;
                for (i, x) in xs.iter().enumerate() {
                    let (v, cty) = self.rewrite_expr(x, env, None)?;
                    if i == 0 {
                        if let CTy::Ty(t) = cty {
                            elem = Some(t);
                        }
                    }
                    out.push(v);
                }
                let cty = match elem {
                    Some(t) => CTy::Ty(Type::Named {
                        name: "set".to_string(),
                        args: vec![t],
                        span: *span,
                    }),
                    None => CTy::Unknown,
                };
                Ok((Expr::Set(out, *span), cty))
            }
            Expr::Dict(items, span) => {
                // Mirror the list-literal case (I#170): infer the key/value types
                // from the first pair (best-effort) so a `dict[K, V]` parameter
                // can be inferred from a dict literal argument; an untyped key or
                // value, or a leading `**expr` merge (#598), leaves the dict
                // `Unknown`.
                let mut np = Vec::with_capacity(items.len());
                let mut kv: Option<(Type, Type)> = None;
                for (i, it) in items.iter().enumerate() {
                    match it {
                        DictItem::Pair(k, v) => {
                            let (kk, kcty) = self.rewrite_expr(k, env, None)?;
                            let (vv, vcty) = self.rewrite_expr(v, env, None)?;
                            if i == 0 {
                                if let (CTy::Ty(kt), CTy::Ty(vt)) = (kcty, vcty) {
                                    kv = Some((kt, vt));
                                }
                            }
                            np.push(DictItem::Pair(kk, vv));
                        }
                        DictItem::Spread(e) => {
                            np.push(DictItem::Spread(self.rw(e, env)?));
                        }
                    }
                }
                let cty = match kv {
                    Some((kt, vt)) => CTy::Ty(Type::Named {
                        name: "dict".to_string(),
                        args: vec![kt, vt],
                        span: *span,
                    }),
                    None => CTy::Unknown,
                };
                Ok((Expr::Dict(np, *span), cty))
            }
            Expr::Starred { value, span } => Ok((
                Expr::Starred {
                    value: Box::new(self.rw(value, env)?),
                    span: *span,
                },
                CTy::Unknown,
            )),
            // `(name := value)` — rewrite the operand (which may carry generic
            // references); the walrus's type is the operand's (#600).
            Expr::Named {
                name,
                name_span,
                value,
                span,
            } => {
                let (value_e, vcty) = self.rewrite_expr(value, env, expected)?;
                Ok((
                    Expr::Named {
                        name: name.clone(),
                        name_span: *name_span,
                        value: Box::new(value_e),
                        span: *span,
                    },
                    vcty,
                ))
            }
            Expr::Slice {
                base,
                lo,
                hi,
                step,
                span,
            } => {
                // A slice yields the base container's own type (#1131): slicing
                // a `list[T]` gives a `list[T]`, a `str` gives `str`, `bytes`
                // gives `bytes`. So `xs[0:2]` on a `list[int64]` pins a
                // `list[int64]` parameter. An opaque/loose base stays `Unknown`.
                let (base_e, base_cty) = self.rewrite_expr(base, env, None)?;
                let cty = self.slice_result(&base_cty);
                Ok((
                    Expr::Slice {
                        base: Box::new(base_e),
                        lo: self.rw_opt(lo, env)?,
                        hi: self.rw_opt(hi, env)?,
                        step: self.rw_opt(step, env)?,
                        span: *span,
                    },
                    cty,
                ))
            }
            Expr::SliceSpec { lo, hi, step, span } => Ok((
                Expr::SliceSpec {
                    lo: self.rw_opt(lo, env)?,
                    hi: self.rw_opt(hi, env)?,
                    step: self.rw_opt(step, env)?,
                    span: *span,
                },
                CTy::Unknown,
            )),
            Expr::IfElse {
                cond,
                then,
                els,
                span,
            } => {
                let cond = Box::new(self.rw(cond, env)?);
                let (then_e, tcty) = self.rewrite_expr(then, env, expected)?;
                let (els_e, _) = self.rewrite_expr(els, env, expected)?;
                Ok((
                    Expr::IfElse {
                        cond,
                        then: Box::new(then_e),
                        els: Box::new(els_e),
                        span: *span,
                    },
                    tcty,
                ))
            }
            Expr::ChanRecv { chan, span } => Ok((
                Expr::ChanRecv {
                    chan: Box::new(self.rw(chan, env)?),
                    span: *span,
                },
                CTy::Unknown,
            )),
            // Comprehensions (#1131): with the loop variable now typed from its
            // iterable (see `rw_comp_clauses`), a list/set/dict comprehension
            // infers `list`/`set`/`dict` of its element/key-value type, so it can
            // pin a generic parameter. An untyped element keeps the result
            // `Unknown`.
            Expr::ListComp {
                elem,
                clauses,
                span,
            } => {
                env.push();
                let clauses = self.rw_comp_clauses(clauses, env)?;
                let (elem_e, elem_cty) = self.rewrite_expr(elem, env, None)?;
                env.pop();
                let cty = match elem_cty {
                    CTy::Ty(t) => CTy::Ty(Type::Named {
                        name: "list".to_string(),
                        args: vec![t],
                        span: *span,
                    }),
                    _ => CTy::Unknown,
                };
                Ok((
                    Expr::ListComp {
                        elem: Box::new(elem_e),
                        clauses,
                        span: *span,
                    },
                    cty,
                ))
            }
            Expr::SetComp {
                elem,
                clauses,
                span,
            } => {
                env.push();
                let clauses = self.rw_comp_clauses(clauses, env)?;
                let (elem_e, elem_cty) = self.rewrite_expr(elem, env, None)?;
                env.pop();
                let cty = match elem_cty {
                    CTy::Ty(t) => CTy::Ty(Type::Named {
                        name: "set".to_string(),
                        args: vec![t],
                        span: *span,
                    }),
                    _ => CTy::Unknown,
                };
                Ok((
                    Expr::SetComp {
                        elem: Box::new(elem_e),
                        clauses,
                        span: *span,
                    },
                    cty,
                ))
            }
            // Generator expression (#596): rewrite its nested exprs like a
            // comprehension (the loop variable is typed for inference inside the
            // body), but the result type stays `Unknown`. A generator's type is
            // sema's internal `$genexpr` iterator — not a nominal type the
            // inferer can name — so it is deliberately left unresolved (an
            // explicit sink is not spellable for it either).
            Expr::GenExp {
                elem,
                clauses,
                span,
            } => {
                env.push();
                let clauses = self.rw_comp_clauses(clauses, env)?;
                let elem = Box::new(self.rw(elem, env)?);
                env.pop();
                Ok((
                    Expr::GenExp {
                        elem,
                        clauses,
                        span: *span,
                    },
                    CTy::Unknown,
                ))
            }
            Expr::DictComp {
                key,
                value,
                clauses,
                span,
            } => {
                env.push();
                let clauses = self.rw_comp_clauses(clauses, env)?;
                let (key_e, kcty) = self.rewrite_expr(key, env, None)?;
                let (value_e, vcty) = self.rewrite_expr(value, env, None)?;
                env.pop();
                let cty = match (kcty, vcty) {
                    (CTy::Ty(kt), CTy::Ty(vt)) => CTy::Ty(Type::Named {
                        name: "dict".to_string(),
                        args: vec![kt, vt],
                        span: *span,
                    }),
                    _ => CTy::Unknown,
                };
                Ok((
                    Expr::DictComp {
                        key: Box::new(key_e),
                        value: Box::new(value_e),
                        clauses,
                        span: *span,
                    },
                    cty,
                ))
            }
            Expr::Lambda { params, body, span } => Ok((
                Expr::Lambda {
                    params: params.clone(),
                    body: Box::new(self.rw(body, env)?),
                    span: *span,
                },
                CTy::Unknown,
            )),
            Expr::Yield { value, span } => Ok((
                Expr::Yield {
                    value: match value {
                        Some(v) => Some(Box::new(self.rw(v, env)?)),
                        None => None,
                    },
                    span: *span,
                },
                CTy::Unknown,
            )),
            Expr::RegexMatch { .. } | Expr::RegexSubst { .. } => Ok((e.clone(), CTy::Unknown)),
        }
    }

    /// Rewrite a comprehension clause list (#599). Each clause's iterable is
    /// rewritten with the previous clauses' variables in scope; its loop
    /// variable is then declared with the iterable's element type (#1131; see
    /// [`Mono::iter_elem`]), falling back to `Unknown` for a non-iterable or
    /// opaque iterable, so later clauses and filters can refer to it. The caller
    /// pushes and pops the surrounding scope.
    fn rw_comp_clauses(
        &mut self,
        clauses: &[CompClause],
        env: &mut Env,
    ) -> Result<Vec<CompClause>, LowerError> {
        let mut out = Vec::with_capacity(clauses.len());
        for c in clauses {
            // Declare the loop variable with the iterable's element type (#1131)
            // instead of `Unknown`, so a comprehension's element expression — and
            // any generic call inside it — can be inferred. A non-iterable or
            // opaque iterable leaves the variable `Unknown`, exactly as before.
            let (iter_e, iter_cty) = self.rewrite_expr(&c.iter, env, None)?;
            let iter = Box::new(iter_e);
            env.declare(&c.var, self.iter_elem(&iter_cty));
            let mut ifs = Vec::with_capacity(c.ifs.len());
            for f in &c.ifs {
                ifs.push(self.rw(f, env)?);
            }
            out.push(CompClause {
                var: c.var.clone(),
                iter,
                ifs,
                span: c.span,
            });
        }
        Ok(out)
    }

    fn rw_opt(
        &mut self,
        x: &Option<Box<Expr>>,
        env: &mut Env,
    ) -> Result<Option<Box<Expr>>, LowerError> {
        Ok(match x {
            Some(e) => Some(Box::new(self.rw(e, env)?)),
            None => None,
        })
    }
}

/// Substitute type parameters (by the given map) in a type.
fn subst_with(t: &Type, map: &HashMap<String, Type>) -> Type {
    match t {
        Type::Named { name, args, span } => {
            if args.is_empty() {
                if let Some(c) = map.get(name) {
                    return c.clone();
                }
            }
            Type::Named {
                name: name.clone(),
                args: args.iter().map(|a| subst_with(a, map)).collect(),
                span: *span,
            }
        }
        Type::Tuple { elems, span } => Type::Tuple {
            elems: elems.iter().map(|e| subst_with(e, map)).collect(),
            span: *span,
        },
        Type::Func { params, ret, span } => Type::Func {
            params: params.iter().map(|p| subst_with(p, map)).collect(),
            ret: ret.as_ref().map(|r| Box::new(subst_with(r, map))),
            span: *span,
        },
    }
}

/// Unify a declared type (which may mention the generic's parameters `ps`)
/// against a concrete actual type, recording parameter bindings.
fn unify(decl: &Type, actual: &Type, ps: &[String], out: &mut HashMap<String, Type>) {
    match decl {
        Type::Named { name, args, .. } if args.is_empty() && ps.iter().any(|p| p == name) => {
            out.entry(name.clone()).or_insert_with(|| actual.clone());
        }
        Type::Named { args: dargs, .. } => {
            if let Type::Named { args: aargs, .. } = actual {
                if dargs.len() == aargs.len() {
                    for (d, a) in dargs.iter().zip(aargs) {
                        unify(d, a, ps, out);
                    }
                }
            }
        }
        Type::Tuple { elems: de, .. } => {
            if let Type::Tuple { elems: ae, .. } = actual {
                if de.len() == ae.len() {
                    for (d, a) in de.iter().zip(ae) {
                        unify(d, a, ps, out);
                    }
                }
            }
        }
        Type::Func {
            params: dp,
            ret: dr,
            ..
        } => {
            if let Type::Func {
                params: ap,
                ret: ar,
                ..
            } = actual
            {
                if dp.len() == ap.len() {
                    for (d, a) in dp.iter().zip(ap) {
                        unify(d, a, ps, out);
                    }
                }
                if let (Some(d), Some(a)) = (dr, ar) {
                    unify(d, a, ps, out);
                }
            }
        }
    }
}

impl Mono<'_> {
    fn is_generic_enum_variant(&self, g: &str, variant: &str) -> bool {
        self.genums
            .get(g)
            .is_some_and(|e| e.variants.iter().any(|v| v.name == variant))
    }

    /// Convert a call-site type-argument index expression into concrete types
    /// (applying the current substitution). `Name`/nested-`Index` forms only.
    /// Number of type parameters the generic named `g` declares — a builtin
    /// collection (`dict` is the only two-parameter one), a generic
    /// struct/enum (`self.params`, which includes the prelude `Optional[T]`),
    /// or a generic free function (`self.gfuncs`). Falls back to `1` when `g`
    /// is not a known generic, matching the single-argument default for a lone
    /// type subscript.
    fn type_param_count(&self, g: &str) -> usize {
        match g {
            "dict" => return 2,
            "list" | "set" | "heap" | "channel" | "deque" | "oset" => return 1,
            _ => {}
        }
        if let Some(ps) = self.params.get(g) {
            return ps.len();
        }
        if let Some(f) = self.gfuncs.get(g) {
            return f.generics.len();
        }
        1
    }

    /// Convert a type-argument *expression* into an `ast::Type` (names, nested
    /// generic instantiations, and tuple types).
    ///
    /// A generic subscript `g[index]` is split into one type argument per
    /// declared parameter of `g` — the same arity rule as
    /// [`type_args_from_index`], applied recursively — because the parser
    /// renders `g[A, B]` and `g[(A, B)]` identically as a tuple index. So a
    /// nested one-parameter generic over a tuple (`Optional[(bytes, bytes)]`,
    /// `list[(bytes, bytes)]`) keeps the tuple whole as its single argument,
    /// while `dict[K, V]` still splits into two (#1478, #1479).
    fn expr_to_type(&self, e: &Expr) -> Option<Type> {
        match e {
            Expr::Name(n, span) => Some(Type::Named {
                name: n.clone(),
                args: Vec::new(),
                span: *span,
            }),
            Expr::Index { base, index, span } => {
                let Expr::Name(g, _) = base.as_ref() else {
                    return None;
                };
                let elems: Vec<&Expr> = match index.as_ref() {
                    Expr::Tuple(xs, _) if xs.len() == self.type_param_count(g) => {
                        xs.iter().collect()
                    }
                    other => vec![other],
                };
                let mut args = Vec::with_capacity(elems.len());
                for el in elems {
                    args.push(self.expr_to_type(el)?);
                }
                Some(Type::Named {
                    name: g.clone(),
                    args,
                    span: *span,
                })
            }
            // A tuple type written in type position — `(A, B)` as an explicit
            // type argument (`run[(Optional[bytes], Optional[bytes])]`, #1478)
            // or nested inside another type argument (`dict[bytes, (bytes,
            // bytes)]`, #1479). Without this a tuple type expression yields
            // `None`, so a generic callee carrying one is not recognized as a
            // generic instantiation at all and falls through to the structural
            // rewrite (e.g. a method call mis-lowered as a field access,
            // `no field 'run'`).
            Expr::Tuple(xs, span) => {
                let mut elems = Vec::with_capacity(xs.len());
                for x in xs {
                    elems.push(self.expr_to_type(x)?);
                }
                Some(Type::Tuple { elems, span: *span })
            }
            _ => None,
        }
    }

    /// Resolve the explicit type arguments in a generic subscript `g[index]`,
    /// given the number of type parameters `g` declares (`nparams`).
    ///
    /// The parser renders `g[A, B]` and `g[(A, B)]` identically as a tuple
    /// index, so arity is the only thing that distinguishes "two type
    /// arguments" from "one tuple-typed argument" (#1478). The tuple is split
    /// into its elements only when its arity matches the declared parameter
    /// count — `dict[K, V]`, `f[A, B]`, `f[(A, B), C]`; otherwise the whole
    /// index is a single type argument, so a one-parameter `run[(A, B)]` binds
    /// `T = (A, B)` rather than being mis-split into two arguments (the first of
    /// which would silently win and the second be dropped). A non-tuple index is
    /// always the sole argument, as before.
    fn type_args_from_index(&self, index: &Expr, nparams: usize) -> Option<Vec<Type>> {
        let elems: Vec<&Expr> = match index {
            Expr::Tuple(xs, _) if xs.len() == nparams => xs.iter().collect(),
            other => vec![other],
        };
        let mut out = Vec::with_capacity(elems.len());
        for e in elems {
            out.push(self.apply_subst(&self.expr_to_type(e)?));
        }
        Some(out)
    }

    /// Re-materialise a collection constructor's type-argument subscript as an
    /// expression from its (already type-parameter-substituted) concrete types
    /// (#1013). A single type is the bare index; several (a `dict`'s `K, V`)
    /// become a tuple. A **user-generic instantiation** element (e.g.
    /// `Optional[int64]`) is requested and emitted by its mangled name — exactly
    /// as the structural rewrite does via `try_type_instance` — so lowering
    /// resolves it as a concrete type rather than the bare generic `Optional`.
    /// `None` (a function-typed element) leaves the call to the structural
    /// fallback.
    fn ctor_subscript_from_types(&mut self, args: &[Type], span: Span) -> Option<Expr> {
        let mut exprs = Vec::with_capacity(args.len());
        for t in args {
            exprs.push(self.type_to_ctor_arg(t, span)?);
        }
        Some(if exprs.len() == 1 {
            exprs.pop().unwrap()
        } else {
            Expr::Tuple(exprs, span)
        })
    }

    fn type_to_ctor_arg(&mut self, t: &Type, span: Span) -> Option<Expr> {
        match t {
            Type::Named { name, args, .. } => {
                if !args.is_empty() && self.is_generic(name) {
                    // A user generic instantiation: request the monomorphised
                    // instance and use its mangled name, so lowering sees a
                    // concrete type (not `Optional`/`Box`/… with a live subscript).
                    let mangled = self.request(&name.clone(), args);
                    Some(Expr::Name(mangled, span))
                } else if args.is_empty() {
                    Some(Expr::Name(name.clone(), span))
                } else {
                    // A builtin collection / nested type (`list[int64]`, …): rebuild
                    // structurally so a nested type parameter is still substituted.
                    let index = self.ctor_subscript_from_types(args, span)?;
                    Some(Expr::Index {
                        base: Box::new(Expr::Name(name.clone(), span)),
                        index: Box::new(index),
                        span,
                    })
                }
            }
            Type::Tuple { elems, .. } => {
                let mut xs = Vec::with_capacity(elems.len());
                for e in elems {
                    xs.push(self.type_to_ctor_arg(e, span)?);
                }
                Some(Expr::Tuple(xs, span))
            }
            Type::Func { .. } => None,
        }
    }

    /// If `base[index]` is a user generic type instantiation used in value
    /// position, request it and return `(mangled, generic_name, concrete_args)`.
    fn try_type_instance(
        &mut self,
        base: &Expr,
        index: &Expr,
    ) -> Option<(String, String, Vec<Type>)> {
        let Expr::Name(g, _) = base else { return None };
        if !self.is_generic(g) {
            return None;
        }
        let cargs = self.type_args_from_index(index, self.type_param_count(g))?;
        let mangled = self.request(&g.clone(), &cargs);
        Some((mangled, g.clone(), cargs))
    }

    /// Resolve the concrete type arguments of a generic `g` from a constructor's
    /// declared field types unified against the (already rewritten) argument
    /// types, falling back to the enclosing instance or the expected type for any
    /// parameter the arguments do not pin down.
    fn instance_args(
        &self,
        g: &str,
        field_decls: &[(String, Type)],
        argpairs: &[(Option<String>, CTy)],
        expected: Option<&Type>,
    ) -> Result<Vec<Type>, LowerError> {
        let ps = self.params.get(g).cloned().unwrap_or_default();
        let mut map: HashMap<String, Type> = HashMap::new();
        let mut pos = 0usize;
        for (name, cty) in argpairs {
            let decl = match name {
                Some(fname) => field_decls.iter().find(|(n, _)| n == fname).map(|(_, t)| t),
                None => {
                    let d = field_decls.get(pos).map(|(_, t)| t);
                    pos += 1;
                    d
                }
            };
            if let (Some(decl), CTy::Ty(actual)) = (decl, cty) {
                unify(decl, actual, &ps, &mut map);
            }
        }
        let expected_args = match expected {
            Some(Type::Named { name, args, .. }) if name == g => Some(args.clone()),
            _ => None,
        };
        let self_args = match &self.cur_self {
            Some((sg, sargs)) if sg == g => Some(sargs.clone()),
            _ => None,
        };
        let mut out = Vec::with_capacity(ps.len());
        for (i, p) in ps.iter().enumerate() {
            if let Some(t) = map.get(p) {
                out.push(t.clone());
            } else if let Some(ea) = expected_args.as_ref().and_then(|a| a.get(i)) {
                out.push(ea.clone());
            } else if let Some(sa) = self_args.as_ref().and_then(|a| a.get(i)) {
                out.push(sa.clone());
            } else {
                return Err(err(format!(
                    "cannot infer type argument '{p}' of generic '{g}'; write it \
                     explicitly (e.g. {g}[...]) (I#154)"
                )));
            }
        }
        Ok(out)
    }

    /// Infer the concrete type arguments for a generic function's or method's own
    /// type parameters (I#161): unify the declared value-parameter types (with any
    /// owner-type substitution already applied) against the inferred argument
    /// types, then fall back to the expected-type sink against the return type for
    /// any parameter the arguments do not pin down (Fork I). Errors naming the
    /// unresolved parameter if neither rule determines it.
    /// Decide whether a binary operator dispatches to a *generic* operator
    /// dunder that must be rewritten to a specialized method call (#1227).
    /// Returns `(is_reflected, owner, owner_args, dunder)`: the owner type that
    /// declares the dunder, its (possibly empty) generic arguments, and the
    /// dunder name. `None` means IR lowering can dispatch the operator as-is —
    /// the operator has no dunder, neither operand's type declares one, or the
    /// declared dunder is non-generic (handled unchanged in `operators.rs`).
    ///
    /// Mirrors the D-0047 / #1180 precedence exactly: the left operand's forward
    /// dunder wins whenever its type declares one (generic or not), which also
    /// suppresses the reflected fallback; otherwise the right operand's reflected
    /// dunder applies. Only a generic match is reported, because a non-generic
    /// dunder already lowers correctly and must keep its operator form (so the
    /// `operator_index_call` / #1201 goldens stay byte-identical).
    fn operator_dunder_dispatch(
        &self,
        op: BinOp,
        lcty: &CTy,
        rcty: &CTy,
    ) -> Option<(bool, String, Vec<Type>, &'static str)> {
        let (fwd, refl) = op_dunders(op)?;
        if let CTy::Ty(Type::Named { name, args, .. }) = lcty {
            if let Some(m) = self.find_method(name, fwd) {
                // A declared forward dunder wins and suppresses the reflected
                // fallback; only its generic form needs the rewrite.
                if m.generics.is_empty() {
                    return None;
                }
                return Some((false, name.clone(), args.clone(), fwd));
            }
        }
        if let CTy::Ty(Type::Named { name, args, .. }) = rcty {
            if let Some(m) = self.find_method(name, refl) {
                if !m.generics.is_empty() {
                    return Some((true, name.clone(), args.clone(), refl));
                }
            }
        }
        None
    }

    /// Rewrite an operator whose dispatch target is a generic dunder into the
    /// specialized method call `recv.dunder$targs(arg)` (#1227). `recv`/`arg` are
    /// the already-rewritten operand expressions placed in call order (for a
    /// reflected dunder the receiver is the right operand and the argument is the
    /// left); `arg_cty` is the argument's inferred type, used — together with the
    /// `expected` sink — to infer the method's type parameters. This produces
    /// exactly the AST an explicit `recv.dunder[targs](arg)` call produces, so
    /// the proven generic-method-call lowering path handles it from here.
    #[allow(clippy::too_many_arguments)]
    fn rewrite_operator_dunder(
        &mut self,
        owner: &str,
        owner_args: &[Type],
        dunder: &str,
        recv_e: Expr,
        arg_e: Expr,
        arg_cty: CTy,
        expected: Option<&Type>,
        span: Span,
    ) -> Result<(Expr, CTy), LowerError> {
        let method = self
            .find_method(owner, dunder)
            .expect("caller verified the generic dunder exists");
        let tparams: Vec<String> = method.generics.iter().map(|g| g.name.clone()).collect();
        let owner_subst = self.subst_map(owner, owner_args);
        let argpairs = vec![(None, arg_cty)];
        let targs = self.infer_fn_targs(
            method,
            &owner_subst,
            &argpairs,
            expected,
            &format!("operator method '{dunder}' of '{owner}'"),
            true,
        )?;
        let ret_cty = self.fn_ret_cty(method.ret.as_ref(), &owner_subst, &tparams, &targs);
        let method_spec = self.request_method(owner, owner_args, dunder, &targs);
        let node = Expr::Call {
            callee: Box::new(Expr::Attr {
                base: Box::new(recv_e),
                name: method_spec,
                span,
            }),
            args: vec![Arg {
                name: None,
                value: arg_e,
                spread: ast::ArgSpread::None,
                span,
            }],
            span,
        };
        Ok((node, ret_cty))
    }

    fn infer_fn_targs(
        &self,
        f: &Function,
        owner_subst: &HashMap<String, Type>,
        argpairs: &[(Option<String>, CTy)],
        expected: Option<&Type>,
        what: &str,
        // `true` for a method (the `self` receiver is passed separately, not among
        // `argpairs`, so exclude it from the parameters inference matches against).
        // `false` for a free function, where a parameter literally named `self` is
        // an ordinary positional argument and must participate (#1324).
        skip_self: bool,
    ) -> Result<Vec<Type>, LowerError> {
        let tparams: Vec<String> = f.generics.iter().map(|g| g.name.clone()).collect();
        let tparams = tparams.as_slice();
        let ret = f.ret.as_ref();
        // Declared value parameters (a method's `self` receiver excluded), with
        // the owner's type parameters substituted so only the fn/method's own type
        // parameters remain free.
        let vparams: Vec<(String, Type)> = f
            .params
            .iter()
            .filter(|p| !(skip_self && p.name == "self"))
            .filter_map(|p| {
                p.ty.as_ref()
                    .map(|t| (p.name.clone(), subst_with(t, owner_subst)))
            })
            .collect();
        let mut map: HashMap<String, Type> = HashMap::new();
        let mut pos = 0usize;
        for (name, cty) in argpairs {
            let decl = match name {
                Some(pname) => vparams.iter().find(|(n, _)| n == pname).map(|(_, t)| t),
                None => {
                    let d = vparams.get(pos).map(|(_, t)| t);
                    pos += 1;
                    d
                }
            };
            if let (Some(decl), CTy::Ty(actual)) = (decl, cty) {
                unify(decl, actual, tparams, &mut map);
            }
        }
        // Expected-type sink: unify the (owner-substituted) return type against
        // the expected type for any parameter still unresolved.
        if tparams.iter().any(|p| !map.contains_key(p)) {
            if let (Some(rd), Some(exp)) = (ret, expected) {
                let rd = subst_with(rd, owner_subst);
                unify(&rd, exp, tparams, &mut map);
            }
        }
        let mut out = Vec::with_capacity(tparams.len());
        for p in tparams {
            match map.get(p) {
                Some(t) => out.push(t.clone()),
                None => {
                    return Err(err(format!(
                        "cannot infer type argument '{p}' of {what}; write it \
                         explicitly (I#161)"
                    )))
                }
            }
        }
        Ok(out)
    }

    /// Per-argument expected types for a call, so a payload-less generic variant
    /// (`Optional.None`) or any inference-ambiguous argument is rewritten with
    /// the callee's declared parameter/field type as its expected-type sink
    /// (#1342) — the way a `return` already passes the function's return type.
    ///
    /// Only **non-generic** callees contribute a type (their parameter/field
    /// types are concrete): a bare-name non-generic struct constructor
    /// (`Box(..)`) uses its field types, a non-generic free function (`take(..)`)
    /// its parameter types. A generic callee is left to the existing type-
    /// argument inference (`instance_args` / `infer_fn_targs`), which drives the
    /// whole-call `expected` sink itself; methods, enum-variant and indexed
    /// callees, spreads, and variadic/kwargs slots stay `None` (unchanged). The
    /// result aligns 1:1 with `args`; `None` reproduces the prior behavior.
    fn call_arg_expected(&self, callee: &Expr, args: &[Arg]) -> Vec<Option<Type>> {
        let none = vec![None; args.len()];
        let Expr::Name(g, _) = callee else {
            return none;
        };
        // Generic callees: their parameter/field types may mention type
        // parameters; leave them to the dedicated inference paths.
        if self.gstructs.contains_key(g) || self.gfuncs.contains_key(g) {
            return none;
        }
        // Ordered (name, declared-type) slots from the callee, stopping at the
        // first variadic / kwargs-collector parameter so a positional argument
        // that lands on or after it keeps `None` (its expected type is the
        // element/collector type, not the slot type — not modelled here).
        let slots: Vec<(String, Type)> = if let Some(s) = self.structs.get(g) {
            s.fields
                .iter()
                .map(|f| (f.name.clone(), f.ty.clone()))
                .collect()
        } else if let Some(f) = self.funcs.get(g) {
            let mut v = Vec::new();
            for p in &f.params {
                if p.variadic || p.kwargs || p.ty.is_none() {
                    break;
                }
                v.push((p.name.clone(), p.ty.clone().unwrap()));
            }
            v
        } else {
            return none;
        };

        let mut out = Vec::with_capacity(args.len());
        let mut pos = 0usize;
        let mut bail = false;
        for a in args {
            if bail || !matches!(a.spread, ArgSpread::None) {
                // A spread fills an unknown number of slots; stop assigning
                // positional expecteds once one appears (fail-safe to `None`).
                bail = true;
                out.push(None);
                continue;
            }
            let ty = match &a.name {
                Some(n) => slots.iter().find(|(sn, _)| sn == n).map(|(_, t)| t.clone()),
                None => {
                    let t = slots.get(pos).map(|(_, t)| t.clone());
                    pos += 1;
                    t
                }
            };
            out.push(ty.map(|t| self.apply_subst(&t)));
        }
        out
    }

    /// Rewrite `reflect.construct(T)` (#1454) to a zero-initialized construction
    /// of the concrete type `T` is substituted to: `ConcreteName(zero(f0), …)`.
    /// v1 supports a **non-generic struct** whose fields are scalars / str /
    /// bytes; a generic instance or an unsupported field type is a clean error
    /// (deferred, like the derive codegen's field-type restrictions).
    fn rewrite_reflect_construct(
        &self,
        ty_arg: &Expr,
        span: Span,
    ) -> Result<(Expr, CTy), LowerError> {
        let Expr::Name(t, _) = ty_arg else {
            return Err(err("reflect.construct(T) expects a type name"));
        }; // Resolve a type parameter to its concrete type; a bare concrete name
           // (not a parameter) is used as-is.
        let concrete = match self.cur_subst.get(t) {
            Some(Type::Named { name, args, .. }) if args.is_empty() => name.clone(),
            Some(Type::Named { name, .. }) => {
                return Err(err(format!(
                    "reflect.construct: a generic instance (`{name}[…]`) is not supported yet (#1454)"
                )));
            }
            Some(_) => return Err(err("reflect.construct(T) expects a named struct type")),
            None => t.clone(),
        };
        let Some(def) = self.structs.get(concrete.as_str()).copied() else {
            return Err(err(format!(
                "reflect.construct: `{concrete}` is not a non-generic struct in scope (#1454)"
            )));
        };
        let mut zargs = Vec::with_capacity(def.fields.len());
        for f in &def.fields {
            let z = zero_expr(&f.ty, span).ok_or_else(|| {
                err(format!(
                    "reflect.construct: field `{}` of `{concrete}` has a type the zero-initializer \
                     does not support yet (scalars / str / bytes only in v1, #1454)",
                    f.name
                ))
            })?;
            zargs.push(Arg {
                name: None,
                value: z,
                spread: ArgSpread::None,
                span,
            });
        }
        let call = Expr::Call {
            callee: Box::new(Expr::Name(concrete.clone(), span)),
            args: zargs,
            span,
        };
        Ok((call, CTy::Ty(self.named(&concrete, Vec::new()))))
    }

    fn rewrite_call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        span: Span,
        env: &mut Env,
        expected: Option<&Type>,
    ) -> Result<(Expr, CTy), LowerError> {
        // Module-qualified construction of a seam-exposed prelude generic —
        // `concurrent.Queue[T](...)` / `concurrent.Queue(...)` — is sugar for the
        // bare prelude constructor `new_queue[T](...)` / `new_queue(...)` (I#996,
        // D-0202). Desugar the callee up front so the existing bare-name generic
        // paths below (explicit type args, or inference from `expected`) key on
        // the constructor the monomorphizer already knows. The sema facet applies
        // the twin rewrite on the type-check path.
        let desugared = zapc_syntax::seam::desugar_seam_ctor_callee(callee);
        let callee = desugared.as_ref().unwrap_or(callee);
        // `reflect.construct(T)` (#1454, E4): a zero-initialized value of `T`, the
        // base of the field-set builder a codec decoder uses before populating
        // each field with `reflect.set_field`. The argument is a TYPE (not a
        // value), so it is handled before the ordinary argument rewrite; at
        // specialization `T` is substituted to a concrete type and this rewrites
        // to `ConcreteName(zero(f0), zero(f1), …)`.
        if let Expr::Attr { base, name, .. } = callee {
            if name == "construct"
                && matches!(&**base, Expr::Name(m, _) if m == "reflect")
                && args.len() == 1
            {
                return self.rewrite_reflect_construct(&args[0].value, span);
            }
        }
        // Rewrite the arguments once; keep their inferred types for unification.
        // Each argument is rewritten with the callee's declared parameter/field
        // type as its expected-type sink (#1342), so a payload-less
        // `Optional.None` (or any inference-ambiguous argument) pins its type
        // argument from the callee the way a `return` does from the return type.
        let arg_expected = self.call_arg_expected(callee, args);
        let mut nargs = Vec::with_capacity(args.len());
        let mut argpairs = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let exp = arg_expected.get(i).and_then(|t| t.as_ref());
            let (v, cty) = self.rewrite_expr(&a.value, env, exp)?;
            argpairs.push((a.name.clone(), cty));
            nargs.push(Arg {
                name: a.name.clone(),
                value: v,
                spread: a.spread,
                span: a.span,
            });
        }

        // Generic enum variant construction: `G.Variant(...)` or
        // `G[args].Variant(...)`.
        if let Expr::Attr {
            base,
            name: variant,
            span: aspan,
        } = callee
        {
            if let Expr::Name(g, _) = base.as_ref() {
                if self.is_generic_enum_variant(g, variant) {
                    let v = self.genums[g.as_str()]
                        .variants
                        .iter()
                        .find(|v| &v.name == variant)
                        .unwrap();
                    let fields: Vec<(String, Type)> = v
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect();
                    let cargs = self.instance_args(g, &fields, &argpairs, expected)?;
                    let mangled = self.request(&g.clone(), &cargs);
                    let node = Expr::Call {
                        callee: Box::new(Expr::Attr {
                            base: Box::new(Expr::Name(mangled, span)),
                            name: variant.clone(),
                            span: *aspan,
                        }),
                        args: nargs,
                        span,
                    };
                    return Ok((node, CTy::Ty(self.named(g, cargs))));
                }
                // Non-generic enum variant construction: leave the base, keep args.
                if self.enums.contains_key(g) {
                    return Ok((
                        Expr::Call {
                            callee: Box::new(callee.clone()),
                            args: nargs,
                            span,
                        },
                        CTy::Ty(self.named(g, Vec::new())),
                    ));
                }
            }
            // Explicit type args on an enum variant: `G[args].Variant(...)`.
            if let Expr::Index {
                base: ibase, index, ..
            } = base.as_ref()
            {
                if let Expr::Name(g, _) = ibase.as_ref() {
                    if self.genums.contains_key(g) {
                        if let Some(cargs) =
                            self.type_args_from_index(index, self.type_param_count(g))
                        {
                            let mangled = self.request(&g.clone(), &cargs);
                            let node = Expr::Call {
                                callee: Box::new(Expr::Attr {
                                    base: Box::new(Expr::Name(mangled, span)),
                                    name: variant.clone(),
                                    span: *aspan,
                                }),
                                args: nargs,
                                span,
                            };
                            return Ok((node, CTy::Ty(self.named(g, cargs))));
                        }
                    }
                }
            }
            // Built-in `datetime.zone_abbreviation(dt) -> Optional[str]` (#1345).
            // The payload is always `str`, but lowering needs the concrete
            // `Optional$str` instance to build the niche result and cannot name
            // it from the lossy element kind alone — so request that instance
            // here (where it can be named) and encode its mangled name into the
            // method symbol (`zone_abbreviation$Optional$str`), which lowering
            // decodes. Mirrors the `dict.get` → `get$Optional$str` precedent.
            // Fires only for the genuine module call, not a local shadowing
            // `datetime` or a user type's own `.zone_abbreviation`.
            if variant == "zone_abbreviation" {
                if let Expr::Name(m, _) = base.as_ref() {
                    if m == "datetime" && env.lookup(m).is_none() {
                        let str_ty = self.named("str", Vec::new());
                        let opt_mangled = self.request("Optional", std::slice::from_ref(&str_ty));
                        let node = Expr::Call {
                            callee: Box::new(Expr::Attr {
                                base: base.clone(),
                                name: format!("zone_abbreviation${opt_mangled}"),
                                span: *aspan,
                            }),
                            args: nargs,
                            span,
                        };
                        return Ok((node, CTy::Ty(self.named("Optional", vec![str_ty]))));
                    }
                }
            }
            // Ordinary method call `recv.m(args)`.
            let (bexpr, bcty) = self.rewrite_expr(base, env, None)?;
            // Built-in `dict.get(key) -> Optional[V]` (ticket #281): a safe read.
            // Lowering sees only the lossy element *kind* (ptr vs scalar), never
            // the precise value type, so it cannot pick the right `Optional$V`
            // enum instance on its own. Here — where the concrete `V` is known —
            // request that instance and encode its mangled name into the method
            // symbol (`get$Optional$str`), which lowering decodes. `V` comes from
            // the receiver's dict type, or (when the receiver's type is opaque to
            // this inferer) from the expected `Optional[..]` sink of the binding.
            //
            // This dict special-casing must never shadow a user type's own `get`
            // method (#1477). A concrete receiver that declares its own `get`
            // dispatches through the ordinary path below — even when the call's
            // expected type is `Optional[..]`, which the `optional_payload_ty`
            // fallback would otherwise latch onto and rewrite to a bogus
            // `get$Optional$V` symbol that no such method exists under. A genuine
            // `dict` receiver (or one opaque to this inferer, e.g. a type
            // parameter) names no user struct/enum, so it still takes the dict
            // path. The `find_method` probe keys on the receiver's declared type
            // exactly as the generic-method path below does.
            let recv_has_own_get = matches!(
                &bcty,
                CTy::Ty(Type::Named { name, .. }) if self.find_method(name, "get").is_some()
            );
            if variant == "get" && !recv_has_own_get {
                // Two-arg `get(key, default) -> V` (ticket #570): returns the
                // stored value when present, else the default — a plain `V`, with
                // no `Optional` wrapper. Lowering uses only the value element
                // *kind* (which it has), so no `Optional$V` instance is needed;
                // the symbol is marked `get_default` so lowering takes the 2-arg
                // path. `V` is the receiver's dict value type — this fires *only*
                // when the receiver is genuinely a `dict`, so an unrelated two-arg
                // `get` (e.g. the `deque.get(d, i)` module function, or a user
                // type's own `get`) is left untouched for normal dispatch.
                if nargs.len() == 2 {
                    if let Some(vty) = dict_value_ty(&bcty) {
                        let node = Expr::Call {
                            callee: Box::new(Expr::Attr {
                                base: Box::new(bexpr),
                                name: "get_default".to_string(),
                                span: *aspan,
                            }),
                            args: nargs,
                            span,
                        };
                        return Ok((node, CTy::Ty(vty)));
                    }
                }
                if let Some(vty) = dict_value_ty(&bcty).or_else(|| optional_payload_ty(expected)) {
                    let opt_mangled = self.request("Optional", std::slice::from_ref(&vty));
                    let node = Expr::Call {
                        callee: Box::new(Expr::Attr {
                            base: Box::new(bexpr),
                            name: format!("get${opt_mangled}"),
                            span: *aspan,
                        }),
                        args: nargs,
                        span,
                    };
                    return Ok((node, CTy::Ty(self.named("Optional", vec![vty]))));
                }
            }
            // Generic method call (I#161): the method carries its own type
            // parameters. Infer them (argument unification + expected-type sink)
            // and dispatch to the specialized method symbol on the receiver's
            // concrete type.
            if let CTy::Ty(Type::Named {
                name: owner,
                args: owner_args,
                ..
            }) = &bcty
            {
                if let Some(method) = self.find_method(owner, variant) {
                    if !method.generics.is_empty() {
                        let (owner, owner_args) = (owner.clone(), owner_args.clone());
                        let tparams: Vec<String> =
                            method.generics.iter().map(|g| g.name.clone()).collect();
                        let owner_subst = self.subst_map(&owner, &owner_args);
                        let targs = self.infer_fn_targs(
                            method,
                            &owner_subst,
                            &argpairs,
                            expected,
                            &format!("method '{variant}' of '{owner}'"),
                            true,
                        )?;
                        let ret_cty =
                            self.fn_ret_cty(method.ret.as_ref(), &owner_subst, &tparams, &targs);
                        let method_spec = self.request_method(&owner, &owner_args, variant, &targs);
                        let node = Expr::Call {
                            callee: Box::new(Expr::Attr {
                                base: Box::new(bexpr),
                                name: method_spec,
                                span: *aspan,
                            }),
                            args: nargs,
                            span,
                        };
                        return Ok((node, ret_cty));
                    }
                }
            }
            let ret = match &bcty {
                CTy::Ty(bt) => self.method_ret(bt, variant),
                _ => CTy::Unknown,
            };
            let node = Expr::Call {
                callee: Box::new(Expr::Attr {
                    base: Box::new(bexpr),
                    name: variant.clone(),
                    span: *aspan,
                }),
                args: nargs,
                span,
            };
            return Ok((node, ret));
        }

        // Generic struct construction by bare name `G(args)`.
        if let Expr::Name(g, nspan) = callee {
            if self.gstructs.contains_key(g) {
                let fields: Vec<(String, Type)> = self.gstructs[g.as_str()]
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.ty.clone()))
                    .collect();
                let cargs = self.instance_args(g, &fields, &argpairs, expected)?;
                let mangled = self.request(&g.clone(), &cargs);
                let node = Expr::Call {
                    callee: Box::new(Expr::Name(mangled, *nspan)),
                    args: nargs,
                    span,
                };
                return Ok((node, CTy::Ty(self.named(g, cargs))));
            }
            // Generic free-function call `f(args)` (I#161): infer the type args.
            if let Some(f) = self.gfuncs.get(g).copied() {
                let tparams: Vec<String> = f.generics.iter().map(|p| p.name.clone()).collect();
                let empty = HashMap::new();
                let targs = self.infer_fn_targs(
                    f,
                    &empty,
                    &argpairs,
                    expected,
                    &format!("function '{g}'"),
                    false,
                )?;
                let ret_cty = self.fn_ret_cty(f.ret.as_ref(), &empty, &tparams, &targs);
                let mangled = self.request_func(&g.clone(), &targs);
                let node = Expr::Call {
                    callee: Box::new(Expr::Name(mangled, *nspan)),
                    args: nargs,
                    span,
                };
                return Ok((node, ret_cty));
            }
            // Non-generic struct construction or user function call.
            let cty = if self.structs.contains_key(g) || self.enums.contains_key(g) {
                CTy::Ty(self.named(g, Vec::new()))
            } else if let Some(Some(rt)) = self.func_ret.get(g) {
                CTy::Ty(self.apply_subst(&rt.clone()))
            } else {
                CTy::Unknown
            };
            return Ok((
                Expr::Call {
                    callee: Box::new(callee.clone()),
                    args: nargs,
                    span,
                },
                cty,
            ));
        }

        // Explicit type args on a generic callee `G[args](...)` /
        // `f[args](...)` / `recv.m[args](...)` (I#161, Fork S).
        if let Expr::Index { base, index, .. } = callee {
            if let Expr::Name(g, nspan) = base.as_ref() {
                // Generic struct construction with explicit type args.
                if self.gstructs.contains_key(g) {
                    if let Some(cargs) = self.type_args_from_index(index, self.type_param_count(g))
                    {
                        let mangled = self.request(&g.clone(), &cargs);
                        let node = Expr::Call {
                            callee: Box::new(Expr::Name(mangled, *nspan)),
                            args: nargs,
                            span,
                        };
                        return Ok((node, CTy::Ty(self.named(g, cargs))));
                    }
                }
                // Generic free-function call with explicit type args `f[args](...)`.
                if let Some(f) = self.gfuncs.get(g).copied() {
                    if let Some(targs) = self.type_args_from_index(index, f.generics.len()) {
                        let tparams: Vec<String> =
                            f.generics.iter().map(|p| p.name.clone()).collect();
                        let empty = HashMap::new();
                        let ret_cty = self.fn_ret_cty(f.ret.as_ref(), &empty, &tparams, &targs);
                        let mangled = self.request_func(&g.clone(), &targs);
                        let node = Expr::Call {
                            callee: Box::new(Expr::Name(mangled, *nspan)),
                            args: nargs,
                            span,
                        };
                        return Ok((node, ret_cty));
                    }
                }
                // Builtin collection constructors with a type-argument subscript
                // — `list[T]()`, `dict[K,V]()`, `set[T]()`, `heap[T]()`,
                // `channel[T]()` (#1013). Unlike user generics these are not in
                // `gstructs`/`gfuncs`, so without this the monomorphiser leaves
                // the subscript's type parameters unsubstituted and a
                // type-parameter element (e.g. `list[T]()` in a generic body)
                // reaches codegen as a bare `T` ("type 'T' not supported").
                // Re-materialise the subscript from the substituted concrete
                // types; for a concrete call this reproduces the identical
                // subscript, so there is no change on the non-generic path.
                if matches!(
                    g.as_str(),
                    "list" | "dict" | "set" | "heap" | "channel" | "deque" | "oset"
                ) {
                    // `dict` is the only two-parameter builtin (`dict[K, V]`);
                    // the rest are single-element containers whose one argument
                    // may itself be a tuple (`list[(A, B)]`).
                    let nparams = if g == "dict" { 2 } else { 1 };
                    if let Some(cargs) = self.type_args_from_index(index, nparams) {
                        if let Some(new_index) = self.ctor_subscript_from_types(&cargs, *nspan) {
                            let node = Expr::Call {
                                callee: Box::new(Expr::Index {
                                    base: Box::new(Expr::Name(g.clone(), *nspan)),
                                    index: Box::new(new_index),
                                    span: *nspan,
                                }),
                                args: nargs,
                                span,
                            };
                            return Ok((node, CTy::Unknown));
                        }
                    }
                }
            }
            // Generic method call with explicit type args `recv.m[args](...)`.
            if let Expr::Attr {
                base: recv,
                name: m,
                span: aspan,
            } = base.as_ref()
            {
                let (rexpr, rcty) = self.rewrite_expr(recv, env, None)?;
                if let CTy::Ty(Type::Named {
                    name: owner,
                    args: owner_args,
                    ..
                }) = &rcty
                {
                    if let Some(method) = self.find_method(owner, m) {
                        if !method.generics.is_empty() {
                            if let Some(targs) =
                                self.type_args_from_index(index, method.generics.len())
                            {
                                let (owner, owner_args) = (owner.clone(), owner_args.clone());
                                let tparams: Vec<String> =
                                    method.generics.iter().map(|g| g.name.clone()).collect();
                                let owner_subst = self.subst_map(&owner, &owner_args);
                                let ret_cty = self.fn_ret_cty(
                                    method.ret.as_ref(),
                                    &owner_subst,
                                    &tparams,
                                    &targs,
                                );
                                let method_spec =
                                    self.request_method(&owner, &owner_args, m, &targs);
                                let node = Expr::Call {
                                    callee: Box::new(Expr::Attr {
                                        base: Box::new(rexpr),
                                        name: method_spec,
                                        span: *aspan,
                                    }),
                                    args: nargs,
                                    span,
                                };
                                return Ok((node, ret_cty));
                            }
                        }
                    }
                }
            }
        }

        // Fallback: rewrite the callee structurally.
        let callee = self.rw(callee, env)?;
        Ok((
            Expr::Call {
                callee: Box::new(callee),
                args: nargs,
                span,
            },
            CTy::Unknown,
        ))
    }

    fn rewrite_attr(
        &mut self,
        base: &Expr,
        name: &str,
        span: Span,
        env: &mut Env,
        expected: Option<&Type>,
    ) -> Result<(Expr, CTy), LowerError> {
        // `reflect.type_info(T).name` as a VALUE (#1430, E4): fold to the concrete
        // type's name as a string literal during specialization, so a derive
        // generator can build `TypeName(...)` (e.g. a library `Repr`). The
        // comptime engine already evaluates `.name`, but only in a comptime-for
        // iterator or a kind-dispatch `if` condition; used as a plain value it
        // otherwise reaches lowering as an unresolved `reflect`. Resolved only
        // when `T` is substituted to a concrete type (an active instantiation);
        // a still-generic `reflect.type_info(T)` falls through unchanged.
        if name == "name" {
            if let Some(tn) = reflect_type_info_arg(base) {
                if let Some(Type::Named { name: concrete, .. }) = self.cur_subst.get(&tn) {
                    return Ok((
                        Expr::Str {
                            value: concrete.clone(),
                            raw: false,
                            bytes: false,
                            fstring: false,
                            span,
                            fstring_fields: Vec::new(),
                        },
                        CTy::Ty(self.named("str", Vec::new())),
                    ));
                }
            }
        }
        // Payload-free generic enum variant value: `G.None` / `G[args].None`.
        if let Expr::Name(g, bspan) = base {
            if self.is_generic_enum_variant(g, name) {
                let v = self.genums[g.as_str()]
                    .variants
                    .iter()
                    .find(|v| v.name == name)
                    .unwrap();
                let fields: Vec<(String, Type)> = v
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.ty.clone()))
                    .collect();
                let cargs = self.instance_args(g, &fields, &[], expected)?;
                let mangled = self.request(&g.clone(), &cargs);
                return Ok((
                    Expr::Attr {
                        base: Box::new(Expr::Name(mangled, *bspan)),
                        name: name.to_string(),
                        span,
                    },
                    CTy::Ty(self.named(g, cargs)),
                ));
            }
            // Non-generic enum member/variant value: leave as-is.
            if self.enums.contains_key(g) {
                return Ok((
                    Expr::Attr {
                        base: Box::new(base.clone()),
                        name: name.to_string(),
                        span,
                    },
                    CTy::Ty(self.named(g, Vec::new())),
                ));
            }
        }
        if let Expr::Index {
            base: ibase, index, ..
        } = base
        {
            if let Expr::Name(g, bspan) = ibase.as_ref() {
                if self.genums.contains_key(g) {
                    if let Some(cargs) = self.type_args_from_index(index, self.type_param_count(g))
                    {
                        let mangled = self.request(&g.clone(), &cargs);
                        return Ok((
                            Expr::Attr {
                                base: Box::new(Expr::Name(mangled, *bspan)),
                                name: name.to_string(),
                                span,
                            },
                            CTy::Ty(self.named(g, cargs)),
                        ));
                    }
                }
            }
        }
        // Ordinary field access (or module constant): rewrite the base and infer
        // the field type for a struct receiver.
        let (bexpr, bcty) = self.rewrite_expr(base, env, None)?;
        let cty = match &bcty {
            CTy::Ty(bt) => self.field_ty(bt, name),
            _ => CTy::Unknown,
        };
        Ok((
            Expr::Attr {
                base: Box::new(bexpr),
                name: name.to_string(),
                span,
            },
            cty,
        ))
    }

    fn named(&self, name: &str, args: Vec<Type>) -> Type {
        Type::Named {
            name: name.to_string(),
            args,
            span: self.span,
        }
    }

    /// The call result type for a specialized generic function/method (I#161):
    /// the declared return type with the owner substitution and the inferred
    /// (type-param -> concrete) bindings applied, in generic form (the
    /// nested-generic renaming happens later, during type rewriting).
    fn fn_ret_cty(
        &self,
        ret: Option<&Type>,
        owner_subst: &HashMap<String, Type>,
        tparams: &[String],
        targs: &[Type],
    ) -> CTy {
        match ret {
            Some(rt) => {
                let mut subst = owner_subst.clone();
                for (p, a) in tparams.iter().zip(targs.iter()) {
                    subst.insert(p.clone(), a.clone());
                }
                CTy::Ty(subst_with(rt, &subst))
            }
            None => CTy::Unknown,
        }
    }

    /// Rewrite a match pattern, renaming a generic-enum variant path to its
    /// concrete instance (from the scrutinee type) and declaring the variant's
    /// payload captures with their concrete field types.
    fn rewrite_pattern(
        &mut self,
        p: &Pattern,
        scty: Option<&Type>,
        env: &mut Env,
    ) -> Result<Pattern, LowerError> {
        match p {
            Pattern::Struct { path, args, span } if path.len() >= 2 => {
                let g = &path[0];
                let variant = &path[1];
                if self.genums.contains_key(g) {
                    // Instance args come from the scrutinee type.
                    let cargs = match scty {
                        Some(Type::Named { name, args, .. }) if name == g => args.clone(),
                        _ => {
                            return Err(err(format!(
                                "cannot determine the instance of generic enum '{g}' for this \
                                 match pattern; annotate the matched value's type (I#154)"
                            )))
                        }
                    };
                    let mangled = self.request(&g.clone(), &cargs);
                    let mut npath = path.clone();
                    npath[0] = mangled;
                    // Bind payload captures with substituted field types.
                    let new_args = self.bind_pattern_args(g, variant, &cargs, args, env)?;
                    return Ok(Pattern::Struct {
                        path: npath,
                        args: new_args,
                        span: *span,
                    });
                }
                // Non-generic enum variant pattern: bind captures with field types.
                let new_args = self.bind_nongeneric_pattern_args(g, variant, args, env)?;
                Ok(Pattern::Struct {
                    path: path.clone(),
                    args: new_args,
                    span: *span,
                })
            }
            Pattern::Capture { name, span } => {
                if let Some(t) = scty {
                    env.declare(name, CTy::Ty(t.clone()));
                }
                Ok(Pattern::Capture {
                    name: name.clone(),
                    span: *span,
                })
            }
            Pattern::Or(ps, span) => {
                let mut np = Vec::with_capacity(ps.len());
                for sub in ps {
                    np.push(self.rewrite_pattern(sub, scty, env)?);
                }
                Ok(Pattern::Or(np, *span))
            }
            Pattern::As {
                pattern,
                name,
                span,
            } => {
                // Rewrite the inner pattern (so any generic-enum captures get
                // concrete field types), then declare the `as` capture with the
                // scrutinee's concrete type (D11, #603).
                let inner = self.rewrite_pattern(pattern, scty, env)?;
                if let Some(t) = scty {
                    env.declare(name, CTy::Ty(t.clone()));
                }
                Ok(Pattern::As {
                    pattern: Box::new(inner),
                    name: name.clone(),
                    span: *span,
                })
            }
            other => Ok(other.clone()),
        }
    }

    fn bind_pattern_args(
        &mut self,
        g: &str,
        variant: &str,
        cargs: &[Type],
        args: &Option<Vec<PatArg>>,
        env: &mut Env,
    ) -> Result<Option<Vec<PatArg>>, LowerError> {
        let Some(args) = args else { return Ok(None) };
        let map = self.subst_map(g, cargs);
        let vfields: Vec<(String, Type)> = self.genums[g]
            .variants
            .iter()
            .find(|v| v.name == variant)
            .map(|v| {
                v.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.ty.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut pos = 0usize;
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            let field = match &a.name {
                Some(fname) => vfields.iter().find(|(n, _)| n == fname).map(|(_, t)| t),
                None => {
                    let t = vfields.get(pos).map(|(_, t)| t);
                    pos += 1;
                    t
                }
            };
            if let (Pattern::Capture { name, .. }, Some(ft)) = (&a.pattern, field) {
                env.declare(name, CTy::Ty(subst_with(ft, &map)));
            }
            out.push(a.clone());
        }
        Ok(Some(out))
    }

    fn bind_nongeneric_pattern_args(
        &mut self,
        g: &str,
        variant: &str,
        args: &Option<Vec<PatArg>>,
        env: &mut Env,
    ) -> Result<Option<Vec<PatArg>>, LowerError> {
        let Some(args) = args else { return Ok(None) };
        let vfields: Vec<(String, Type)> = self
            .enums
            .get(g)
            .and_then(|e| e.variants.iter().find(|v| v.name == variant))
            .map(|v| {
                v.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.ty.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut pos = 0usize;
        for a in args {
            let field = match &a.name {
                Some(fname) => vfields.iter().find(|(n, _)| n == fname).map(|(_, t)| t),
                None => {
                    let t = vfields.get(pos).map(|(_, t)| t);
                    pos += 1;
                    t
                }
            };
            if let (Pattern::Capture { name, .. }, Some(ft)) = (&a.pattern, field) {
                env.declare(name, CTy::Ty(ft.clone()));
            }
        }
        Ok(Some(args.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zapc_syntax::parser::parse_str;

    fn mono_ok(src: &str) -> Module {
        let (module, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        monomorphize(&module).expect("monomorphization succeeds")
    }

    fn fn_names(m: &Module) -> Vec<String> {
        m.items
            .iter()
            .filter_map(|i| match i {
                Item::Function(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect()
    }

    fn struct_named<'a>(m: &'a Module, name: &str) -> Option<&'a StructDef> {
        m.items.iter().find_map(|i| match i {
            Item::Struct(s) if s.name == name => Some(s),
            _ => None,
        })
    }

    // ---- derive `required because of …` chain builder (#1293) -------------

    /// Parse a one-field struct and return (the field, the module) so a test can
    /// navigate the field's declared type AST (with real spans).
    fn one_field_struct(src: &str) -> Module {
        let (module, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty() && lerrs.is_empty(), "{perrs:?} {lerrs:?}");
        module
    }

    #[test]
    fn render_type_reconstructs_container_labels() {
        let m = one_field_struct(
            "struct Widget:\n    int64 w\nstruct Bag:\n    dict[str, list[Widget]] b\n",
        );
        let field = &struct_named(&m, "Bag").unwrap().fields[0];
        assert_eq!(render_type(&field.ty), "dict[str, list[Widget]]");
    }

    #[test]
    fn chain_is_built_outermost_first_with_nested_spans() {
        // `dict[str, list[Widget]] buckets`, cause = the inner `Widget`.
        let m = one_field_struct(
            "struct Widget:\n    int64 w\nstruct Bag:\n    dict[str, list[Widget]] buckets\n",
        );
        let field = &struct_named(&m, "Bag").unwrap().fields[0];
        // Navigate dict -> value `list[Widget]` -> element `Widget`.
        let Type::Named { args: dargs, .. } = &field.ty else {
            panic!("dict");
        };
        let Type::Named { args: largs, .. } = &dargs[1] else {
            panic!("list");
        };
        let cause = largs[0].span();

        let mut frames = vec![(format!("field `{}` of `Bag`", field.name), field.span)];
        push_container_frames(&mut frames, &field.ty, cause);

        let labels: Vec<&str> = frames.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "field `buckets` of `Bag`",
                "the value type of `dict[str, list[Widget]]`",
                "the element type of `list[Widget]`",
            ],
            "frames are outermost-first; the renderer prints them inner-first"
        );
        // Spans zoom inward: each frame encloses the next, and the innermost
        // encloses the cause.
        for w in frames.windows(2) {
            assert!(
                span_contains(w[0].1, w[1].1),
                "{:?} should enclose {:?}",
                w[0],
                w[1]
            );
        }
        assert!(span_contains(frames.last().unwrap().1, cause));
    }

    #[test]
    fn chain_names_tuple_element_position_one_based() {
        let m = one_field_struct(
            "struct Widget:\n    int64 w\nstruct Bag:\n    (int64, Widget) pair\n",
        );
        let field = &struct_named(&m, "Bag").unwrap().fields[0];
        let Type::Tuple { elems, .. } = &field.ty else {
            panic!("tuple");
        };
        let cause = elems[1].span(); // the `Widget`, second element

        let mut frames = Vec::new();
        push_container_frames(&mut frames, &field.ty, cause);
        assert_eq!(
            frames.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
            vec!["element 2 of `(int64, Widget)`"],
        );
    }

    #[test]
    fn chain_follows_only_the_child_on_the_path() {
        // dict key `str` is off the path; only the value branch is walked.
        let m =
            one_field_struct("struct Widget:\n    int64 w\nstruct Bag:\n    dict[str, Widget] m\n");
        let field = &struct_named(&m, "Bag").unwrap().fields[0];
        let Type::Named { args, .. } = &field.ty else {
            panic!("dict");
        };
        let cause = args[1].span(); // the value `Widget`

        let mut frames = Vec::new();
        push_container_frames(&mut frames, &field.ty, cause);
        assert_eq!(
            frames.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
            vec!["the value type of `dict[str, Widget]`"],
        );
    }

    #[test]
    fn chain_is_empty_when_cause_is_the_whole_field_type() {
        // A generator pointing at a plain struct field (`comptime.error(f.span)`
        // or `f.ty.span`) crosses no container level.
        let m = one_field_struct("struct Widget:\n    int64 w\nstruct Bag:\n    Widget w\n");
        let field = &struct_named(&m, "Bag").unwrap().fields[0];
        let mut frames = Vec::new();
        push_container_frames(&mut frames, &field.ty, field.ty.span());
        assert!(
            frames.is_empty(),
            "no container levels for a bare struct field"
        );
    }

    // ---- transparent type aliases (#606, D-0105) --------------------------

    #[test]
    fn plain_type_alias_is_erased_and_expanded() {
        // `Count = int64` used in a struct field becomes `int64`; the alias item
        // itself emits no declaration.
        let m =
            mono_ok("type Count = int64\nstruct Row:\n    Count n\ndef main():\n    print(1)\n");
        assert!(
            !m.items.iter().any(|i| matches!(i, Item::TypeAlias(_))),
            "a transparent alias must leave no declaration behind"
        );
        let s = struct_named(&m, "Row").expect("Row emitted");
        assert_eq!(type_name(&s.fields[0].ty), "int64");
    }

    #[test]
    fn generic_type_alias_expands_to_its_target() {
        // `type Pair[T] = (T, T)` used as `Pair[int64]` expands to the concrete
        // tuple `(int64, int64)`.
        let m = mono_ok(
            "type Pair[T] = (T, T)\nstruct Row:\n    Pair[int64] p\ndef main():\n    print(1)\n",
        );
        let s = struct_named(&m, "Row").expect("Row emitted");
        assert_eq!(type_name(&s.fields[0].ty), "(int64, int64)");
    }

    #[test]
    fn type_alias_to_generic_struct_participates_in_monomorphization() {
        // `type Boxed = Box[int64]` expands to the generic instance, which mono
        // then requests and mangles alongside the existing D-0017 generics: the
        // field is typed `Box$int64` and that instance declaration is emitted.
        let m = mono_ok(
            "struct Box[T]:\n    T value\ntype Boxed = Box[int64]\n\
             struct Row:\n    Boxed b\ndef main():\n    print(1)\n",
        );
        let s = struct_named(&m, "Row").expect("Row emitted");
        assert_eq!(type_name(&s.fields[0].ty), "Box$int64");
        assert!(
            struct_named(&m, "Box$int64").is_some(),
            "the alias's generic target instance must be emitted"
        );
    }

    #[test]
    fn generic_fn_is_monomorphized_and_deduped() {
        // identity is instantiated at int64 twice (dedup -> one body) and str
        // once; the generic template itself is erased (I#161).
        let m = mono_ok(
            "def identity[T](T x) -> T:\n    return x\n\
             def main():\n    print(identity(1))\n    print(identity(2))\n    print(identity(\"a\"))\n",
        );
        let names = fn_names(&m);
        assert_eq!(
            names.iter().filter(|n| *n == "identity$int64").count(),
            1,
            "identity$int64 should be emitted exactly once (dedup): {names:?}"
        );
        assert_eq!(
            names.iter().filter(|n| *n == "identity$str").count(),
            1,
            "identity$str should be emitted once: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "identity"),
            "the generic template must be erased: {names:?}"
        );
    }

    #[test]
    fn uninstantiated_generic_fn_is_dropped() {
        let m = mono_ok("def unused[T](T x) -> T:\n    return x\ndef main():\n    print(1)\n");
        let names = fn_names(&m);
        assert!(
            names.iter().all(|n| !n.starts_with("unused")),
            "a never-instantiated generic fn emits nothing: {names:?}"
        );
    }

    #[test]
    fn generic_method_attaches_to_concrete_owner() {
        // A generic method with its own type parameter is specialized per call
        // and attached to the owner's concrete instance (I#161).
        let m = mono_ok(
            "struct Box[T]:\n    T value\n    def repack[U](self, U y) -> Box[U]:\n        return Box(y)\n\
             def main():\n    Box[int64] b = Box(3)\n    Box[str] c = b.repack(\"s\")\n    print(c.value)\n",
        );
        let owner = struct_named(&m, "Box$int64").expect("Box$int64 emitted");
        assert!(
            owner.methods.iter().any(|mm| mm.name == "repack$str"),
            "the specialized repack$str must be attached to Box$int64: {:?}",
            owner.methods.iter().map(|mm| &mm.name).collect::<Vec<_>>()
        );
        // Box$str is the method's return instance, discovered transitively.
        assert!(struct_named(&m, "Box$str").is_some(), "Box$str discovered");
    }

    // ---- #1478 / #1479: tuple types as explicit generic type arguments -----
    //
    // The parser renders `g[A, B]` and `g[(A, B)]` identically as a tuple
    // index, so the number of declared type parameters is the only thing that
    // tells "two type arguments" apart from "one tuple-typed argument". These
    // pin the rewrite that decides it: a one-parameter generic keeps the whole
    // tuple as a single argument, a two-parameter one still splits, and a
    // tuple nested inside a type argument resolves rather than vanishing (which
    // used to leave the call unrecognized, mis-lowered as a field access).

    const OPT_ENUM: &str = "enum Optional[T]:\n    Some(T value)\n    None\n";

    #[test]
    fn tuple_type_arg_binds_single_param_as_one_tuple() {
        // #1478: `run[(Optional[bytes], Optional[bytes])]` on a one-parameter
        // `run[T]` binds `T` to the whole 2-tuple — the symbol carries a single
        // `tuple$...` argument, never two split `Optional$bytes` arguments.
        let m = mono_ok(&format!(
            "{OPT_ENUM}struct Router:\n    int64 unused\n\
             \x20   def run[T](self, T fallback) -> T:\n        return fallback\n\
             def main():\n    Router r = Router(0)\n\
             \x20   auto v = r.run[(Optional[bytes], Optional[bytes])]((None, None))\n\
             \x20   auto a, b = v\n    print(a.is_none())\n"
        ));
        let syms = method_call_syms(&m);
        assert!(
            syms.iter()
                .any(|s| s == "run$tuple$Optional$bytes$Optional$bytes"),
            "the single tuple type argument must mangle as one `tuple$...`: {syms:?}"
        );
        assert!(
            !syms.iter().any(|s| s == "run" || s == "run$Optional$bytes"),
            "the call must not stay a bare `run` nor split the tuple into \
             `run$Optional$bytes`: {syms:?}"
        );
    }

    #[test]
    fn nested_tuple_in_dict_type_arg_is_recognized() {
        // #1479: `run[dict[bytes, (bytes, bytes)]]` — the nested `(bytes, bytes)`
        // value type must resolve so the call is recognized as a generic method
        // (a `run$dict$...` symbol), not fall through to a field access.
        let m = mono_ok(
            "struct Router:\n    int64 unused\n\
             \x20   def run[T](self, T fallback) -> T:\n        return fallback\n\
             def main():\n    Router r = Router(0)\n\
             \x20   dict[bytes, (bytes, bytes)] empty = {}\n\
             \x20   auto d = r.run[dict[bytes, (bytes, bytes)]](empty)\n    print(len(d))\n",
        );
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "run$dict$bytes$tuple$bytes$bytes"),
            "the dict-of-tuple type argument must be recognized and mangled: {syms:?}"
        );
    }

    #[test]
    fn two_parameter_method_still_splits_tuple_index() {
        // Arity guard: a two-parameter generic method whose index arity matches
        // (`two[int64, str]`) still splits into two separate type arguments.
        let m = mono_ok(
            "struct Pair:\n    int64 unused\n\
             \x20   def two[A, B](self, A a, B b) -> B:\n        return b\n\
             def main():\n    Pair p = Pair(0)\n    print(p.two[int64, str](1, \"x\"))\n",
        );
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "two$int64$str"),
            "a two-parameter method must split its tuple index into two args: {syms:?}"
        );
    }

    #[test]
    fn single_param_free_fn_tuple_arg_not_split() {
        // The free-function path shares the same arity-gated rewrite.
        let m = mono_ok(&format!(
            "{OPT_ENUM}def run[T](T fallback) -> T:\n    return fallback\n\
             def main():\n\
             \x20   auto v = run[(Optional[bytes], Optional[bytes])]((None, None))\n\
             \x20   auto a, b = v\n    print(a.is_none())\n"
        ));
        let names = fn_names(&m);
        assert!(
            names
                .iter()
                .any(|n| n == "run$tuple$Optional$bytes$Optional$bytes"),
            "the specialized free function must carry one tuple type argument: {names:?}"
        );
    }

    /// Run monomorphization and return its error message, expecting failure.
    fn mono_err(src: &str) -> String {
        let (module, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        monomorphize(&module)
            .expect_err("monomorphization should reject the unsatisfied bound")
            .message
    }

    const DISPLAY: &str = "interface Display:\n    def show(self) -> str\n\
         struct Greeter:\n    str msg\n    def show(self) -> str:\n        return self.msg\n\
         struct Mute:\n    int64 n\n";

    #[test]
    fn satisfied_bound_dispatches_statically() {
        // A conforming instantiation lowers to a direct call of the concrete
        // method — static dispatch, no fat pointer (I#164).
        let m = mono_ok(&format!(
            "{DISPLAY}def announce[T: Display](T x) -> str:\n    return x.show()\n\
             def main():\n    print(announce(Greeter(msg=\"hi\")))\n"
        ));
        assert!(
            fn_names(&m).iter().any(|n| n == "announce$Greeter"),
            "the conforming instance announce$Greeter is emitted"
        );
    }

    #[test]
    fn unsatisfied_bound_on_function_is_rejected() {
        let msg = mono_err(&format!(
            "{DISPLAY}def announce[T: Display](T x) -> str:\n    return x.show()\n\
             def main():\n    print(announce(Mute(5)))\n"
        ));
        assert!(
            msg.contains("'Mute' does not satisfy the bound 'Display'")
                && msg.contains("missing method 'show'"),
            "clear bound-not-satisfied diagnostic, got: {msg}"
        );
    }

    #[test]
    fn unsatisfied_bound_on_type_is_rejected() {
        let msg = mono_err(&format!(
            "{DISPLAY}struct Box[T: Display]:\n    T item\n    def reveal(self) -> str:\n        return self.item.show()\n\
             def main():\n    Box[Mute] b = Box(Mute(5))\n    print(b.reveal())\n"
        ));
        assert!(
            msg.contains("'Mute' does not satisfy the bound 'Display'")
                && msg.contains("of type 'Box'"),
            "bound checked on a generic type too, got: {msg}"
        );
    }

    #[test]
    fn mismatched_signature_fails_the_bound() {
        // `Wrong.show` returns int64, not str: present but non-conforming.
        let msg = mono_err(
            "interface Display:\n    def show(self) -> str\n\
             struct Wrong:\n    int64 n\n    def show(self) -> int64:\n        return self.n\n\
             def announce[T: Display](T x) -> str:\n    return x.show()\n\
             def main():\n    print(announce(Wrong(1)))\n",
        );
        assert!(
            msg.contains("method 'show' has a mismatched signature"),
            "signature mismatch is reported, got: {msg}"
        );
    }

    // ---- generic-interface bounds (#1205) ---------------------------------
    //
    // A bound may name a *generic* interface with concrete type arguments,
    // `[T: Container[int64]]`. At each instantiation the interface's own type
    // parameters are substituted (`E := int64`) before the structural match, so
    // a conformer at that argument is accepted and a non-conformer rejected with
    // the substituted requirement named in the diagnostic.

    const CONTAINER: &str = "interface Container[E]:\n    def get(self) -> E\n\
         struct IntBox:\n    int64 v\n    def get(self) -> int64:\n        return self.v\n\
         struct StrBox:\n    str s\n    def get(self) -> str:\n        return self.s\n";

    #[test]
    fn generic_interface_bound_accepts_substituted_conformer() {
        // IntBox.get -> int64 satisfies Container[int64] (E := int64): the
        // conforming instance is emitted.
        let m = mono_ok(&format!(
            "{CONTAINER}def use[T: Container[int64]](T c) -> int64:\n    return 0\n\
             def main():\n    print(use(IntBox(v=1)))\n"
        ));
        assert!(
            fn_names(&m).iter().any(|n| n == "use$IntBox"),
            "the conforming instance use$IntBox is emitted: {:?}",
            fn_names(&m)
        );
    }

    #[test]
    fn generic_interface_bound_rejects_wrong_argument() {
        // StrBox.get -> str does not satisfy Container[int64]; the diagnostic
        // names the bound with its argument and the mismatched method.
        let msg = mono_err(&format!(
            "{CONTAINER}def use[T: Container[int64]](T c) -> int64:\n    return 0\n\
             def main():\n    print(use(StrBox(s=\"x\")))\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Container[int64]'")
                && msg.contains("method 'get' has a mismatched signature"),
            "substituted bound diagnostic, got: {msg}"
        );
    }

    #[test]
    fn generic_interface_bound_is_per_argument() {
        // The same interface with a different argument flips which type conforms:
        // StrBox satisfies Container[str], IntBox does not.
        let m = mono_ok(&format!(
            "{CONTAINER}def use[T: Container[str]](T c) -> int64:\n    return 0\n\
             def main():\n    print(use(StrBox(s=\"x\")))\n"
        ));
        assert!(fn_names(&m).iter().any(|n| n == "use$StrBox"));
        let msg = mono_err(&format!(
            "{CONTAINER}def use[T: Container[str]](T c) -> int64:\n    return 0\n\
             def main():\n    print(use(IntBox(v=1)))\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Container[str]'"),
            "got: {msg}"
        );
    }

    #[test]
    fn generic_interface_bound_on_a_type_is_checked() {
        // The bound is enforced on a generic *type*'s parameter too, not only a
        // function's — a non-conformer at the type's instantiation is rejected.
        let msg = mono_err(&format!(
            "{CONTAINER}struct Holder[T: Container[int64]]:\n    T item\n\
             def main():\n    Holder[StrBox] h = Holder(StrBox(s=\"x\"))\n    print(1)\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Container[int64]'")
                && msg.contains("of type 'Holder'"),
            "got: {msg}"
        );
    }

    #[test]
    fn two_parameter_generic_interface_bound() {
        // A two-parameter interface substitutes each argument independently (the
        // `[L, Out]` shape the operator-capability interfaces use, modeled with
        // plain methods to stay generics-only).
        let pairish =
            "interface Pairish[A, B]:\n    def first(self) -> A\n    def second(self) -> B\n\
             struct IS:\n    int64 i\n    str s\n\
             \x20   def first(self) -> int64:\n        return self.i\n\
             \x20   def second(self) -> str:\n        return self.s\n";
        let m = mono_ok(&format!(
            "{pairish}def take[T: Pairish[int64, str]](T p) -> int64:\n    return 0\n\
             def main():\n    print(take(IS(i=1, s=\"a\")))\n"
        ));
        assert!(fn_names(&m).iter().any(|n| n == "take$IS"));
        // Swapped arguments: `first` must now be str — IS no longer conforms.
        let msg = mono_err(&format!(
            "{pairish}def take[T: Pairish[str, int64]](T p) -> int64:\n    return 0\n\
             def main():\n    print(take(IS(i=1, s=\"a\")))\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Pairish[str, int64]'"),
            "got: {msg}"
        );
    }

    #[test]
    fn generic_interface_bound_arity_mismatch_is_rejected() {
        // Supplying the wrong number of type arguments to a generic-interface
        // bound is reported at the instantiation.
        let msg = mono_err(&format!(
            "{CONTAINER}def use[T: Container[int64, str]](T c) -> int64:\n    return 0\n\
             def main():\n    print(use(IntBox(v=1)))\n"
        ));
        assert!(
            msg.contains("expects 1 type argument(s)") && msg.contains("supplies 2"),
            "got: {msg}"
        );
    }

    // ---- bounds referencing other type params (#1206) ---------------------
    //
    // A parametrized bound's argument may reference the generic's own type
    // parameter: `[T: Scaler[f64, T]]`. At each instantiation the generic's
    // parameter is substituted into the bound first (`T := Meters` makes the
    // bound `Scaler[f64, Meters]`), then the interface's own parameters are
    // substituted, and the structural match runs against the result.

    const SCALER: &str = "interface Scaler[In, Out]:\n    def scale(self, In factor) -> Out\n\
         struct Meters:\n    f64 v\n    def scale(self, f64 factor) -> Meters:\n        return Meters(v=self.v * factor)\n\
         struct Bad:\n    f64 v\n    def scale(self, f64 factor) -> int64:\n        return 0\n";

    #[test]
    fn self_referential_bound_accepts_conformer() {
        // T := Meters makes the bound Scaler[f64, Meters]; Meters.scale(f64) ->
        // Meters conforms, so the instance is emitted.
        let m = mono_ok(&format!(
            "{SCALER}def use[T: Scaler[f64, T]](T x) -> int64:\n    return 0\n\
             def main():\n    print(use(Meters(v=2.0)))\n"
        ));
        assert!(
            fn_names(&m).iter().any(|n| n == "use$Meters"),
            "conforming instance emitted: {:?}",
            fn_names(&m)
        );
    }

    #[test]
    fn self_referential_bound_rejects_non_conformer() {
        // T := Bad makes the bound Scaler[f64, Bad]; Bad.scale(f64) -> int64 does
        // not conform (its result must be Bad). The diagnostic names the bound
        // with the self-reference substituted to the concrete type.
        let msg = mono_err(&format!(
            "{SCALER}def use[T: Scaler[f64, T]](T x) -> int64:\n    return 0\n\
             def main():\n    print(use(Bad(v=2.0)))\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Scaler[float64, Bad]'")
                && msg.contains("method 'scale' has a mismatched signature"),
            "self-reference substituted in diagnostic, got: {msg}"
        );
    }

    #[test]
    fn self_referential_bound_on_a_type() {
        // The self-referential bound is enforced on a generic type's parameter
        // too: Pipe[Meters] conforms, Pipe[Bad] does not.
        let m = mono_ok(&format!(
            "{SCALER}struct Pipe[E: Scaler[f64, E]]:\n    E item\n\
             def main():\n    Pipe[Meters] p = Pipe(Meters(v=1.0))\n    print(1)\n"
        ));
        assert!(struct_named(&m, "Pipe$Meters").is_some());
        let msg = mono_err(&format!(
            "{SCALER}struct Pipe[E: Scaler[f64, E]]:\n    E item\n\
             def main():\n    Pipe[Bad] p = Pipe(Bad(v=1.0))\n    print(1)\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Scaler[float64, Bad]'")
                && msg.contains("of type 'Pipe'"),
            "got: {msg}"
        );
    }

    // ---- associated / output types (#1207) --------------------------------
    //
    // An interface may declare `type Out`; a conformer binds it structurally (by
    // the return of the method the interface declares as its producer). At each
    // instantiation the projection `T.Out` in a specialized signature resolves to
    // that concrete binding, before codegen sees it.

    const PRODUCER: &str = "interface Producer[In]:\n    type Out\n    def make(self, In seed) -> Out\n\
         struct Widget:\n    int64 n\n\
         struct Factory:\n    int64 base\n    def make(self, f64 seed) -> Widget:\n        return Widget(n=self.base)\n\
         struct Mute:\n    int64 n\n";

    #[test]
    fn associated_type_projection_resolves_to_concrete_binding() {
        // `build[Factory]`: the return `T.Out` resolves to Factory's `make`
        // return — Widget — in the emitted signature (so codegen sees Widget).
        let m = mono_ok(&format!(
            "{PRODUCER}def build[T: Producer[f64]](T f, f64 s) -> T.Out:\n    return f.make(s)\n\
             def main():\n    Widget w = build(Factory(base=1), 2.0)\n    print(w.n)\n"
        ));
        let f = m
            .items
            .iter()
            .find_map(|i| match i {
                Item::Function(f) if f.name == "build$Factory" => Some(f),
                _ => None,
            })
            .expect("build$Factory emitted");
        assert_eq!(
            type_name(f.ret.as_ref().expect("a declared return")),
            "Widget",
            "the T.Out projection resolves to the concrete binding"
        );
    }

    #[test]
    fn associated_type_conformer_accepted() {
        // Factory provides `make(self, f64) -> Widget`, binding Out := Widget, so
        // it satisfies `Producer[f64]`.
        let m = mono_ok(&format!(
            "{PRODUCER}def use[T: Producer[f64]](T f) -> int64:\n    return 0\n\
             def main():\n    print(use(Factory(base=1)))\n"
        ));
        assert!(fn_names(&m).iter().any(|n| n == "use$Factory"));
    }

    #[test]
    fn associated_type_non_conformer_rejected() {
        // Mute lacks `make`, so it does not satisfy `Producer[f64]`.
        let msg = mono_err(&format!(
            "{PRODUCER}def use[T: Producer[f64]](T f) -> int64:\n    return 0\n\
             def main():\n    print(use(Mute(n=1)))\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Producer[float64]'")
                && msg.contains("missing method 'make'"),
            "got: {msg}"
        );
    }

    // ---- capability bounds on user generic containers (D-0139/I#811) ------
    //
    // A capability interface (`Equatable`/`Hashable`/`Comparable`) is satisfied
    // intrinsically by the shape of the concrete argument, so a user generic
    // container admits exactly the element types a built-in container does.

    const BST: &str = "value struct BST[K: Comparable, V]:\n    K key\n    V val\n\
         \x20   def get(self) -> V:\n        return self.val\n";
    const HMAP: &str = "value struct HMap[K: Hashable, V]:\n    K k\n    V v\n\
         \x20   def get(self) -> V:\n        return self.v\n";

    #[test]
    fn comparable_bound_accepts_scalar_keys() {
        // Every scalar (ints, str, char, float, bool) is intrinsically
        // Comparable. A field of the instance type requests the instantiation, so
        // the bound is checked without needing a method body.
        for key in ["int64", "str", "char", "float64", "bool", "uint32", "bytes"] {
            let prog =
                format!("{BST}struct _Use:\n    BST[{key}, int64] b\ndef main():\n    print(0)\n");
            assert!(
                monomorphize(&parse_str(&prog).0).is_ok(),
                "scalar key '{key}' must satisfy the Comparable bound"
            );
        }
    }

    #[test]
    fn comparable_bound_accepts_tier1_enum_key() {
        let prog = format!(
            "enum Color:\n    Red\n    Green\n{BST}struct _Use:\n    BST[Color, int64] b\n\
             def main():\n    print(0)\n"
        );
        assert!(
            monomorphize(&parse_str(&prog).0).is_ok(),
            "a tier-1 enum is intrinsically Comparable"
        );
    }

    #[test]
    fn comparable_bound_accepts_value_struct_deriving_ord() {
        let prog = format!(
            "@derive(Eq, Ord)\nvalue struct P:\n    int64 x\n{BST}struct _Use:\n    BST[P, int64] b\n\
             def main():\n    print(0)\n"
        );
        assert!(
            monomorphize(&parse_str(&prog).0).is_ok(),
            "a value struct with @derive(Ord) satisfies Comparable"
        );
    }

    #[test]
    fn comparable_bound_rejects_non_comparable_struct_naming_capability() {
        // The keystone acceptance case: a struct with no ordering is rejected at
        // instantiation, and the message names the capability + why.
        let msg = mono_err(&format!(
            "struct Widget:\n    int64 w\n{BST}struct _Use:\n    BST[Widget, int64] b\n\
             def main():\n    print(0)\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Comparable'")
                && msg.contains("type parameter 'K'")
                && msg.contains("of type 'BST'")
                && msg.contains("no '__cmp__'")
                && msg.contains("@derive(Ord)"),
            "capability-named rejection, got: {msg}"
        );
    }

    #[test]
    fn comparable_bound_rejects_value_struct_without_ord() {
        // A plain value struct auto-derives Eq/Hash but never ordering: it does
        // not satisfy Comparable (ordering is a choice).
        let msg = mono_err(&format!(
            "value struct Plain:\n    int64 x\n{BST}struct _Use:\n    BST[Plain, int64] b\n\
             def main():\n    print(0)\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Comparable'")
                && msg.contains("ordering is never auto-derived"),
            "value struct without Ord is not Comparable, got: {msg}"
        );
    }

    #[test]
    fn hashable_bound_accepts_value_struct_auto_and_derived() {
        for decl in [
            "value struct P:\n    int64 x\n    int64 y\n", // auto-derive (#808)
            "@derive(Eq, Hash)\nvalue struct P:\n    int64 x\n",
            "@derive(Eq, Hash)\nstruct P:\n    int64 x\n", // reference struct with the pair
        ] {
            let prog = format!(
                "{decl}{HMAP}struct _Use:\n    HMap[P, str] h\ndef main():\n    print(0)\n"
            );
            assert!(
                monomorphize(&parse_str(&prog).0).is_ok(),
                "P must satisfy Hashable for:\n{decl}"
            );
        }
    }

    #[test]
    fn hashable_bound_refinement_carries_equatable() {
        // `Hashable` refines `Equatable` (#806): a type satisfying Hashable is
        // usable where Equatable is required too. A `value struct P` auto-derives
        // both, so it satisfies an `Equatable` bound.
        let prog = "value struct P:\n    int64 x\nvalue struct EqBox[K: Equatable]:\n    K k\n\
             struct _Use:\n    EqBox[P] e\ndef main():\n    print(0)\n";
        assert!(
            monomorphize(&parse_str(prog).0).is_ok(),
            "a Hashable (auto-derived) value struct is Equatable"
        );
    }

    #[test]
    fn hashable_bound_rejects_reference_struct_with_only_eq() {
        // A partial set (`@derive(Eq)` only) does not satisfy Hashable: a
        // reference struct needs both dunders.
        let msg = mono_err(&format!(
            "@derive(Eq)\nstruct RK:\n    int64 x\n{HMAP}struct _Use:\n    HMap[RK, str] h\n\
             def main():\n    print(0)\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Hashable'")
                && msg.contains("does not auto-derive"),
            "reference struct with only Eq is not Hashable, got: {msg}"
        );
    }

    #[test]
    fn hashable_bound_names_the_blocking_field_of_a_value_aggregate() {
        // A value struct whose field is a mutable collection does not auto-derive
        // Hashable; the diagnostic names the blocking field and its type.
        let msg = mono_err(&format!(
            "value struct Bad:\n    list[int64] items\n{HMAP}struct _Use:\n    HMap[Bad, str] h\n\
             def main():\n    print(0)\n"
        ));
        assert!(
            msg.contains("does not satisfy the bound 'Hashable'")
                && msg.contains("field 'items'")
                && msg.contains("list[int64]")
                && msg.contains("mutable collection"),
            "blocking field named, got: {msg}"
        );
    }

    #[test]
    fn hashable_bound_accepts_tier3_enum_with_hashable_payloads() {
        // Uniform with built-in dict/set keys (#809): a data-carrying enum whose
        // payloads are all hashable auto-satisfies Hashable.
        let prog = format!(
            "enum Shape:\n    Circle(int64 r)\n    Rect(int64 w, int64 h)\n{HMAP}\
             struct _Use:\n    HMap[Shape, str] h\ndef main():\n    print(0)\n"
        );
        assert!(
            monomorphize(&parse_str(&prog).0).is_ok(),
            "a tier-3 enum with hashable payloads satisfies Hashable"
        );
    }

    #[test]
    fn hashable_bound_rejects_list_and_bigint_keys() {
        for (key, needle) in [
            ("list[int64]", "mutable collection"),
            ("bigint", "no representation"),
        ] {
            let msg = mono_err(&format!(
                "{HMAP}struct _Use:\n    HMap[{key}, str] h\ndef main():\n    print(0)\n"
            ));
            assert!(
                msg.contains("does not satisfy the bound 'Hashable'") && msg.contains(needle),
                "key '{key}' rejected with '{needle}', got: {msg}"
            );
        }
    }

    #[test]
    fn nested_user_container_of_user_container_instantiates() {
        // A user container whose value is another user container instance: both
        // bounds are enforced and both instances are emitted.
        let prog = format!(
            "{HMAP}struct _Use:\n    HMap[int64, HMap[str, int64]] outer\ndef main():\n    print(0)\n"
        );
        let m = monomorphize(&parse_str(&prog).0).expect("nested instantiation succeeds");
        assert!(
            m.items
                .iter()
                .any(|i| matches!(i, Item::Struct(s) if s.name.starts_with("HMap$"))),
            "a specialized HMap instance is emitted"
        );
    }

    #[test]
    fn user_interface_refining_capability_flattens_requirement() {
        // A user `interface Keyish: Hashable` used as a bound carries the
        // capability requirement: a value struct satisfying Hashable *and* the
        // interface's own method conforms; one missing `__hash__` (reference
        // struct, only its own method) is rejected, naming the supertype.
        let base = "interface Keyish: Hashable\n    def tag(self) -> int64\n\
             value struct Good:\n    int64 x\n    def tag(self) -> int64:\n        return self.x\n\
             value struct Box[K: Keyish]:\n    K k\n";
        let ok = format!("{base}struct _Use:\n    Box[Good] b\ndef main():\n    print(0)\n");
        assert!(
            monomorphize(&parse_str(&ok).0).is_ok(),
            "a value struct that is Hashable and has tag() conforms to Keyish"
        );
        let bad = "interface Keyish: Hashable\n    def tag(self) -> int64\n\
             struct Ref:\n    int64 x\n    def tag(self) -> int64:\n        return self.x\n\
             value struct Box[K: Keyish]:\n    K k\n";
        let msg = mono_err(&format!(
            "{bad}struct _Use:\n    Box[Ref] b\ndef main():\n    print(0)\n"
        ));
        assert!(
            msg.contains("'Hashable'") && msg.contains("supertype 'Hashable'"),
            "the refined capability supertype requirement is attributed, got: {msg}"
        );
    }

    // ---- module-global receiver-type resolution (issue #671) --------------
    //
    // Receiver-type-driven specialization (the built-in `dict.get` rewrite, and
    // generic-method dispatch) keys off the *concrete type* of the receiver.
    // Mono learns a local's/parameter's type from the function's type
    // environment; a module-level global (epic #661 / D-0112) was absent from
    // that environment, so a global receiver read back as opaque and the rewrite
    // was silently skipped — the bare `get` then reached lowering and was
    // rejected (ZAP6001). Feeding module globals into the environment makes a
    // global receiver resolve exactly as a local does. These tests pin the
    // rewrite at the mono boundary (the specialized symbol must appear, the bare
    // `get` must not); the golden suite covers the end-to-end runtime behavior.

    /// Every method-call symbol `recv.NAME(...)` that appears anywhere in the
    /// emitted module, in traversal order. Used to assert which specialized
    /// `dict.get` variant mono chose (`get_default`, `get$Optional$V`) and that
    /// no unspecialized bare `get` survived.
    fn method_call_syms(m: &Module) -> Vec<String> {
        fn walk_expr(e: &Expr, out: &mut Vec<String>) {
            if let Expr::Call { callee, args, .. } = e {
                if let Expr::Attr { base, name, .. } = callee.as_ref() {
                    out.push(name.clone());
                    walk_expr(base, out);
                } else {
                    walk_expr(callee, out);
                }
                for a in args {
                    walk_expr(&a.value, out);
                }
                return;
            }
            // Recurse into the expression forms that can nest a call relevant to
            // these fixtures (binary arithmetic such as `t.get(k, 0) + 1`, an
            // index base/index, an attribute base, and a parenthesized/awaited
            // child). The fixtures are small and deliberate, so this need not be
            // exhaustive over every `Expr` variant.
            match e {
                Expr::Binary { lhs, rhs, .. } => {
                    walk_expr(lhs, out);
                    walk_expr(rhs, out);
                }
                Expr::Attr { base, .. } => walk_expr(base, out),
                Expr::Index { base, index, .. } => {
                    walk_expr(base, out);
                    walk_expr(index, out);
                }
                _ => {}
            }
        }
        fn walk_block(b: &Block, out: &mut Vec<String>) {
            for st in &b.stmts {
                match st {
                    Stmt::Let { value, .. } => walk_expr(value, out),
                    Stmt::Assign { target, value, .. } => {
                        walk_expr(target, out);
                        walk_expr(value, out);
                    }
                    Stmt::AugAssign { target, value, .. } => {
                        walk_expr(target, out);
                        walk_expr(value, out);
                    }
                    Stmt::Expr(e) => walk_expr(e, out),
                    Stmt::Return { value: Some(e), .. } => walk_expr(e, out),
                    Stmt::If {
                        branches,
                        else_block,
                        ..
                    } => {
                        for (c, body) in branches {
                            walk_expr(c, out);
                            walk_block(body, out);
                        }
                        if let Some(eb) = else_block {
                            walk_block(eb, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        for item in &m.items {
            if let Item::Function(f) = item {
                walk_block(&f.body, &mut out);
            }
        }
        out
    }

    #[test]
    fn module_global_dict_get_two_arg_specializes_to_get_default() {
        // The issue's smallest repro: a two-arg `get` on a *mutable* module-
        // global dict must rewrite to `get_default` (the plain-`V` form), just
        // as it does on a local — not fall through as a bare `get`.
        let m = mono_ok(
            "dict[str, int64] t = {}\n\
             def main():\n    t[\"a\"] = t.get(\"a\", 0) + 1\n    print(t[\"a\"])\n",
        );
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get_default"),
            "two-arg get on a module global must specialize to get_default: {syms:?}"
        );
        assert!(
            !syms.iter().any(|s| s == "get"),
            "no unspecialized bare `get` may survive to lowering: {syms:?}"
        );
    }

    /// Minimal stand-in for the prelude `Optional[T]` so the isolated mono unit
    /// tests can exercise the one-arg `get` rewrite (which requests an
    /// `Optional[V]` instance); the real compiler supplies this via the prelude.
    const OPTIONAL: &str = "enum Optional[T]:\n    Some(T value)\n    None\n";

    #[test]
    fn module_global_dict_get_one_arg_specializes_to_optional_instance() {
        // One-arg `get` on a module-global dict must encode the concrete
        // `Optional[V]` instance into the symbol (`get$Optional$int64`).
        let m = mono_ok(&format!(
            "{OPTIONAL}dict[str, int64] t = {{}}\n\
             def main():\n    Optional[int64] v = t.get(\"a\")\n    print(1)\n"
        ));
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get$Optional$int64"),
            "one-arg get on a module global must carry the Optional$int64 instance: {syms:?}"
        );
        assert!(
            !syms.iter().any(|s| s == "get"),
            "no unspecialized bare `get` may survive to lowering: {syms:?}"
        );
    }

    #[test]
    fn const_module_global_dict_get_specializes_like_mutable() {
        // A `const` module-global dict (#658) resolves its receiver type for
        // specialization exactly as a mutable one (#662) does.
        let m = mono_ok(
            "const dict[str, int64] t = {\"a\": 5}\n\
             def main():\n    print(t.get(\"a\", 0))\n",
        );
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get_default") && !syms.iter().any(|s| s == "get"),
            "const module-global dict.get must specialize: {syms:?}"
        );
    }

    #[test]
    fn local_dict_get_specialization_is_undisturbed() {
        // Regression guard: feeding globals into the environment must not change
        // the local path — the identical body on a local dict still specializes.
        let m = mono_ok(
            "def main():\n    dict[str, int64] t = {}\n\
             \x20   t[\"a\"] = t.get(\"a\", 0) + 1\n    print(t[\"a\"])\n",
        );
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get_default") && !syms.iter().any(|s| s == "get"),
            "local dict.get must still specialize: {syms:?}"
        );
    }

    #[test]
    fn module_global_and_local_pick_the_same_specialization() {
        // The whole point of the fix: a global receiver yields the *same* rewrite
        // as the equivalent local, so behavior is identical. Both the global and
        // local one-arg forms must select `get$Optional$int64`.
        let global = method_call_syms(&mono_ok(&format!(
            "{OPTIONAL}dict[str, int64] t = {{}}\n\
             def main():\n    Optional[int64] v = t.get(\"a\")\n    print(1)\n"
        )));
        let local = method_call_syms(&mono_ok(&format!(
            "{OPTIONAL}def main():\n    dict[str, int64] t = {{}}\n\
             \x20   Optional[int64] v = t.get(\"a\")\n    print(1)\n"
        )));
        assert!(
            global.iter().any(|s| s == "get$Optional$int64")
                && local.iter().any(|s| s == "get$Optional$int64"),
            "global and local must pick the same get$Optional$int64 rewrite: global={global:?} local={local:?}"
        );
    }

    #[test]
    fn a_local_shadows_a_module_global_in_receiver_resolution() {
        // A local declaration of the same name shadows the global (D-0112): the
        // receiver type — and hence the specialization — must come from the
        // local (`dict[str, str]` -> `Optional$str`), never the global
        // (`dict[str, int64]`). Inner environment scopes are searched first.
        let m = mono_ok(&format!(
            "{OPTIONAL}dict[str, int64] t = {{}}\n\
             def main():\n    dict[str, str] t = {{}}\n\
             \x20   Optional[str] v = t.get(\"a\")\n    print(1)\n"
        ));
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get$Optional$str"),
            "the shadowing local dict[str,str] must drive the rewrite: {syms:?}"
        );
        assert!(
            !syms.iter().any(|s| s == "get$Optional$int64"),
            "the global's type must not leak past the shadowing local: {syms:?}"
        );
    }

    // ---- #1477: a user type's own `get` must never be hijacked by the builtin
    // `dict.get` special-casing. The `dict.get(key) -> Optional[V]` rewrite used
    // the call's expected `Optional[..]` sink as a fallback value type, which
    // latched onto ANY `.get` whose binding/return type was `Optional[..]` —
    // even when the receiver was a user struct/enum with its own `get` method —
    // and rewrote the symbol to a bogus `get$Optional$V` that no method existed
    // under (ZAP6001 "no method 'get$Optional$bytes'"). The regression guards
    // each context (Optional binding, return, struct and enum receivers,
    // one- and two-arg) that drove the misrewrite.

    const CLIENT_GET: &str = "struct Client:\n    int64 unused\n\
         \x20   def get(self, bytes key, Optional[bytes] default = None) -> Optional[bytes]:\n\
         \x20       return default\n";

    #[test]
    fn user_struct_get_not_hijacked_by_dict_get_optional_binding() {
        // Two-arg `c.get(k, default)` bound to `Optional[bytes]`: the expected
        // sink must not forge `get$Optional$bytes`; it dispatches as a plain
        // user `get`.
        let m = mono_ok(&format!(
            "{OPTIONAL}{CLIENT_GET}def main():\n    Client c = Client(0)\n\
             \x20   Optional[bytes] v = c.get(b\"k\", Optional.Some(b\"x\"))\n    print(1)\n"
        ));
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get"),
            "the user struct's own `get` must survive as a bare method: {syms:?}"
        );
        assert!(
            !syms
                .iter()
                .any(|s| s.starts_with("get$") || s == "get_default"),
            "no dict.get rewrite may apply to a user struct's `get`: {syms:?}"
        );
    }

    #[test]
    fn user_struct_get_not_hijacked_by_dict_get_return_context() {
        // The identical body in a `return` whose declared type is `Optional[..]`
        // — the exact #1477 reproducer shape — likewise stays a plain `get`.
        let m = mono_ok(&format!(
            "{OPTIONAL}{CLIENT_GET}\
             def fetch(Client c) -> Optional[bytes]:\n\
             \x20   return c.get(b\"k\", Optional.Some(b\"x\"))\n\
             def main():\n    print(fetch(Client(0)).unwrap())\n"
        ));
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get") && !syms.iter().any(|s| s.starts_with("get$")),
            "return-context `get` on a user struct must not be rewritten: {syms:?}"
        );
    }

    #[test]
    fn user_struct_one_arg_get_not_hijacked() {
        // One-arg `c.get(k)` bound to `Optional[bytes]`: the one-arg dict path
        // (which encodes `get$Optional$V`) must also yield to the user method.
        let m = mono_ok(&format!(
            "{OPTIONAL}{CLIENT_GET}def main():\n    Client c = Client(0)\n\
             \x20   Optional[bytes] v = c.get(b\"k\")\n    print(1)\n"
        ));
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get") && !syms.iter().any(|s| s.starts_with("get$")),
            "one-arg user `get` must not be rewritten to get$Optional$bytes: {syms:?}"
        );
    }

    #[test]
    fn user_enum_get_not_hijacked_by_dict_get() {
        // A user *enum* method named `get` is guarded identically (the probe
        // searches enums as well as structs).
        let m = mono_ok(&format!(
            "{OPTIONAL}enum Store:\n    Empty\n    Full(bytes v)\n\
             \x20   def get(self, bytes key) -> Optional[bytes]:\n\
             \x20       return None\n\
             def main():\n    Store s = Store.Empty\n\
             \x20   Optional[bytes] v = s.get(b\"k\")\n    print(1)\n"
        ));
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get") && !syms.iter().any(|s| s.starts_with("get$")),
            "a user enum's `get` must not be rewritten: {syms:?}"
        );
    }

    #[test]
    fn generic_homonym_get_does_not_poison_concrete_get() {
        // The full #1477 shape: a generic `Typed[T].get` homonym coexisting with
        // concrete `Client.get` must not disturb a field receiver's dispatch.
        // Both lower as plain `get`; neither becomes `get$Optional$bytes`.
        let m = mono_ok(&format!(
            "{OPTIONAL}struct Typed[T]:\n    T value\n\
             \x20   def get(self, bytes key, Optional[T] default = None) -> Optional[T]:\n\
             \x20       return default\n\
             {CLIENT_GET}\
             struct Lease:\n    Client client\n\
             struct Pool:\n    Client client\n\
             \x20   def get(self, bytes key, Optional[bytes] default = None) -> Optional[bytes]:\n\
             \x20       Lease lease = Lease(self.client)\n\
             \x20       return lease.client.get(key, default)\n\
             def main():\n    Typed[str] typed = Typed(\"present\")\n\
             \x20   Optional[str] ignored = typed.get(b\"k\")\n\
             \x20   Pool pool = Pool(Client(0))\n\
             \x20   print(pool.get(b\"k\", Optional.Some(b\"fallback\")).unwrap())\n"
        ));
        let syms = method_call_syms(&m);
        assert!(
            !syms.iter().any(|s| s.starts_with("get$")),
            "no `get` call may carry a forged dict-style Optional suffix: {syms:?}"
        );
    }

    #[test]
    fn dict_get_still_specializes_alongside_a_user_get() {
        // Guard the guard: a genuine `dict.get` in the SAME module as a user
        // struct's `get` still specializes normally — the fix narrows only the
        // user-type case, it does not disable the dict path.
        let m = mono_ok(&format!(
            "{OPTIONAL}{CLIENT_GET}def main():\n    dict[str, int64] t = {{}}\n\
             \x20   Optional[int64] v = t.get(\"a\")\n\
             \x20   int64 n = t.get(\"b\", 0)\n    print(n)\n"
        ));
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "get$Optional$int64"),
            "the real dict.get must still specialize: {syms:?}"
        );
        assert!(
            syms.iter().any(|s| s == "get_default"),
            "the real two-arg dict.get must still specialize to get_default: {syms:?}"
        );
    }

    #[test]
    fn collection_literal_args_drive_type_param_inference() {
        // I#170: a dict/set *literal* passed directly to a generic function
        // infers the function's type parameters, exactly as a list literal
        // already did. Before the fix mono raised "cannot infer type argument";
        // now it resolves and emits the concrete instance. (The collections stay
        // special-cased; this only makes their inference uniform with generics.)
        let m = mono_ok(
            "def countDict[K, V](dict[K, V] d) -> int64:\n    return len(d)\n\
             def countSet[T](set[T] s) -> int64:\n    return len(s)\n\
             def main():\n    print(countDict({\"x\": 1}))\n    print(countSet({4, 5}))\n",
        );
        let names = fn_names(&m);
        assert!(
            names.iter().any(|n| n == "countDict$str$int64"),
            "dict literal infers K=str, V=int64: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "countSet$int64"),
            "set literal infers T=int64: {names:?}"
        );
    }

    // ---- user-generic-type module globals (#690, epic #661 / D-0112) ------

    /// The declared type of a module-level global `name` (its explicit
    /// annotation), rendered via [`type_name`]. Panics if there is no such
    /// single-name global.
    fn global_ty(m: &Module, name: &str) -> String {
        m.items
            .iter()
            .find_map(|i| match i {
                Item::Stmt(Stmt::Let {
                    name: n,
                    ty: Some(t),
                    ..
                }) if n == name => Some(type_name(t)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no single-name global '{name}'"))
    }

    const BOX: &str = "struct Box[T]:\n    T item\n";

    #[test]
    fn module_level_generic_global_is_monomorphized() {
        // #690: a module-level global of a user generic type must have its
        // declared type requested as an instance and its initializer rewritten,
        // exactly as a local `Stmt::Let` does. Before the fix the top-level
        // `Item::Stmt` passed through unchanged, so `Box$int64` was never emitted
        // and the global stayed typed `Box` (unknown to codegen).
        let m = mono_ok(&format!(
            "{BOX}Box[int64] g = Box(7)\ndef main():\n    print(g.item)\n"
        ));
        assert_eq!(
            global_ty(&m, "g"),
            "Box$int64",
            "the global's declared type must be rewritten to the concrete instance"
        );
        assert!(
            struct_named(&m, "Box$int64").is_some(),
            "the requested Box[int64] instance must be emitted"
        );
        // The generic template itself must not leak through as a concrete decl.
        assert!(
            struct_named(&m, "Box").is_none_or(|s| s.generics.is_empty()),
            "no generic `Box` template should remain as a concrete declaration"
        );
    }

    #[test]
    fn module_generic_global_and_local_pick_the_same_instance() {
        // Parity with the #671 global/local principle: a generic global and the
        // equivalent local select the identical specialization.
        let global = global_ty(
            &mono_ok(&format!(
                "{BOX}Box[int64] g = Box(7)\ndef main():\n    print(g.item)\n"
            )),
            "g",
        );
        let local = mono_ok(&format!(
            "{BOX}def main():\n    Box[int64] g = Box(7)\n    print(g.item)\n"
        ));
        assert_eq!(global, "Box$int64");
        assert!(
            struct_named(&local, "Box$int64").is_some(),
            "the local must request the same Box$int64 instance"
        );
    }

    #[test]
    fn const_generic_global_is_monomorphized_like_mutable() {
        // A `const` generic global is instantiated exactly like a mutable one.
        let m = mono_ok(&format!(
            "{BOX}const Box[int64] g = Box(7)\ndef main():\n    print(g.item)\n"
        ));
        assert_eq!(global_ty(&m, "g"), "Box$int64");
        assert!(struct_named(&m, "Box$int64").is_some());
    }

    #[test]
    fn two_generic_globals_emit_distinct_instances() {
        // Two different instantiations held as separate globals each resolve to
        // their own instance (`Box$int64` and `Box$str`).
        let m = mono_ok(&format!(
            "{BOX}Box[int64] gi = Box(7)\nBox[str] gs = Box(\"hi\")\n\
             def main():\n    print(gi.item)\n    print(gs.item)\n"
        ));
        assert_eq!(global_ty(&m, "gi"), "Box$int64");
        assert_eq!(global_ty(&m, "gs"), "Box$str");
        assert!(struct_named(&m, "Box$int64").is_some(), "Box$int64 emitted");
        assert!(struct_named(&m, "Box$str").is_some(), "Box$str emitted");
    }

    #[test]
    fn generic_global_with_method_requests_the_instance() {
        // A method call on a generic global drives the same instantiation; the
        // instance's declaration (carrying the method) must be emitted.
        let m = mono_ok(
            "struct Box[T]:\n    T item\n    def peek(self) -> T:\n        return self.item\n\
             Box[int64] g = Box(7)\ndef main():\n    print(g.peek())\n",
        );
        assert_eq!(global_ty(&m, "g"), "Box$int64");
        assert!(struct_named(&m, "Box$int64").is_some());
    }

    // ---- module-qualified Queue construction (I#996, D-0202) --------------

    /// Monomorphize a program that uses prelude seam types: inject the prelude
    /// (as the lowering path does) before monomorphization, so `Queue[T]` /
    /// `new_queue` are in scope.
    fn mono_prelude(src: &str) -> Module {
        let (module, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        let module = zapc_syntax::parser::with_prelude(&module);
        monomorphize(&module).expect("monomorphization succeeds")
    }

    /// Like [`mono_prelude`] but expects monomorphization to REJECT the program
    /// (e.g. a concrete type that does not satisfy a prelude operator bound).
    fn mono_prelude_err(src: &str) -> String {
        let (module, perrs, lerrs) = parse_str(src);
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        let module = zapc_syntax::parser::with_prelude(&module);
        monomorphize(&module)
            .expect_err("monomorphization should reject the unsatisfied bound")
            .message
    }

    // ---- operator-capability interface bounds (#1199) ---------------------
    //
    // The real prelude operator interfaces (`RMul[L, Out]`, `Add[R, Out]`, …)
    // used as generic bounds, enforced structurally at the instantiation just
    // like any other generic-interface bound (static dispatch, no fat pointer).

    #[test]
    fn operator_reflected_bound_accepts_conformer() {
        // `T := Vec2` makes the bound `RMul[f64, Vec2]`; Vec2 defines
        // `__rmul__(self, f64) -> Vec2`, so it conforms and the instance emits.
        let m = mono_prelude(
            "value struct Vec2:\n    f64 x\n\
             \x20   def __rmul__(self, f64 k) -> Vec2:\n        return Vec2(x=self.x * k)\n\n\
             def scale_all[T: RMul[f64, T]](list[T] xs, f64 k) -> list[T]:\n    \
             return [k * x for x in xs]\n\n\
             def main():\n    list[Vec2] v = scale_all([Vec2(x=1.0)], 2.0)\n    print(str(len(v)))\n",
        );
        assert!(
            fn_names(&m).iter().any(|n| n == "scale_all$Vec2"),
            "conforming instance emitted: {:?}",
            fn_names(&m)
        );
    }

    #[test]
    fn operator_reflected_bound_rejects_non_conformer() {
        // `Bad` has no `__rmul__`, so `T := Bad` fails the `RMul[f64, Bad]` bound;
        // the diagnostic names the bound.
        let msg = mono_prelude_err(
            "value struct Bad:\n    f64 x\n\n\
             def scale_all[T: RMul[f64, T]](list[T] xs, f64 k) -> list[T]:\n    \
             return [k * x for x in xs]\n\n\
             def main():\n    list[Bad] v = scale_all([Bad(x=1.0)], 2.0)\n    print(str(len(v)))\n",
        );
        assert!(
            msg.contains("RMul[float64, Bad]") && msg.contains("Bad"),
            "diagnostic names the unsatisfied operator bound: {msg}"
        );
    }

    #[test]
    fn operator_forward_bound_accepts_conformer() {
        // The forward `Add[T, T]` bound: `Num` defines `__add__(self, Num) -> Num`.
        let m = mono_prelude(
            "value struct Num:\n    int64 n\n\
             \x20   def __add__(self, Num o) -> Num:\n        return Num(n=self.n + o.n)\n\n\
             def add2[T: Add[T, T]](T a, T b) -> T:\n    return a + b\n\n\
             def main():\n    Num s = add2(Num(n=1), Num(n=2))\n    print(str(s.n))\n",
        );
        assert!(
            fn_names(&m).iter().any(|n| n == "add2$Num"),
            "conforming instance emitted: {:?}",
            fn_names(&m)
        );
    }

    // ---- generic operator dunders in operator syntax (#1227) --------------
    //
    // A *generic* operator dunder (`def __add__[T]` / `def __radd__[T]`) cannot
    // be dispatched by IR lowering, which only knows the un-instantiated mangled
    // name. Mono resolves the D-0047 / #1180 dispatch and rewrites the operator
    // into the specialized method call the explicit `x.__radd__[T](y)` form
    // already lowers. A *non-generic* dunder must stay an operator so IR lowering
    // dispatches it unchanged (the `operator_index_call` / #1201 behavior).

    #[test]
    fn generic_reflected_operator_dispatches_per_left_operand_type() {
        // The issue's shape: `a + delta` / `a - delta` where `delta`'s type has a
        // generic reflected dunder rewrites to the specialized method call,
        // instantiated with the concrete left-operand type. Both a `Date` and a
        // `DateTime` left operand and both `__radd__`/`__rsub__` are covered.
        let m = mono_ok(
            "struct Date:\n    int64 day\n\
             struct DateTime:\n    int64 day\n\
             struct Shift:\n    int64 n\n\
             \x20   def __radd__[T](self, T x) -> T:\n        return x\n\
             \x20   def __rsub__[T](self, T x) -> T:\n        return x\n\
             def main():\n    \
             auto d = Date(1)\n    auto t = DateTime(2)\n    \
             auto a = d + Shift(1)\n    auto b = t + Shift(1)\n    \
             auto c = d - Shift(1)\n    auto e = t - Shift(1)\n    \
             print(a.day)\n    print(b.day)\n    print(c.day)\n    print(e.day)\n",
        );
        let syms = method_call_syms(&m);
        for want in [
            "__radd__$Date",
            "__radd__$DateTime",
            "__rsub__$Date",
            "__rsub__$DateTime",
        ] {
            assert!(
                syms.iter().any(|s| s == want),
                "operator rewritten to {want}: {syms:?}"
            );
        }
        // Each specialized reflected method is emitted on its owner struct.
        let shift = struct_named(&m, "Shift").expect("Shift emitted");
        let methods: Vec<&String> = shift.methods.iter().map(|mth| &mth.name).collect();
        for want in [
            "__radd__$Date",
            "__radd__$DateTime",
            "__rsub__$Date",
            "__rsub__$DateTime",
        ] {
            assert!(
                methods.iter().any(|n| *n == want),
                "{want} attached to Shift: {methods:?}"
            );
        }
    }

    #[test]
    fn generic_forward_operator_dispatches_to_specialized_call() {
        // A generic *forward* dunder dispatches too, keyed on the right-operand
        // (argument) type: `c + 1` -> `__add__$int64`, `c + "x"` -> `__add__$str`.
        let m = mono_ok(
            "struct Counter:\n    int64 n\n\
             \x20   def __add__[T](self, T other) -> int64:\n        return self.n\n\
             def main():\n    auto c = Counter(7)\n    print(c + 1)\n    print(c + \"x\")\n",
        );
        let syms = method_call_syms(&m);
        assert!(
            syms.iter().any(|s| s == "__add__$int64"),
            "int64 instance: {syms:?}"
        );
        assert!(
            syms.iter().any(|s| s == "__add__$str"),
            "str instance: {syms:?}"
        );
    }

    #[test]
    fn nongeneric_reflected_operator_stays_an_operator() {
        // A non-generic reflected dunder is dispatched by IR lowering, so mono
        // must leave the operator untouched — no specialized method-call symbol.
        let m = mono_ok(
            "struct Scale:\n    int64 k\n\
             \x20   def __rmul__(self, int64 x) -> int64:\n        return x * self.k\n\
             def main():\n    print(3 * Scale(10))\n",
        );
        let syms = method_call_syms(&m);
        assert!(
            !syms.iter().any(|s| s.starts_with("__rmul__")),
            "non-generic reflected op stays an operator: {syms:?}"
        );
    }

    #[test]
    fn forward_dunder_wins_over_a_generic_reflected_dunder() {
        // D-0047 precedence: when the left operand's type declares a forward
        // dunder it wins, and the right operand's reflected dunder is never
        // consulted — even a generic one (which would otherwise be rewritten).
        let m = mono_ok(
            "struct L:\n    int64 v\n\
             \x20   def __add__(self, R o) -> int64:\n        return self.v\n\
             struct R:\n    int64 w\n\
             \x20   def __radd__[T](self, T x) -> T:\n        return x\n\
             def main():\n    print(L(1) + R(2))\n",
        );
        let syms = method_call_syms(&m);
        assert!(
            !syms.iter().any(|s| s.starts_with("__add__")),
            "forward dunder stays an operator: {syms:?}"
        );
        assert!(
            !syms.iter().any(|s| s.starts_with("__radd__")),
            "reflected dunder not consulted when forward wins: {syms:?}"
        );
    }

    #[test]
    fn generic_operator_with_uninferable_type_arg_is_rejected() {
        // When the generic dunder's own type parameter appears in neither the
        // argument nor the return type it cannot be inferred from the operator;
        // the rewrite reports the same clean "cannot infer type argument" error
        // the explicit generic-method-call form produces.
        let msg = mono_err(
            "struct D:\n    int64 d\n\
             \x20   def __radd__[U](self, int64 x) -> int64:\n        return x\n\
             def main():\n    print(5 + D(1))\n",
        );
        assert!(
            msg.contains("cannot infer type argument 'U'"),
            "clean inference error: {msg}"
        );
    }

    #[test]
    fn qualified_queue_explicit_desugars_to_new_queue_instance() {
        // `concurrent.Queue[int64]()` monomorphizes to exactly the bare
        // `new_queue[int64]()` specialization — proving the callee desugar routes
        // through the existing bare-name machinery, which keys on `new_queue`.
        let m = mono_prelude(
            "import concurrent\ndef main():\n    \
             Queue[int64] q = concurrent.Queue[int64]()\n    \
             q.push(1)\n",
        );
        let names = fn_names(&m);
        assert!(
            names.iter().any(|n| n == "new_queue$int64"),
            "expected a new_queue$int64 specialization, got {names:?}"
        );
    }

    #[test]
    fn qualified_queue_inferred_desugars_to_new_queue_instance() {
        // The arg-less dotted form infers T from the dotted annotation sink
        // `concurrent.Queue[str]`, yielding the `new_queue$str` specialization.
        let m = mono_prelude(
            "import concurrent\ndef main():\n    \
             concurrent.Queue[str] q = concurrent.Queue()\n    \
             q.push(\"x\")\n",
        );
        let names = fn_names(&m);
        assert!(
            names.iter().any(|n| n == "new_queue$str"),
            "expected a new_queue$str specialization, got {names:?}"
        );
    }

    #[test]
    fn qualified_queue_distinct_element_types_monomorphize_distinctly() {
        // Different element types produce distinct specializations, whether built
        // via the dotted or the bare form — they must not collapse.
        let m = mono_prelude(
            "import concurrent\ndef main():\n    \
             concurrent.Queue[int64] qi = concurrent.Queue[int64]()\n    \
             concurrent.Queue[str] qs = concurrent.Queue()\n    \
             Queue[bool] qb = new_queue[bool]()\n    \
             qi.push(1)\n    \
             qs.push(\"x\")\n    \
             qb.push(True)\n",
        );
        let names = fn_names(&m);
        for want in ["new_queue$int64", "new_queue$str", "new_queue$bool"] {
            assert!(
                names.iter().any(|n| n == want),
                "expected a {want} specialization, got {names:?}"
            );
        }
    }

    #[test]
    fn bare_new_queue_still_monomorphizes() {
        // Back-compat: the bare form is byte-for-byte the same request the dotted
        // form now produces.
        let m = mono_prelude(
            "def main():\n    \
             Queue[int64] q = new_queue()\n    \
             q.push(1)\n",
        );
        let names = fn_names(&m);
        assert!(
            names.iter().any(|n| n == "new_queue$int64"),
            "expected a new_queue$int64 specialization, got {names:?}"
        );
    }

    #[test]
    fn generic_body_collection_ctor_substitutes_element_type() {
        // #1013: `list[T]()` inside a generic body must monomorphise to the
        // concrete element type, not leave a bare `T` for codegen to reject.
        let m = mono_ok(
            "def collect[T](T x) -> int64:\n    \
             auto xs = list[T]()\n    \
             xs.append(x)\n    \
             return len(xs)\n\
             def main():\n    \
             print(str(collect[int64](5)))\n",
        );
        let f = m
            .items
            .iter()
            .find_map(|i| match i {
                Item::Function(f) if f.name == "collect$int64" => Some(f),
                _ => None,
            })
            .expect("collect$int64 specialization present");
        let Stmt::Let { value, .. } = &f.body.stmts[0] else {
            panic!(
                "expected `auto xs = list[...]()`, got {:?}",
                f.body.stmts[0]
            );
        };
        let rendered = zapc_syntax::ast::render_expr(value);
        assert!(
            rendered.contains("int64") && !rendered.contains("[T]"),
            "list[T]() subscript was not substituted: {rendered}"
        );
    }

    #[test]
    fn generic_body_deque_oset_ctor_substitutes_element_type() {
        // #983: `deque[T]()` / `oset[T]()` inside a generic body must monomorphise
        // to the concrete element type (the same path as `list[T]()`, #1013), not
        // leave a bare `T` for codegen to reject.
        let m = mono_ok(
            "def collect[T](T x) -> int64:\n    \
             auto d = deque[T]()\n    \
             auto s = oset[T]()\n    \
             d.append(x)\n    \
             s.add(x)\n    \
             return len(d) + len(s)\n\
             def main():\n    \
             print(str(collect[int64](5)))\n",
        );
        let f = m
            .items
            .iter()
            .find_map(|i| match i {
                Item::Function(f) if f.name == "collect$int64" => Some(f),
                _ => None,
            })
            .expect("collect$int64 specialization present");
        for (idx, want) in [(0usize, "deque"), (1usize, "oset")] {
            let Stmt::Let { value, .. } = &f.body.stmts[idx] else {
                panic!(
                    "expected a `let` at stmt {idx}, got {:?}",
                    f.body.stmts[idx]
                );
            };
            let rendered = zapc_syntax::ast::render_expr(value);
            assert!(
                rendered.contains("int64") && !rendered.contains("[T]"),
                "{want}[T]() subscript was not substituted: {rendered}"
            );
        }
    }

    #[test]
    fn concrete_generic_element_ctor_mangles_not_leaves_bare_generic() {
        // Regression (#1015): a *concrete* generic element — `set[Optional[int64]]()`
        // — must have its ctor subscript rewritten to the mangled instance
        // (`Optional$int64`), exactly as the structural rewrite did before, so
        // lowering resolves it; the earlier reconstruction left a bare generic
        // `Optional` and broke the `optional_set_elem` golden.
        // Prelude-loaded so `Optional` is a known generic enum (as in the driver).
        let m = mono_prelude(
            "def main():\n    \
             auto s = set[Optional[int64]]()\n    \
             s.add(Optional.Some(5))\n",
        );
        let f = m
            .items
            .iter()
            .find_map(|i| match i {
                Item::Function(f) if f.name == "main" => Some(f),
                _ => None,
            })
            .expect("main present");
        let Stmt::Let { value, .. } = &f.body.stmts[0] else {
            panic!("expected `auto s = set[...]()`, got {:?}", f.body.stmts[0]);
        };
        let rendered = zapc_syntax::ast::render_expr(value);
        assert!(
            rendered.contains("Optional$") && !rendered.contains("Optional[int64]"),
            "set[Optional[int64]]() subscript not mangled: {rendered}"
        );
    }

    // ---- struct field-default initializers (#1152) -----------------------
    //
    // A per-instance field default (D-0038) is an initializer expression that
    // must be monomorphized exactly like a function body or a parameter
    // default: a generic constructor in the default (`Atomic[int64] state =
    // Atomic(0)`) has to be specialized to its mangled instance
    // (`Atomic$int64(0)`). Before #1152 the field loops rewrote only the field
    // *type* and left the default expression untouched, so the bare generic
    // name survived into IR lowering and failed as "call to unknown function
    // 'Atomic'". These tests pin the rewrite at the mono boundary (the mangled
    // symbol must appear, the bare generic name must not); the golden suite
    // covers the end-to-end runtime behavior against the real prelude `Atomic`.

    /// The default expression of field `fname` on struct `sname`, rendered.
    fn field_default_rendered(m: &Module, sname: &str, fname: &str) -> String {
        let s = struct_named(m, sname).unwrap_or_else(|| panic!("struct {sname} emitted"));
        let f = s
            .fields
            .iter()
            .find(|f| f.name == fname)
            .unwrap_or_else(|| panic!("field {fname} on {sname}"));
        let d = f
            .default
            .as_ref()
            .unwrap_or_else(|| panic!("field {fname} has a default"));
        zapc_syntax::ast::render_expr(d)
    }

    #[test]
    fn field_default_generic_ctor_is_mangled_nongeneric_struct() {
        // The non-generic-struct path (`rewrite_struct_nongeneric`): a generic
        // constructor used as a field default must be specialized, driven by
        // the field's declared instance type.
        let m = mono_ok(
            "struct Box[T]:\n    T value\n\
             struct Client:\n    Box[int64] state = Box(0)\n\
             def main():\n    auto c = Client()\n    print(c.state.value)\n",
        );
        let rendered = field_default_rendered(&m, "Client", "state");
        assert!(
            rendered.contains("Box$int64") && !rendered.contains("Box(0)"),
            "field default generic ctor not mangled: {rendered}"
        );
        assert!(
            struct_named(&m, "Box$int64").is_some(),
            "the field default's generic instance must be emitted"
        );
    }

    #[test]
    fn field_default_generic_ctor_is_mangled_generic_struct() {
        // The generic-struct specialization path
        // (`rewrite_struct_instance`): a field default that constructs another
        // generic instance keyed off the *owner's* type parameter is rewritten
        // under the ambient substitution.
        let m = mono_ok(
            "struct Box[T]:\n    T value\n\
             struct Holder[T]:\n    Box[T] slot = Box(0)\n\
             def main():\n    auto h = Holder[int64]()\n    print(h.slot.value)\n",
        );
        let rendered = field_default_rendered(&m, "Holder$int64", "slot");
        assert!(
            rendered.contains("Box$int64") && !rendered.contains("Box(0)"),
            "generic-struct field default not mangled under subst: {rendered}"
        );
    }

    #[test]
    fn field_default_rewrite_matches_let_initializer() {
        // The field default must be monomorphized *identically* to the same
        // expression written as a `let` initializer — that parity is the fix.
        // Here the generic constructor is resolved by argument unification
        // (exactly as `auto x = Box(0)` is), so both forms yield `Box$int64`.
        let fd = {
            let m = mono_ok(
                "struct Box[T]:\n    T value\n\
                 struct Client:\n    Box[int64] state = Box(0)\n\
                 def main():\n    auto c = Client()\n    print(c.state.value)\n",
            );
            field_default_rendered(&m, "Client", "state")
        };
        let letv = {
            let m = mono_ok(
                "struct Box[T]:\n    T value\n\
                 def main():\n    Box[int64] x = Box(0)\n    print(x.value)\n",
            );
            let f = m
                .items
                .iter()
                .find_map(|i| match i {
                    Item::Function(f) if f.name == "main" => Some(f),
                    _ => None,
                })
                .expect("main present");
            let Stmt::Let { value, .. } = &f.body.stmts[0] else {
                panic!("expected a let, got {:?}", f.body.stmts[0]);
            };
            zapc_syntax::ast::render_expr(value)
        };
        assert_eq!(
            fd, letv,
            "a field default must monomorphize exactly like the same `let` initializer"
        );
    }

    #[test]
    fn field_default_argless_generic_ctor_uses_field_type_sink() {
        // An *arg-less* generic constructor cannot infer its instance from
        // arguments, so the field's declared type is the expected-type sink
        // (mirroring the `Stmt::Let` rule). Without threading the field type
        // through, this would fail to resolve `T`.
        let m = mono_ok(
            "struct Box[T]:\n    T value = 0\n\
             struct Client:\n    Box[int64] state = Box()\n\
             def main():\n    auto c = Client()\n    print(c.state.value)\n",
        );
        let rendered = field_default_rendered(&m, "Client", "state");
        assert!(
            rendered.contains("Box$int64") && !rendered.contains("Box()"),
            "arg-less generic ctor default not resolved via field-type sink: {rendered}"
        );
    }

    #[test]
    fn field_default_decorator_generic_ctor_is_mangled() {
        // A computed default via `@default(expr)` carries the same kind of
        // initializer expression and must be rewritten too.
        let m = mono_ok(
            "struct Box[T]:\n    T value\n\
             struct Client:\n    @default(Box(0))\n    Box[int64] state\n\
             def main():\n    auto c = Client()\n    print(c.state.value)\n",
        );
        let s = struct_named(&m, "Client").expect("Client emitted");
        let f = s.fields.iter().find(|f| f.name == "state").unwrap();
        let dec = f
            .decorators
            .iter()
            .find(|d| d.name == "default")
            .expect("@default decorator preserved");
        let rendered = zapc_syntax::ast::render_expr(&dec.args[0].value);
        assert!(
            rendered.contains("Box$int64") && !rendered.contains("Box(0)"),
            "@default decorator generic ctor not mangled: {rendered}"
        );
    }

    // ---- interface method signature rewriting (#1309) ---------------------

    fn interface_named<'a>(m: &'a Module, name: &str) -> Option<&'a InterfaceDef> {
        m.items.iter().find_map(|i| match i {
            Item::Interface(iface) if iface.name == name => Some(iface),
            _ => None,
        })
    }

    /// The `(head-name, arg-count)` of a `Type::Named`; panics otherwise. A
    /// mangled generic instance is a nullary `Named` whose name carries `$`.
    fn named_head(t: &Type) -> (&str, usize) {
        match t {
            Type::Named { name, args, .. } => (name.as_str(), args.len()),
            other => panic!("expected a named type, got {other:?}"),
        }
    }

    fn method_sig<'a>(iface: &'a InterfaceDef, method: &str) -> &'a MethodSig {
        iface
            .methods
            .iter()
            .find(|m| m.name == method)
            .expect("method present")
    }

    #[test]
    fn interface_generic_return_type_is_mangled_and_requested() {
        // #1309: an interface method returning a generic instance (`Optional[int64]`)
        // must have that return renamed to the mangled concrete declaration
        // (`Optional$int64`, a nullary named type) AND that instance emitted —
        // exactly as a struct field or a function signature is. Before the fix the
        // interface passed through verbatim, so lowering saw an un-mangled head
        // with its type arguments ignored and lost the Optional type.
        let m = mono_ok(&format!(
            "{OPTIONAL}interface Reader:\n    def end_time(self) -> Optional[int64]\n\
             def finished(Reader reader) -> bool:\n    return reader.end_time().is_some()\n"
        ));
        let iface = interface_named(&m, "Reader").expect("Reader interface emitted");
        let ret = method_sig(iface, "end_time")
            .ret
            .as_ref()
            .expect("has a return");
        let (head, argc) = named_head(ret);
        assert_eq!(head, "Optional$int64", "return head not mangled: {head}");
        assert_eq!(argc, 0, "mangled instance must be nullary, got {argc} args");
        // The instance it names must be emitted (requested onto the worklist), or
        // lowering has no concrete declaration to resolve the head against.
        let emitted_enum = m.items.iter().any(|i| match i {
            Item::Enum(e) => e.name == "Optional$int64",
            _ => false,
        });
        assert!(
            emitted_enum,
            "the Optional$int64 instance named by the interface return was not emitted"
        );
    }

    #[test]
    fn interface_generic_param_type_is_mangled() {
        // The parameter side of a method signature must be rewritten identically:
        // `def accept(self, Optional[int64] v)` → the mangled instance. The
        // leading `self` is left untouched.
        let m = mono_ok(&format!(
            "{OPTIONAL}interface Sink:\n    def accept(self, Optional[int64] v) -> bool\n\
             def main():\n    print(1)\n"
        ));
        let iface = interface_named(&m, "Sink").expect("Sink interface emitted");
        let method = method_sig(iface, "accept");
        let v = method
            .params
            .iter()
            .find(|p| p.name == "v")
            .expect("parameter v present");
        let (head, argc) = named_head(v.ty.as_ref().expect("v annotated"));
        assert_eq!(head, "Optional$int64", "param head not mangled: {head}");
        assert_eq!(argc, 0, "mangled param instance must be nullary");
        // `self` is never rewritten (it carries no spellable annotation).
        assert_eq!(method.params.first().map(|p| p.name.as_str()), Some("self"));
    }

    #[test]
    fn interface_nested_generic_return_is_mangled() {
        // A compound generic return (`Optional[list[int64]]`) renames its outer
        // head to a nullary mangled instance (the `list[int64]` argument is
        // mangled into that single name), so no bracketed head survives.
        let m = mono_ok(&format!(
            "{OPTIONAL}interface Rows:\n    def head(self) -> Optional[list[int64]]\n\
             def has(Rows r) -> bool:\n    return r.head().is_some()\n"
        ));
        let iface = interface_named(&m, "Rows").expect("Rows interface emitted");
        let ret = method_sig(iface, "head")
            .ret
            .as_ref()
            .expect("has a return");
        let (head, argc) = named_head(ret);
        assert!(
            head.starts_with("Optional$") && argc == 0,
            "nested generic interface return not mangled to a nullary instance: \
             {head} ({argc} args)"
        );
    }

    #[test]
    fn plain_interface_passes_through_unchanged() {
        // An interface whose signatures name no generic instance is emitted
        // verbatim: the rewrite is a pure augmentation on the generic case.
        let m = mono_ok(
            "interface Named:\n    def name(self) -> int64\n\
             def label(Named n) -> int64:\n    return n.name()\n",
        );
        let iface = interface_named(&m, "Named").expect("Named interface emitted");
        let ret = method_sig(iface, "name")
            .ret
            .as_ref()
            .expect("has a return");
        assert_eq!(named_head(ret), ("int64", 0));
    }

    #[test]
    fn generic_interface_is_left_untouched() {
        // A *generic* interface's method signatures mention the interface's own
        // type parameters, which must not be mistaken for instances to mangle, so
        // a generic interface is passed through unchanged (its return stays the
        // bare associated/parameter name, never a `$`-mangled instance).
        let m = mono_ok(
            "interface Producer[In]:\n    type Out\n    def make(self, In seed) -> Out\n\
             def main():\n    print(1)\n",
        );
        let iface = interface_named(&m, "Producer").expect("Producer interface emitted");
        let ret = method_sig(iface, "make")
            .ret
            .as_ref()
            .expect("has a return");
        assert_eq!(
            named_head(ret),
            ("Out", 0),
            "generic interface return must be left as-is"
        );
    }
}
