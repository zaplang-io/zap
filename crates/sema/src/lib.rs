//! Zap semantic analysis — the type checker (Phase 1).
//!
//! Scope for this pass: scalars and functions with local inference, enough to
//! type-check the milestone programs (`fib`, `loop_sum`). It resolves function
//! signatures, infers local variable types (`auto`), checks declared types and
//! assignments (with implicit widening per D-0020), and checks calls and
//! returns. Richer features — struct fields, generics, interfaces, full
//! pattern typing — are intentionally handled loosely (as `Unknown`) for now.

use std::collections::{HashMap, HashSet};

use zapc_syntax::ast::*;
use zapc_syntax::span::Span;

/// A type-checking error with the offending source span.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

/// A resolved type.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Int64,
    Int32,
    Float64,
    Float32,
    Bool,
    Str,
    Bytes,
    Char,
    Bigint,
    Complex,
    /// An untyped integer literal (Go-style), assignable to any numeric type;
    /// defaults to `int64` when it must become concrete.
    UntypedInt,
    /// An untyped float literal; defaults to `float64`.
    UntypedFloat,
    /// No value (void function result / statement).
    Unit,
    /// The result of `range(...)`: an iterable yielding `int64`.
    Range,
    List(Box<Ty>),
    Dict(Box<Ty>, Box<Ty>),
    Set(Box<Ty>),
    /// A fixed-arity, heterogeneous, immutable tuple (D-0019/D-0049) carrying its
    /// element types in positional order. The empty tuple `()` is the unit value.
    Tuple(Vec<Ty>),
    /// A typed channel `channel[T]` (ticket #25, D-0031): a first-class reference
    /// value over which `T` values are sent (`ch <- v`) and received (`<-ch`).
    Chan(Box<Ty>),
    /// A user-defined type (struct/enum), treated opaquely for now.
    Named(String),
    /// A regex match object (result of `subject =~ m/.../`, D-0010). Truthy on
    /// success; carries captured groups accessed via `.group(...)`.
    Match,
    /// A Python object/handle (D-0012, `design/0003-python-interop.md`): a
    /// Python module imported with `import python`, or any value produced by a
    /// Python attribute access or call. Python is dynamic, so this type permits
    /// attribute access and calls without further constraint, and — like
    /// [`Ty::Unknown`] — is assignable to and from anything (values marshal at
    /// the boundary). Explicit conversions (`int64(x)`, `float64(x)`) and
    /// `print` marshal it back to a Zap value.
    Py,
    /// Not inferable yet — assignable to and from anything to avoid cascading
    /// errors.
    Unknown,
}

impl Ty {
    fn is_numeric(&self) -> bool {
        matches!(
            self,
            Ty::Int64
                | Ty::Int32
                | Ty::Float64
                | Ty::Float32
                | Ty::Bigint
                | Ty::Complex
                | Ty::UntypedInt
                | Ty::UntypedFloat
        )
    }

    fn is_int(&self) -> bool {
        matches!(self, Ty::Int64 | Ty::Int32 | Ty::Bigint | Ty::UntypedInt)
    }

    fn display(&self) -> String {
        match self {
            Ty::Int64 => "int64".into(),
            Ty::Int32 => "int32".into(),
            Ty::Float64 => "float64".into(),
            Ty::Float32 => "float32".into(),
            Ty::Bool => "bool".into(),
            Ty::Str => "str".into(),
            Ty::Bytes => "bytes".into(),
            Ty::Char => "char".into(),
            Ty::Bigint => "bigint".into(),
            Ty::Complex => "complex".into(),
            Ty::UntypedInt => "int".into(),
            Ty::UntypedFloat => "float".into(),
            Ty::Unit => "()".into(),
            Ty::Range => "range".into(),
            Ty::List(t) => format!("list[{}]", t.display()),
            Ty::Dict(k, v) => format!("dict[{}, {}]", k.display(), v.display()),
            Ty::Set(t) => format!("set[{}]", t.display()),
            Ty::Tuple(ts) => {
                let inner = ts
                    .iter()
                    .map(|t| t.display())
                    .collect::<Vec<_>>()
                    .join(", ");
                // A 1-tuple prints with the Python-style trailing comma.
                if ts.len() == 1 {
                    format!("({inner},)")
                } else {
                    format!("({inner})")
                }
            }
            Ty::Chan(t) => format!("channel[{}]", t.display()),
            Ty::Named(n) => n.clone(),
            Ty::Match => "Match".into(),
            Ty::Py => "py".into(),
            Ty::Unknown => "<unknown>".into(),
        }
    }
}

/// Is a value of type `from` assignable where `to` is expected? Implements
/// implicit widening (D-0020); narrowing is rejected.
fn assignable(from: &Ty, to: &Ty) -> bool {
    if from == to {
        return true;
    }
    if matches!(from, Ty::Unknown | Ty::Py) || matches!(to, Ty::Unknown | Ty::Py) {
        // Python handles (D-0012) are dynamic: values marshal across the
        // boundary, so a `py` value fits anywhere and anything fits into `py`.
        return true;
    }
    match (from, to) {
        // Untyped literals fit any appropriate numeric type.
        (
            Ty::UntypedInt,
            Ty::Int32
            | Ty::Int64
            | Ty::Float32
            | Ty::Float64
            | Ty::Bigint
            | Ty::Complex
            | Ty::UntypedFloat,
        ) => true,
        (Ty::UntypedFloat, Ty::Float32 | Ty::Float64 | Ty::Complex) => true,
        // Concrete widening.
        (Ty::Int32, Ty::Int64) => true,
        (Ty::Int32 | Ty::Int64, Ty::Float64 | Ty::Float32) => true,
        (Ty::Float32, Ty::Float64) => true,
        (Ty::Int32 | Ty::Int64, Ty::Bigint) => true,
        (Ty::List(a), Ty::List(b)) => assignable(a, b),
        (Ty::Set(a), Ty::Set(b)) => assignable(a, b),
        // A tuple is assignable to a tuple of the same arity when each element is
        // (covariantly) assignable — this covers returning untyped literals into
        // a declared tuple type (`-> (int64, int64)` with `return 1, 2`).
        (Ty::Tuple(a), Ty::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| assignable(x, y))
        }
        _ => false,
    }
}

/// Make an untyped literal type concrete (its default).
fn default_type(t: Ty) -> Ty {
    match t {
        Ty::UntypedInt => Ty::Int64,
        Ty::UntypedFloat => Ty::Float64,
        // Defaulting is element-wise for tuples, so `auto t = (1, 2.0)` becomes
        // `(int64, float64)` rather than carrying untyped-literal elements.
        Ty::Tuple(ts) => Ty::Tuple(ts.into_iter().map(default_type).collect()),
        other => other,
    }
}

/// The result type of a numeric binary operation (promotion). Untyped operands
/// are made concrete first, so arithmetic always yields a concrete type.
fn num_join(l: &Ty, r: &Ty) -> Ty {
    let l = default_type(l.clone());
    let r = default_type(r.clone());
    if matches!(l, Ty::Complex) || matches!(r, Ty::Complex) {
        Ty::Complex
    } else if matches!(l, Ty::Float64 | Ty::Float32) || matches!(r, Ty::Float64 | Ty::Float32) {
        Ty::Float64
    } else if matches!(l, Ty::Bigint) || matches!(r, Ty::Bigint) {
        Ty::Bigint
    } else {
        Ty::Int64
    }
}

/// A compile-time-constant non-negative integer index, if `e` is one (used for
/// tuple element access `t[k]`, D-0049). Only integer literals (with an optional
/// unary `+`) are constant folds here; a negative or non-literal index is not a
/// valid tuple index.
fn const_index(e: &Expr) -> Option<u64> {
    match e {
        Expr::Int(s, _) => {
            let clean: String = s.chars().filter(|c| *c != '_').collect();
            if let Some(h) = clean
                .strip_prefix("0x")
                .or_else(|| clean.strip_prefix("0X"))
            {
                u64::from_str_radix(h, 16).ok()
            } else if let Some(o) = clean
                .strip_prefix("0o")
                .or_else(|| clean.strip_prefix("0O"))
            {
                u64::from_str_radix(o, 8).ok()
            } else if let Some(b) = clean
                .strip_prefix("0b")
                .or_else(|| clean.strip_prefix("0B"))
            {
                u64::from_str_radix(b, 2).ok()
            } else {
                clean.parse::<u64>().ok()
            }
        }
        Expr::Unary {
            op: UnOp::Pos,
            expr,
            ..
        } => const_index(expr),
        _ => None,
    }
}

/// The inner target of a `*rest`, or the target itself otherwise. Used when the
/// value type is unknown and every leaf binds loosely.
fn rest_inner(t: &BindTarget) -> &BindTarget {
    match t {
        BindTarget::Rest(inner, _) => inner,
        other => other,
    }
}

/// The least type both `a` and `b` are assignable to (best-effort).
fn join(a: &Ty, b: &Ty) -> Ty {
    if assignable(a, b) {
        b.clone()
    } else if assignable(b, a) {
        a.clone()
    } else {
        Ty::Unknown
    }
}

#[derive(Clone)]
struct ParamInfo {
    name: String,
    ty: Ty,
    has_default: bool,
    variadic: bool,
}

#[derive(Clone)]
struct FnSig {
    params: Vec<ParamInfo>,
    ret: Ty,
}

/// A registered enum type (D-0018 tier-1 / D-0046 tier-3). `is_sum` is true when
/// any variant carries payload fields — then the whole enum is a tagged value
/// and the two tiers do not mix (D-0046).
#[derive(Clone)]
struct EnumInfo {
    is_sum: bool,
    variants: Vec<EnumVariantInfo>,
}

#[derive(Clone)]
struct EnumVariantInfo {
    name: String,
    /// Payload fields in declaration order (`(name, type)`); empty for a
    /// tier-1 constant or a payload-free variant.
    fields: Vec<(String, Ty)>,
}

impl EnumInfo {
    fn variant(&self, name: &str) -> Option<&EnumVariantInfo> {
        self.variants.iter().find(|v| v.name == name)
    }
}

/// Type-check a module, returning any errors.
pub fn check(module: &Module) -> Vec<TypeError> {
    let mut c = Checker::new();
    c.collect_types(module);
    c.collect_sigs(module);
    c.validate_decorators(module);
    c.check_module(module);
    c.errors
}

/// Convenience for tests: lex, parse, and type-check `src`.
pub fn check_str(
    src: &str,
) -> (
    Vec<TypeError>,
    Vec<zapc_syntax::parser::ParseError>,
    Vec<zapc_syntax::lexer::LexError>,
) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    let terrs = check(&module);
    (terrs, perrs, lerrs)
}

struct Checker {
    sigs: HashMap<String, FnSig>,
    user_types: HashSet<String>,
    /// Registered enums (tier-1 and tier-3), keyed by type name.
    enums: HashMap<String, EnumInfo>,
    /// Declared `annotation` names -> their field names (D-0036). Used to
    /// validate `@Annotation(...)` applications (keyword args must name fields).
    annotations: HashMap<String, Vec<String>>,
    /// Declared `decorator` names (behavioral, D-0036).
    user_decorators: HashSet<String>,
    scopes: Vec<HashMap<String, Ty>>,
    ret_ty: Ty,
    errors: Vec<TypeError>,
}

const BUILTINS: &[&str] = &[
    "print",
    "range",
    "len",
    "sum",
    "reflect_has",
    "reflect_names",
];

/// The curated built-in decorator set shipped in v1 (D-0036,
/// `design/0004-decorators.md` §"Built-in set"). An `@name` whose name is not in
/// this set, not a declared `annotation`, and not a declared `decorator` is a
/// compile-time error (consistent with compile-time typo-catching, D-0016).
const BUILTIN_DECORATORS: &[&str] = &[
    "derive",
    "cache",
    "default",
    "validate",
    "range",
    "deprecated",
    "inject",
];

/// The traits `@derive(...)` can generate (D-0036). Only `Eq` and `Repr` are
/// lowered to code in this increment; the rest are accepted here and reported as
/// deferred by the lowering pass.
const DERIVABLE_TRAITS: &[&str] = &["Eq", "Ord", "Json", "Repr", "Hash"];

/// Where a decorator/annotation is attached — used for target-suitability
/// checks (e.g. `@derive` only on a struct).
#[derive(Clone, Copy, PartialEq)]
enum DecoTarget {
    Struct,
    Field,
    Function,
    Enum,
    Param,
}

impl DecoTarget {
    fn describe(self) -> &'static str {
        match self {
            DecoTarget::Struct => "a struct",
            DecoTarget::Field => "a field",
            DecoTarget::Function => "a function",
            DecoTarget::Enum => "an enum",
            DecoTarget::Param => "a parameter",
        }
    }
}

impl Checker {
    fn new() -> Checker {
        Checker {
            sigs: HashMap::new(),
            user_types: HashSet::new(),
            enums: HashMap::new(),
            annotations: HashMap::new(),
            user_decorators: HashSet::new(),
            scopes: Vec::new(),
            ret_ty: Ty::Unit,
            errors: Vec::new(),
        }
    }

    fn error(&mut self, message: impl Into<String>, span: Span) {
        self.errors.push(TypeError {
            message: message.into(),
            span,
        });
    }

    // --- scopes ---

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    // --- collection passes ---

    fn collect_types(&mut self, module: &Module) {
        // First register every type name so field/payload types (which may
        // reference a type declared later, or the enum itself — a recursive
        // variant, D-0046) resolve in the second pass.
        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    self.user_types.insert(s.name.clone());
                }
                Item::Enum(e) => {
                    self.user_types.insert(e.name.clone());
                }
                Item::Interface(i) => {
                    self.user_types.insert(i.name.clone());
                }
                Item::Annotation(a) => {
                    self.user_types.insert(a.name.clone());
                    self.annotations.insert(
                        a.name.clone(),
                        a.fields.iter().map(|f| f.name.clone()).collect(),
                    );
                }
                Item::Decorator(d) => {
                    self.user_decorators.insert(d.name.clone());
                }
                _ => {}
            }
        }
        // Second pass: build the enum registry now that all type names exist.
        for item in &module.items {
            if let Item::Enum(e) = item {
                let is_sum = e.variants.iter().any(|v| !v.fields.is_empty());
                let variants = e
                    .variants
                    .iter()
                    .map(|v| EnumVariantInfo {
                        name: v.name.clone(),
                        fields: v
                            .fields
                            .iter()
                            .map(|f| (f.name.clone(), self.resolve_type(&f.ty)))
                            .collect(),
                    })
                    .collect();
                self.enums
                    .insert(e.name.clone(), EnumInfo { is_sum, variants });
            }
        }
    }

    fn collect_sigs(&mut self, module: &Module) {
        for item in &module.items {
            if let Item::Function(f) = item {
                let sig = self.sig_of(f);
                self.sigs.insert(f.name.clone(), sig);
            }
        }
    }

    fn sig_of(&self, f: &Function) -> FnSig {
        let params = f
            .params
            .iter()
            .map(|p| ParamInfo {
                name: p.name.clone(),
                ty: p
                    .ty
                    .as_ref()
                    .map(|t| self.resolve_type(t))
                    .unwrap_or(Ty::Unknown),
                has_default: p.default.is_some(),
                variadic: p.variadic,
            })
            .collect();
        let ret = f
            .ret
            .as_ref()
            .map(|t| self.resolve_type(t))
            .unwrap_or(Ty::Unit);
        FnSig { params, ret }
    }

    fn resolve_type(&self, t: &Type) -> Ty {
        match t {
            Type::Named { name, args, .. } => match name.as_str() {
                "int64" | "i64" => Ty::Int64,
                "int32" | "i32" => Ty::Int32,
                "float64" | "f64" => Ty::Float64,
                "float32" | "f32" => Ty::Float32,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "bytes" => Ty::Bytes,
                "char" => Ty::Char,
                "bigint" => Ty::Bigint,
                "complex" => Ty::Complex,
                "Match" => Ty::Match,
                "pyobject" => Ty::Py,
                "list" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::List(Box::new(el))
                }
                "array" => {
                    // Numeric arrays (ticket #24) are typed loosely for now:
                    // element/dtype typing stays `Unknown` so array programs
                    // type-check without false errors (modeling them as lists
                    // would wrongly reject `array * array`). The ZIR lowering
                    // carries the precise dtype.
                    for a in args {
                        let _ = self.resolve_type(a);
                    }
                    Ty::Unknown
                }
                "dict" => {
                    let k = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    let v = args
                        .get(1)
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::Dict(Box::new(k), Box::new(v))
                }
                "set" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::Set(Box::new(el))
                }
                "channel" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::Chan(Box::new(el))
                }
                other => {
                    if self.user_types.contains(other) {
                        Ty::Named(other.to_string())
                    } else {
                        Ty::Unknown
                    }
                }
            },
            Type::Tuple { elems, .. } => {
                Ty::Tuple(elems.iter().map(|e| self.resolve_type(e)).collect())
            }
            Type::Func { .. } => Ty::Unknown,
        }
    }

    /// The channel element type named by a `channel[T]` subscript, where `T`
    /// appears as a bare name in expression position (ticket #25, D-0031).
    fn chan_elem_from_expr(&self, e: &Expr) -> Ty {
        match e {
            Expr::Name(n, _) => match n.as_str() {
                "int64" | "i64" => Ty::Int64,
                "int32" | "i32" => Ty::Int32,
                "float64" | "f64" => Ty::Float64,
                "float32" | "f32" => Ty::Float32,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "bytes" => Ty::Bytes,
                "char" => Ty::Char,
                "bigint" => Ty::Bigint,
                "complex" => Ty::Complex,
                other if self.user_types.contains(other) => Ty::Named(other.to_string()),
                _ => Ty::Unknown,
            },
            _ => Ty::Unknown,
        }
    }

    // --- decorator / annotation validation (D-0036) ---
    //
    // Every `@name` application is validated here: the name must be a built-in
    // decorator, a declared `annotation`, or a declared `decorator`; an unknown
    // name is a compile error (compile-time typo-catching, D-0016). The
    // built-ins additionally get argument-shape and target-suitability checks.
    // The *behavior* of the built-ins (code generation, reflection metadata) is
    // produced later, by the AST->ZIR lowering pass.

    fn validate_decorators(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    for d in &s.decorators {
                        self.check_decorator(d, DecoTarget::Struct);
                    }
                    for f in &s.fields {
                        for d in &f.decorators {
                            self.check_decorator(d, DecoTarget::Field);
                        }
                    }
                    for m in &s.methods {
                        self.check_fn_decorators(m);
                    }
                }
                Item::Function(f) => self.check_fn_decorators(f),
                Item::Enum(e) => {
                    for d in &e.decorators {
                        self.check_decorator(d, DecoTarget::Enum);
                    }
                }
                _ => {}
            }
        }
    }

    fn check_fn_decorators(&mut self, f: &Function) {
        for d in &f.decorators {
            self.check_decorator(d, DecoTarget::Function);
        }
        for p in &f.params {
            for d in &p.decorators {
                self.check_decorator(d, DecoTarget::Param);
            }
        }
    }

    fn check_decorator(&mut self, d: &Decorator, target: DecoTarget) {
        if BUILTIN_DECORATORS.contains(&d.name.as_str()) {
            self.check_builtin_decorator(d, target);
            return;
        }
        if let Some(fields) = self.annotations.get(&d.name).cloned() {
            // An annotation application: every keyword arg must name a field
            // (typo-catching). Positional args are matched by declaration order.
            for a in &d.args {
                if let Some(argname) = &a.name {
                    if !fields.contains(argname) {
                        self.error(
                            format!("annotation '@{}' has no field '{argname}'", d.name),
                            d.span,
                        );
                    }
                }
            }
            if d.args.len() > fields.len() {
                self.error(
                    format!(
                        "annotation '@{}' takes at most {} argument(s), found {}",
                        d.name,
                        fields.len(),
                        d.args.len()
                    ),
                    d.span,
                );
            }
            return;
        }
        if self.user_decorators.contains(&d.name) {
            // User-defined behavioral decorator — accepted; its arguments are
            // checked loosely for now (the user-macro API is deferred,
            // OPEN-QUESTIONS.md).
            return;
        }
        self.error(
            format!(
                "unknown decorator or annotation '@{}' (not a built-in, \
                 `annotation`, or `decorator`)",
                d.name
            ),
            d.span,
        );
    }

    fn check_builtin_decorator(&mut self, d: &Decorator, target: DecoTarget) {
        let keywords = d.args.iter().filter(|a| a.name.is_some()).count();
        let positionals = d.args.len() - keywords;
        match d.name.as_str() {
            "derive" => {
                self.require_target(d, target, DecoTarget::Struct);
                if d.args.is_empty() {
                    self.error(
                        "@derive(...) needs at least one trait, e.g. @derive(Eq, Repr)",
                        d.span,
                    );
                }
                for a in &d.args {
                    if a.name.is_some() {
                        self.error("@derive takes positional trait names, not keywords", d.span);
                    } else if let Expr::Name(t, _) = &a.value {
                        if !DERIVABLE_TRAITS.contains(&t.as_str()) {
                            self.error(
                                format!(
                                    "unknown derivable trait '{t}' (expected one of \
                                     Eq, Ord, Json, Repr, Hash)"
                                ),
                                d.span,
                            );
                        }
                    } else {
                        self.error("@derive expects trait names, e.g. @derive(Eq)", d.span);
                    }
                }
            }
            "default" => {
                self.require_target(d, target, DecoTarget::Field);
                if positionals != 1 || keywords != 0 {
                    self.error(
                        "@default(expr) takes exactly one expression, e.g. @default(now())",
                        d.span,
                    );
                }
            }
            "range" => {
                self.require_target(d, target, DecoTarget::Field);
                if positionals != 0 {
                    self.error(
                        "@range(...) takes keyword bounds: @range(min=0, max=150)",
                        d.span,
                    );
                }
                for a in &d.args {
                    if let Some(n) = &a.name {
                        if n != "min" && n != "max" {
                            self.error(
                                format!("@range has no bound '{n}' (expected 'min' or 'max')"),
                                d.span,
                            );
                        }
                    }
                }
                if d.args.is_empty() {
                    self.error("@range(...) needs at least one of min/max", d.span);
                }
            }
            "validate" => {
                self.require_target(d, target, DecoTarget::Field);
                if positionals != 0 {
                    self.error(
                        "@validate(...) takes keyword constraints, e.g. @validate(max_len=50)",
                        d.span,
                    );
                }
            }
            "cache" => {
                self.require_target(d, target, DecoTarget::Function);
                if !d.args.is_empty() {
                    self.error("@cache takes no arguments", d.span);
                }
            }
            "inject" => {
                self.require_target(d, target, DecoTarget::Field);
                if !d.args.is_empty() {
                    self.error("@inject takes no arguments", d.span);
                }
            }
            "deprecated" => {
                // Allowed on any target; optional single positional message.
                let well_formed = d.args.len() <= 1 && keywords == 0;
                if !well_formed {
                    self.error(
                        "@deprecated takes an optional message string, e.g. @deprecated(\"use X\")",
                        d.span,
                    );
                }
            }
            _ => {}
        }
    }

    fn require_target(&mut self, d: &Decorator, got: DecoTarget, want: DecoTarget) {
        if got != want {
            self.error(
                format!(
                    "@{} applies to {}, not {}",
                    d.name,
                    want.describe(),
                    got.describe()
                ),
                d.span,
            );
        }
    }

    fn check_reflect_args(&mut self, fname: &str, arity: usize, args: &[Arg], span: Span) {
        if args.len() != arity {
            self.error(
                format!(
                    "{fname}() takes exactly {arity} argument(s), found {}",
                    args.len()
                ),
                span,
            );
        }
        for a in args {
            let t = self.infer(&a.value);
            if !matches!(t, Ty::Str | Ty::Unknown) {
                self.error(
                    format!("{fname}() expects str arguments, found {}", t.display()),
                    span,
                );
            }
        }
    }

    // --- checking ---

    fn check_module(&mut self, module: &Module) {
        self.push_scope();
        // Python imports (D-0012): bind each imported module's name to a Python
        // handle type in module scope so references like `np.array(...)` resolve
        // (and don't trip the "unknown name" check). Declared up front so a
        // function defined before the `import` still sees the alias.
        for item in &module.items {
            if let Item::Import(Import::Python {
                module: m, alias, ..
            }) = item
            {
                let name = alias.clone().unwrap_or_else(|| m.clone());
                self.declare(&name, Ty::Py);
            }
        }
        for item in &module.items {
            match item {
                Item::Function(f) => self.check_function(f, None),
                Item::Struct(s) => {
                    for m in &s.methods {
                        self.check_function(m, Some(&s.name));
                    }
                }
                Item::Enum(e) => {
                    // Methods on enums are locked (D-0046) but their lowering is
                    // a documented follow-up; reject rather than silently drop.
                    if let Some(m) = e.methods.first() {
                        self.error(
                            "methods on enums are not implemented yet (deferred to a \
                             follow-up; D-0046)"
                                .to_string(),
                            m.span,
                        );
                    }
                }
                Item::Stmt(st) => self.check_stmt(st),
                _ => {}
            }
        }
        self.pop_scope();
    }

    fn check_function(&mut self, f: &Function, self_ty: Option<&str>) {
        self.push_scope();
        let new_ret = f
            .ret
            .as_ref()
            .map(|t| self.resolve_type(t))
            .unwrap_or(Ty::Unit);
        let saved = std::mem::replace(&mut self.ret_ty, new_ret);
        if let Some(name) = self_ty {
            self.declare("self", Ty::Named(name.to_string()));
        }
        for p in &f.params {
            if p.name == "self" {
                self.declare(
                    "self",
                    self_ty
                        .map(|n| Ty::Named(n.to_string()))
                        .unwrap_or(Ty::Unknown),
                );
                continue;
            }
            let base =
                p.ty.as_ref()
                    .map(|t| self.resolve_type(t))
                    .unwrap_or(Ty::Unknown);
            let ty = if p.variadic {
                Ty::List(Box::new(base))
            } else {
                base
            };
            self.declare(&p.name, ty);
        }
        for st in &f.body.stmts {
            self.check_stmt(st);
        }
        self.ret_ty = saved;
        self.pop_scope();
    }

    fn check_block(&mut self, b: &Block) {
        self.push_scope();
        for st in &b.stmts {
            self.check_stmt(st);
        }
        self.pop_scope();
    }

    fn check_stmt(&mut self, st: &Stmt) {
        match st {
            Stmt::Let {
                ty, name, value, ..
            } => {
                let vt = self.infer(value);
                match ty {
                    None => self.declare(name, default_type(vt)),
                    Some(t) => {
                        let dt = self.resolve_type(t);
                        if !assignable(&vt, &dt) {
                            self.error(
                                format!(
                                    "cannot assign value of type {} to {} {}",
                                    vt.display(),
                                    dt.display(),
                                    name
                                ),
                                expr_span(value),
                            );
                        }
                        self.declare(name, dt);
                    }
                }
            }
            Stmt::Assign { target, value, .. } => {
                let vt = self.infer(value);
                if let Expr::Name(n, span) = target {
                    match self.lookup(n) {
                        Some(dt) => {
                            if !assignable(&vt, &dt) {
                                self.error(
                                    format!(
                                        "cannot assign value of type {} to {} of type {}",
                                        vt.display(),
                                        n,
                                        dt.display()
                                    ),
                                    expr_span(value),
                                );
                            }
                        }
                        None => {
                            self.error(format!("assignment to undeclared variable '{n}'"), *span)
                        }
                    }
                } else {
                    self.infer(target);
                }
            }
            Stmt::AugAssign {
                target, op, value, ..
            } => {
                let vt = self.infer(value);
                if let Expr::Name(n, span) = target {
                    match self.lookup(n) {
                        Some(tt) => {
                            let r = self.arith_result(*op, &tt, &vt, *span);
                            if !assignable(&r, &tt) {
                                self.error(
                                    format!(
                                        "augmented assignment produces {} but '{}' has type {}",
                                        r.display(),
                                        n,
                                        tt.display()
                                    ),
                                    *span,
                                );
                            }
                        }
                        None => {
                            self.error(format!("assignment to undeclared variable '{n}'"), *span)
                        }
                    }
                } else {
                    self.infer(target);
                }
            }
            Stmt::Expr(e) => {
                self.infer(e);
            }
            Stmt::If {
                branches,
                else_block,
                ..
            } => {
                for (cond, body) in branches {
                    self.infer(cond);
                    self.check_block(body);
                }
                if let Some(b) = else_block {
                    self.check_block(b);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.infer(cond);
                self.check_block(body);
            }
            Stmt::DoWhile { body, cond, .. } => {
                self.check_block(body);
                self.infer(cond);
            }
            Stmt::For {
                target, iter, body, ..
            } => {
                // Iterating an enum *type* yields its members in declaration
                // order (`for col in Color:`, D-0018). The iterable is a type
                // name, not a value, so it is matched before inferring.
                let elem = if let Expr::Name(n, _) = iter {
                    if self.enums.contains_key(n) {
                        Ty::Named(n.clone())
                    } else {
                        match self.infer(iter) {
                            Ty::Range => Ty::Int64,
                            Ty::List(t) => *t,
                            Ty::Str => Ty::Char,
                            _ => Ty::Unknown,
                        }
                    }
                } else {
                    match self.infer(iter) {
                        Ty::Range => Ty::Int64,
                        Ty::List(t) => *t,
                        Ty::Str => Ty::Char,
                        _ => Ty::Unknown,
                    }
                };
                self.push_scope();
                match target {
                    Expr::Name(n, _) => self.declare(n, elem),
                    Expr::Tuple(elems, _) => {
                        for e in elems {
                            if let Expr::Name(n, _) = e {
                                self.declare(n, Ty::Unknown);
                            }
                        }
                    }
                    _ => {}
                }
                for st in &body.stmts {
                    self.check_stmt(st);
                }
                self.pop_scope();
            }
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrut = self.infer(scrutinee);
                let mut covered: HashSet<String> = HashSet::new();
                let mut has_catch_all = false;
                let mut guarded_catch_all = false;
                for arm in arms {
                    self.push_scope();
                    let (vs, catch_all) = self.check_pattern(&arm.pattern, &scrut);
                    for v in vs {
                        covered.insert(v);
                    }
                    if catch_all {
                        // A guarded catch-all does not make the match exhaustive
                        // (the guard may fail).
                        if arm.guard.is_some() {
                            guarded_catch_all = true;
                        } else {
                            has_catch_all = true;
                        }
                    }
                    if let Some(g) = &arm.guard {
                        self.infer(g);
                    }
                    for st in &arm.body.stmts {
                        self.check_stmt(st);
                    }
                    self.pop_scope();
                }
                // Exhaustiveness (D-0040): a `match` on an enum that omits a
                // variant and has no `_`/catch-all arm is a compile error. This
                // one upgrade over Python applies only to enum scrutinees;
                // non-enum matches stay Python-like (no-op on no match).
                let _ = guarded_catch_all;
                if let Ty::Named(ename) = &scrut {
                    if let Some(info) = self.enums.get(ename).cloned() {
                        if !has_catch_all {
                            let missing: Vec<String> = info
                                .variants
                                .iter()
                                .filter(|v| !covered.contains(&v.name))
                                .map(|v| format!("{ename}.{}", v.name))
                                .collect();
                            if !missing.is_empty() {
                                self.error(
                                    format!(
                                        "non-exhaustive match on enum '{ename}': missing {} \
                                         (add the arm(s) or a '_' case)",
                                        missing.join(", ")
                                    ),
                                    *span,
                                );
                            }
                        }
                    }
                }
            }
            Stmt::Return { value, span } => match value {
                Some(e) => {
                    let vt = self.infer(e);
                    let ret = self.ret_ty.clone();
                    if !assignable(&vt, &ret) {
                        self.error(
                            format!(
                                "return type mismatch: expected {}, found {}",
                                ret.display(),
                                vt.display()
                            ),
                            expr_span(e),
                        );
                    }
                }
                None => {
                    if !matches!(self.ret_ty, Ty::Unit | Ty::Unknown) {
                        self.error(
                            format!(
                                "function must return a value of type {}",
                                self.ret_ty.display()
                            ),
                            *span,
                        );
                    }
                }
            },
            Stmt::Raise { value, .. } => {
                if let Some(e) = value {
                    self.infer(e);
                }
            }
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => {
                self.check_block(body);
                for ex in excepts {
                    self.push_scope();
                    if let Some(b) = &ex.binding {
                        self.declare(b, Ty::Unknown);
                    }
                    for st in &ex.body.stmts {
                        self.check_stmt(st);
                    }
                    self.pop_scope();
                }
                if let Some(b) = finally {
                    self.check_block(b);
                }
            }
            Stmt::With { items, body, .. } => {
                self.push_scope();
                for it in items {
                    self.infer(&it.expr);
                    if let Some(a) = &it.alias {
                        self.declare(a, Ty::Unknown);
                    }
                }
                for st in &body.stmts {
                    self.check_stmt(st);
                }
                self.pop_scope();
            }
            Stmt::Spawn { call, .. } => {
                self.infer(call);
            }
            Stmt::LetTuple {
                ty,
                target,
                value,
                span,
                is_const: _,
            } => {
                let vt = self.infer(value);
                let declared = match ty {
                    Some(t) => {
                        let dt = self.resolve_type(t);
                        if !assignable(&vt, &dt) {
                            self.error(
                                format!(
                                    "cannot assign value of type {} to {}",
                                    vt.display(),
                                    dt.display()
                                ),
                                expr_span(value),
                            );
                        }
                        dt
                    }
                    None => default_type(vt),
                };
                self.bind_target(target, &declared, true, *span);
            }
            Stmt::AssignTuple {
                target,
                value,
                span,
            } => {
                let vt = default_type(self.infer(value));
                self.bind_target(target, &vt, false, *span);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    /// Type-check a destructuring bind target against the value type `ty`.
    /// `is_decl` distinguishes a declaration (leaves introduce new names) from a
    /// reassignment (leaves are existing lvalues). D-0049.
    fn bind_target(&mut self, t: &BindTarget, ty: &Ty, is_decl: bool, _span: Span) {
        match t {
            BindTarget::Elem(Expr::Name(n, nspan)) => {
                if is_decl {
                    self.declare(n, ty.clone());
                } else {
                    match self.lookup(n) {
                        Some(dt) => {
                            if !assignable(ty, &dt) {
                                self.error(
                                    format!(
                                        "cannot assign value of type {} to {} of type {}",
                                        ty.display(),
                                        n,
                                        dt.display()
                                    ),
                                    *nspan,
                                );
                            }
                        }
                        None => {
                            self.error(format!("assignment to undeclared variable '{n}'"), *nspan)
                        }
                    }
                }
            }
            BindTarget::Elem(e) => {
                if is_decl {
                    self.error(
                        "a declaration target must be a name".to_string(),
                        expr_span(e),
                    );
                } else {
                    // An lvalue such as `a[i]` or `a.f`: type-check it as an
                    // ordinary expression (its mutability is enforced elsewhere).
                    self.infer(e);
                }
            }
            BindTarget::Rest(_, rspan) => {
                self.error(
                    "'*rest' is only valid as an element of a tuple target".to_string(),
                    *rspan,
                );
            }
            BindTarget::Tuple(elems, tspan) => match ty {
                Ty::Tuple(ts) => self.bind_tuple(elems, ts, is_decl, *tspan),
                Ty::Unknown => {
                    // The value type is unknown (e.g. a Python handle): bind every
                    // leaf loosely rather than cascading errors.
                    for e in elems {
                        self.bind_target(rest_inner(e), &Ty::Unknown, is_decl, *tspan);
                    }
                }
                other => self.error(
                    format!("cannot destructure a value of type {}", other.display()),
                    *tspan,
                ),
            },
        }
    }

    /// Bind a tuple pattern's element targets against the tuple's element types,
    /// honoring at most one `*rest` which captures the remaining elements as a
    /// sub-tuple (D-0049).
    fn bind_tuple(&mut self, elems: &[BindTarget], ts: &[Ty], is_decl: bool, span: Span) {
        let rest_positions: Vec<usize> = elems
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, BindTarget::Rest(..)))
            .map(|(i, _)| i)
            .collect();
        if rest_positions.len() > 1 {
            self.error(
                "a tuple target may have at most one '*rest' element".to_string(),
                span,
            );
            return;
        }
        match rest_positions.first() {
            None => {
                if elems.len() != ts.len() {
                    self.error(
                        format!(
                            "tuple destructuring expects {} element(s), found {}",
                            ts.len(),
                            elems.len()
                        ),
                        span,
                    );
                    return;
                }
                for (e, t) in elems.iter().zip(ts) {
                    self.bind_target(e, t, is_decl, span);
                }
            }
            Some(&r) => {
                // `elems.len() - 1` fixed targets (before and after the rest)
                // must fit within the tuple's arity.
                let fixed = elems.len() - 1;
                if ts.len() < fixed {
                    self.error(
                        format!(
                            "tuple destructuring with '*rest' needs at least {fixed} \
                             element(s), found {}",
                            ts.len()
                        ),
                        span,
                    );
                    return;
                }
                let after = elems.len() - r - 1;
                let tail_start = ts.len() - after;
                for (e, t) in elems[..r].iter().zip(&ts[..r]) {
                    self.bind_target(e, t, is_decl, span);
                }
                let rest_ty = Ty::Tuple(ts[r..tail_start].to_vec());
                if let BindTarget::Rest(inner, _) = &elems[r] {
                    self.bind_target(inner, &rest_ty, is_decl, span);
                }
                for (e, t) in elems[r + 1..].iter().zip(&ts[tail_start..]) {
                    self.bind_target(e, t, is_decl, span);
                }
            }
        }
    }

    // --- expression inference ---

    fn infer(&mut self, e: &Expr) -> Ty {
        match e {
            Expr::Int(..) => Ty::UntypedInt,
            Expr::Float(..) => Ty::UntypedFloat,
            Expr::Imaginary(..) => Ty::Complex,
            Expr::Str { bytes, .. } => {
                if *bytes {
                    Ty::Bytes
                } else {
                    Ty::Str
                }
            }
            Expr::Bool(..) => Ty::Bool,
            Expr::None(_) => Ty::Unknown,
            Expr::Name(n, span) => {
                if let Some(t) = self.lookup(n) {
                    t
                } else if self.sigs.contains_key(n) || BUILTINS.contains(&n.as_str()) {
                    Ty::Unknown // a function used as a value
                } else {
                    self.error(format!("unknown name '{n}'"), *span);
                    Ty::Unknown
                }
            }
            Expr::Tuple(elems, _) => {
                // A tuple literal's type is the (defaulted) element types in
                // positional order (D-0049); `()` is the empty tuple.
                Ty::Tuple(
                    elems
                        .iter()
                        .map(|el| default_type(self.infer(el)))
                        .collect(),
                )
            }
            Expr::List(elems, _) => {
                let mut elem = Ty::Unknown;
                for (i, el) in elems.iter().enumerate() {
                    let t = self.infer(el);
                    elem = if i == 0 { t } else { join(&elem, &t) };
                }
                Ty::List(Box::new(elem))
            }
            Expr::Dict(items, _) => {
                let mut kt = Ty::Unknown;
                let mut vt = Ty::Unknown;
                for (i, (k, v)) in items.iter().enumerate() {
                    let k2 = self.infer(k);
                    let v2 = self.infer(v);
                    if i == 0 {
                        kt = k2;
                        vt = v2;
                    } else {
                        kt = join(&kt, &k2);
                        vt = join(&vt, &v2);
                    }
                }
                Ty::Dict(Box::new(kt), Box::new(vt))
            }
            Expr::ListComp {
                elem,
                var,
                iter,
                cond,
                ..
            } => {
                let it = self.infer(iter);
                let e_ty = iter_elem(&it);
                self.push_scope();
                self.declare(var, e_ty);
                if let Some(c) = cond {
                    self.infer(c);
                }
                let et = self.infer(elem);
                self.pop_scope();
                Ty::List(Box::new(et))
            }
            Expr::DictComp {
                key,
                value,
                var,
                iter,
                cond,
                ..
            } => {
                let it = self.infer(iter);
                let e_ty = iter_elem(&it);
                self.push_scope();
                self.declare(var, e_ty);
                if let Some(c) = cond {
                    self.infer(c);
                }
                let kt = self.infer(key);
                let vt = self.infer(value);
                self.pop_scope();
                Ty::Dict(Box::new(kt), Box::new(vt))
            }
            Expr::Set(elems, _) => {
                let mut elem = Ty::Unknown;
                for (i, el) in elems.iter().enumerate() {
                    let t = self.infer(el);
                    elem = if i == 0 { t } else { join(&elem, &t) };
                }
                Ty::Set(Box::new(elem))
            }
            Expr::SetComp {
                elem,
                var,
                iter,
                cond,
                ..
            } => {
                let it = self.infer(iter);
                let e_ty = iter_elem(&it);
                self.push_scope();
                self.declare(var, e_ty);
                if let Some(c) = cond {
                    self.infer(c);
                }
                let et = self.infer(elem);
                self.pop_scope();
                Ty::Set(Box::new(et))
            }
            Expr::Unary { op, expr, span } => {
                let t = self.infer(expr);
                match op {
                    UnOp::Not => Ty::Bool,
                    UnOp::Neg | UnOp::Pos => {
                        if t.is_numeric() || matches!(t, Ty::Unknown) {
                            t
                        } else {
                            self.error(
                                format!("unary operator needs a number, found {}", t.display()),
                                *span,
                            );
                            Ty::Unknown
                        }
                    }
                    UnOp::BitNot => {
                        if t.is_int() || matches!(t, Ty::Unknown) {
                            t
                        } else {
                            self.error(
                                format!("'~' needs an integer, found {}", t.display()),
                                *span,
                            );
                            Ty::Unknown
                        }
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, span } => {
                let l = self.infer(lhs);
                let r = self.infer(rhs);
                self.arith_result(*op, &l, &r, *span)
            }
            Expr::Call { callee, args, span } => self.check_call(callee, args, *span),
            Expr::Index { base, index, span } => {
                let b = self.infer(base);
                // Tuple element access `t[k]` requires a compile-time-constant
                // `k` and yields that position's static type (D-0049) — the key
                // difference from `list[i]`, which takes a runtime index.
                if let Ty::Tuple(elems) = &b {
                    match const_index(index) {
                        Some(k) if (k as usize) < elems.len() => {
                            return elems[k as usize].clone();
                        }
                        Some(k) => {
                            self.error(
                                format!(
                                    "tuple index {k} out of range for a {}-element tuple",
                                    elems.len()
                                ),
                                *span,
                            );
                            return Ty::Unknown;
                        }
                        None => {
                            self.infer(index);
                            self.error(
                                "a tuple index must be a compile-time-constant \
                                 integer (a tuple is heterogeneous, so the element \
                                 type must be known statically)"
                                    .to_string(),
                                *span,
                            );
                            return Ty::Unknown;
                        }
                    }
                }
                self.infer(index);
                match b {
                    Ty::List(t) => *t,
                    Ty::Dict(_, v) => *v,
                    Ty::Str => Ty::Char,
                    Ty::Range => Ty::Int64,
                    _ => Ty::Unknown,
                }
            }
            Expr::Slice {
                base, lo, hi, step, ..
            } => {
                let b = self.infer(base);
                for part in [lo, hi, step].into_iter().flatten() {
                    self.infer(part);
                }
                match b {
                    Ty::List(_) => b,
                    Ty::Str => Ty::Str,
                    _ => Ty::Unknown,
                }
            }
            Expr::Attr { base, name, span } => {
                // Qualified enum member/variant access: `Color.RED`,
                // `Shape.Unit` (D-0018/D-0046). The base is a *type* name, not a
                // value, so it is matched before inferring the base as a value.
                if let Expr::Name(tyname, _) = base.as_ref() {
                    if let Some(info) = self.enums.get(tyname).cloned() {
                        match info.variant(name) {
                            Some(v) if v.fields.is_empty() => {
                                return Ty::Named(tyname.clone());
                            }
                            Some(_) => {
                                self.error(
                                    format!(
                                        "variant '{name}' of enum '{tyname}' carries data; \
                                         construct it with '{tyname}.{name}(...)'"
                                    ),
                                    *span,
                                );
                                return Ty::Named(tyname.clone());
                            }
                            None => {
                                self.error(
                                    format!("enum '{tyname}' has no variant '{name}'"),
                                    *span,
                                );
                                return Ty::Unknown;
                            }
                        }
                    }
                }
                let b = self.infer(base);
                // `.name` on an enum value yields its member name (D-0018).
                if matches!(&b, Ty::Named(n) if self.enums.contains_key(n)) && name == "name" {
                    return Ty::Str;
                }
                // Attribute access on a Python handle yields another Python
                // handle (D-0012); everything else is modeled loosely for now.
                if matches!(b, Ty::Py) {
                    Ty::Py
                } else {
                    Ty::Unknown
                }
            }
            Expr::Lambda { .. } => Ty::Unknown,
            Expr::RegexMatch {
                subject,
                negated,
                span,
                ..
            } => {
                let t = self.infer(subject);
                if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
                    self.error(
                        format!(
                            "the left operand of '=~'/'!~' must be a str, found {}",
                            t.display()
                        ),
                        *span,
                    );
                }
                // `!~` is a boolean test; `=~ m/.../` yields a match object
                // (truthy on success, `None` on no match — D-0010).
                if *negated {
                    Ty::Bool
                } else {
                    Ty::Match
                }
            }
            Expr::RegexSubst { subject, span, .. } => {
                let t = self.infer(subject);
                if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
                    self.error(
                        format!(
                            "the left operand of a substitution 's/.../.../' must be a str, found {}",
                            t.display()
                        ),
                        *span,
                    );
                }
                // `s///` returns a new string; the subject is never mutated.
                Ty::Str
            }
            Expr::ChanRecv { chan, span } => {
                let ct = self.infer(chan);
                match ct {
                    Ty::Chan(elem) => *elem,
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.error(
                            format!(
                                "the operand of '<-' (receive) must be a channel, found {}",
                                other.display()
                            ),
                            *span,
                        );
                        Ty::Unknown
                    }
                }
            }
            Expr::Yield { value, .. } => {
                if let Some(v) = value {
                    self.infer(v);
                }
                Ty::Unknown
            }
            Expr::IfElse {
                cond, then, els, ..
            } => {
                self.infer(cond);
                let t = self.infer(then);
                let e = self.infer(els);
                join(&t, &e)
            }
        }
    }

    fn arith_result(&mut self, op: BinOp, l: &Ty, r: &Ty, span: Span) -> Ty {
        use BinOp::*;
        let numeric = (l.is_numeric() || matches!(l, Ty::Unknown))
            && (r.is_numeric() || matches!(r, Ty::Unknown));
        match op {
            Add => {
                if l.is_numeric() && r.is_numeric() {
                    num_join(l, r)
                } else if matches!(l, Ty::Unknown) || matches!(r, Ty::Unknown) {
                    Ty::Unknown
                } else if matches!(l, Ty::Str) && matches!(r, Ty::Str) {
                    Ty::Str
                } else if matches!(l, Ty::List(_)) && matches!(r, Ty::List(_)) {
                    l.clone()
                } else {
                    self.error(
                        format!("cannot add {} and {}", l.display(), r.display()),
                        span,
                    );
                    Ty::Unknown
                }
            }
            Sub | Mul | FloorDiv | Mod | Pow => {
                if numeric {
                    num_join(l, r)
                } else {
                    self.error(
                        format!(
                            "arithmetic needs numbers, found {} and {}",
                            l.display(),
                            r.display()
                        ),
                        span,
                    );
                    Ty::Unknown
                }
            }
            Div => {
                if numeric {
                    Ty::Float64
                } else {
                    self.error(
                        format!(
                            "division needs numbers, found {} and {}",
                            l.display(),
                            r.display()
                        ),
                        span,
                    );
                    Ty::Unknown
                }
            }
            BitAnd | BitOr | BitXor | Shl | Shr => {
                if (l.is_int() || matches!(l, Ty::Unknown))
                    && (r.is_int() || matches!(r, Ty::Unknown))
                {
                    Ty::Int64
                } else {
                    self.error(
                        format!(
                            "bitwise operator needs integers, found {} and {}",
                            l.display(),
                            r.display()
                        ),
                        span,
                    );
                    Ty::Unknown
                }
            }
            Eq | Ne | Lt | Le | Gt | Ge | In => Ty::Bool,
            And | Or => Ty::Bool,
            // Channel send `ch <- v` (ticket #25, D-0031): the left operand must
            // be a channel and the value must be assignable to its element type.
            Send => {
                match l {
                    Ty::Chan(elem) => {
                        if !assignable(r, elem) {
                            self.error(
                                format!(
                                    "cannot send a value of type {} on a channel[{}]",
                                    r.display(),
                                    elem.display()
                                ),
                                span,
                            );
                        }
                    }
                    Ty::Unknown => {}
                    other => self.error(
                        format!(
                            "the left operand of '<-' (send) must be a channel, found {}",
                            other.display()
                        ),
                        span,
                    ),
                }
                Ty::Unit
            }
        }
    }

    fn check_call(&mut self, callee: &Expr, args: &[Arg], span: Span) -> Ty {
        // Qualified enum-variant construction: `Shape.Circle(r=2.0)` /
        // `Shape.Circle(2.0)` (D-0046), accepting named or positional args.
        if let Expr::Attr { base, name, .. } = callee {
            if let Expr::Name(tyname, _) = base.as_ref() {
                if let Some(info) = self.enums.get(tyname).cloned() {
                    return self.check_variant_construction(tyname, name, &info, args, span);
                }
            }
        }
        // Array construction `array[dtype]([...])` (ticket #24): the callee is a
        // subscript whose base (`array`) and index (the dtype name) are not
        // ordinary value names, so they are not inferred as such. Typed loosely.
        if let Expr::Index { base, index, .. } = callee {
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "array") {
                for a in args {
                    self.infer(&a.value);
                }
                return Ty::Unknown;
            }
            // Channel construction `channel[T]()` / `channel[T](cap)` (ticket #25,
            // D-0031). The element type `T` appears as the subscript; an optional
            // single argument is the buffer capacity (an integer).
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "channel") {
                let elem = self.chan_elem_from_expr(index);
                if args.len() > 1 {
                    self.error(
                        "channel[T](...) takes at most one argument (the buffer capacity)"
                            .to_string(),
                        span,
                    );
                }
                for a in args {
                    let at = self.infer(&a.value);
                    if !at.is_int() && !matches!(at, Ty::Unknown) {
                        self.error(
                            format!(
                                "channel capacity must be an integer, found {}",
                                at.display()
                            ),
                            expr_span(&a.value),
                        );
                    }
                }
                return Ty::Chan(Box::new(elem));
            }
        }
        if let Expr::Name(fname, _) = callee {
            match fname.as_str() {
                "sum" => {
                    // Reduction over an array -> scalar; typed loosely for now.
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
                "print" => {
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unit;
                }
                "range" => {
                    for a in args {
                        let t = self.infer(&a.value);
                        if !t.is_int() && !matches!(t, Ty::Unknown) {
                            self.error(
                                format!("range expects integers, found {}", t.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Range;
                }
                "len" => {
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Int64;
                }
                // `close(ch)` (ticket #25, D-0031): the argument must be a channel.
                "close" => {
                    if args.len() != 1 {
                        self.error("close() takes exactly one argument".to_string(), span);
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !matches!(at, Ty::Chan(_) | Ty::Unknown) {
                            self.error(
                                format!("close() expects a channel, found {}", at.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Unit;
                }
                "int32" | "int64" | "float32" | "float64" | "i32" | "i64" | "f32" | "f64" => {
                    // Explicit numeric conversion (D-0020): the way to narrow or
                    // cross the int/float boundary that implicit widening refuses.
                    let to = match fname.as_str() {
                        "int32" | "i32" => Ty::Int32,
                        "int64" | "i64" => Ty::Int64,
                        "float32" | "f32" => Ty::Float32,
                        _ => Ty::Float64,
                    };
                    if args.len() != 1 {
                        self.error(format!("{fname}() takes exactly one argument"), span);
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        // A Python handle is accepted: the conversion marshals it
                        // back to a Zap number (D-0012). A tier-1 enum converts to
                        // its backing int (`int64(c)`, D-0018 explicit narrowing).
                        if !at.is_numeric()
                            && !matches!(at, Ty::Unknown | Ty::Py)
                            && !self.is_tier1_enum(&at)
                        {
                            self.error(
                                format!("{}() expects a number, found {}", fname, at.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return to;
                }
                // Runtime reflection (D-0036). `reflect_has(type, name)` reports
                // whether the type was annotated/decorated with `name`;
                // `reflect_names(type)` returns the comma-joined names. The type
                // and name are identified by string (the reflection surface is
                // intentionally string-keyed; see `design/0004-decorators.md`).
                "reflect_has" => {
                    self.check_reflect_args(fname, 2, args, span);
                    return Ty::Bool;
                }
                "reflect_names" => {
                    self.check_reflect_args(fname, 1, args, span);
                    return Ty::Str;
                }
                _ => {
                    if let Some(sig) = self.sigs.get(fname).cloned() {
                        return self.check_user_call(&sig, fname, args, span);
                    }
                    // Tier-1 enum conversion `Color(x)` — the explicit int->enum
                    // direction (D-0018/D-0020): one integer argument.
                    if let Some(info) = self.enums.get(fname).cloned() {
                        if info.is_sum {
                            self.error(
                                format!(
                                    "'{fname}' is a sum type; construct a variant \
                                     (e.g. '{fname}.Variant(...)'), not '{fname}(...)'"
                                ),
                                span,
                            );
                        } else if args.len() != 1 {
                            self.error(format!("{fname}() takes exactly one argument"), span);
                        } else {
                            let at = self.infer(&args[0].value);
                            if !at.is_int() && !matches!(at, Ty::Unknown | Ty::Py) {
                                self.error(
                                    format!("{fname}() expects an integer, found {}", at.display()),
                                    expr_span(&args[0].value),
                                );
                            }
                        }
                        return Ty::Named(fname.clone());
                    }
                    // Struct construction: `StructName(fields...)`.
                    if self.user_types.contains(fname) {
                        for a in args {
                            self.infer(&a.value);
                        }
                        return Ty::Named(fname.clone());
                    }
                    self.error(format!("call to unknown function '{fname}'"), span);
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
            }
        }
        // Method or other callable. A call through a Python handle (e.g.
        // `np.array(...)`, or a Python callable held in a variable) yields a
        // Python handle (D-0012); anything else is modeled loosely for now.
        let ct = self.infer(callee);
        for a in args {
            self.infer(&a.value);
        }
        if matches!(ct, Ty::Py) {
            Ty::Py
        } else {
            Ty::Unknown
        }
    }

    fn check_user_call(&mut self, sig: &FnSig, fname: &str, args: &[Arg], span: Span) -> Ty {
        let positional: Vec<&Arg> = args.iter().filter(|a| a.name.is_none()).collect();
        let keyword: Vec<&Arg> = args.iter().filter(|a| a.name.is_some()).collect();
        let non_variadic = sig.params.iter().filter(|p| !p.variadic).count();
        let has_var = sig.params.iter().any(|p| p.variadic);
        let required = sig
            .params
            .iter()
            .filter(|p| !p.has_default && !p.variadic)
            .count();

        if !has_var && positional.len() > non_variadic {
            self.error(
                format!("too many arguments to '{fname}' (expected at most {non_variadic})"),
                span,
            );
        }
        if positional.len() + keyword.len() < required {
            self.error(
                format!("too few arguments to '{fname}' (expected at least {required})"),
                span,
            );
        }

        for (i, a) in positional.iter().enumerate() {
            let at = self.infer(&a.value);
            let expected = if let Some(p) = sig.params.get(i) {
                Some(p.ty.clone())
            } else if has_var {
                sig.params.iter().find(|p| p.variadic).map(|p| p.ty.clone())
            } else {
                None
            };
            if let Some(pt) = expected {
                if !assignable(&at, &pt) {
                    self.error(
                        format!(
                            "argument {} to '{}' has type {}, expected {}",
                            i + 1,
                            fname,
                            at.display(),
                            pt.display()
                        ),
                        expr_span(&a.value),
                    );
                }
            }
        }

        for a in &keyword {
            let name = a.name.clone().unwrap();
            let at = self.infer(&a.value);
            match sig.params.iter().find(|p| p.name == name) {
                Some(p) => {
                    if !assignable(&at, &p.ty) {
                        self.error(
                            format!(
                                "keyword argument '{}' to '{}' has type {}, expected {}",
                                name,
                                fname,
                                at.display(),
                                p.ty.display()
                            ),
                            expr_span(&a.value),
                        );
                    }
                }
                None => self.error(
                    format!("'{fname}' has no parameter named '{name}'"),
                    expr_span(&a.value),
                ),
            }
        }

        sig.ret.clone()
    }

    /// Is `t` a tier-1 (all-constant) enum, convertible to/from an integer?
    fn is_tier1_enum(&self, t: &Ty) -> bool {
        matches!(t, Ty::Named(n) if self.enums.get(n).is_some_and(|e| !e.is_sum))
    }

    /// Type a qualified enum-variant construction `Enum.Variant(args)` (D-0046),
    /// accepting named or positional payload arguments and checking their
    /// types. Returns the enum type.
    fn check_variant_construction(
        &mut self,
        enum_name: &str,
        variant: &str,
        info: &EnumInfo,
        args: &[Arg],
        span: Span,
    ) -> Ty {
        let Some(v) = info.variant(variant) else {
            self.error(
                format!("enum '{enum_name}' has no variant '{variant}'"),
                span,
            );
            for a in args {
                self.infer(&a.value);
            }
            return Ty::Named(enum_name.to_string());
        };
        let positional = args.iter().filter(|a| a.name.is_none()).count();
        if positional > v.fields.len() {
            self.error(
                format!(
                    "variant '{enum_name}.{variant}' takes {} field(s), found {}",
                    v.fields.len(),
                    positional
                ),
                span,
            );
        }
        let mut pos = 0usize;
        for a in args {
            let at = self.infer(&a.value);
            let field = match &a.name {
                Some(fname) => {
                    let f = v.fields.iter().find(|(n, _)| n == fname);
                    if f.is_none() {
                        self.error(
                            format!("variant '{enum_name}.{variant}' has no field '{fname}'"),
                            expr_span(&a.value),
                        );
                    }
                    f
                }
                None => {
                    let f = v.fields.get(pos);
                    pos += 1;
                    f
                }
            };
            if let Some((fname, fty)) = field {
                if !assignable(&at, fty) {
                    self.error(
                        format!(
                            "field '{fname}' of '{enum_name}.{variant}' has type {}, found {}",
                            fty.display(),
                            at.display()
                        ),
                        expr_span(&a.value),
                    );
                }
            }
        }
        Ty::Named(enum_name.to_string())
    }

    /// Type-check one `match` arm's pattern against the scrutinee type,
    /// declaring its bindings into the current scope. Returns the set of enum
    /// variant names this pattern covers (for exhaustiveness) and whether it is
    /// an irrefutable catch-all (`_` or a bare capture).
    fn check_pattern(&mut self, p: &Pattern, scrut: &Ty) -> (Vec<String>, bool) {
        match p {
            Pattern::Wildcard(_) => (Vec::new(), true),
            Pattern::Capture { name, .. } => {
                self.declare(name, scrut.clone());
                (Vec::new(), true)
            }
            Pattern::Literal(e) => {
                self.infer(e);
                (Vec::new(), false)
            }
            Pattern::Or(ps, _) => {
                let mut covered = Vec::new();
                let mut catch_all = false;
                for sub in ps {
                    let (c, ca) = self.check_pattern(sub, scrut);
                    covered.extend(c);
                    catch_all |= ca;
                }
                (covered, catch_all)
            }
            Pattern::Sequence(ps, _) | Pattern::Tuple(ps, _) => {
                for sub in ps {
                    self.check_pattern(sub, &Ty::Unknown);
                }
                (Vec::new(), false)
            }
            Pattern::Rest { name: Some(n), .. } => {
                self.declare(n, Ty::Unknown);
                (Vec::new(), false)
            }
            Pattern::Rest { name: None, .. } => (Vec::new(), false),
            Pattern::Struct { path, args, span } => {
                self.check_qualified_pattern(path, args.as_deref(), *span, scrut)
            }
        }
    }

    /// Type a qualified/struct pattern: an enum member/variant (`Color.RED`,
    /// `Shape.Circle(r)`) or a plain struct pattern (`Point(x=0)`). Binds
    /// payload fields positionally or by name (D-0046). Returns covered enum
    /// variants and whether it is a catch-all (never, for these).
    fn check_qualified_pattern(
        &mut self,
        path: &[String],
        args: Option<&[PatArg]>,
        span: Span,
        scrut: &Ty,
    ) -> (Vec<String>, bool) {
        // Qualified `Enum.Variant`: the head names an enum type.
        if path.len() == 2 {
            if let Some(info) = self.enums.get(&path[0]).cloned() {
                let (enum_name, variant) = (&path[0], &path[1]);
                // The scrutinee must be this enum (keep going to bind on error).
                if let Ty::Named(sn) = scrut {
                    if sn != enum_name {
                        self.error(
                            format!(
                                "pattern '{enum_name}.{variant}' does not match a value of \
                                 type {}",
                                scrut.display()
                            ),
                            span,
                        );
                    }
                }
                let Some(v) = info.variant(variant) else {
                    self.error(
                        format!("enum '{enum_name}' has no variant '{variant}'"),
                        span,
                    );
                    return (Vec::new(), false);
                };
                self.bind_variant_fields(enum_name, v, args, span);
                return (vec![variant.clone()], false);
            }
        }
        // A plain struct pattern (`Point(x=0, y=y)`) or an unresolved qualified
        // name: typed loosely (struct-field typing is not modeled yet), but its
        // bindings are still declared so arm bodies type-check.
        if let Some(args) = args {
            for a in args {
                self.check_pattern(&a.pattern, &Ty::Unknown);
            }
        }
        (Vec::new(), false)
    }

    /// Declare the bindings of a variant pattern's payload fields, matching them
    /// positionally or by name and binding each capture to the field's type.
    fn bind_variant_fields(
        &mut self,
        enum_name: &str,
        v: &EnumVariantInfo,
        args: Option<&[PatArg]>,
        span: Span,
    ) {
        let Some(args) = args else {
            return; // `case Shape.Unit:` — no fields to bind.
        };
        let positional = args.iter().filter(|a| a.name.is_none()).count();
        if positional > v.fields.len() {
            self.error(
                format!(
                    "pattern '{enum_name}.{}' binds {} field(s) but the variant has {}",
                    v.name,
                    positional,
                    v.fields.len()
                ),
                span,
            );
        }
        let mut pos = 0usize;
        for a in args {
            let fty = match &a.name {
                Some(fname) => match v.fields.iter().find(|(n, _)| n == fname) {
                    Some((_, t)) => t.clone(),
                    None => {
                        self.error(
                            format!("variant '{enum_name}.{}' has no field '{fname}'", v.name),
                            a.span,
                        );
                        Ty::Unknown
                    }
                },
                None => {
                    let t = v
                        .fields
                        .get(pos)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Ty::Unknown);
                    pos += 1;
                    t
                }
            };
            self.check_pattern(&a.pattern, &fty);
        }
    }
}

fn iter_elem(it: &Ty) -> Ty {
    match it {
        Ty::Range => Ty::Int64,
        Ty::List(t) => (**t).clone(),
        Ty::Set(t) => (**t).clone(),
        Ty::Str => Ty::Char,
        _ => Ty::Unknown,
    }
}

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
        | Expr::ListComp { span, .. }
        | Expr::DictComp { span, .. }
        | Expr::SetComp { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Call { span, .. }
        | Expr::Index { span, .. }
        | Expr::Slice { span, .. }
        | Expr::Attr { span, .. }
        | Expr::Lambda { span, .. }
        | Expr::RegexMatch { span, .. }
        | Expr::RegexSubst { span, .. }
        | Expr::ChanRecv { span, .. }
        | Expr::Yield { span, .. }
        | Expr::IfElse { span, .. } => *span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(src: &str) -> Vec<TypeError> {
        let (terrs, perrs, lerrs) = check_str(src);
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        terrs
    }

    fn ok(src: &str) {
        let e = errors(src);
        assert!(e.is_empty(), "unexpected type errors: {e:?}");
    }

    #[test]
    fn fib_type_checks() {
        ok(include_str!("../../../tests/golden/fib.zap"));
    }

    #[test]
    fn loop_sum_type_checks() {
        ok(include_str!("../../../tests/golden/loop_sum.zap"));
    }

    #[test]
    fn goroutines_type_check() {
        // The full goroutines fixture type-checks: channel construction,
        // `spawn`, send/receive, and `close` (ticket #25, D-0013/D-0031).
        ok(include_str!("../../../tests/golden/goroutines.zap"));
    }

    #[test]
    fn channel_send_and_recv_are_typed() {
        // `<-ch` yields the element type, and a send of a compatible value is
        // accepted; the whole program type-checks.
        ok("def main():\n    auto ch = channel[int64]()\n    ch <- 7\n    int64 v = <-ch\n    print(v)\n");
    }

    #[test]
    fn channel_send_type_mismatch_errors() {
        // Sending a str on a channel[int64] is a type error.
        let e = errors("def main():\n    auto ch = channel[int64]()\n    ch <- \"x\"\n");
        assert_eq!(e.len(), 1, "expected a send-type error, got {e:?}");
    }

    #[test]
    fn send_requires_a_channel_on_the_left() {
        // `x <- v` where `x` is not a channel is rejected.
        let e = errors("def main():\n    auto x = 1\n    x <- 2\n");
        assert_eq!(e.len(), 1, "expected a non-channel send error, got {e:?}");
    }

    #[test]
    fn receive_requires_a_channel() {
        // `<-x` where `x` is not a channel is rejected.
        let e = errors("def main():\n    auto x = 1\n    auto y = <-x\n    print(y)\n");
        assert_eq!(
            e.len(),
            1,
            "expected a non-channel receive error, got {e:?}"
        );
    }

    #[test]
    fn close_requires_a_channel() {
        let e = errors("def main():\n    auto x = 1\n    close(x)\n");
        assert_eq!(
            e.len(),
            1,
            "expected a close-on-non-channel error, got {e:?}"
        );
    }

    #[test]
    fn regex_type_checks() {
        // The full regex fixture type-checks (match objects, captures, `!~`,
        // substitution) — ticket #23, D-0010.
        ok(include_str!("../../../tests/golden/regex.zap"));
    }

    #[test]
    fn regex_subject_must_be_a_string() {
        // The left operand of `=~`/`!~` and of `s///` must be a str.
        let e = errors("def main():\n    auto m = 42 =~ m/x/\n");
        assert_eq!(e.len(), 1, "expected a subject-type error, got {e:?}");
        let e = errors("def main():\n    auto s = 42 =~ s/a/b/\n");
        assert_eq!(e.len(), 1, "expected a subject-type error, got {e:?}");
    }

    #[test]
    fn auto_inference() {
        ok("auto x = 1\nauto y = x + 2\nint64 z = y\n");
    }

    #[test]
    fn widening_is_allowed() {
        ok("int32 a = 0\nint64 b = a\nfloat64 c = b\n");
    }

    #[test]
    fn narrowing_is_rejected() {
        let e = errors("int64 b = 0\nint32 c = b\n");
        assert_eq!(e.len(), 1, "expected one narrowing error, got {e:?}");
    }

    #[test]
    fn explicit_conversion_allows_narrowing() {
        // The explicit cast is the sanctioned way around the narrowing rule
        // (D-0020), and also crosses the int/float boundary.
        ok("int64 b = 0\nint32 c = int32(b)\n");
        ok("float64 x = 1.5\nint32 n = int32(x)\nfloat32 f = float32(x)\n");
    }

    #[test]
    fn conversion_rejects_non_numeric() {
        let e = errors("auto s = \"hi\"\nint32 n = int32(s)\n");
        assert_eq!(
            e.len(),
            1,
            "expected a non-numeric conversion error, got {e:?}"
        );
    }

    #[test]
    fn power_and_floordiv_type_check() {
        // `**` and `//` are ordinary numeric ops: int result for ints, float
        // when a float operand is present; `/` is always float (D-0024).
        ok("int64 p = 2 ** 10\nint64 q = 7 // 2\nfloat64 r = 2.0 ** 0.5\nfloat64 s = 7.0 // 2.0\nfloat64 t = 7 / 2\n");
    }

    #[test]
    fn type_mismatch_in_declaration() {
        let e = errors("int64 x = \"hi\"\n");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn unknown_variable() {
        let e = errors("x = 1\n");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn unknown_function() {
        let e = errors("def main():\n    nope(1)\n");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn return_type_mismatch() {
        let e = errors("def f() -> int64:\n    return \"x\"\n");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn call_arity_and_type() {
        // fib wants one int64; passing a string and too many args errors.
        let e = errors("def fib(int64 n) -> int64:\n    return n\ndef main():\n    print(fib(\"x\"))\n    print(fib(1, 2))\n");
        assert_eq!(e.len(), 2, "got {e:?}");
    }

    #[test]
    fn string_concat_ok() {
        ok("def greet(str name) -> str:\n    return \"hi \" + name\n");
    }

    #[test]
    fn for_over_range_binds_int() {
        ok("def main():\n    auto s = 0\n    for i in range(0, 10):\n        s = s + i\n");
    }

    #[test]
    fn array_programs_type_check() {
        // Arrays are typed loosely (ticket #24): construction, element-wise ops,
        // scalar broadcast, indexing, len, and sum must not raise false errors.
        ok(include_str!("../../../tests/golden/array_int.zap"));
        ok(include_str!("../../../tests/golden/array_f64.zap"));
    }

    // --- decorators & annotations (ticket #16, D-0036) ---

    #[test]
    fn decorators_golden_type_checks() {
        ok(include_str!("../../../tests/golden/decorators.zap"));
    }

    #[test]
    fn unknown_decorator_is_error() {
        // Compile-time typo-catching (D-0016): an undeclared `@name` is rejected.
        let e = errors("@frobnicate\nstruct S:\n    int64 x\n");
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("unknown decorator"), "{e:?}");
    }

    #[test]
    fn known_annotation_and_decorator_accepted() {
        ok("annotation Tag:\n    str name = \"\"\n\n@Tag(name=\"x\")\nstruct S:\n    int64 x\n");
    }

    #[test]
    fn annotation_unknown_field_is_error() {
        let e = errors(
            "annotation Tag:\n    str name = \"\"\n\n@Tag(nope=\"x\")\nstruct S:\n    int64 x\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("no field 'nope'"), "{e:?}");
    }

    #[test]
    fn derive_unknown_trait_is_error() {
        let e = errors("@derive(Nonsense)\nstruct S:\n    int64 x\n");
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("unknown derivable trait"), "{e:?}");
    }

    #[test]
    fn derive_needs_a_trait() {
        let e = errors("@derive\nstruct S:\n    int64 x\n");
        assert_eq!(e.len(), 1, "got {e:?}");
    }

    #[test]
    fn derive_only_on_structs() {
        let e = errors("@derive(Eq)\ndef f():\n    print(1)\n");
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("applies to a struct"), "{e:?}");
    }

    #[test]
    fn builtin_arg_shapes_checked() {
        // @cache takes no args; @default takes exactly one expr; @inject none.
        // (`@range` is in the locked set but its name collides with the `range`
        // keyword, so it cannot currently be written — see the PR notes.)
        assert_eq!(errors("@cache(1)\ndef f():\n    print(1)\n").len(), 1);
        assert_eq!(
            errors("struct S:\n    @default(1, 2)\n    int64 x\n").len(),
            1
        );
        assert_eq!(errors("struct S:\n    @inject(1)\n    int64 x\n").len(), 1);
    }

    #[test]
    fn reflection_builtins_type_check() {
        ok("def main():\n    if reflect_has(\"S\", \"Tag\"):\n        print(reflect_names(\"S\"))\n");
        // Non-string argument is rejected.
        let e = errors("def main():\n    print(reflect_has(1, 2))\n");
        assert!(!e.is_empty(), "reflect_has should want str args");
    }

    // --- Python interop (ticket #27, D-0012) ---

    #[test]
    fn python_interop_golden_type_checks() {
        // The golden program type-checks: imported module aliases resolve, and
        // attribute access / calls on Python handles plus scalar conversions are
        // accepted (Python values are dynamic).
        ok(include_str!("../../../tests/golden/python_interop.zap"));
    }

    #[test]
    fn python_module_alias_is_in_scope() {
        // Referencing an imported alias is not an "unknown name" error, and a
        // Python call result flows as a dynamic value usable anywhere.
        ok("import python \"math\" as m\n\ndef main():\n    auto r = m.sqrt(2.0)\n    print(int64(r))\n");
        ok("import python \"json\" as j\n\ndef main():\n    auto s = j.dumps(\"x\")\n    print(s)\n");
    }

    #[test]
    fn python_conversion_accepts_a_handle() {
        // `int64(...)` / `float64(...)` accept a Python handle (they marshal it
        // back), where a non-numeric Zap value would be rejected.
        ok("import python \"math\" as m\n\ndef main():\n    int64 g = int64(m.gcd(6, 4))\n    print(g)\n");
    }

    #[test]
    fn pyobject_type_annotation_is_accepted() {
        // The opaque `pyobject` handle type can be written explicitly.
        ok("import python \"math\" as m\n\ndef main():\n    pyobject r = m.sqrt(2.0)\n    print(r)\n");
    }

    // --- enums & match (ticket #51, D-0018/D-0040/D-0046) ---

    fn msgs(src: &str) -> String {
        errors(src)
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn tier1_enum_members_conversions_and_exhaustive_match() {
        // Member access, explicit int<->enum conversions, and an exhaustive
        // match all type-check (D-0018).
        ok("enum Color:\n    RED\n    GREEN = 10\n    BLUE\n\n\
            def main():\n    auto c = Color.GREEN\n\
            \x20   int64 n = int64(c)\n    auto d = Color(n)\n\
            \x20   match c:\n        case Color.RED:\n            print(1)\n\
            \x20       case Color.GREEN:\n            print(2)\n\
            \x20       case Color.BLUE:\n            print(3)\n");
    }

    #[test]
    fn enum_match_missing_variant_is_an_error() {
        // D-0040 exhaustiveness: omitting a variant with no `_` is a compile
        // error naming the missing variant.
        let m = msgs(
            "enum Color:\n    RED\n    GREEN\n    BLUE\n\n\
            def main():\n    auto c = Color.RED\n    match c:\n\
            \x20       case Color.RED:\n            print(1)\n\
            \x20       case Color.GREEN:\n            print(2)\n",
        );
        assert!(
            m.contains("non-exhaustive") && m.contains("Color.BLUE"),
            "expected a non-exhaustive error naming Color.BLUE, got:\n{m}"
        );
    }

    #[test]
    fn enum_match_wildcard_is_exhaustive() {
        // A `_` arm makes any enum match exhaustive.
        ok("enum Color:\n    RED\n    GREEN\n    BLUE\n\n\
            def main():\n    auto c = Color.RED\n    match c:\n\
            \x20       case Color.RED:\n            print(1)\n\
            \x20       case _:\n            print(0)\n");
    }

    #[test]
    fn tier3_construction_and_destructuring_type_check() {
        // Data-carrying variants construct with named or positional args and
        // destructure positionally or by name; payload captures are typed
        // (D-0046). The exhaustive match needs no `_`.
        ok(
            "enum Shape:\n    Circle(f64 r)\n    Rect(f64 w, f64 h)\n    Unit\n\n\
            def area(Shape s) -> f64:\n    match s:\n\
            \x20       case Shape.Circle(r):\n            return 3.14 * r * r\n\
            \x20       case Shape.Rect(w=w, h=h):\n            return w * h\n\
            \x20       case Shape.Unit:\n            return 1.0\n\n\
            def main():\n\
            \x20   print(area(Shape.Circle(r=2.0)))\n\
            \x20   print(area(Shape.Rect(3.0, 4.0)))\n",
        );
    }

    #[test]
    fn tier3_unknown_variant_and_field_are_errors() {
        let m = msgs(
            "enum Shape:\n    Circle(f64 r)\n    Unit\n\n\
            def main():\n    auto s = Shape.Square(1.0)\n",
        );
        assert!(m.contains("no variant 'Square'"), "got:\n{m}");
        let m = msgs(
            "enum Shape:\n    Circle(f64 r)\n    Unit\n\n\
            def main():\n    auto s = Shape.Circle(radius=1.0)\n",
        );
        assert!(m.contains("no field 'radius'"), "got:\n{m}");
    }

    #[test]
    fn recursive_enum_type_checks() {
        // A (mutually) recursive variant field references the enum itself
        // (D-0046); the whole vertical type-checks.
        ok("enum Expr:\n    Num(int64 v)\n    Add(Expr l, Expr r)\n\n\
            def eval(Expr e) -> int64:\n    match e:\n\
            \x20       case Expr.Num(v):\n            return v\n\
            \x20       case Expr.Add(l, r):\n            return eval(l) + eval(r)\n\n\
            def main():\n    print(eval(Expr.Add(Expr.Num(2), Expr.Num(3))))\n");
    }

    #[test]
    fn enum_is_a_distinct_type_from_int() {
        // An enum is its own type: assigning a raw int to an enum (or vice
        // versa) without an explicit conversion is rejected (D-0018/D-0020).
        let m = msgs(
            "enum Color:\n    RED\n    GREEN\n\n\
            def main():\n    Color c = 1\n",
        );
        assert!(m.contains("int") && m.contains("Color"), "got:\n{m}");
    }

    #[test]
    fn enum_methods_are_deferred() {
        // Methods on enums are locked (D-0046) but their lowering is a
        // follow-up; declaring one is reported, not silently dropped.
        let m = msgs(
            "enum Color:\n    RED\n    GREEN\n\n\
            \x20   def tag(self) -> int64:\n        return 0\n",
        );
        assert!(m.contains("methods on enums"), "got:\n{m}");
    }

    // --- tuples (ticket #54, D-0049) ---

    #[test]
    fn tuple_literal_elements_are_typed() {
        // A tuple literal carries its element types positionally: `t[1]` is a
        // `str`, so assigning it into a `str` is fine but into an `int64` is not.
        ok("def main():\n    auto t = (1, \"two\", 3.0)\n    str s = t[1]\n    print(s)\n");
        let e = errors(
            "def main():\n    auto t = (1, \"two\", 3.0)\n    int64 n = t[1]\n    print(n)\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
    }

    #[test]
    fn tuple_const_index_access_types() {
        // Constant indices yield each position's static type.
        ok("def main():\n    auto t = (10, 20)\n    int64 a = t[0]\n    int64 b = t[1]\n    print(a + b)\n");
    }

    #[test]
    fn tuple_nonconstant_index_is_error() {
        // A runtime index is rejected: a tuple is heterogeneous, so the element
        // type must be known statically (the key difference from `list[i]`).
        let e = errors(
            "def main():\n    auto t = (1, 2)\n    auto i = 0\n    auto x = t[i]\n    print(x)\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("compile-time-constant"), "got {e:?}");
    }

    #[test]
    fn tuple_index_out_of_range_is_error() {
        let e = errors("def main():\n    auto t = (1, 2)\n    auto x = t[5]\n    print(x)\n");
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("out of range"), "got {e:?}");
    }

    #[test]
    fn tuple_destructuring_binds_element_types() {
        // Positional destructuring binds each name to its element's type.
        ok("def main():\n    auto (a, b, c) = (1, \"two\", 3.0)\n    int64 x = a\n    str y = b\n    float64 z = c\n    print(x)\n    print(y)\n    print(z)\n");
    }

    #[test]
    fn tuple_destructuring_arity_mismatch_is_error() {
        let e = errors("def main():\n    auto (a, b) = (1, 2, 3)\n    print(a)\n");
        assert!(
            e.iter().any(|x| x.message.contains("element")),
            "expected an arity error, got {e:?}"
        );
    }

    #[test]
    fn tuple_rest_captures_remaining_as_tuple() {
        // `*rest` binds the remaining elements as a sub-tuple, so `rest[k]` is
        // typed by position.
        ok("def main():\n    auto (first, *rest) = (10, 20, 30, 40)\n    int64 a = first\n    int64 b = rest[0]\n    int64 c = rest[2]\n    print(a + b + c)\n");
    }

    #[test]
    fn multiple_return_is_tuple_typed() {
        // A function returning `(int64, int64)` accepts a bare comma-list return
        // and the caller may destructure or bind-and-index it.
        ok(include_str!("../../../tests/golden/tuple_multiret.zap"));
    }

    #[test]
    fn multiple_return_arity_mismatch_is_error() {
        // Returning a single value where a 2-tuple is declared is rejected.
        let e = errors(
            "def f() -> (int64, int64):\n    return 1\n\ndef main():\n    auto t = f()\n    print(t[0])\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("return type mismatch"), "got {e:?}");
    }

    #[test]
    fn swap_reassignment_type_checks() {
        ok(include_str!("../../../tests/golden/tuple_swap.zap"));
    }
}
