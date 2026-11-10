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
//! rule (re-derived here at the AST level, since ir does not depend on sema);
//! the bound's methods then resolve statically per instance through ordinary
//! type substitution — no fat pointer / vtable is emitted (design/0012 slice 5).

use std::collections::{HashMap, HashSet};

use zapc_syntax::ast::{
    self, Arg, Block, CompClause, DictItem, EnumDef, Expr, Function, InterfaceDef, Item, MethodSig,
    Module, PatArg, Pattern, Stmt, StructDef, Type, TypeParam,
};
use zapc_syntax::span::Span;

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
}

/// Monomorphize a module: produce an equivalent AST with all user generics
/// instantiated to concrete declarations and every generic reference rewritten.
pub fn monomorphize(module: &Module) -> Result<Module, LowerError> {
    Mono::new(module).run()
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

struct Mono<'a> {
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
    /// (ir does not depend on sema; it re-derives types — see module docs).
    interfaces: HashMap<String, &'a InterfaceDef>,
    /// Free-function return types, for inference of `f(...)` results.
    func_ret: HashMap<String, Option<Type>>,
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
/// (primitive aliases canonicalized).
fn method_sig_matches(have: &Function, want: &MethodSig) -> bool {
    let hp = have.params.get(1..).unwrap_or(&[]);
    let wp = want.params.get(1..).unwrap_or(&[]);
    hp.len() == wp.len()
        && hp
            .iter()
            .zip(wp.iter())
            .all(|(a, b)| opt_type_eq(a.ty.as_ref(), b.ty.as_ref()))
        && opt_type_eq(have.ret.as_ref(), want.ret.as_ref())
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
    fn new(module: &'a Module) -> Mono<'a> {
        let mut m = Mono {
            params: HashMap::new(),
            gstructs: HashMap::new(),
            genums: HashMap::new(),
            gfuncs: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            interfaces: HashMap::new(),
            func_ret: HashMap::new(),
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
                    if !f.generics.is_empty() {
                        m.gfuncs.insert(f.name.clone(), f);
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
                if args.is_empty() {
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
                    name: name.clone(),
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
        for (p, arg) in generics.iter().zip(args.iter()) {
            let Some(bound) = &p.bound else { continue };
            let Type::Named { name: iface, .. } = bound else {
                return Err(err(format!(
                    "type parameter '{}' of {what} has an invalid bound: a bound must name \
                     an interface",
                    p.name
                )));
            };
            if let Err(reasons) = self.satisfies_bound(arg, iface) {
                return Err(err(format!(
                    "type '{}' does not satisfy the bound '{}' on type parameter '{}' of \
                     {what}: {}",
                    type_name(arg),
                    iface,
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
    fn satisfies_bound(&self, arg: &Type, iface: &str) -> Result<(), Vec<String>> {
        if let Some(cap) = Capability::from_iface(iface) {
            let mut visited = HashSet::new();
            return self.type_has_capability(arg, cap, &mut visited);
        }
        if !self.interfaces.contains_key(iface) {
            return Err(vec![format!("'{iface}' is not an interface")]);
        }
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
            for im in &idef.methods {
                match methods.iter().find(|m| m.name == im.name) {
                    None => reasons.push(format!("missing method '{}'{attr}", im.name)),
                    Some(cm) if !method_sig_matches(cm, im) => reasons.push(format!(
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
            "list" | "dict" | "set" | "heap" | "channel" | "array" => Err(vec![format!(
                "a mutable collection ('{name}') does not implement '{}' (it has no stable hash \
                 or ordering)",
                cap.name()
            )]),
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
                let mut nf = f.clone();
                nf.ty = self.rewrite_type(&f.ty);
                fields.push(nf);
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
        Ok(())
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
            let mut nf = f.clone();
            nf.ty = self.rewrite_type(&f.ty);
            fields.push(nf);
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
            if p.name != "self" {
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
        let saved_subst = std::mem::replace(&mut self.cur_subst, subst);
        let saved_self = self.cur_self.take();
        let spec = self.rewrite_fn_inner(f, None, Some(mangled));
        self.cur_subst = saved_subst;
        self.cur_self = saved_self;
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
        let saved_subst = std::mem::replace(&mut self.cur_subst, subst);
        let saved_self = self
            .cur_self
            .replace((mi.owner.clone(), mi.owner_args.clone()));
        let spec = self.rewrite_fn_inner(method, Some(self_ty), Some(method_spec));
        self.cur_subst = saved_subst;
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
            stmts.push(self.rewrite_stmt(st, env)?);
        }
        Ok(Block {
            stmts,
            span: b.span,
        })
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
                let iter = self.rw(iter, env)?;
                env.push();
                if let Expr::Name(n, _) = target {
                    env.declare(n, CTy::Unknown);
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
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                let (sexpr, scty) = self.rewrite_expr(scrutinee, env, None)?;
                let sty = cty_type(scty);
                let mut narms = Vec::with_capacity(arms.len());
                for arm in arms {
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

    fn rewrite_expr(
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
                let base = self.rw(base, env)?;
                let index = self.rw(index, env)?;
                Ok((
                    Expr::Index {
                        base: Box::new(base),
                        index: Box::new(index),
                        span: *span,
                    },
                    CTy::Unknown,
                ))
            }
            Expr::Unary { op, expr, span } => Ok((
                Expr::Unary {
                    op: *op,
                    expr: Box::new(self.rw(expr, env)?),
                    span: *span,
                },
                CTy::Unknown,
            )),
            Expr::Binary { op, lhs, rhs, span } => Ok((
                Expr::Binary {
                    op: *op,
                    lhs: Box::new(self.rw(lhs, env)?),
                    rhs: Box::new(self.rw(rhs, env)?),
                    span: *span,
                },
                CTy::Unknown,
            )),
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
            Expr::Tuple(xs, span) => Ok((Expr::Tuple(self.rw_vec(xs, env)?, *span), CTy::Unknown)),
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
            } => Ok((
                Expr::Slice {
                    base: Box::new(self.rw(base, env)?),
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
            // Comprehensions, lambdas, regex, yield: rewrite nested exprs where
            // they can carry generic references; bind the comprehension var as
            // unknown. These are not generic-typed in the first vertical.
            Expr::ListComp {
                elem,
                clauses,
                span,
            } => {
                env.push();
                let clauses = self.rw_comp_clauses(clauses, env)?;
                let elem = Box::new(self.rw(elem, env)?);
                env.pop();
                Ok((
                    Expr::ListComp {
                        elem,
                        clauses,
                        span: *span,
                    },
                    CTy::Unknown,
                ))
            }
            Expr::SetComp {
                elem,
                clauses,
                span,
            } => {
                env.push();
                let clauses = self.rw_comp_clauses(clauses, env)?;
                let elem = Box::new(self.rw(elem, env)?);
                env.pop();
                Ok((
                    Expr::SetComp {
                        elem,
                        clauses,
                        span: *span,
                    },
                    CTy::Unknown,
                ))
            }
            // Generator expression (#596): rewrite its nested exprs like a
            // comprehension; it is not generic-typed in the first vertical.
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
                let key = Box::new(self.rw(key, env)?);
                let value = Box::new(self.rw(value, env)?);
                env.pop();
                Ok((
                    Expr::DictComp {
                        key,
                        value,
                        clauses,
                        span: *span,
                    },
                    CTy::Unknown,
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

    fn rw_vec(&mut self, xs: &[Expr], env: &mut Env) -> Result<Vec<Expr>, LowerError> {
        let mut out = Vec::with_capacity(xs.len());
        for x in xs {
            out.push(self.rw(x, env)?);
        }
        Ok(out)
    }

    /// Rewrite a comprehension clause list (#599). Each clause's iterable is
    /// rewritten with the previous clauses' variables in scope; its loop
    /// variable is then declared (as `Unknown` — comprehensions are not
    /// generic-typed in the first vertical) so later clauses and filters can
    /// refer to it. The caller pushes and pops the surrounding scope.
    fn rw_comp_clauses(
        &mut self,
        clauses: &[CompClause],
        env: &mut Env,
    ) -> Result<Vec<CompClause>, LowerError> {
        let mut out = Vec::with_capacity(clauses.len());
        for c in clauses {
            let iter = Box::new(self.rw(&c.iter, env)?);
            env.declare(&c.var, CTy::Unknown);
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
    fn type_args_from_index(&self, index: &Expr) -> Option<Vec<Type>> {
        let elems: Vec<&Expr> = match index {
            Expr::Tuple(xs, _) => xs.iter().collect(),
            other => vec![other],
        };
        let mut out = Vec::with_capacity(elems.len());
        for e in elems {
            out.push(self.apply_subst(&expr_to_type(e)?));
        }
        Some(out)
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
        let cargs = self.type_args_from_index(index)?;
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
    fn infer_fn_targs(
        &self,
        f: &Function,
        owner_subst: &HashMap<String, Type>,
        argpairs: &[(Option<String>, CTy)],
        expected: Option<&Type>,
        what: &str,
    ) -> Result<Vec<Type>, LowerError> {
        let tparams: Vec<String> = f.generics.iter().map(|g| g.name.clone()).collect();
        let tparams = tparams.as_slice();
        let ret = f.ret.as_ref();
        // Declared value parameters (the `self` receiver excluded), with the
        // owner's type parameters substituted so only the fn/method's own type
        // parameters remain free.
        let vparams: Vec<(String, Type)> = f
            .params
            .iter()
            .filter(|p| p.name != "self")
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

    fn rewrite_call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        span: Span,
        env: &mut Env,
        expected: Option<&Type>,
    ) -> Result<(Expr, CTy), LowerError> {
        // Rewrite the arguments once; keep their inferred types for unification.
        let mut nargs = Vec::with_capacity(args.len());
        let mut argpairs = Vec::with_capacity(args.len());
        for a in args {
            let (v, cty) = self.rewrite_expr(&a.value, env, None)?;
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
                        if let Some(cargs) = self.type_args_from_index(index) {
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
            if variant == "get" {
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
                    if let Some(cargs) = self.type_args_from_index(index) {
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
                    if let Some(targs) = self.type_args_from_index(index) {
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
                            if let Some(targs) = self.type_args_from_index(index) {
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
                    if let Some(cargs) = self.type_args_from_index(index) {
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

/// Convert a type-argument *expression* into an `ast::Type` (names and nested
/// generic instantiations only).
fn expr_to_type(e: &Expr) -> Option<Type> {
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
                Expr::Tuple(xs, _) => xs.iter().collect(),
                other => vec![other],
            };
            let mut args = Vec::with_capacity(elems.len());
            for el in elems {
                args.push(expr_to_type(el)?);
            }
            Some(Type::Named {
                name: g.clone(),
                args,
                span: *span,
            })
        }
        _ => None,
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
}
