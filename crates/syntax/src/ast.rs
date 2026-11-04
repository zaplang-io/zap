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

/// A top-level item. The parser represents bare statements here; module
/// linking rejects executable ones (D-0090), while allowing a module docstring.
#[derive(Clone, Debug)]
pub enum Item {
    Function(Function),
    Struct(StructDef),
    Interface(InterfaceDef),
    Enum(EnumDef),
    /// A transparent type alias `type Name = <type>` (+ generic form
    /// `type Name[T] = <type>`, #606 / D-0105). The alias is *not* a new nominal
    /// type: it resolves to its target everywhere a type is resolved, with the
    /// same identity (struct/enum already cover nominal types). `type` is a soft
    /// keyword recognized only at item start.
    TypeAlias(TypeAliasDef),
    Annotation(AnnotationDef),
    Decorator(DecoratorDef),
    Import(Import),
    Stmt(Stmt),
    /// A compile-time `when target.*:` conditional-compilation block in
    /// declaration/import position (I#139, D-0079). The arm whose predicate
    /// matches the active build target is spliced into the enclosing item list
    /// during cfg resolution ([`crate::cfg::resolve_cfg`]); the other arms are
    /// parsed (so they must be syntactically valid) but never name-resolved,
    /// type-checked, or lowered against the wrong target ("dead-arm skip").
    Cfg(CfgItemBlock),
}

// ----------------------------------------------------------------------------
// Conditional compilation (`when target.*`, I#139 / D-0079)
// ----------------------------------------------------------------------------

/// A single `when`/`elif` predicate over the build target (I#139). Only the
/// closed v1 surface is representable: an architecture or OS equality, or a
/// compile-time feature probe. Boolean combinators are intentionally absent
/// (deferred); each arm carries exactly one atomic predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CfgPredicate {
    /// `target.arch == .<arch>` — true when the active target's architecture is
    /// `arch`.
    Arch(CfgArch),
    /// `target.os == .<os>` — true when the active target's OS is `os`.
    Os(CfgOs),
    /// `target.has(.<feature>)` — true when `feature` was baked into the build
    /// (via `--target-feature`). This is the **compile-time** feature probe,
    /// distinct from the (later) runtime `cpu.has` (§4/§6.3).
    HasFeature(String),
}

/// The closed v1 set of target architectures (design/0032 §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfgArch {
    X86_64,
    Aarch64,
}

impl CfgArch {
    /// The canonical spelling used after the `.` in `target.arch == .<arch>`.
    pub fn as_str(self) -> &'static str {
        match self {
            CfgArch::X86_64 => "x86_64",
            CfgArch::Aarch64 => "aarch64",
        }
    }
}

/// The closed v1 set of target operating systems (design/0032 §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CfgOs {
    Linux,
    Windows,
    Macos,
}

impl CfgOs {
    /// The canonical spelling used after the `.` in `target.os == .<os>`.
    pub fn as_str(self) -> &'static str {
        match self {
            CfgOs::Linux => "linux",
            CfgOs::Windows => "windows",
            CfgOs::Macos => "macos",
        }
    }
}

/// One `when`/`elif` arm: a predicate and the body it guards.
#[derive(Clone, Debug)]
pub struct CfgItemArm {
    pub predicate: CfgPredicate,
    pub body: Vec<Item>,
    pub span: Span,
}

/// A `when target.*:` block in item position (D-0079). Holds every arm in
/// source order plus the optional `else` body; cfg resolution keeps the first
/// matching arm (or the `else`) and discards the rest.
#[derive(Clone, Debug)]
pub struct CfgItemBlock {
    pub arms: Vec<CfgItemArm>,
    pub else_body: Option<Vec<Item>>,
    /// Span of the `else` keyword when `else_body` is present (used by the
    /// formatter to place the `else:` line and any comments before it).
    pub else_span: Option<Span>,
    pub span: Span,
}

/// One `when`/`elif` arm in statement position.
#[derive(Clone, Debug)]
pub struct CfgStmtArm {
    pub predicate: CfgPredicate,
    pub body: Block,
    pub span: Span,
}

/// A `when target.*:` block in statement position (D-0079).
#[derive(Clone, Debug)]
pub struct CfgStmtBlock {
    pub arms: Vec<CfgStmtArm>,
    pub else_body: Option<Block>,
    /// Span of the `else` keyword when `else_body` is present.
    pub else_span: Option<Span>,
    pub span: Span,
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

/// A generic type parameter (D-0017/I#154), e.g. the `T` in `struct Box[T]` or
/// the `T: Ordered` in `def max_of[T: Ordered](...)`. The optional interface
/// `bound` is parsed and carried now so the grammar round-trips; bound
/// *enforcement* is deferred to a later slice (Fork B), so the bound is unused
/// by the first vertical.
#[derive(Clone, Debug)]
pub struct TypeParam {
    pub name: String,
    pub bound: Option<Type>,
    pub span: Span,
}

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
    pub generics: Vec<TypeParam>,
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
    /// `true` for a `V **name` keyword-collector parameter (#601): it collects
    /// any extra keyword arguments into a `dict[str, V]`, where `V` is the
    /// declared value type held in `ty`. At most one, always the last parameter.
    pub kwargs: bool,
    /// `true` when this parameter follows the bare `*` separator and is therefore
    /// keyword-only (I#496): it may only be supplied by name at a call site.
    pub keyword_only: bool,
    /// `true` when this parameter precedes the bare `/` separator and is therefore
    /// positional-only (#602): it may only be supplied positionally at a call
    /// site, never by name. Mirror of [`Param::keyword_only`].
    pub positional_only: bool,
    pub span: Span,
}

/// A transparent type alias (#606 / D-0105), e.g. `type Row = (int64, str)`
/// or the generic form `type Pair[T] = (T, T)`. The alias introduces no new
/// nominal identity: every reference to `name[args…]` resolves to `target` with
/// the type parameters substituted, recursively, so an alias and its target are
/// fully interchangeable. Generic aliases are expanded during monomorphization
/// alongside the existing D-0017 generics.
#[derive(Clone, Debug)]
pub struct TypeAliasDef {
    pub name: String,
    /// Generic type parameters, e.g. the `[T]` in `type Pair[T] = (T, T)`.
    /// Empty for a plain alias.
    pub generics: Vec<TypeParam>,
    /// The aliased (target) type the name expands to.
    pub target: Type,
    pub span: Span,
}

/// A struct definition (D-0015).
#[derive(Clone, Debug)]
pub struct StructDef {
    pub decorators: Vec<Decorator>,
    pub name: String,
    pub generics: Vec<TypeParam>,
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
    /// C-style bitfield width in bits (I#173, D-0068): the `3` in
    /// `int64 kind : 3`. `Some(n)` packs this field into `n` bits of the value
    /// struct's backing integer; `None` is an ordinary full-width field.
    /// Only valid on `value struct` fields of a fixed-width integer, `bool`, or
    /// tier-1 `enum` type (enforced in sema).
    pub bits: Option<u32>,
    /// Literal field default (D-0038); computed defaults use `@default(...)`.
    pub default: Option<Expr>,
    pub span: Span,
}

/// A structural interface (D-0016).
#[derive(Clone, Debug)]
pub struct InterfaceDef {
    pub name: String,
    pub generics: Vec<TypeParam>,
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
    pub generics: Vec<TypeParam>,
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
///
/// A decorator is either **declared-only** (an empty `pass` body — `members` is
/// empty — which attaches reflection metadata, D-0087) or **behavioral** (it
/// declares one or more `before`/`after`/`wrap` [`DecoratorMember`]s whose code
/// the decorator-expansion pass generates around/for the decorated target).
#[derive(Clone, Debug)]
pub struct DecoratorDef {
    pub name: String,
    /// The decorator's own arguments, e.g. the `int64 times` in
    /// `decorator retry(int64 times)`. Ordinary type-first parameters.
    pub params: Vec<Param>,
    /// `before`/`after`/`wrap` member hooks declared in the body, in source
    /// order. Empty for a declared-only (`pass`) metadata decorator.
    pub members: Vec<DecoratorMember>,
    /// The span of the indented body (`:` through the end of the last body
    /// line), used by the formatter for blank-line/trivia accounting.
    pub body_span: Span,
    pub span: Span,
}

/// A `before`/`after`/`wrap` member hook of a behavioral decorator (D-0036).
///
/// Unlike ordinary functions, a member's parameters (`ctx`, `result`, `call`)
/// are **untyped macro bindings** substituted during decorator expansion, not
/// type-first Zap parameters — so they are carried as plain names.
#[derive(Clone, Debug)]
pub struct DecoratorMember {
    /// The hook name as written: `before`, `after`, `wrap`, or any other name
    /// (an unknown member is a stable diagnostic during expansion).
    pub name: String,
    /// Untyped macro-parameter names, in order (`ctx`; `ctx, result`; `call`).
    pub params: Vec<String>,
    /// The hook body statements.
    ///
    /// - For a `before`/`after` hook: the hook body itself.
    /// - For a `wrap(call)` hook (#653): the body of the nested
    ///   `def inner(...)`; the inner signature and the `return inner` trailer
    ///   are carried separately in [`DecoratorMember::wrap`].
    ///
    /// `None` only for a malformed member whose body failed to parse.
    pub body: Option<Block>,
    /// Present only for a `wrap(call)` hook (#653): the nested
    /// `def inner(...)` signature plus the `wrap`-body statements that follow
    /// it (expected to be exactly `return inner`). The inner function's *body*
    /// lives in [`DecoratorMember::body`].
    pub wrap: Option<WrapForm>,
    pub span: Span,
}

/// The structural shape of a `wrap(call)` decorator hook (D-0036, #653):
///
/// ```text
/// def wrap(call):
///     def inner(*args):        # inner_name = "inner", inner_params = [*args]
///         ...                  # -> DecoratorMember::body
///     return inner             # -> trailer
/// ```
///
/// Validated during expansion: `inner_params` must be a single `*args`
/// forwarding parameter and `trailer` must be exactly `return <inner_name>`.
#[derive(Clone, Debug)]
pub struct WrapForm {
    /// The nested function's name, e.g. `inner`.
    pub inner_name: String,
    /// The nested function's parameters as written (the `*args` forwarder).
    pub inner_params: Vec<WrapParam>,
    /// The statements in the `wrap` body *after* the nested `def` — expected to
    /// be exactly `return <inner_name>`.
    pub trailer: Vec<Stmt>,
    pub span: Span,
}

/// A parameter of a `wrap` hook's nested `def inner(...)` (#653). These are
/// untyped macro bindings (`*args`), not type-first Zap parameters.
#[derive(Clone, Debug)]
pub struct WrapParam {
    pub name: String,
    /// `true` for a `*name` positional forwarder.
    pub variadic: bool,
    /// `true` for a `**name` keyword forwarder.
    pub kwargs: bool,
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
    /// Chained assignment `t0 = t1 = ... = value` (#594). The right-hand `value`
    /// is evaluated exactly once and assigned to every target left to right
    /// (Python semantics). `targets` holds the two-or-more lvalues; each is an
    /// ordinary single-assignment target (a name, a subscript, or a field) and,
    /// like a plain `Assign`, must already be a declared binding — chained
    /// assignment never introduces a new binding (declarations stay type-first /
    /// `auto`, D-0003).
    AssignChain {
        targets: Vec<Expr>,
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
    /// A compile-time `when target.*:` conditional-compilation block in
    /// statement position (I#139, D-0079). Resolved exactly like the item-level
    /// form: the matching arm's statements are spliced into the enclosing block
    /// and the dead arms are skipped (parsed, never checked/lowered).
    Cfg(CfgStmtBlock),
    /// `return [expr]`.
    Return {
        value: Option<Expr>,
        span: Span,
    },
    /// `raise [expr [ "from" cause ]]` (D-0007, D-0103). `cause` is the explicit
    /// `from <cause>` chaining operand, recorded on the raised exception's
    /// `cause` slot and rendered as a two-section traceback (#604, epic #614 D12).
    Raise {
        value: Option<Expr>,
        cause: Option<Expr>,
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
    /// `pass` — a no-op statement (#591). Accepted anywhere a statement is
    /// expected and lowered to nothing (no IR). `pass` is a contextual
    /// statement-position word, not a reserved keyword, so it remains usable as
    /// an ordinary identifier in every other position.
    Pass(Span),
    /// `del d[k]` / `del xs[i]` — element removal (#595). `target` is always an
    /// [`Expr::Index`]: this statement is scoped to subscript targets (dict key
    /// removal and list element removal) and never deletes a binding. Like
    /// `pass`, `del` is a contextual statement-position word, not a reserved
    /// keyword, so `del` stays usable as an ordinary identifier everywhere else.
    Del {
        target: Expr,
        span: Span,
    },
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

/// How a call argument is spread into the callee's parameters (#597).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgSpread {
    /// An ordinary positional or keyword argument.
    None,
    /// `*expr` — unpack an iterable/list/tuple into positional arguments.
    Iterable,
    /// `**expr` — unpack a `dict[str, V]` into keyword arguments.
    Mapping,
}

/// A call argument; `name` is set for keyword arguments (D-0038). A `*expr` /
/// `**expr` spread argument (#597) carries `spread != None` and has no `name`.
#[derive(Clone, Debug)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
    pub spread: ArgSpread,
    pub span: Span,
}

/// An entry in a dict display (#598). A `**expr` merge has no key, so it cannot
/// be a [`Pair`](DictItem::Pair) and gets its own variant.
#[derive(Clone, Debug)]
pub enum DictItem {
    /// `key: value`.
    Pair(Expr, Expr),
    /// `**expr` — merge another `dict[K, V]`'s entries, later entries winning
    /// (#598).
    Spread(Expr),
}

/// One `for TARGET in ITER` clause of a comprehension together with the `if`
/// filters that immediately follow it (#599). Each filter constrains the
/// clauses to its left: it is evaluated inside this clause's loop, before the
/// next clause runs. A single-clause comprehension has one entry with at most
/// one filter, matching the pre-#599 shape.
#[derive(Clone, Debug)]
pub struct CompClause {
    pub var: String,
    pub iter: Box<Expr>,
    pub ifs: Vec<Expr>,
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
        /// Monomorphized replacement-field expressions, in source order, for an
        /// f-string (`fstring == true`). Empty everywhere else, and empty until
        /// monomorphization fills it (#498): the parser leaves f-string fields
        /// inside `value`, and sema/lowering re-split them. Because f-string
        /// field expressions are re-parsed at lowering time they are invisible
        /// to the monomorphization pass, so inline generics/`dict.get` in a
        /// field would never be specialized; monomorphization records the
        /// rewritten field expressions here for lowering to use instead of the
        /// freshly-parsed ones.
        fstring_fields: Vec<Expr>,
    },
    Bool(bool, Span),
    None(Span),

    Name(String, Span),

    Tuple(Vec<Expr>, Span),
    List(Vec<Expr>, Span),
    Dict(Vec<DictItem>, Span),
    /// `{a, b, c}` — a set literal: braces with comma-separated elements and no
    /// `key: value` pairs (empty `{}` parses as an empty [`Expr::Dict`]).
    Set(Vec<Expr>, Span),

    /// `*expr` spliced into a list/set/tuple display (#598): the operand's
    /// elements are spread in place. Legal only as a direct element of a
    /// list/set/tuple display; sema/lowering reject it elsewhere. Dict `**expr`
    /// merges are carried by [`DictItem::Spread`] instead (no key to attach to).
    Starred {
        value: Box<Expr>,
        span: Span,
    },

    /// `[elem for CLAUSES]` where `CLAUSES` is one or more `for` clauses with
    /// interleaved `if` filters (#599). `clauses` is non-empty and its first
    /// entry is the leading `for`.
    ListComp {
        elem: Box<Expr>,
        clauses: Vec<CompClause>,
        span: Span,
    },
    /// `{key: value for CLAUSES}` (#599).
    DictComp {
        key: Box<Expr>,
        value: Box<Expr>,
        clauses: Vec<CompClause>,
        span: Span,
    },
    /// `{elem for CLAUSES}` (#599).
    SetComp {
        elem: Box<Expr>,
        clauses: Vec<CompClause>,
        span: Span,
    },
    /// `(elem for CLAUSES)` — a bare **generator expression** (#596, epic #614
    /// D4). Unlike a list/set/dict comprehension it does not materialize a
    /// collection: it evaluates to a lazy `Iterator` (D-0048) that yields each
    /// `elem` on demand, reusing the generator state-machine machinery (I#67).
    /// `clauses` is non-empty and its first entry is the leading `for`, exactly
    /// like a comprehension (#599). The surrounding parentheses are part of the
    /// form — a genexpr is only produced in a parenthesized position, and is
    /// distinct from a parenthesized expression `(expr)` and a tuple `(a, b)`.
    GenExp {
        elem: Box<Expr>,
        clauses: Vec<CompClause>,
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

    /// Chained comparison `x0 op1 x1 op2 x2 ... opn xn` with Python semantics
    /// (I#386): equivalent to `(x0 op1 x1) and (x1 op2 x2) and ...`, where each
    /// operand is evaluated exactly once, left to right, and evaluation stops at
    /// the first link that does not hold (so a later operand with side effects
    /// does not run). The parser only builds this when two or more comparison
    /// operators appear at the same level without parentheses; a single
    /// comparison stays an [`Expr::Binary`]. `rest` is always non-empty with at
    /// least two links.
    Compare {
        first: Box<Expr>,
        rest: Vec<(BinOp, Expr)>,
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

    /// `(name := expr)` — a named (walrus) assignment-expression (#600, epic
    /// #614 D8). It evaluates `value` exactly once, binds `name` by inference
    /// (like `auto`) into the lexically **enclosing** scope — so the name is
    /// usable after the `if`/`while`/comprehension in which the walrus appears,
    /// matching Python's PEP 572 enclosing-scope rule — and itself evaluates to
    /// the value. The parser only produces it inside a parenthesized position
    /// (as part of a condition or a comprehension filter); a bare statement
    /// `a := 10` is rejected in favour of `auto a = 10` (D-0003).
    Named {
        name: String,
        name_span: Span,
        value: Box<Expr>,
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
    NotIn,    // not in  (negated membership, #593)
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
        NotIn => "not in",
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
        Expr::Starred { value, .. } => format!("*{}", render_expr(value)),
        Expr::Dict(items, _) => {
            let body = items
                .iter()
                .map(|it| match it {
                    DictItem::Pair(k, v) => format!("{}: {}", render_expr(k), render_expr(v)),
                    DictItem::Spread(e) => format!("**{}", render_expr(e)),
                })
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
        Expr::Named { name, value, .. } => format!("({name} := {})", render_expr(value)),
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
