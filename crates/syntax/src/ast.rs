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

/// A tier-1 enum: named constants with optional explicit values (D-0018).
#[derive(Clone, Debug)]
pub struct EnumDef {
    pub decorators: Vec<Decorator>,
    pub name: String,
    pub variants: Vec<EnumVariant>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumVariant {
    pub name: String,
    pub value: Option<Expr>,
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
    /// `spawn call(...)` (D-0013).
    Spawn {
        call: Expr,
        span: Span,
    },
    Break(Span),
    Continue(Span),
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
    /// `Point(x=0, y=y)`
    Struct {
        name: String,
        fields: Vec<(String, Pattern)>,
        span: Span,
    },
    /// `*rest` inside a sequence pattern.
    Rest { name: Option<String>, span: Span },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

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
