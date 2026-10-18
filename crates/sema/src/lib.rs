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
        _ => false,
    }
}

/// Make an untyped literal type concrete (its default).
fn default_type(t: Ty) -> Ty {
    match t {
        Ty::UntypedInt => Ty::Int64,
        Ty::UntypedFloat => Ty::Float64,
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
            Type::Tuple { .. } | Type::Func { .. } => Ty::Unknown,
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
                let it = self.infer(iter);
                let elem = match it {
                    Ty::Range => Ty::Int64,
                    Ty::List(t) => *t,
                    Ty::Str => Ty::Char,
                    _ => Ty::Unknown,
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
                scrutinee, arms, ..
            } => {
                self.infer(scrutinee);
                for arm in arms {
                    self.push_scope();
                    let mut names = Vec::new();
                    collect_pattern_bindings(&arm.pattern, &mut names);
                    for n in names {
                        self.declare(&n, Ty::Unknown);
                    }
                    if let Some(g) = &arm.guard {
                        self.infer(g);
                    }
                    for st in &arm.body.stmts {
                        self.check_stmt(st);
                    }
                    self.pop_scope();
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
            Stmt::Break(_) | Stmt::Continue(_) => {}
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
                for el in elems {
                    self.infer(el);
                }
                Ty::Unknown
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
            Expr::Index { base, index, .. } => {
                let b = self.infer(base);
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
            Expr::Attr { base, .. } => {
                // Attribute access on a Python handle yields another Python
                // handle (D-0012); everything else is modeled loosely for now.
                let b = self.infer(base);
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
                        // back to a Zap number (D-0012).
                        if !at.is_numeric() && !matches!(at, Ty::Unknown | Ty::Py) {
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

fn collect_pattern_bindings(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Capture { name, .. } => out.push(name.clone()),
        Pattern::Rest { name: Some(n), .. } => out.push(n.clone()),
        Pattern::Or(ps, _) | Pattern::Sequence(ps, _) | Pattern::Tuple(ps, _) => {
            for sub in ps {
                collect_pattern_bindings(sub, out);
            }
        }
        Pattern::Struct { fields, .. } => {
            for (_, sub) in fields {
                collect_pattern_bindings(sub, out);
            }
        }
        _ => {}
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
}
