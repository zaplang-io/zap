//! The Zap abstract syntax tree.
//!
//! These are the shapes the parser produces and the type checker consumes.
//! They mirror `SPEC.md` v0.2; semantics (type inference, exhaustiveness,
//! decorator expansion) are applied in later phases, not encoded here.
//!
//! This is the frozen front-end ↔ back-end boundary for Phase 0: once stable,
//! the parser and type-checker can be built against it independently.

use crate::span::Span;

/// A whole source file (one module, D-0028).
#[derive(Clone, Debug)]
pub struct Module {
    pub items: Vec<Item>,
    pub span: Span,
}

/// A top-level item. Bare statements are allowed at top level (D-0021).
#[derive(Clone, Debug)]
pub enum Item {
    Function(Function),
    Struct(StructDef),
    Interface(InterfaceDef),
    Enum(EnumDef),
    Annotation(AnnotationDef),
    Decorator(DecoratorDef),
    Import(Import),
    Stmt(Stmt),
}

// ----------------------------------------------------------------------------
// Types
// ----------------------------------------------------------------------------

/// A type expression, e.g. `int64`, `list[T]`, `dict[str, int64]`, `Point`,
/// `(int64, str)`, `fn(int64) -> bool`.
#[derive(Clone, Debug)]
pub enum Type {
    /// A named type with optional generic arguments.
    Named {
        name: String,
        args: Vec<Type>,
        span: Span,
    },
    /// A tuple type `(A, B, ...)`.
    Tuple { elems: Vec<Type>, span: Span },
    /// A function type `fn(params) -> ret`.
    Func {
        params: Vec<Type>,
        ret: Option<Box<Type>>,
        span: Span,
    },
}

// ----------------------------------------------------------------------------
// Declarations
// ----------------------------------------------------------------------------

/// A decorator/annotation application, e.g. `@retry(times=3)` (D-0036).
#[derive(Clone, Debug)]
pub struct Decorator {
    pub name: String,
    pub args: Vec<Arg>,
    pub span: Span,
}

/// A function or method definition.
#[derive(Clone, Debug)]
pub struct Function {
    pub decorators: Vec<Decorator>,
    pub name: String,
    /// Generic type parameters, e.g. `[T, U]` (D-0017).
    pub generics: Vec<String>,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub body: Block,
    pub span: Span,
}

/// A function/method parameter (type-first, D-0003/D-0038).
#[derive(Clone, Debug)]
pub struct Param {
    pub decorators: Vec<Decorator>,
    /// `None` only for an untyped `self` receiver.
    pub ty: Option<Type>,
    pub name: String,
    /// Default value (evaluated per call, D-0038).
    pub default: Option<Expr>,
    /// `true` for a `TYPE... name` variadic (D-0038).
    pub variadic: bool,
    pub span: Span,
}

/// A struct definition (D-0015).
#[derive(Clone, Debug)]
pub struct StructDef {
    pub decorators: Vec<Decorator>,
    pub name: String,
    pub generics: Vec<String>,
    pub fields: Vec<Field>,
    pub methods: Vec<Function>,
    /// `true` for a `value struct` (opt-in value type, D-0051): copied on
    /// assign/pass/return, laid out inline (no heap header). A plain `struct`
    /// stays a reference type (D-0023) and has `is_value == false`.
    pub is_value: bool,
    pub span: Span,
}

/// A struct or annotation field.
#[derive(Clone, Debug)]
pub struct Field {
    pub decorators: Vec<Decorator>,
    pub ty: Type,
    pub name: String,
    /// Literal field default (D-0038); computed defaults use `@default(...)`.
    pub default: Option<Expr>,
    pub span: Span,
}

/// A structural interface (D-0016).
#[derive(Clone, Debug)]
pub struct InterfaceDef {
    pub name: String,
    pub generics: Vec<String>,
    pub methods: Vec<MethodSig>,
    pub span: Span,
}

/// A method signature inside an interface.
#[derive(Clone, Debug)]
pub struct MethodSig {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub span: Span,
}

/// An enum definition (D-0018 tier-1 named constants; D-0046 tier-3
/// data-carrying variants / sum types). A variant with no payload fields is a
/// tier-1 constant (optionally with an explicit value); a variant with payload
/// fields is a tier-3 data-carrying variant. The two tiers do not mix: any
/// payload-carrying variant makes the whole enum a tagged value (D-0046).
#[derive(Clone, Debug)]
pub struct EnumDef {
    pub decorators: Vec<Decorator>,
    pub name: String,
    /// Generic type parameters, e.g. `[T]` (reserved; D-0017/D-0046).
    pub generics: Vec<String>,
    pub variants: Vec<EnumVariant>,
    /// Methods declared in the enum body (`def` with `self`, D-0046). Parsed
    /// now; lowering is a documented follow-up.
    pub methods: Vec<Function>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumVariant {
    pub name: String,
    /// Explicit discriminant for a tier-1 constant (`GREEN = 10`); `None`
    /// otherwise. Never set for a data-carrying variant.
    pub value: Option<Expr>,
    /// Type-first named payload fields (`Circle(f64 r)`, D-0046); empty for a
    /// tier-1 constant.
    pub fields: Vec<VariantField>,
    pub span: Span,
}

/// A type-first named payload field of a data-carrying enum variant (D-0046),
/// e.g. the `f64 r` in `Circle(f64 r)`.
#[derive(Clone, Debug)]
pub struct VariantField {
    pub ty: Type,
    pub name: String,
    pub span: Span,
}

/// A metadata annotation declaration (declared like a struct, D-0036).
#[derive(Clone, Debug)]
pub struct AnnotationDef {
    pub name: String,
    pub fields: Vec<Field>,
    pub span: Span,
}

/// A behavioral decorator declaration (`before`/`after`/`wrap`, D-0036).
#[derive(Clone, Debug)]
pub struct DecoratorDef {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Block,
    pub span: Span,
}

/// An import (D-0028 / D-0012).
#[derive(Clone, Debug)]
pub enum Import {
    /// `import path` / `import path as alias`.
    Module {
        path: String,
        alias: Option<String>,
        span: Span,
    },
    /// `import python "module" as alias`.
    Python {
        module: String,
        alias: Option<String>,
        span: Span,
    },
}

// ----------------------------------------------------------------------------
// Statements
// ----------------------------------------------------------------------------

/// An indented block of statements.
#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// `TYPE name = expr`, `auto name = expr`, or `const ... = expr` (D-0003).
    Let {
        is_const: bool,
        /// `None` means `auto` (inferred).
        ty: Option<Type>,
        name: String,
        value: Expr,
        span: Span,
    },
    /// `target = expr`.
    Assign {
        target: Expr,
        value: Expr,
        span: Span,
    },
    /// `target OP= expr` (e.g. `x += 1`).
    AugAssign {
        target: Expr,
        op: BinOp,
        value: Expr,
        span: Span,
    },
    /// An expression evaluated for effect.
    Expr(Expr),
    /// `if` / `elif` / `else`.
    If {
        /// One `(condition, body)` per `if`/`elif` branch, in order.
        branches: Vec<(Expr, Block)>,
        else_block: Option<Block>,
        span: Span,
    },
    /// `while cond:`.
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    /// `do: ... while cond` (bottom-tested, D-0039).
    DoWhile {
        body: Block,
        cond: Expr,
        span: Span,
    },
    /// `for target in iter:`.
    For {
        target: Expr,
        iter: Expr,
        body: Block,
        span: Span,
    },
    /// `match scrutinee: case ...` (D-0040).
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// `return [expr]`.
    Return {
        value: Option<Expr>,
        span: Span,
    },
    /// `raise [expr]` (D-0007).
    Raise {
        value: Option<Expr>,
        span: Span,
    },
    /// `assert cond [, msg]` with power-assert diagnostics (D-0054). Always
    /// evaluated in v1; on failure the condition and its operand values are
    /// reported. `cond_src` is the exact source spelling of `cond` (the parser
    /// retains it because power-assert needs to echo the expression text).
    Assert {
        cond: Expr,
        msg: Option<Expr>,
        cond_src: String,
        span: Span,
    },
    /// `try: ... except ...: ... finally: ...` (D-0007/D-0029).
    Try {
        body: Block,
        excepts: Vec<ExceptClause>,
        finally: Option<Block>,
        span: Span,
    },
    /// `with items: ...` (D-0027).
    With {
        items: Vec<WithItem>,
        body: Block,
        span: Span,
    },
    /// `spawn call(...)` (D-0013), or `spawn detached call(...)` (D-0044).
    /// A plain `spawn` is owned by the nearest enclosing [`Stmt::Scope`] (or the
    /// program's implicit root scope); `detached` opts out of all scope
    /// ownership (never awaited, not joined at program exit).
    Spawn {
        call: Expr,
        detached: bool,
        span: Span,
    },
    /// `scope: ...` / `scope(on_error=OnError.Wait): ...` — a structured-
    /// concurrency region that owns every `spawn` lexically inside it and joins
    /// (or cancels) them at the dedent (D-0044/D-0045, `design/0005`). `opts`
    /// holds the parenthesized options (`on_error`, `timeout`); an absent
    /// option list is the empty vec.
    Scope {
        opts: Vec<Arg>,
        body: Block,
        span: Span,
    },
    /// Tuple-destructuring declaration (D-0049): `auto (a, b) = t`,
    /// `auto (first, *rest) = xs`, `(int64, str) (a, b) = p`. `target` is always
    /// a [`BindTarget::Tuple`] (a single-name decl stays [`Stmt::Let`]).
    LetTuple {
        is_const: bool,
        /// `None` means `auto` (inferred).
        ty: Option<Type>,
        target: BindTarget,
        value: Expr,
        span: Span,
    },
    /// Tuple-destructuring assignment (D-0049): `(x, y) = (y, x)` (swap),
    /// `a, b = f()`, `first, *rest = xs`. `target` is always a
    /// [`BindTarget::Tuple`] (a single lvalue stays [`Stmt::Assign`]).
    AssignTuple {
        target: BindTarget,
        value: Expr,
        span: Span,
    },
    Break(Span),
    Continue(Span),
}

/// A destructuring bind target (D-0049), used by [`Stmt::LetTuple`] and
/// [`Stmt::AssignTuple`]. Positional, possibly nested, with at most one `*rest`
/// per tuple capturing the remainder as a sub-tuple.
#[derive(Clone, Debug)]
pub enum BindTarget {
    /// A leaf target: a new name (in a decl) or an lvalue (`name`, `a.b`,
    /// `a[i]`) in an assignment.
    Elem(Expr),
    /// `*target` — captures the remaining elements as a tuple. At most one per
    /// tuple (enforced in sema/lowering).
    Rest(Box<BindTarget>, Span),
    /// `(a, b, ...)` — positional destructuring, possibly nested.
    Tuple(Vec<BindTarget>, Span),
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ExceptClause {
    pub ty: Option<Type>,
    pub binding: Option<String>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct WithItem {
    pub expr: Expr,
    pub alias: Option<String>,
    pub span: Span,
}

// ----------------------------------------------------------------------------
// Patterns (match-case, D-0040)
// ----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Pattern {
    /// `_`
    Wildcard(Span),
    /// A bare name that binds (capture).
    Capture { name: String, span: Span },
    /// A literal to match against.
    Literal(Expr),
    /// `a | b | c`
    Or(Vec<Pattern>, Span),
    /// `[a, b, *rest]`
    Sequence(Vec<Pattern>, Span),
    /// `(a, b)`
    Tuple(Vec<Pattern>, Span),
    /// A dotted/qualified pattern: a struct pattern (`Point(x=0, y=y)`), an
    /// enum-variant pattern (`Shape.Circle(r)`, D-0046), or a qualified
    /// constant / payload-free variant with no parens (`Color.RED`,
    /// `Shape.Unit`). `path` is the dotted name in declaration order
    /// (`["Shape", "Circle"]`); `args` is `None` when there are no parentheses
    /// (a bare qualified constant) and `Some` otherwise (fields bound
    /// positionally or by name, developer's choice — D-0040/D-0046).
    Struct {
        path: Vec<String>,
        args: Option<Vec<PatArg>>,
        span: Span,
    },
    /// `*rest` inside a sequence pattern.
    Rest { name: Option<String>, span: Span },
}

/// One argument of a struct/variant pattern (D-0040/D-0046). `name` is `Some`
/// for a by-name binding (`h=h`) and `None` for a positional one (`r`).
#[derive(Clone, Debug)]
pub struct PatArg {
    pub name: Option<String>,
    pub pattern: Pattern,
    pub span: Span,
}

// ----------------------------------------------------------------------------
// Expressions
// ----------------------------------------------------------------------------

/// A call argument; `name` is set for keyword arguments (D-0038).
#[derive(Clone, Debug)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Expr {
    // Literals (raw lexeme retained for numerics).
    Int(String, Span),
    Float(String, Span),
    Imaginary(String, Span),
    Str {
        value: String,
        raw: bool,
        bytes: bool,
        fstring: bool,
        span: Span,
    },
    Bool(bool, Span),
    None(Span),

    Name(String, Span),

    Tuple(Vec<Expr>, Span),
    List(Vec<Expr>, Span),
    Dict(Vec<(Expr, Expr)>, Span),
    /// `{a, b, c}` — a set literal: braces with comma-separated elements and no
    /// `key: value` pairs (empty `{}` parses as an empty [`Expr::Dict`]).
    Set(Vec<Expr>, Span),

    /// `[elem for var in iter if cond]`
    ListComp {
        elem: Box<Expr>,
        var: String,
        iter: Box<Expr>,
        cond: Option<Box<Expr>>,
        span: Span,
    },
    /// `{key: value for var in iter if cond}`
    DictComp {
        key: Box<Expr>,
        value: Box<Expr>,
        var: String,
        iter: Box<Expr>,
        cond: Option<Box<Expr>>,
        span: Span,
    },
    /// `{elem for var in iter if cond}`
    SetComp {
        elem: Box<Expr>,
        var: String,
        iter: Box<Expr>,
        cond: Option<Box<Expr>>,
        span: Span,
    },

    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },

    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
        span: Span,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    /// `base[lo:hi:step]` (D-0030).
    Slice {
        base: Box<Expr>,
        lo: Option<Box<Expr>>,
        hi: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
        span: Span,
    },
    Attr {
        base: Box<Expr>,
        name: String,
        span: Span,
    },

    /// `lambda a, b: expr` (D-0026).
    Lambda {
        params: Vec<String>,
        body: Box<Expr>,
        span: Span,
    },

    /// `subject =~ m/.../` or `subject !~ m/.../` (D-0010).
    RegexMatch {
        subject: Box<Expr>,
        pattern: String,
        flags: String,
        negated: bool,
        span: Span,
    },
    /// `subject =~ s/.../.../` (D-0010).
    RegexSubst {
        subject: Box<Expr>,
        pattern: String,
        replacement: String,
        flags: String,
        span: Span,
    },

    /// `<-ch` channel receive (D-0031).
    ChanRecv {
        chan: Box<Expr>,
        span: Span,
    },
    /// `yield [expr]` as an expression (D-0026).
    Yield {
        value: Option<Box<Expr>>,
        span: Span,
    },
    /// `then if cond else els` — conditional expression.
    IfElse {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
        span: Span,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,    // -
    Pos,    // +
    Not,    // not
    BitNot, // ~
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,      // +
    Sub,      // -
    Mul,      // *
    Div,      // /   (true division, D-0024)
    FloorDiv, // //  (floor division, D-0024)
    Mod,      // %
    Pow,      // **
    And,      // and
    Or,       // or
    BitAnd,   // &
    BitOr,    // |
    BitXor,   // ^
    Shl,      // <<
    Shr,      // >>
    Eq,       // ==
    Ne,       // !=
    Lt,       // <
    Le,       // <=
    Gt,       // >
    Ge,       // >=
    In,       // in  (membership)
    Send,     // ch <- v  (channel send, D-0031)
}

/// The docstring of a function, if its first statement is a bare string literal
/// (D-0041). Google-style docstrings (summary + `Args:`/`Returns:`/`Raises:`)
/// are a plain triple-quoted first statement; this extracts the raw text. Full
/// `zap doc` generation is deferred (D-0053), but this is the seam it will use.
pub fn docstring(func: &Function) -> Option<&str> {
    match func.body.stmts.first() {
        Some(Stmt::Expr(Expr::Str {
            value,
            fstring: false,
            ..
        })) => Some(value.as_str()),
        _ => None,
    }
}

/// The source spelling of a binary operator.
pub fn binop_str(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        FloorDiv => "//",
        Mod => "%",
        Pow => "**",
        And => "and",
        Or => "or",
        BitAnd => "&",
        BitOr => "|",
        BitXor => "^",
        Shl => "<<",
        Shr => ">>",
        Eq => "==",
        Ne => "!=",
        Lt => "<",
        Le => "<=",
        Gt => ">",
        Ge => ">=",
        In => "in",
        Send => "<-",
    }
}

/// Render an expression back to a readable, source-like string. This is a
/// best-effort unparser used for diagnostics (power-assert operand labels,
/// D-0054) and `@parametrize` case labels — not a round-tripping formatter, so
/// spacing/parenthesization is normalized rather than preserved.
pub fn render_expr(e: &Expr) -> String {
    match e {
        Expr::Int(s, _) | Expr::Float(s, _) | Expr::Imaginary(s, _) => s.clone(),
        Expr::Str {
            value,
            raw,
            bytes,
            fstring,
            ..
        } => {
            let prefix = format!(
                "{}{}{}",
                if *raw { "r" } else { "" },
                if *bytes { "b" } else { "" },
                if *fstring { "f" } else { "" },
            );
            format!("{prefix}\"{value}\"")
        }
        Expr::Bool(b, _) => if *b { "True" } else { "False" }.to_string(),
        Expr::None(_) => "None".to_string(),
        Expr::Name(n, _) => n.clone(),
        Expr::Tuple(xs, _) => {
            if xs.len() == 1 {
                format!("({},)", render_expr(&xs[0]))
            } else {
                format!("({})", render_list(xs))
            }
        }
        Expr::List(xs, _) => format!("[{}]", render_list(xs)),
        Expr::Set(xs, _) => format!("{{{}}}", render_list(xs)),
        Expr::Dict(pairs, _) => {
            let body = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", render_expr(k), render_expr(v)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{body}}}")
        }
        Expr::Unary { op, expr, .. } => {
            let sym = match op {
                UnOp::Neg => "-",
                UnOp::Pos => "+",
                UnOp::Not => "not ",
                UnOp::BitNot => "~",
            };
            format!("{sym}{}", render_expr(expr))
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            format!(
                "{} {} {}",
                render_expr(lhs),
                binop_str(*op),
                render_expr(rhs)
            )
        }
        Expr::Call { callee, args, .. } => {
            let a = args
                .iter()
                .map(|arg| match &arg.name {
                    Some(n) => format!("{n}={}", render_expr(&arg.value)),
                    None => render_expr(&arg.value),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({a})", render_expr(callee))
        }
        Expr::Index { base, index, .. } => {
            format!("{}[{}]", render_expr(base), render_expr(index))
        }
        Expr::Attr { base, name, .. } => format!("{}.{name}", render_expr(base)),
        other => format!("{other:?}"),
    }
}

fn render_list(xs: &[Expr]) -> String {
    xs.iter().map(render_expr).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    #[test]
    fn docstring_extraction() {
        use crate::parser::parse_str;
        let (m, perrs, lerrs) = parse_str(
            "def greet(str n) -> str:\n    \"\"\"Greet someone.\n\n    Args:\n        n: name\n    \"\"\"\n    return n\n",
        );
        assert!(perrs.is_empty() && lerrs.is_empty());
        let Item::Function(f) = &m.items[0] else {
            panic!("fn");
        };
        let doc = docstring(f).expect("docstring");
        assert!(doc.starts_with("Greet someone."));
        assert!(doc.contains("Args:"));

        // A function without a leading string has no docstring.
        let (m2, _, _) = parse_str("def f():\n    return 1\n");
        let Item::Function(f2) = &m2.items[0] else {
            panic!("fn");
        };
        assert!(docstring(f2).is_none());
    }

    #[test]
    fn render_expr_round_trips_common_forms() {
        use crate::parser::parse_str;
        let cases = [
            ("a + 1 == b", "a + 1 == b"),
            ("f(x, y)", "f(x, y)"),
            ("xs[0]", "xs[0]"),
            ("not ready", "not ready"),
            ("(1, 2)", "(1, 2)"),
        ];
        for (src, want) in cases {
            let (m, perrs, lerrs) = parse_str(&format!("def main():\n    assert {src}\n"));
            assert!(
                perrs.is_empty() && lerrs.is_empty(),
                "{src}: {perrs:?} {lerrs:?}"
            );
            let Item::Function(f) = &m.items[0] else {
                panic!("fn");
            };
            let Stmt::Assert { cond, .. } = &f.body.stmts[0] else {
                panic!("assert");
            };
            assert_eq!(render_expr(cond), want, "render of {src}");
        }
    }

    #[test]
    fn build_trivial_module() {
        // `def main(): return 0` constructed by hand — smoke-tests the shapes.
        let sp = Span::new(0, 0);
        let body = Block {
            stmts: vec![Stmt::Return {
                value: Some(Expr::Int("0".to_string(), sp)),
                span: sp,
            }],
            span: sp,
        };
        let func = Function {
            decorators: vec![],
            name: "main".to_string(),
            generics: vec![],
            params: vec![],
            ret: None,
            body,
            span: sp,
        };
        let module = Module {
            items: vec![Item::Function(func)],
            span: sp,
        };
        assert_eq!(module.items.len(), 1);
        match &module.items[0] {
            Item::Function(f) => assert_eq!(f.name, "main"),
            _ => panic!("expected a function"),
        }
    }

    #[test]
    fn binop_is_copy() {
        let op = BinOp::FloorDiv;
        let also = op; // Copy
        assert_eq!(op, also);
    }
}
