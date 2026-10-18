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
    /// A user-defined type (struct/enum), treated opaquely for now.
    Named(String),
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
            Ty::Named(n) => n.clone(),
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
    if matches!(from, Ty::Unknown) || matches!(to, Ty::Unknown) {
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
    scopes: Vec<HashMap<String, Ty>>,
    ret_ty: Ty,
    errors: Vec<TypeError>,
}

const BUILTINS: &[&str] = &["print", "range", "len"];

impl Checker {
    fn new() -> Checker {
        Checker {
            sigs: HashMap::new(),
            user_types: HashSet::new(),
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
                "list" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::List(Box::new(el))
                }
                "array" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::List(Box::new(el)) // arrays modeled as lists for now
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

    // --- checking ---

    fn check_module(&mut self, module: &Module) {
        self.push_scope();
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
                self.infer(base);
                Ty::Unknown
            }
            Expr::Lambda { .. } => Ty::Unknown,
            Expr::RegexMatch { subject, .. } => {
                self.infer(subject);
                Ty::Unknown
            }
            Expr::RegexSubst { subject, .. } => {
                self.infer(subject);
                Ty::Str
            }
            Expr::ChanRecv { chan, .. } => {
                self.infer(chan);
                Ty::Unknown
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
            Send => Ty::Unit,
        }
    }

    fn check_call(&mut self, callee: &Expr, args: &[Arg], span: Span) -> Ty {
        if let Expr::Name(fname, _) = callee {
            match fname.as_str() {
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
        // Method or other callable: not modeled yet.
        self.infer(callee);
        for a in args {
            self.infer(&a.value);
        }
        Ty::Unknown
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
        | Expr::Dict(_, s) => *s,
        Expr::Str { span, .. }
        | Expr::ListComp { span, .. }
        | Expr::DictComp { span, .. }
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
}
