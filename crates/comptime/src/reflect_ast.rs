//! Reflection records built **directly from the concrete AST**, with no sema
//! `Checker` (#1251).
//!
//! E1's resolver (`zapc_sema::analyze_reflect`) produces [`TypeInfo`] from the
//! checker's registries; that path depends on sema. Monomorphization does *not*
//! depend on sema — by the time it specializes a `comptime for` the concrete
//! `struct`/`enum` declaration is in hand as plain AST, with field names,
//! declaration order, attributes and spans all present (#1249: the
//! generator/verification boundary). So the unroller reflects the shape it needs
//! straight off the `StructDef` / `EnumDef`, mirroring E1's field/variant shape.
//!
//! `FieldInfo::ty` is a best-effort primitive classification of the *declared*
//! type (`decl_ty` holds the exact declared [`Type`]); resolving a field's full
//! comptime type — the kind-switch recursion — is deferred to #1253, and the
//! evaluator reports `.ty` as out of fragment until then, so an unresolved
//! element type never silently misleads.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use zapc_sema_types::{
    primitive_name, AttrArg, AttrInfo, FieldInfo, Ty, TypeInfo, TypeShape, VariantInfo,
};
use zapc_syntax::ast::{
    Decorator, EnumDef, EnumVariant, Expr, Field, Module, StructDef, Type, VariantField,
};

use crate::CtError;
use zapc_syntax::span::Span;

/// Build the [`TypeInfo`] for a concrete `struct` declaration: its declared data
/// fields in declaration order (#1066), each a full [`FieldInfo`].
pub fn type_info_of_struct(def: &StructDef) -> TypeInfo {
    TypeInfo {
        shape: TypeShape::Struct {
            name: def.name.clone(),
            fields: def
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| field_info(i, f))
                .collect(),
        },
    }
}

/// Build the [`TypeInfo`] for a concrete `enum` declaration: its variants in
/// declaration order, each with its payload fields (empty for a tier-1 constant).
pub fn type_info_of_enum(def: &EnumDef) -> TypeInfo {
    TypeInfo {
        shape: TypeShape::Enum {
            name: def.name.clone(),
            variants: def
                .variants
                .iter()
                .enumerate()
                .map(|(i, v)| variant_info(i, v))
                .collect(),
        },
    }
}

fn field_info(index: usize, f: &Field) -> FieldInfo {
    FieldInfo {
        name: f.name.clone(),
        ty: ty_of_decl(&f.ty),
        decl_ty: f.ty.clone(),
        index,
        attrs: f.decorators.iter().map(attr_info_of).collect(),
        span: f.span,
    }
}

fn variant_info(index: usize, v: &EnumVariant) -> VariantInfo {
    VariantInfo {
        name: v.name.clone(),
        index,
        fields: v
            .fields
            .iter()
            .enumerate()
            .map(|(i, vf)| variant_field_info(i, vf))
            .collect(),
        // Enum variant payloads carry no attribute grammar (E1, HQ8).
        attrs: vec![],
        span: v.span,
    }
}

fn variant_field_info(index: usize, vf: &VariantField) -> FieldInfo {
    FieldInfo {
        name: vf.name.clone(),
        ty: ty_of_decl(&vf.ty),
        decl_ty: vf.ty.clone(),
        index,
        attrs: vec![],
        span: vf.span,
    }
}

/// Mirror of E1's `attr_info_of`: a decorator rendered to structured,
/// comptime-queryable data (name + args; no per-attribute compiler meaning).
fn attr_info_of(d: &Decorator) -> AttrInfo {
    AttrInfo {
        name: d.name.clone(),
        args: d
            .args
            .iter()
            .map(|a| AttrArg {
                name: a.name.clone(),
                value: render_attr_value(&a.value),
            })
            .collect(),
        span: d.span,
    }
}

/// Mirror of E1's `render_attr_value`: an attribute argument expression rendered
/// to its canonical string (a string literal without quotes, a numeric lexeme,
/// `true`/`false`, a bare name, or `…` for a form E1 does not render).
fn render_attr_value(e: &Expr) -> String {
    match e {
        Expr::Int(lex, _) | Expr::Float(lex, _) | Expr::Imaginary(lex, _) => lex.clone(),
        Expr::Str { value, .. } => value.clone(),
        Expr::Bool(b, _) => b.to_string(),
        Expr::None(_) => "None".to_string(),
        Expr::Name(n, _) => n.clone(),
        _ => "…".to_string(),
    }
}

/// Best-effort classification of a declared primitive type to its canonical
/// [`Ty`]. A non-primitive (container, user type, generic parameter) maps to
/// [`Ty::Unknown`] — `decl_ty` keeps the exact declared type, and full
/// resolution of a field's element/kind is #1253.
fn ty_of_decl(t: &Type) -> Ty {
    let Type::Named { name, args, .. } = t else {
        return Ty::Unknown;
    };
    if !args.is_empty() {
        return Ty::Unknown;
    }
    match name.as_str() {
        "int64" => Ty::Int64,
        "int32" => Ty::Int32,
        "int16" => Ty::Int16,
        "int8" => Ty::Int8,
        "uint8" => Ty::Uint8,
        "uint16" => Ty::Uint16,
        "uint32" => Ty::Uint32,
        "uint64" => Ty::Uint64,
        "usize" => Ty::Usize,
        "float64" => Ty::Float64,
        "float32" => Ty::Float32,
        "bool" => Ty::Bool,
        "str" => Ty::Str,
        "bytes" => Ty::Bytes,
        "char" => Ty::Char,
        "bigint" => Ty::Bigint,
        "complex" => Ty::Complex,
        _ => Ty::Unknown,
    }
}

// ---------------------------------------------------------------------------
// TypeRegistry — the no-sema `Type`/`Ty` -> `TypeInfo` resolver (#1253)
// ---------------------------------------------------------------------------

/// A registry of a module's user `struct`/`enum` declarations, built once from
/// the AST, that resolves a declared [`Type`] or a resolved [`Ty`] to its
/// [`TypeInfo`] — the no-sema analog of E1's `Checker::reflect_type_info`.
///
/// This is what drives `type_info(T).kind`-switch **container recursion**
/// (#1253): a container shape stores its element/key/value type, and the
/// evaluator resolves that back to a nested `TypeInfo` so a comptime kind-switch
/// can recurse on `info.elem` / `info.key` / `info.value` / `info.inner` /
/// `info.elems`. Resolution is **one level** (like E1): it never flattens the
/// type graph, so a depth-capped walk over recursive type shapes terminates.
#[derive(Debug, Default, Clone)]
pub struct TypeRegistry {
    /// User struct/enum name -> its one-level `TypeInfo`.
    types: HashMap<String, TypeInfo>,
    /// User type name -> declared method names (for the override hook).
    methods: HashMap<String, HashSet<String>>,
    /// `type_info(T)` memo (#1255, HQ5): each type's record is computed once and
    /// reused on every later introspection, keyed by a canonical type key. An
    /// in-process cache for v1 (on-disk / cross-build caching deferred).
    cache: RefCell<HashMap<String, Rc<TypeInfo>>>,
    /// Memo accounting, so a benchmark can assert each type is computed once.
    stats: Cell<MemoStats>,
}

/// `type_info(T)` memo accounting (#1255): the comptime-reflection compile-time
/// cost signal. `computed` is the number of distinct type records actually
/// built (memo misses); `reused` the number served from the memo (hits). The
/// stable, deterministic metric a CI compile-time benchmark tracks — growth or a
/// regression (more computations than expected) is visible without timing
/// flakiness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoStats {
    /// Distinct type records built (memo misses).
    pub computed: u64,
    /// Type records served from the memo (memo hits).
    pub reused: u64,
}

impl MemoStats {
    /// Total introspection requests (`computed + reused`).
    pub fn total(self) -> u64 {
        self.computed + self.reused
    }
}

impl TypeRegistry {
    /// An empty registry.
    pub fn new() -> TypeRegistry {
        TypeRegistry::default()
    }

    /// Register a struct declaration.
    pub fn add_struct(&mut self, def: &StructDef) {
        self.types
            .insert(def.name.clone(), type_info_of_struct(def));
        self.methods.insert(
            def.name.clone(),
            def.methods.iter().map(|m| m.name.clone()).collect(),
        );
    }

    /// Register an enum declaration.
    pub fn add_enum(&mut self, def: &EnumDef) {
        self.types.insert(def.name.clone(), type_info_of_enum(def));
        self.methods.insert(
            def.name.clone(),
            def.methods.iter().map(|m| m.name.clone()).collect(),
        );
    }

    /// Build a registry from every struct/enum declared in `module`.
    pub fn from_module(module: &Module) -> TypeRegistry {
        let mut reg = TypeRegistry::new();
        for item in &module.items {
            match item {
                zapc_syntax::ast::Item::Struct(s) => reg.add_struct(s),
                zapc_syntax::ast::Item::Enum(e) => reg.add_enum(e),
                _ => {}
            }
        }
        reg
    }

    /// `true` when the named user type declares a method `method` — the override
    /// hook (#1248): a type that supplies its own handler (e.g. `serialize`) is
    /// preferred over reflection, so a comptime kind-switch can short-circuit to
    /// it. Full override *policy* (which method, how it is called) is E3; this
    /// is the mechanism.
    pub fn has_method(&self, type_name: &str, method: &str) -> bool {
        self.methods
            .get(type_name)
            .is_some_and(|set| set.contains(method))
    }

    /// The `type_info(T)` memo accounting so far (#1255).
    pub fn memo_stats(&self) -> MemoStats {
        self.stats.get()
    }

    /// Reset the memo accounting (keeps the cache); for benchmarking a region.
    pub fn reset_stats(&self) {
        self.stats.set(MemoStats::default());
    }

    /// Serve a cached record for `key`, or compute it with `build` once and
    /// cache it (#1255). Errors are not cached (and don't count as computed).
    fn memoized(
        &self,
        key: String,
        build: impl FnOnce() -> Result<TypeInfo, CtError>,
    ) -> Result<TypeInfo, CtError> {
        if let Some(rc) = self.cache.borrow().get(&key) {
            let mut s = self.stats.get();
            s.reused += 1;
            self.stats.set(s);
            return Ok((**rc).clone());
        }
        let ti = build()?;
        let mut s = self.stats.get();
        s.computed += 1;
        self.stats.set(s);
        self.cache.borrow_mut().insert(key, Rc::new(ti.clone()));
        Ok(ti)
    }

    /// Resolve a resolved [`Ty`] (as stored in a container `TypeShape`) to its
    /// [`TypeInfo`]. Used by the evaluator's `.elem`/`.key`/`.value`/`.inner`/
    /// `.elems` container accessors. Memoized per type (#1255).
    pub fn resolve_ty(&self, ty: &Ty, span: Span) -> Result<TypeInfo, CtError> {
        self.memoized(format!("ty:{}", ty.display()), || {
            self.resolve_ty_uncached(ty, span)
        })
    }

    fn resolve_ty_uncached(&self, ty: &Ty, span: Span) -> Result<TypeInfo, CtError> {
        if let Some(name) = primitive_name(ty) {
            return Ok(TypeInfo {
                shape: TypeShape::Primitive {
                    name: name.to_string(),
                },
            });
        }
        let shape = match ty {
            Ty::List(e) => TypeShape::List {
                elem: (**e).clone(),
            },
            Ty::Dict(k, v) => TypeShape::Dict {
                key: (**k).clone(),
                value: (**v).clone(),
            },
            Ty::Tuple(es) => TypeShape::Tuple { elems: es.clone() },
            // `Optional[T]` now carries its payload in the `Ty` (#1452), so a
            // container element that is itself an optional reflects with its real
            // element type. The legacy `Named("Optional")` spelling (payload
            // erased) still reflects as a bare optional (`Unknown` element).
            Ty::Optional(e) => TypeShape::Optional {
                elem: (**e).clone(),
            },
            Ty::Named(n) if n == "Optional" => TypeShape::Optional { elem: Ty::Unknown },
            Ty::Named(n) if !n.starts_with('$') => {
                return self.named(n, span);
            }
            other => {
                return Err(CtError::not_available(
                    format!("a comptime type `{}`", other.display()),
                    span,
                ));
            }
        };
        Ok(TypeInfo { shape })
    }

    /// Resolve a declared [`Type`] (as written) to its [`TypeInfo`]. Used by the
    /// evaluator's `field.ty` accessor and for a top-level entry; unlike
    /// [`resolve_ty`](Self::resolve_ty) it recovers an `Optional[T]`'s element
    /// from the declared argument (E1 parity). Memoized per type (#1255).
    pub fn resolve_type(&self, t: &Type, span: Span) -> Result<TypeInfo, CtError> {
        self.memoized(format!("type:{}", type_key(t)), || {
            self.resolve_type_uncached(t, span)
        })
    }

    fn resolve_type_uncached(&self, t: &Type, span: Span) -> Result<TypeInfo, CtError> {
        match t {
            Type::Tuple { elems, .. } => Ok(TypeInfo {
                shape: TypeShape::Tuple {
                    elems: elems.iter().map(ty_of_type).collect(),
                },
            }),
            Type::Named { name, args, .. } => {
                if name == "Optional" {
                    let elem = args.first().map(ty_of_type).unwrap_or(Ty::Unknown);
                    return Ok(TypeInfo {
                        shape: TypeShape::Optional { elem },
                    });
                }
                if (name == "list" || name == "List") && args.len() == 1 {
                    return Ok(TypeInfo {
                        shape: TypeShape::List {
                            elem: ty_of_type(&args[0]),
                        },
                    });
                }
                if (name == "dict" || name == "Dict") && args.len() == 2 {
                    return Ok(TypeInfo {
                        shape: TypeShape::Dict {
                            key: ty_of_type(&args[0]),
                            value: ty_of_type(&args[1]),
                        },
                    });
                }
                let bare = Ty::Named(name.clone());
                if args.is_empty() {
                    if let Some(prim) = primitive_name(&ty_of_type(t)) {
                        return Ok(TypeInfo {
                            shape: TypeShape::Primitive {
                                name: prim.to_string(),
                            },
                        });
                    }
                    if !name.starts_with('$') {
                        return self.named(name, span);
                    }
                }
                Err(CtError::not_available(
                    format!("a comptime type `{}`", bare.display()),
                    span,
                ))
            }
            Type::Func { .. } => Err(CtError::not_available("a comptime function type", span)),
        }
    }

    fn named(&self, name: &str, span: Span) -> Result<TypeInfo, CtError> {
        self.types
            .get(name)
            .cloned()
            .ok_or_else(|| CtError::not_available(format!("a comptime type `{name}`"), span))
    }
}

/// A canonical, faithful key for a declared [`Type`] — distinct types get
/// distinct keys (so the memo never collides `Optional[int64]` with
/// `Optional[str]`), unlike the `Optional`-erasing [`ty_of_type`].
fn type_key(t: &Type) -> String {
    match t {
        Type::Named { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = args.iter().map(type_key).collect();
                format!("{name}[{}]", inner.join(","))
            }
        }
        Type::Tuple { elems, .. } => {
            let inner: Vec<String> = elems.iter().map(type_key).collect();
            format!("({})", inner.join(","))
        }
        Type::Func { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(type_key).collect();
            let r = ret.as_ref().map(|r| type_key(r)).unwrap_or_default();
            format!("fn({})->{}", ps.join(","), r)
        }
    }
}

/// Best-effort map of a declared [`Type`] to a resolved [`Ty`], for the
/// container/primitive cases the registry resolves. Carries the `Optional`
/// payload (→ `Ty::Optional(T)`, un-erased #1452).
fn ty_of_type(t: &Type) -> Ty {
    match t {
        Type::Tuple { elems, .. } => Ty::Tuple(elems.iter().map(ty_of_type).collect()),
        Type::Func { .. } => Ty::Unknown,
        Type::Named { name, args, .. } => match (name.as_str(), args.len()) {
            ("list" | "List", 1) => Ty::List(Box::new(ty_of_type(&args[0]))),
            ("dict" | "Dict", 2) => Ty::Dict(
                Box::new(ty_of_type(&args[0])),
                Box::new(ty_of_type(&args[1])),
            ),
            ("Optional", _) => Ty::Optional(Box::new(
                args.first().map(ty_of_type).unwrap_or(Ty::Unknown),
            )),
            _ => {
                // A bare name: a primitive maps to its `Ty`, any other (user
                // struct/enum, or an unsupported parameterized container) keeps
                // its name so the registry can resolve it (or fail cleanly).
                let prim = ty_of_decl(t);
                if matches!(prim, Ty::Unknown) {
                    Ty::Named(name.clone())
                } else {
                    prim
                }
            }
        },
    }
}
