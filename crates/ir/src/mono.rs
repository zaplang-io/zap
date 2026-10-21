//! Monomorphization (I#154, D-0017 / D-0064): the ZIR-stage pass between sema
//! and codegen that turns user-defined generics into ordinary concrete code.
//!
//! It runs on the AST at the very top of [`crate::lower::lower_module`] and
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
    self, Arg, Block, EnumDef, Expr, Function, InterfaceDef, Item, MethodSig, Module, PatArg,
    Pattern, Stmt, StructDef, Type, TypeParam,
};
use zapc_syntax::span::Span;

use crate::lower::LowerError;

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
    LowerError { message: m.into() }
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
                _ => {}
            }
        }
        m
    }

    fn run(mut self) -> Result<Module, LowerError> {
        // First rewrite all non-generic top-level items (dropping the generic
        // declarations themselves), which seeds the instantiation worklist.
        let mut out: Vec<Item> = Vec::new();
        let src = std::mem::take(&mut self.src_items);
        for item in &src {
            match item {
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
                Type::Named {
                    name: name.clone(),
                    args: args.iter().map(|a| self.apply_subst(a)).collect(),
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

    /// Does the concrete type `arg` structurally conform to interface `iface`
    /// (D-0016/I#84, re-derived at the AST level for monomorphization, I#164)?
    /// Every interface method must be present on the concrete type with a
    /// matching signature (parameter types excluding `self`, and return type).
    /// Returns the human-readable reasons on failure. Only user structs and enums
    /// carry methods; a primitive or built-in argument cannot satisfy a non-empty
    /// interface bound (operator-dunder interfaces as bounds on primitives are a
    /// documented follow-up).
    fn satisfies_bound(&self, arg: &Type, iface: &str) -> Result<(), Vec<String>> {
        let Some(idef) = self.interfaces.get(iface) else {
            return Err(vec![format!("'{iface}' is not an interface")]);
        };
        let Type::Named { name: tyname, .. } = arg else {
            return Err(vec![format!(
                "'{}' is not a struct or enum and cannot implement interface methods",
                type_name(arg)
            )]);
        };
        let methods: &[Function] = if let Some(s) = self.structs.get(tyname.as_str()) {
            &s.methods
        } else if let Some(e) = self.enums.get(tyname.as_str()) {
            &e.methods
        } else {
            return Err(vec![format!(
                "'{tyname}' is not a struct or enum and cannot implement interface methods"
            )]);
        };
        let mut reasons = Vec::new();
        for im in &idef.methods {
            match methods.iter().find(|m| m.name == im.name) {
                None => reasons.push(format!("missing method '{}'", im.name)),
                Some(cm) if !method_sig_matches(cm, im) => {
                    reasons.push(format!("method '{}' has a mismatched signature", im.name));
                }
                Some(_) => {}
            }
        }
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
            Stmt::Raise { value, span } => Stmt::Raise {
                value: match value {
                    Some(v) => Some(self.rw(v, env)?),
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
        })
    }

    fn rewrite_arg(&mut self, a: &Arg, env: &mut Env) -> Result<Arg, LowerError> {
        Ok(Arg {
            name: a.name.clone(),
            value: self.rw(&a.value, env)?,
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
            Expr::Str { bytes, .. } => {
                Ok((e.clone(), self.prim(if *bytes { "bytes" } else { "str" })))
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
            Expr::Set(xs, span) => Ok((Expr::Set(self.rw_vec(xs, env)?, *span), CTy::Unknown)),
            Expr::Dict(pairs, span) => {
                let mut np = Vec::with_capacity(pairs.len());
                for (k, v) in pairs {
                    np.push((self.rw(k, env)?, self.rw(v, env)?));
                }
                Ok((Expr::Dict(np, *span), CTy::Unknown))
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
                var,
                iter,
                cond,
                span,
            } => {
                let iter = Box::new(self.rw(iter, env)?);
                env.push();
                env.declare(var, CTy::Unknown);
                let elem = Box::new(self.rw(elem, env)?);
                let cond = self.rw_opt(cond, env)?;
                env.pop();
                Ok((
                    Expr::ListComp {
                        elem,
                        var: var.clone(),
                        iter,
                        cond,
                        span: *span,
                    },
                    CTy::Unknown,
                ))
            }
            Expr::SetComp {
                elem,
                var,
                iter,
                cond,
                span,
            } => {
                let iter = Box::new(self.rw(iter, env)?);
                env.push();
                env.declare(var, CTy::Unknown);
                let elem = Box::new(self.rw(elem, env)?);
                let cond = self.rw_opt(cond, env)?;
                env.pop();
                Ok((
                    Expr::SetComp {
                        elem,
                        var: var.clone(),
                        iter,
                        cond,
                        span: *span,
                    },
                    CTy::Unknown,
                ))
            }
            Expr::DictComp {
                key,
                value,
                var,
                iter,
                cond,
                span,
            } => {
                let iter = Box::new(self.rw(iter, env)?);
                env.push();
                env.declare(var, CTy::Unknown);
                let key = Box::new(self.rw(key, env)?);
                let value = Box::new(self.rw(value, env)?);
                let cond = self.rw_opt(cond, env)?;
                env.pop();
                Ok((
                    Expr::DictComp {
                        key,
                        value,
                        var: var.clone(),
                        iter,
                        cond,
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
}
