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
use zapc_syntax::genanalysis;
use zapc_syntax::span::Span;

/// Built-in stdlib module registry (ticket #121): per-module type-checking lives
/// in `modules/<name>.rs`, dispatched by a data table instead of a hand-written
/// `match (module, method)` block, so new modules are conflict-free parallel-
/// addable. See [`modules`] for the seam contract.
mod modules;

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
    /// A built-in `heap[T]` priority queue (D-0057): a min-heap by default, with
    /// an optional `cmp=` selecting max/custom ordering. A GC-managed reference
    /// type, joining `list`/`dict`/`set`/`array`.
    Heap(Box<Ty>),
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
            Ty::Heap(t) => format!("heap[{}]", t.display()),
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
        (Ty::Heap(a), Ty::Heap(b)) => assignable(a, b),
        (Ty::Dict(ak, av), Ty::Dict(bk, bv)) => assignable(ak, bk) && assignable(av, bv),
        // A tuple is assignable to a tuple of the same arity when each element is
        // (covariantly) assignable — this covers returning untyped literals into
        // a declared tuple type (`-> (int64, int64)` with `return 1, 2`).
        (Ty::Tuple(a), Ty::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| assignable(x, y))
        }
        _ => false,
    }
}

/// Does a concrete struct method signature `have` match what an interface
/// requires (`want`) for structural conformance (D-0016)? The receiver (`self`,
/// parameter 0) is ignored — it is the differing concrete/interface type — so
/// the match compares the remaining parameter types pairwise and the return
/// type. Comparison is by exact type identity (no widening): an interface method
/// is satisfied by a method with the very same signature.
fn sigs_match(have: &FnSig, want: &FnSig) -> bool {
    let hp = &have.params.get(1..).unwrap_or(&[]);
    let wp = &want.params.get(1..).unwrap_or(&[]);
    hp.len() == wp.len()
        && hp.iter().zip(wp.iter()).all(|(a, b)| a.ty == b.ty)
        && have.ret == want.ret
}

/// A nameless, non-defaulted, non-variadic parameter of the given type — used to
/// synthesize `@derive`d dunder signatures (D-0047).
fn param(ty: Ty) -> ParamInfo {
    ParamInfo {
        name: String::new(),
        ty,
        has_default: false,
        variadic: false,
    }
}

/// The compiler-known structural operator interfaces for arithmetic / bitwise /
/// shift operators (D-0047): each maps to the dunder a user struct defines and
/// the interface name that provides it. Comparison (`Eq`/`Ord`), membership
/// (`Contains`), size (`Len`), truthiness (`Bool`), indexing (`Index`), call
/// (`Call`), and repr (`Str`/`Repr`) are handled separately. The result of one
/// of these operators is typed from the dunder's own return type.
fn binary_op_dunder(op: BinOp) -> Option<(&'static str, &'static str)> {
    use BinOp::*;
    Some(match op {
        Add => ("__add__", "Add"),
        Sub => ("__sub__", "Sub"),
        Mul => ("__mul__", "Mul"),
        Div => ("__truediv__", "TrueDiv"),
        FloorDiv => ("__floordiv__", "FloorDiv"),
        Mod => ("__mod__", "Mod"),
        Pow => ("__pow__", "Pow"),
        BitAnd => ("__and__", "BitAnd"),
        BitOr => ("__or__", "BitOr"),
        BitXor => ("__xor__", "BitXor"),
        Shl => ("__lshift__", "LShift"),
        Shr => ("__rshift__", "RShift"),
        // Equality/relational/membership/logical/send are not arithmetic dunders.
        Eq | Ne | Lt | Le | Gt | Ge | And | Or | In | Send => return None,
    })
}

/// The in-place augmented-assignment dunder for an operator (D-0047): if the
/// LHS type of `a OP= b` defines this, the assignment calls `a.__iOP__(b)`
/// (mutating in place, D-0023) instead of deriving `a = a.__OP__(b)` from the
/// binary operator. Only the operators that can head an augmented assignment
/// have one. Its interface mirrors the binary one (`Add` -> `IAdd`, etc.).
fn inplace_op_dunder(op: BinOp) -> Option<&'static str> {
    use BinOp::*;
    Some(match op {
        Add => "__iadd__",
        Sub => "__isub__",
        Mul => "__imul__",
        Div => "__itruediv__",
        FloorDiv => "__ifloordiv__",
        Mod => "__imod__",
        Pow => "__ipow__",
        BitAnd => "__iand__",
        BitOr => "__ior__",
        BitXor => "__ixor__",
        Shl => "__ilshift__",
        Shr => "__irshift__",
        Eq | Ne | Lt | Le | Gt | Ge | And | Or | In | Send => return None,
    })
}

/// The source symbol for an operator, for diagnostics.
fn binop_symbol(op: BinOp) -> &'static str {
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

/// A registered struct type. Carries just enough to check operator overloading
/// (D-0047): the method signatures, including the dunders `@derive(...)`
/// generates, so an operator's result is typed from its dunder and the compiler
/// can warn on an `Eq`-without-`Hash` dict/set key.
#[derive(Clone)]
struct StructInfo {
    /// Source method name -> signature (`self` is the first parameter).
    methods: HashMap<String, FnSig>,
    /// `true` for a `value struct` (D-0051): value semantics (copied on
    /// assign/pass/return, no shared identity), laid out inline. A plain
    /// `struct` is a reference type (D-0023) with `is_value == false`.
    is_value: bool,
}

impl StructInfo {
    /// Does this struct provide `dunder`, either hand-written or `@derive`d?
    fn has(&self, dunder: &str) -> bool {
        self.methods.contains_key(dunder)
    }
}

/// A registered structural interface (D-0016). The method set is the interface's
/// declared signatures, in declaration order (which is the dispatch-table slot
/// order a conforming struct is checked against and lowered to).
#[derive(Clone)]
struct InterfaceInfo {
    /// Interface method name -> signature (`self` is the first parameter, typed
    /// as the interface itself), in declaration order.
    methods: Vec<(String, FnSig)>,
}

impl InterfaceInfo {
    fn method(&self, name: &str) -> Option<&FnSig> {
        self.methods.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }
}

/// Type-check a module, returning any errors.
pub fn check(module: &Module) -> Vec<TypeError> {
    analyze(module).0
}

/// Type-check a module, returning `(errors, warnings)`. Warnings are non-fatal
/// diagnostics (e.g. the `Eq`-without-`Hash` dict/set key warning, D-0047).
pub fn analyze(module: &Module) -> (Vec<TypeError>, Vec<TypeError>) {
    // Prepend the built-in prelude (`Signal` enum, `MemStats` struct) so the
    // signals surface reuses ordinary enum/struct type-checking (ticket #77).
    let module = &zapc_syntax::parser::with_prelude(module);
    let mut c = Checker::new();
    c.collect_types(module);
    c.check_value_struct_recursion(module);
    c.collect_sigs(module);
    c.validate_decorators(module);
    c.check_module(module);
    (c.errors, c.warnings)
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

/// Convenience for tests: lex, parse, and type-check `src`, returning the
/// non-fatal warnings.
pub fn warnings_str(src: &str) -> Vec<TypeError> {
    let (module, _perrs, _lerrs) = zapc_syntax::parser::parse_str(src);
    analyze(&module).1
}

struct Checker {
    sigs: HashMap<String, FnSig>,
    user_types: HashSet<String>,
    /// Registered enums (tier-1 and tier-3), keyed by type name.
    enums: HashMap<String, EnumInfo>,
    /// Registered structs, keyed by type name (operator overloading, D-0047).
    structs: HashMap<String, StructInfo>,
    /// Registered structural interfaces, keyed by name (D-0016). A struct is
    /// assignable to an interface type iff it structurally conforms.
    interfaces: HashMap<String, InterfaceInfo>,
    /// Declared `annotation` names -> their field names (D-0036). Used to
    /// validate `@Annotation(...)` applications (keyword args must name fields).
    annotations: HashMap<String, Vec<String>>,
    /// Declared `decorator` names (behavioral, D-0036).
    user_decorators: HashSet<String>,
    scopes: Vec<HashMap<String, Ty>>,
    ret_ty: Ty,
    /// `true` while checking the body of a generator function (a free `def`
    /// whose body contains `yield`, I#67/D-0026). Gates `yield`/`return`
    /// validation.
    in_generator: bool,
    errors: Vec<TypeError>,
    /// Non-fatal diagnostics (e.g. `Eq`-without-`Hash` used as a dict/set key,
    /// D-0047). Reported by the driver but do not fail compilation.
    warnings: Vec<TypeError>,
}

const BUILTINS: &[&str] = &[
    "print",
    "range",
    "len",
    "sum",
    "reflect_has",
    "reflect_names",
    "checkpoint",
    // Internal netpoller integration-test hook (ticket #80). No public I/O
    // surface exists yet (sockets #79, signals #77), so this drives the
    // netpoller end-to-end from the runtime; removed/replaced when #79 lands.
    "__netpoll_selftest",
    // Internal signal integration-test hook (ticket #77): drives the self-pipe +
    // netpoller + dispatch + cap-1 coalescing end-to-end from the runtime before
    // the full `signal` surface is exercised in a program.
    "__signal_selftest",
    // Internal test hooks (ticket #77): send a signal to this process, and block
    // until the signal thread has dispatched everything raised so far, so the
    // signal goldens are deterministic. Not public surface.
    "__raise_signal",
    "__signal_drain",
    // Internal finalizer integration-test hook (ticket #92): exercises the
    // precise collector's register + post-mark finalizer queue end-to-end with a
    // stand-in finalizer and returns the number that ran. Not public surface.
    "__gc_finalizer_selftest",
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
    // Signals (ticket #77, D-0058): `@on_signal(Signal.X)` on a function
    // registers it as a handler for signal X (sugar over `signal.notify`).
    "on_signal",
    // Test framework (D-0054, issue #73). These mark functions for `zap test`;
    // normal builds strip the annotated functions, so they never reach codegen.
    "test",
    "before",
    "after",
    "before_all",
    "after_all",
    "parametrize",
    "skip",
    "skip_if",
    "expect_raises",
];

/// The `@test`-suite decorators (D-0054). A function carrying any of these is a
/// test or a lifecycle hook: it is compiled only under `zap test` and excluded
/// from release binaries.
pub const TEST_DECORATORS: &[&str] = &[
    "test",
    "before",
    "after",
    "before_all",
    "after_all",
    "parametrize",
    "skip",
    "skip_if",
    "expect_raises",
];

/// Does `f` carry any `@test`-suite decorator (so it belongs to the test
/// harness, not a release binary)?
pub fn is_test_item(f: &zapc_syntax::ast::Function) -> bool {
    f.decorators
        .iter()
        .any(|d| TEST_DECORATORS.contains(&d.name.as_str()))
}

/// The traits `@derive(...)` can generate (D-0036). Only `Eq` and `Repr` are
/// lowered to code in this increment; the rest are accepted here and reported as
/// deferred by the lowering pass.
const DERIVABLE_TRAITS: &[&str] = &["Eq", "Ord", "Json", "Csv", "Yaml", "Repr", "Hash"];

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
            structs: HashMap::new(),
            interfaces: HashMap::new(),
            annotations: HashMap::new(),
            user_decorators: HashSet::new(),
            scopes: Vec::new(),
            ret_ty: Ty::Unit,
            in_generator: false,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn error(&mut self, message: impl Into<String>, span: Span) {
        self.errors.push(TypeError {
            message: message.into(),
            span,
        });
    }

    fn warn(&mut self, message: impl Into<String>, span: Span) {
        self.warnings.push(TypeError {
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
        // Second pass: build the enum and struct registries now that all type
        // names exist (fields/methods may reference a type declared later).
        for item in &module.items {
            match item {
                Item::Enum(e) => {
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
                Item::Struct(s) => {
                    let info = self.struct_info_of(s);
                    self.structs.insert(s.name.clone(), info);
                }
                Item::Interface(i) => {
                    let info = self.interface_info_of(i);
                    self.interfaces.insert(i.name.clone(), info);
                }
                _ => {}
            }
        }
    }

    /// Build an interface's [`InterfaceInfo`] (D-0016): resolve each declared
    /// method signature, in declaration order. `self` is typed as the interface
    /// itself; the remaining parameter and return types are resolved normally.
    fn interface_info_of(&self, i: &InterfaceDef) -> InterfaceInfo {
        let methods = i
            .methods
            .iter()
            .map(|m| (m.name.clone(), self.sig_of_method_sig(m, &i.name)))
            .collect();
        InterfaceInfo { methods }
    }

    /// Resolve an interface [`MethodSig`] to a [`FnSig`] (its `self` typed as the
    /// enclosing interface `iname`).
    fn sig_of_method_sig(&self, m: &MethodSig, iname: &str) -> FnSig {
        let params = m
            .params
            .iter()
            .map(|p| {
                let ty = if p.name == "self" {
                    Ty::Named(iname.to_string())
                } else {
                    p.ty.as_ref()
                        .map(|t| self.resolve_type_self(t, iname))
                        .unwrap_or(Ty::Unknown)
                };
                ParamInfo {
                    name: p.name.clone(),
                    ty,
                    has_default: false,
                    variadic: p.variadic,
                }
            })
            .collect();
        let ret = m
            .ret
            .as_ref()
            .map(|t| self.resolve_type_self(t, iname))
            .unwrap_or(Ty::Unit);
        FnSig { params, ret }
    }

    /// Is `name` a declared structural interface (D-0016)?
    fn is_interface(&self, name: &str) -> bool {
        self.interfaces.contains_key(name)
    }

    /// Does a value of type `from` satisfy where `to` is expected, allowing
    /// structural interface conformance (D-0016) in addition to ordinary
    /// [`assignable`] widening? A concrete struct conforms to an interface iff it
    /// provides every interface method with a matching signature.
    fn coercible(&self, from: &Ty, to: &Ty) -> bool {
        if assignable(from, to) {
            return true;
        }
        match (from, to) {
            // A struct value used where an interface is expected: structural
            // conformance (the concrete -> interface fat-pointer conversion,
            // D-0016). A `value struct` conforms too — on conversion it is boxed
            // (copied to the heap) so the fat pointer's `data` word holds a
            // stable, GC-traced pointer (I#75 Part 1, D-0051); the boxing is
            // emitted in lowering.
            (Ty::Named(s), Ty::Named(i)) if self.is_interface(i) => self.conforms(s, i).is_ok(),
            _ => false,
        }
    }

    /// Check that struct `sname` structurally conforms to interface `iname`
    /// (D-0016): every interface method is present with a matching signature
    /// (parameter types excluding `self`, and return type). On failure, returns
    /// the list of human-readable reasons (missing / mismatched methods) for a
    /// precise diagnostic. A non-struct `sname` (e.g. not yet registered) simply
    /// fails to conform.
    fn conforms(&self, sname: &str, iname: &str) -> Result<(), Vec<String>> {
        let Some(iface) = self.interfaces.get(iname) else {
            return Err(vec![format!("'{iname}' is not an interface")]);
        };
        let Some(st) = self.structs.get(sname) else {
            return Err(vec![format!("'{sname}' is not a struct")]);
        };
        let mut reasons = Vec::new();
        for (mname, isig) in &iface.methods {
            match st.methods.get(mname) {
                None => reasons.push(format!("missing method '{mname}'")),
                Some(msig) => {
                    if !sigs_match(msig, isig) {
                        reasons.push(format!("method '{mname}' has a mismatched signature"));
                    }
                }
            }
        }
        if reasons.is_empty() {
            Ok(())
        } else {
            Err(reasons)
        }
    }

    /// Build a struct's [`StructInfo`] (operator overloading, D-0047): resolve
    /// fields and method signatures, collect `@derive`d traits, and synthesize
    /// the signatures of the dunders those derives will generate.
    fn struct_info_of(&self, s: &StructDef) -> StructInfo {
        let mut methods: HashMap<String, FnSig> = HashMap::new();
        for m in &s.methods {
            methods.insert(m.name.clone(), self.sig_of_method(m, &s.name));
        }
        let mut derived: HashSet<String> = HashSet::new();
        for d in &s.decorators {
            if d.name == "derive" {
                for a in &d.args {
                    if let Expr::Name(t, _) = &a.value {
                        derived.insert(t.clone());
                    }
                }
            }
        }
        // Dunders `@derive(...)` generates (D-0047) — registered so an operator
        // on a derived type resolves and is typed, unless the user hand-wrote the
        // method (then their signature wins).
        let named = Ty::Named(s.name.clone());
        let unary = |ret: Ty| FnSig {
            params: vec![param(named.clone())],
            ret,
        };
        let binary = |ret: Ty| FnSig {
            params: vec![param(named.clone()), param(named.clone())],
            ret,
        };
        if derived.contains("Eq") {
            methods
                .entry("__eq__".into())
                .or_insert_with(|| binary(Ty::Bool));
        }
        if derived.contains("Ord") {
            methods
                .entry("__cmp__".into())
                .or_insert_with(|| binary(Ty::Int64));
        }
        if derived.contains("Hash") {
            methods
                .entry("__hash__".into())
                .or_insert_with(|| unary(Ty::Int64));
        }
        if derived.contains("Repr") {
            methods
                .entry("__repr__".into())
                .or_insert_with(|| unary(Ty::Str));
        }
        if derived.contains("Json") {
            // Compile-time JSON codec (D-0059, I#100): `self.to_json() -> str`.
            // The decode direction, `Struct.from_json(s)`, is an associated
            // function resolved in `check_call` (keyed on `to_json` existing).
            methods
                .entry("to_json".into())
                .or_insert_with(|| unary(Ty::Str));
        }
        if derived.contains("Csv") {
            // Compile-time CSV record codec (D-0059 §4b, I#132):
            // `self.to_csv() -> str`; `Struct.from_csv(s)` is the associated
            // decode function (resolved in `check_call`, keyed on `to_csv`).
            methods
                .entry("to_csv".into())
                .or_insert_with(|| unary(Ty::Str));
        }
        if derived.contains("Yaml") {
            // Compile-time YAML codec (D-0063, I#149): `self.to_yaml() -> str`;
            // `Struct.from_yaml(s)` is the associated decode function (resolved
            // in `check_call`, keyed on `to_yaml`).
            methods
                .entry("to_yaml".into())
                .or_insert_with(|| unary(Ty::Str));
        }
        StructInfo {
            methods,
            is_value: s.is_value,
        }
    }

    /// Is `name` a `value struct` (value type, D-0051)?
    fn is_value_struct(&self, name: &str) -> bool {
        self.structs.get(name).is_some_and(|s| s.is_value)
    }

    /// A `value struct` is laid out inline (D-0051), so it cannot contain itself
    /// by value — directly or through a chain of value-struct fields — as that
    /// would be an infinitely large aggregate. (A plain/reference `struct` field,
    /// a `list`/`dict`/`set`, or any other pointer-backed type breaks the chain,
    /// so recursive *reference* types remain fine, D-0023.) Report a clear error
    /// on each value struct that lies on such a cycle.
    fn check_value_struct_recursion(&mut self, module: &Module) {
        // Direct value-struct → value-struct field edges (by type name), with the
        // field span for diagnostics.
        let mut edges: HashMap<String, Vec<(String, Span)>> = HashMap::new();
        for item in &module.items {
            if let Item::Struct(s) = item {
                if !s.is_value {
                    continue;
                }
                let mut es = Vec::new();
                for f in &s.fields {
                    if let Type::Named { name, .. } = &f.ty {
                        if self.is_value_struct(name) {
                            es.push((name.clone(), f.span));
                        }
                    }
                }
                edges.insert(s.name.clone(), es);
            }
        }
        // DFS from each value struct; a back-edge to a node on the current stack
        // is a cycle. Report once per struct that participates in one.
        let mut reported: HashSet<String> = HashSet::new();
        let roots: Vec<String> = edges.keys().cloned().collect();
        for root in roots {
            let mut stack: Vec<String> = Vec::new();
            let mut on_stack: HashSet<String> = HashSet::new();
            self.detect_value_cycle(&root, &edges, &mut stack, &mut on_stack, &mut reported);
        }
    }

    fn detect_value_cycle(
        &mut self,
        node: &str,
        edges: &HashMap<String, Vec<(String, Span)>>,
        stack: &mut Vec<String>,
        on_stack: &mut HashSet<String>,
        reported: &mut HashSet<String>,
    ) {
        if on_stack.contains(node) {
            if !reported.contains(node) {
                reported.insert(node.to_string());
                // Blame the field that re-enters the cycle, if we can find it.
                let span = stack
                    .last()
                    .and_then(|p| edges.get(p))
                    .and_then(|es| es.iter().find(|(n, _)| n == node))
                    .map(|(_, sp)| *sp)
                    .unwrap_or_default();
                self.error(
                    format!(
                        "value struct '{node}' cannot contain itself by value \
                         (an inline value type would be infinitely large); use a \
                         plain (reference) struct for a recursive type (D-0051)"
                    ),
                    span,
                );
            }
            return;
        }
        if reported.contains(node) {
            return;
        }
        stack.push(node.to_string());
        on_stack.insert(node.to_string());
        if let Some(es) = edges.get(node) {
            for (next, _) in es.clone() {
                self.detect_value_cycle(&next, edges, stack, on_stack, reported);
            }
        }
        stack.pop();
        on_stack.remove(node);
    }

    /// A struct method's signature, mapping the `self` receiver and any `Self`
    /// annotation to the enclosing struct type.
    fn sig_of_method(&self, f: &Function, sname: &str) -> FnSig {
        let params = f
            .params
            .iter()
            .map(|p| {
                let ty = if p.name == "self" {
                    Ty::Named(sname.to_string())
                } else {
                    p.ty.as_ref()
                        .map(|t| self.resolve_type_self(t, sname))
                        .unwrap_or(Ty::Unknown)
                };
                ParamInfo {
                    name: p.name.clone(),
                    ty,
                    has_default: p.default.is_some(),
                    variadic: p.variadic,
                }
            })
            .collect();
        let ret = f
            .ret
            .as_ref()
            .map(|t| self.resolve_type_self(t, sname))
            .unwrap_or(Ty::Unit);
        FnSig { params, ret }
    }

    /// Like [`resolve_type`], but resolves the `Self` type name to the enclosing
    /// struct (used in dunder signatures such as `__add__(self, other: Self)`).
    fn resolve_type_self(&self, t: &Type, sname: &str) -> Ty {
        if let Type::Named { name, .. } = t {
            if name == "Self" {
                return Ty::Named(sname.to_string());
            }
        }
        self.resolve_type(t)
    }

    fn collect_sigs(&mut self, module: &Module) {
        for item in &module.items {
            if let Item::Function(f) = item {
                let sig = self.sig_of(f);
                self.sigs.insert(f.name.clone(), sig);
            }
        }
        self.register_generators(module);
    }

    /// Register the compiler-synthesized iterator type for every free-function
    /// generator (I#67, D-0026/D-0048), so a call `gen(...)` types as an
    /// `Iterator` and drops into `for`/comprehensions through the #53 protocol.
    ///
    /// The synthetic type `$gen:<name>` is given the two structural `Iterator`
    /// dunders — `__iter__(self) -> Self` and comma-ok `__next__(self) ->
    /// (T, bool)` — and the generator function's return type is rewritten to it.
    /// The element type `T` is left permissive (`Unknown`) here: it is not
    /// needed to type-check the consuming side (any numeric op against it stays
    /// permissive), and the precise element type is computed by lowering from
    /// the `yield` operand types for codegen (`crates/ir`).
    fn register_generators(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Function(f) = item else { continue };
            if !genanalysis::is_generator(f) {
                continue;
            }
            // `main` and methods-with-`yield` are rejected in `check_function`;
            // do not give them an iterator type.
            if f.name == "main" {
                continue;
            }
            let gen_name = genanalysis::gen_state_name(&f.name);
            let self_param = |ty: Ty| ParamInfo {
                name: "self".to_string(),
                ty,
                has_default: false,
                variadic: false,
            };
            let mut methods = HashMap::new();
            methods.insert(
                "__iter__".to_string(),
                FnSig {
                    params: vec![self_param(Ty::Named(gen_name.clone()))],
                    ret: Ty::Named(gen_name.clone()),
                },
            );
            methods.insert(
                "__next__".to_string(),
                FnSig {
                    params: vec![self_param(Ty::Named(gen_name.clone()))],
                    ret: Ty::Tuple(vec![Ty::Unknown, Ty::Bool]),
                },
            );
            self.structs.insert(
                gen_name.clone(),
                StructInfo {
                    methods,
                    is_value: false,
                },
            );
            if let Some(sig) = self.sigs.get_mut(&f.name) {
                sig.ret = Ty::Named(gen_name);
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
                "heap" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::Heap(Box::new(el))
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
            "on_signal" => {
                // `@on_signal(Signal.X)` on a function (ticket #77, D-0058):
                // register it as a handler for signal X. Exactly one positional
                // argument, a `Signal` enum member.
                self.require_target(d, target, DecoTarget::Function);
                if positionals != 1 || keywords != 0 {
                    self.error(
                        "@on_signal(Signal.X) takes exactly one Signal argument, \
                         e.g. @on_signal(Signal.SIGINT)",
                        d.span,
                    );
                } else {
                    let ok = matches!(
                        &d.args[0].value,
                        Expr::Attr { base, name, .. }
                            if matches!(base.as_ref(), Expr::Name(n, _) if n == "Signal")
                                && self
                                    .enums
                                    .get("Signal")
                                    .is_some_and(|e| e.variant(name).is_some())
                    );
                    if !ok {
                        self.error(
                            "@on_signal expects a Signal member, e.g. @on_signal(Signal.SIGINT)",
                            d.span,
                        );
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
            // --- test framework (D-0054, issue #73) ---
            "test" | "before" | "after" | "before_all" | "after_all" => {
                self.require_target(d, target, DecoTarget::Function);
                if !d.args.is_empty() {
                    self.error(format!("@{} takes no arguments", d.name), d.span);
                }
            }
            "skip" => {
                self.require_target(d, target, DecoTarget::Function);
                // Optional single positional reason string.
                if d.args.len() > 1 || keywords != 0 {
                    self.error(
                        "@skip takes an optional reason string, e.g. @skip(\"flaky\")",
                        d.span,
                    );
                }
            }
            "skip_if" => {
                self.require_target(d, target, DecoTarget::Function);
                // `@skip_if(cond [, reason])`: a runtime condition and an
                // optional reason. The condition is evaluated in the harness, so
                // it is not type-checked here.
                if !(1..=2).contains(&positionals) || keywords != 0 {
                    self.error(
                        "@skip_if takes a condition and an optional reason, \
                         e.g. @skip_if(slow, \"CI only\")",
                        d.span,
                    );
                }
            }
            "expect_raises" => {
                self.require_target(d, target, DecoTarget::Function);
                // `@expect_raises(ErrType)`: exactly one positional naming the
                // expected exception type. Typed matching is deferred (catch-all
                // today), so the name is not resolved here.
                if positionals != 1 || keywords != 0 {
                    self.error(
                        "@expect_raises takes one exception type, e.g. @expect_raises(ValueError)",
                        d.span,
                    );
                }
            }
            "parametrize" => {
                self.require_target(d, target, DecoTarget::Function);
                // `@parametrize(cases=[...])`: a keyword `cases` list of case
                // rows. Each row is spread across the test's parameters.
                let has_cases = d.args.iter().any(|a| a.name.as_deref() == Some("cases"));
                let only_cases = d.args.iter().all(|a| a.name.as_deref() == Some("cases"));
                if !has_cases || !only_cases || d.args.len() != 1 {
                    self.error(
                        "@parametrize takes a single `cases` list, \
                         e.g. @parametrize(cases=[(1, 2, 3), (4, 5, 9)])",
                        d.span,
                    );
                } else if let Some(a) = d.args.first() {
                    if !matches!(a.value, Expr::List(..)) {
                        self.error("@parametrize(cases=...) expects a list of cases", d.span);
                    }
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
        // Generator validation (I#67, D-0026/D-0048): a free `def` whose body
        // contains `yield` is a generator and returns an `Iterator`. v1 does not
        // support generator methods, a generator `main`, or an explicit return
        // type on a generator (its return type is the synthetic iterator).
        let is_gen = genanalysis::is_generator(f);
        let saved_in_gen = std::mem::replace(&mut self.in_generator, is_gen);
        if is_gen {
            if self_ty.is_some() {
                self.error(
                    "generators as methods are not supported yet (I#67 v1); define \
                     the generator as a free function"
                        .to_string(),
                    f.span,
                );
            } else if f.name == "main" {
                self.error(
                    "the entry point 'main' cannot be a generator (it contains \
                     'yield', I#67)"
                        .to_string(),
                    f.span,
                );
            }
            if f.ret.is_some() {
                self.error(
                    "a generator must not declare a return type: calling it returns \
                     an Iterator over the yielded values (I#67, D-0048)"
                        .to_string(),
                    f.span,
                );
            }
        }
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
        self.in_generator = saved_in_gen;
        self.pop_scope();
    }

    fn check_block(&mut self, b: &Block) {
        self.push_scope();
        for st in &b.stmts {
            self.check_stmt(st);
        }
        self.pop_scope();
    }

    /// Check the options on a `scope(...)` block (D-0044). `on_error` takes the
    /// `OnError` enum (`OnError.Cancel` | `OnError.Wait`) — a typed policy so a
    /// misspelling is a compile-time error, not a silent runtime surprise. The
    /// `timeout=` deadline is parsed but still deferred (it needs the timer API,
    /// an open issue in `design/0005`), so it is rejected with a clear message.
    fn check_scope_opts(&mut self, opts: &[Arg], span: Span) {
        for a in opts {
            match a.name.as_deref() {
                Some("on_error") => {
                    if !is_on_error_policy(&a.value) {
                        self.error(
                            "scope on_error expects OnError.Cancel or OnError.Wait".to_string(),
                            expr_span(&a.value),
                        );
                    }
                }
                Some("timeout") => {
                    self.infer(&a.value);
                    self.error(
                        "scope(timeout=...) is not supported yet (the timer API is deferred)"
                            .to_string(),
                        expr_span(&a.value),
                    );
                }
                // `scope(cancel_on=[Signal.X, ...])` (ticket #77, D-0058): a
                // signal in the list cancels the scope tree. The value is a list
                // of `Signal` members.
                Some("cancel_on") => {
                    if let Expr::List(elems, _) = &a.value {
                        for e in elems {
                            let et = self.infer(e);
                            if !matches!(&et, Ty::Named(n) if n == "Signal")
                                && !matches!(et, Ty::Unknown)
                            {
                                self.error(
                                    format!(
                                        "scope cancel_on expects a list of Signal, found {}",
                                        et.display()
                                    ),
                                    expr_span(e),
                                );
                            }
                        }
                    } else {
                        self.error(
                            "scope cancel_on expects a list, e.g. \
                             cancel_on=[Signal.SIGINT, Signal.SIGTERM]"
                                .to_string(),
                            expr_span(&a.value),
                        );
                    }
                }
                // `scope(grace=<ms>)` (ticket #77): hard-exit deadline, in
                // milliseconds, measured from the first cancel_on signal.
                Some("grace") => {
                    let gt = self.infer(&a.value);
                    if !gt.is_int() && !matches!(gt, Ty::Unknown) {
                        self.error(
                            format!(
                                "scope grace must be an integer number of milliseconds, found {}",
                                gt.display()
                            ),
                            expr_span(&a.value),
                        );
                    }
                }
                Some(other) => self.error(
                    format!(
                        "scope() has no option '{other}' \
                         (expected 'on_error', 'cancel_on', or 'grace')"
                    ),
                    expr_span(&a.value),
                ),
                None => self.error(
                    "scope() options must be keyword arguments (e.g. on_error=OnError.Wait)"
                        .to_string(),
                    span,
                ),
            }
        }
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
                        if !self.coercible(&vt, &dt) {
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
                            if !self.coercible(&vt, &dt) {
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
                            // `t += v` on a user struct calls the in-place dunder
                            // (`__iadd__` etc.) when defined, else derives from the
                            // binary operator's `__add__` etc. (D-0047).
                            let r = match self.check_struct_aug_assign(*op, &tt, &vt, *span) {
                                Some(ty) => ty,
                                None => self.arith_result(*op, &tt, &vt, *span),
                            };
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
                target,
                iter,
                body,
                span,
            } => {
                // Iterating an enum *type* yields its members in declaration
                // order (`for col in Color:`, D-0018). The iterable is a type
                // name, not a value, so it is matched before inferring.
                let elem = if let Expr::Name(n, _) = iter {
                    if self.enums.contains_key(n) {
                        Ty::Named(n.clone())
                    } else {
                        let it = self.infer(iter);
                        self.iter_element(&it, *span)
                    }
                } else {
                    let it = self.infer(iter);
                    self.iter_element(&it, *span)
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
            Stmt::Return { value, span } if self.in_generator => {
                // A generator ends by falling off the end or a bare `return`
                // (D-0048/PEP 479); `return value` (which would set a
                // StopIteration value in Python) is deferred past v1.
                if let Some(e) = value {
                    self.infer(e);
                    self.error(
                        "a generator's 'return' cannot carry a value yet (I#67 v1); \
                         use a bare 'return' to end iteration"
                            .to_string(),
                        *span,
                    );
                }
            }
            Stmt::Return { value, span } => match value {
                Some(e) => {
                    let vt = self.infer(e);
                    let ret = self.ret_ty.clone();
                    if !self.coercible(&vt, &ret) {
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
                // `raise StopIteration` (the iterator exhaustion signal, D-0048)
                // names a compiler-known exception, not a user value, so it is
                // not inferred (which would flag it as an unknown name).
                if let Some(e) = value {
                    if !is_stop_iteration(e) {
                        self.infer(e);
                    }
                }
            }
            Stmt::Assert { cond, msg, .. } => {
                // `assert cond [, msg]` (D-0054). The condition is inferred for
                // its side effects / name resolution; the optional message is a
                // normal expression (conventionally a str).
                self.infer(cond);
                if let Some(m) = msg {
                    self.infer(m);
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
                    // `with expect_raises(ErrType):` (D-0054) is the one supported
                    // context manager: it names an exception type, not a value, so
                    // its callee/argument are not inferred (which would flag them
                    // as unknown names). Typed matching is deferred (D-0029).
                    if !is_expect_raises(&it.expr) {
                        self.infer(&it.expr);
                    }
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
            Stmt::Scope { opts, body, span } => {
                self.check_scope_opts(opts, *span);
                self.check_block(body);
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
            Expr::Dict(items, span) => {
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
                self.warn_struct_key(&kt, *span);
                Ty::Dict(Box::new(kt), Box::new(vt))
            }
            Expr::ListComp {
                elem,
                var,
                iter,
                cond,
                span,
            } => {
                let it = self.infer(iter);
                let e_ty = self.iter_element(&it, *span);
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
                span,
            } => {
                let it = self.infer(iter);
                let e_ty = self.iter_element(&it, *span);
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
            Expr::Set(elems, span) => {
                let mut elem = Ty::Unknown;
                for (i, el) in elems.iter().enumerate() {
                    let t = self.infer(el);
                    elem = if i == 0 { t } else { join(&elem, &t) };
                }
                self.warn_struct_key(&elem, *span);
                Ty::Set(Box::new(elem))
            }
            Expr::SetComp {
                elem,
                var,
                iter,
                cond,
                span,
            } => {
                let it = self.infer(iter);
                let e_ty = self.iter_element(&it, *span);
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
                if let Some(ty) = self.check_struct_unary(*op, &t, *span) {
                    return ty;
                }
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
                // Operator overloading on user structs (D-0047) takes precedence
                // over the built-in-type rules.
                if let Some(ty) = self.check_struct_binop(*op, &l, &r, *span) {
                    ty
                } else {
                    self.arith_result(*op, &l, &r, *span)
                }
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
                // Indexing overloading (D-0047): `a[i]` -> `a.__getitem__(i)`.
                if self.struct_info(&b).is_some() {
                    return self.op_result(&b, "__getitem__", "Index", "[]", *span);
                }
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
                    // Built-in module constant `math.pi`-style access (ticket
                    // #98, D-0056/D-0058): recognized only when the name is not
                    // shadowed by a local value, and intercepted before the base
                    // is resolved as a value (module names are not bindings).
                    if self.lookup(tyname).is_none() {
                        if let Some(ty) = modules::check_builtin_module_const(tyname, name) {
                            return ty;
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
            Expr::Yield { value, span } => {
                // `yield` is only meaningful inside a generator; detection makes
                // any free `def` containing it a generator, so an out-of-context
                // `yield` can only occur where the function itself was already
                // rejected (a method/`main`). v1 requires an operand so the
                // element type `T` is well-defined.
                match value {
                    Some(v) => {
                        self.infer(v);
                    }
                    None => self.error(
                        "'yield' requires a value in this version (I#67 v1)".to_string(),
                        *span,
                    ),
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

    // --- operator overloading (D-0047) ---

    /// The struct type behind a `Ty`, if it is a user struct (not an enum, which
    /// is also `Ty::Named`).
    fn struct_info(&self, t: &Ty) -> Option<&StructInfo> {
        if let Ty::Named(n) = t {
            if !self.enums.contains_key(n) {
                return self.structs.get(n);
            }
        }
        None
    }

    /// The element type produced by iterating a value of type `it`, following the
    /// iteration protocol (D-0048), or `None` if `it` is not iterable.
    ///
    /// Built-ins (`range`/list/set/str) keep their known element types. A user
    /// struct is iterable when it provides `__iter__` (the `Iterable` interface);
    /// its return names the iterator, whose `__next__` (`Iterator`) gives the
    /// element type. A struct that itself has `__next__` is treated as its own
    /// iterator. The element type comes from `__next__`'s return signature: the
    /// comma-ok form `-> (T, bool)` and the Python/`StopIteration` form `-> T`
    /// both yield `T`. `Unknown`/`Py` stay permissive.
    fn iterable_elem(&self, it: &Ty) -> Option<Ty> {
        match it {
            Ty::Range => Some(Ty::Int64),
            Ty::List(t) | Ty::Set(t) | Ty::Heap(t) => Some((**t).clone()),
            Ty::Str => Some(Ty::Char),
            Ty::Unknown | Ty::Py => Some(Ty::Unknown),
            Ty::Named(_) => {
                let info = self.struct_info(it)?;
                let iter_ty = if let Some(sig) = info.methods.get("__iter__") {
                    sig.ret.clone()
                } else if info.methods.contains_key("__next__") {
                    it.clone()
                } else {
                    return None;
                };
                let next = self.struct_info(&iter_ty)?.methods.get("__next__")?;
                Some(next_elem(&next.ret))
            }
            _ => None,
        }
    }

    /// The element type of `for`/comprehension iteration over `it`, emitting a
    /// compile error (D-0048) when `it` is not iterable. Returns [`Ty::Unknown`]
    /// on error so checking continues.
    fn iter_element(&mut self, it: &Ty, span: Span) -> Ty {
        match self.iterable_elem(it) {
            Some(e) => e,
            None => {
                self.error(
                    format!(
                        "type '{}' is not iterable: it must define '__iter__' \
                         (the 'Iterable' interface, D-0048)",
                        it.display()
                    ),
                    span,
                );
                Ty::Unknown
            }
        }
    }

    /// The signature of a struct's dunder, if defined or derived.
    fn lookup_dunder(&self, t: &Ty, dunder: &str) -> Option<FnSig> {
        self.struct_info(t)
            .and_then(|i| i.methods.get(dunder).cloned())
    }

    /// Require a struct to provide `dunder` for operator `sym` (interface
    /// `iface`), erroring with a clear message if it does not (D-0047).
    fn require_dunder(&mut self, t: &Ty, dunder: &str, iface: &str, sym: &str, span: Span) {
        if self.lookup_dunder(t, dunder).is_none() {
            self.error(
                format!(
                    "type '{}' does not support operator '{sym}': it must define \
                     '{dunder}' (the '{iface}' operator interface, D-0047)",
                    t.display()
                ),
                span,
            );
        }
    }

    /// The result type of an operator whose result is the dunder's return type
    /// (arithmetic / bitwise / unary / indexing). Errors if the dunder is missing.
    fn op_result(&mut self, t: &Ty, dunder: &str, iface: &str, sym: &str, span: Span) -> Ty {
        match self.lookup_dunder(t, dunder) {
            Some(sig) => sig.ret,
            None => {
                self.require_dunder(t, dunder, iface, sym, span);
                Ty::Unknown
            }
        }
    }

    /// Check a binary operator with a user-struct operand (D-0047), returning the
    /// result type, or `None` to fall back to the built-in-type rules.
    fn check_struct_binop(&mut self, op: BinOp, l: &Ty, r: &Ty, span: Span) -> Option<Ty> {
        use BinOp::*;
        // `x in a` dispatches on the container (right operand). A struct that
        // defines `__contains__` uses it; otherwise, if the container is iterable
        // (D-0048), membership falls back to iterating and comparing. Only a
        // struct that is neither is an error.
        if matches!(op, In) {
            if self.struct_info(r).is_some() {
                if self.lookup_dunder(r, "__contains__").is_none()
                    && self.iterable_elem(r).is_none()
                {
                    self.error(
                        format!(
                            "type '{}' does not support operator 'in': define \
                             '__contains__' or make it iterable ('__iter__', D-0048)",
                            r.display()
                        ),
                        span,
                    );
                }
                return Some(Ty::Bool);
            }
            return None;
        }
        // Every other operator dispatches on the left operand.
        self.struct_info(l)?;
        match op {
            Eq | Ne => {
                self.require_dunder(l, "__eq__", "Eq", binop_symbol(op), span);
                Some(Ty::Bool)
            }
            Lt | Le | Gt | Ge => {
                // The four relational operators derive from a single `__cmp__`.
                self.require_dunder(l, "__cmp__", "Ord", binop_symbol(op), span);
                Some(Ty::Bool)
            }
            And | Or => Some(Ty::Bool),
            Send => None,
            _ => {
                let (dunder, iface) = binary_op_dunder(op)?;
                Some(self.op_result(l, dunder, iface, binop_symbol(op), span))
            }
        }
    }

    /// Check an augmented assignment `a OP= b` with a user-struct LHS (D-0047),
    /// returning the result type, or `None` to fall back to the built-in-type
    /// rules. If the LHS type defines the in-place dunder (`__iadd__` etc.), the
    /// assignment calls `a.__iOP__(b)` and is typed from that dunder's return
    /// type; otherwise it derives from the binary operator exactly like `a OP b`
    /// (so a struct with only `__add__` still supports `+=`).
    fn check_struct_aug_assign(&mut self, op: BinOp, l: &Ty, r: &Ty, span: Span) -> Option<Ty> {
        self.struct_info(l)?;
        if let Some(dunder) = inplace_op_dunder(op) {
            if let Some(sig) = self.lookup_dunder(l, dunder) {
                return Some(sig.ret);
            }
        }
        self.check_struct_binop(op, l, r, span)
    }

    /// Check a unary operator with a user-struct operand (D-0047).
    fn check_struct_unary(&mut self, op: UnOp, t: &Ty, span: Span) -> Option<Ty> {
        self.struct_info(t)?;
        Some(match op {
            UnOp::Not => Ty::Bool, // truthiness; `__bool__` is optional
            UnOp::Neg => self.op_result(t, "__neg__", "Neg", "-", span),
            UnOp::Pos => self.op_result(t, "__pos__", "Pos", "+", span),
            UnOp::BitNot => self.op_result(t, "__invert__", "Invert", "~", span),
        })
    }

    /// Warn when a struct that defines equality but not `__hash__` is used as a
    /// `dict`/`set` key — then `a == b ⟹ hash(a) == hash(b)` is not guaranteed
    /// (D-0047). `@derive(Hash)` or a hand-written `__hash__` silences it.
    fn warn_struct_key(&mut self, key: &Ty, span: Span) {
        let (has_eq, has_hash) = match self.struct_info(key) {
            Some(i) => (i.has("__eq__"), i.has("__hash__")),
            None => return,
        };
        if has_eq && !has_hash {
            self.warn(
                format!(
                    "type '{}' defines equality but no '__hash__', yet is used as a \
                     dict/set key; add @derive(Hash) or define '__hash__' so that \
                     'a == b' implies 'hash(a) == hash(b)' (D-0047)",
                    key.display()
                ),
                span,
            );
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
                // `Struct.from_json(s)` — the decode associated function a
                // `@derive(Json)` struct gets (D-0059, I#100). Recognized when
                // the struct derived the codec (its `to_<fmt>` method exists).
                let decode_encode = match name.as_str() {
                    "from_json" => Some("to_json"),
                    "from_csv" => Some("to_csv"),
                    "from_yaml" => Some("to_yaml"),
                    _ => None,
                };
                if let Some(enc) = decode_encode {
                    if self
                        .structs
                        .get(tyname)
                        .is_some_and(|i| i.methods.contains_key(enc))
                    {
                        if args.len() != 1 {
                            self.error(
                                format!(
                                    "{tyname}.{name}(s) takes 1 argument, found {}",
                                    args.len()
                                ),
                                span,
                            );
                        } else {
                            let t = self.infer(&args[0].value);
                            if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
                                self.error(
                                    format!(
                                        "{tyname}.{name}() expects a str or bytes argument, found {}",
                                        t.display()
                                    ),
                                    expr_span(&args[0].value),
                                );
                            }
                        }
                        return Ty::Named(tyname.clone());
                    }
                }
                // Built-in `signal`/`gc`/`runtime` module calls (ticket #77,
                // D-0058). Recognized only when the name is not shadowed by a
                // local value, and is intercepted before the base is resolved as
                // a value (these module names are not value bindings).
                if self.lookup(tyname).is_none() {
                    if let Some(ty) =
                        modules::check_builtin_module_call(self, tyname, name, args, span)
                    {
                        return ty;
                    }
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
            // Heap construction `heap[T]()` / `heap[T](cmp="max")` (D-0057). The
            // element type `T` is the subscript; the only accepted argument is the
            // keyword `cmp`, whose value is the string "min" (default) or "max"
            // selecting heap direction.
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "heap") {
                let elem = self.chan_elem_from_expr(index);
                for a in args {
                    let at = self.infer(&a.value);
                    match a.name.as_deref() {
                        Some("cmp") => {
                            if let Expr::Str { value, .. } = &a.value {
                                if value != "min" && value != "max" {
                                    self.error(
                                        format!(
                                            "heap cmp must be \"min\" or \"max\", found {value:?}"
                                        ),
                                        expr_span(&a.value),
                                    );
                                }
                            } else if !matches!(at, Ty::Unknown) {
                                self.error(
                                    "heap cmp must be a string literal (\"min\" or \"max\")"
                                        .to_string(),
                                    expr_span(&a.value),
                                );
                            }
                        }
                        Some(other) => self.error(
                            format!("heap[T](...) has no keyword argument '{other}' (only 'cmp')"),
                            expr_span(&a.value),
                        ),
                        None => self.error(
                            "heap[T](...) takes only the keyword argument 'cmp' (e.g. cmp=\"max\")"
                                .to_string(),
                            expr_span(&a.value),
                        ),
                    }
                }
                return Ty::Heap(Box::new(elem));
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
                // `checkpoint()` (D-0044): an explicit cancellation point for long
                // CPU-bound loops. Takes no arguments; may raise `Cancelled`.
                "checkpoint" => {
                    if !args.is_empty() {
                        self.error("checkpoint() takes no arguments".to_string(), span);
                    }
                    return Ty::Unit;
                }
                // `__netpoll_selftest()` (ticket #80): runs the netpoller
                // integration self-test and returns the number of sub-checks
                // that passed. Internal; takes no arguments.
                "__netpoll_selftest" => {
                    if !args.is_empty() {
                        self.error("__netpoll_selftest() takes no arguments".to_string(), span);
                    }
                    return Ty::Int64;
                }
                // `__signal_selftest()` (ticket #77): runs the signal subsystem
                // integration self-test (self-pipe + netpoller + cap-1 coalescing)
                // and returns the number of sub-checks that passed. Internal.
                "__signal_selftest" => {
                    if !args.is_empty() {
                        self.error("__signal_selftest() takes no arguments".to_string(), span);
                    }
                    return Ty::Int64;
                }
                // `__gc_finalizer_selftest()` (ticket #92): runs the precise
                // collector's finalizer self-test and returns the number of
                // finalizers that ran. Internal; takes no arguments.
                "__gc_finalizer_selftest" => {
                    if !args.is_empty() {
                        self.error(
                            "__gc_finalizer_selftest() takes no arguments".to_string(),
                            span,
                        );
                    }
                    return Ty::Int64;
                }
                "__raise_signal" => {
                    if args.len() != 1 {
                        self.error("__raise_signal(n) takes one argument".to_string(), span);
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !at.is_int() && !matches!(at, Ty::Unknown) {
                            self.error(
                                "__raise_signal(n) expects an integer signal number".to_string(),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Unit;
                }
                "__signal_drain" => {
                    if !args.is_empty() {
                        self.error("__signal_drain() takes no arguments".to_string(), span);
                    }
                    return Ty::Unit;
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
                    // A callable user struct held in a variable (D-0047):
                    // `f(args)` desugars to `f.__call__(args)`.
                    if let Some(vt) = self.lookup(fname) {
                        if self.struct_info(&vt).is_some() {
                            for a in args {
                                self.infer(&a.value);
                            }
                            return self.op_result(&vt, "__call__", "Call", "()", span);
                        }
                    }
                    self.error(format!("call to unknown function '{fname}'"), span);
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
            }
        }
        // Dynamic dispatch on an interface-typed receiver (D-0016): resolve the
        // method against the interface's declared set so the call is typed by its
        // return type and a misspelled method is caught at compile time. Handled
        // for a simple `name.method(...)` receiver (a binding or parameter), the
        // form interface values take today.
        if let Expr::Attr { base, name, span } = callee {
            if let Expr::Name(bn, _) = base.as_ref() {
                if let Some(Ty::Named(tn)) = self.lookup(bn) {
                    if let Some(iface) = self.interfaces.get(&tn) {
                        let ret = match iface.method(name) {
                            Some(sig) => sig.ret.clone(),
                            None => {
                                self.error(
                                    format!("interface '{tn}' has no method '{name}'"),
                                    *span,
                                );
                                Ty::Unknown
                            }
                        };
                        for a in args {
                            self.infer(&a.value);
                        }
                        return ret;
                    }
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
                if !self.coercible(&at, &pt) {
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
                    if !self.coercible(&at, &p.ty) {
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

/// `StopIteration` or `StopIteration()` — the iterator exhaustion signal
/// (D-0048), a compiler-known exception name.
fn is_stop_iteration(e: &Expr) -> bool {
    match e {
        Expr::Name(n, _) => n == "StopIteration",
        Expr::Call { callee, .. } => {
            matches!(callee.as_ref(), Expr::Name(n, _) if n == "StopIteration")
        }
        _ => false,
    }
}

/// `expect_raises(ErrType)` — the exception-assertion context manager (D-0054).
fn is_expect_raises(e: &Expr) -> bool {
    matches!(e, Expr::Call { callee, .. }
        if matches!(callee.as_ref(), Expr::Name(n, _) if n == "expect_raises"))
}

/// `OnError.Cancel` / `OnError.Wait` — the typed `scope(on_error=...)` policy
/// (D-0044). Recognized structurally: the `OnError` enum is a compiler-known
/// policy in this increment (a real stdlib enum is a follow-up), so a misspelled
/// member or a non-`OnError` value is rejected by [`check_scope_opts`].
fn is_on_error_policy(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Attr { base, name, .. }
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "OnError")
                && (name == "Cancel" || name == "Wait")
    )
}

/// The element type an iterator's `__next__` yields (D-0048): the comma-ok form
/// `-> (T, bool)` yields `T`; the plain `-> T` (StopIteration) form yields `T`.
fn next_elem(ret: &Ty) -> Ty {
    if let Ty::Tuple(ts) = ret {
        if ts.len() == 2 && matches!(ts[1], Ty::Bool) {
            return ts[0].clone();
        }
    }
    ret.clone()
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
    fn generator_drives_a_for_loop() {
        // A `def` with `yield` is a generator (I#67); its call is an Iterator
        // and drops into a `for` loop through the D-0048 protocol.
        ok(
            "def count(int64 n):\n    auto i = 0\n    while i < n:\n        yield i\n        \
            i = i + 1\n\ndef main():\n    auto total = 0\n    for x in count(5):\n        \
            total = total + x\n    print(total)\n",
        );
    }

    #[test]
    fn generator_feeds_a_comprehension() {
        ok(
            "def ups(int64 n):\n    auto i = 0\n    while i < n:\n        yield i * i\n        \
            i = i + 1\n\ndef main():\n    auto xs = [v for v in ups(4)]\n    print(len(xs))\n",
        );
    }

    #[test]
    fn generator_return_value_is_rejected() {
        assert!(!errors(
            "def g():\n    yield 1\n    return 7\n\ndef main():\n    for x in g():\n        \
             print(x)\n"
        )
        .is_empty());
    }

    #[test]
    fn generator_with_declared_return_type_is_rejected() {
        assert!(
            !errors("def g() -> int64:\n    yield 1\n\ndef main():\n    print(1)\n").is_empty()
        );
    }

    #[test]
    fn yield_without_value_is_rejected() {
        assert!(!errors(
            "def g():\n    yield\n\ndef main():\n    for x in g():\n        print(x)\n"
        )
        .is_empty());
    }

    #[test]
    fn generator_main_is_rejected() {
        assert!(!errors("def main():\n    yield 1\n").is_empty());
    }

    #[test]
    fn test_decorators_are_accepted() {
        // The whole `@test` suite (D-0054) passes decorator validation.
        ok("@test\ndef test_a():\n    assert True\n");
        ok("@before\ndef setup():\n    print(1)\n");
        ok("@after_all\ndef teardown():\n    print(1)\n");
        ok("@test\n@skip(\"later\")\ndef test_s():\n    assert False\n");
        ok("@test\n@skip_if(True, \"ci\")\ndef test_si():\n    assert False\n");
        ok("@test\n@expect_raises(ValueError)\ndef test_r():\n    raise \"x\"\n");
        ok("@test\n@parametrize(cases=[(1, 2)])\ndef test_p(int64 a, int64 b):\n    assert a < b\n");
    }

    #[test]
    fn test_decorator_misuse_is_reported() {
        // Wrong arity / shape produces a clear error (typo-catching, D-0016).
        assert!(!errors("@test(1)\ndef t():\n    assert True\n").is_empty());
        assert!(!errors("@expect_raises\ndef t():\n    assert True\n").is_empty());
        assert!(!errors("@parametrize([1, 2])\ndef t():\n    assert True\n").is_empty());
    }

    #[test]
    fn assert_and_expect_raises_check() {
        ok("def main():\n    assert 1 == 1, \"ok\"\n");
        ok("def boom():\n    raise \"x\"\n\
            def main():\n    with expect_raises(ValueError):\n        boom()\n");
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
    fn structured_concurrency_fixtures_type_check() {
        // The structured-concurrency goldens type-check: `scope:` join, the
        // aggregate-failure path, cooperative cancellation via `checkpoint()`,
        // and `spawn detached` (ticket #50, D-0044/D-0045).
        ok(include_str!("../../../tests/golden/scope_join.zap"));
        ok(include_str!("../../../tests/golden/scope_group.zap"));
        ok(include_str!("../../../tests/golden/scope_cancel.zap"));
        ok(include_str!("../../../tests/golden/scope_detached.zap"));
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
    fn scope_block_and_spawn_detached_type_check() {
        // A `scope:` with owned spawns, a `spawn detached`, `on_error=OnError.Wait`
        // and a `checkpoint()` all type-check (ticket #50, D-0044).
        ok("def f():\n    checkpoint()\n\ndef main():\n    scope(on_error=OnError.Wait):\n        spawn f()\n        spawn f()\n    spawn detached f()\n");
    }

    #[test]
    fn scope_on_error_rejects_bad_policy() {
        // A misspelled / non-OnError policy is a compile-time error, not a silent
        // runtime surprise (D-0044).
        let e = errors("def f():\n    print(1)\n\ndef main():\n    scope(on_error=OnError.Nope):\n        spawn f()\n");
        assert_eq!(
            e.len(),
            1,
            "expected a bad on_error policy error, got {e:?}"
        );
    }

    #[test]
    fn scope_timeout_is_deferred() {
        // `scope(timeout=...)` is parsed but not yet supported (the timer API is
        // deferred); it is reported rather than silently ignored.
        let e = errors(
            "def f():\n    print(1)\n\ndef main():\n    scope(timeout=30):\n        spawn f()\n",
        );
        assert!(!e.is_empty(), "expected scope(timeout=...) to be reported");
    }

    #[test]
    fn checkpoint_takes_no_arguments() {
        let e = errors("def main():\n    checkpoint(1)\n");
        assert_eq!(e.len(), 1, "expected a checkpoint arity error, got {e:?}");
    }

    // --- signals (ticket #77, D-0058) ---

    #[test]
    fn signal_notify_and_gc_runtime_modules_type_check() {
        // `signal.notify` returns a channel[Signal]; `gc`/`runtime` module calls
        // type-check; the built-in `Signal` enum and `MemStats` struct are
        // available without an explicit declaration.
        ok("def main():\n\
            \x20   auto ch = signal.notify(Signal.SIGINT)\n\
            \x20   auto s = <-ch\n\
            \x20   gc.collect()\n\
            \x20   auto m = gc.stats()\n\
            \x20   print(m.heap_bytes)\n\
            \x20   print(runtime.num_goroutines())\n\
            \x20   runtime.goroutine_dump()\n");
    }

    #[test]
    fn signal_notify_rejects_non_signal() {
        let e = errors("def main():\n    auto ch = signal.notify(42)\n");
        assert!(!e.is_empty(), "expected signal.notify(non-Signal) to error");
    }

    #[test]
    fn unknown_module_function_is_an_error() {
        let e = errors("def main():\n    gc.frobnicate()\n");
        assert!(!e.is_empty(), "expected unknown gc.* function to error");
    }

    #[test]
    fn math_module_functions_and_constants_type_check() {
        // Constants are attributes (`math.pi`); functions widen their args to
        // float64 (ticket #98). `import math` is accepted but not required.
        ok("import math\n\n\
            def main():\n\
            \x20   float64 a = math.sqrt(2.0)\n\
            \x20   float64 b = math.pow(2.0, 10.0)\n\
            \x20   float64 c = math.sin(math.pi)\n\
            \x20   float64 d = math.hypot(3.0, 4.0)\n\
            \x20   float64 e = math.log(math.e)\n\
            \x20   bool f = math.isnan(math.nan)\n\
            \x20   bool g = math.isinf(math.inf)\n\
            \x20   float64 h = math.floor(math.tau)\n\
            \x20   print(a + b + c + d + e + h)\n\
            \x20   print(f)\n\
            \x20   print(g)\n");
    }

    #[test]
    fn math_accepts_int_arguments() {
        // Integer arguments widen to float64 (ticket #98).
        ok("def main():\n    float64 r = math.sqrt(4)\n    print(r)\n");
    }

    #[test]
    fn unknown_math_function_and_constant_are_errors() {
        assert!(
            !errors("def main():\n    auto x = math.frobnicate(1.0)\n").is_empty(),
            "expected unknown math.* function to error"
        );
        // `math.quux` is not a known constant, so the base resolves as a value
        // and fails (no `math` binding).
        assert!(
            !errors("def main():\n    auto x = math.quux\n").is_empty(),
            "expected unknown math constant to error"
        );
    }

    #[test]
    fn random_module_surface_type_checks() {
        // `choice` yields the list element type; `shuffle` returns unit
        // (ticket #98).
        ok("import random\n\n\
            def main():\n\
            \x20   random.seed(42)\n\
            \x20   float64 a = random.random()\n\
            \x20   float64 b = random.uniform(1.0, 2.0)\n\
            \x20   int64 c = random.randint(1, 6)\n\
            \x20   auto xs = [10, 20, 30]\n\
            \x20   int64 pick = random.choice(xs)\n\
            \x20   random.shuffle(xs)\n\
            \x20   print(a + b)\n\
            \x20   print(c + pick)\n");
    }

    #[test]
    fn random_choice_requires_a_list() {
        let e = errors("def main():\n    auto x = random.choice(5)\n");
        assert!(!e.is_empty(), "expected random.choice(non-list) to error");
    }

    #[test]
    fn logging_module_surface_type_checks() {
        // Level thresholds are int64 attributes (`logging.DEBUG`), emitters and
        // config setters return unit, `get_level` returns int64 (ticket #101).
        // `import logging` is accepted but not required.
        ok("import logging\n\n\
            def main():\n\
            \x20   logging.set_level(logging.DEBUG)\n\
            \x20   logging.set_stream(sys.stderr_fd())\n\
            \x20   logging.set_format(\"%(levelname)s:%(message)s\")\n\
            \x20   logging.debug(\"d\")\n\
            \x20   logging.info(\"i\")\n\
            \x20   logging.warning(\"w\")\n\
            \x20   logging.error(\"e\")\n\
            \x20   logging.critical(\"c\")\n\
            \x20   logging.log(logging.ERROR, \"x\")\n\
            \x20   int64 lvl = logging.get_level()\n\
            \x20   print(lvl)\n");
    }

    #[test]
    fn unknown_logging_function_and_level_are_errors() {
        assert!(
            !errors("def main():\n    logging.frobnicate(\"x\")\n").is_empty(),
            "expected unknown logging.* function to error"
        );
        // `logging.NOTSET` is not a known level, so the base resolves as a value
        // and fails (no `logging` binding).
        assert!(
            !errors("def main():\n    auto x = logging.NOTSET\n").is_empty(),
            "expected unknown logging level to error"
        );
    }

    #[test]
    fn argparse_module_surface_type_checks() {
        // Declarations/parse return unit; typed getters return str/int64/bool;
        // `format_help` -> str, `help_requested` -> bool (ticket #101).
        ok("import argparse\n\n\
            def main():\n\
            \x20   argparse.set_prog(\"demo\")\n\
            \x20   argparse.set_description(\"desc\")\n\
            \x20   argparse.add_flag(\"verbose\")\n\
            \x20   argparse.add_option(\"name\", \"anon\")\n\
            \x20   argparse.add_int(\"count\", 1)\n\
            \x20   argparse.add_positional(\"src\")\n\
            \x20   auto argv = [\"prog\", \"src.txt\"]\n\
            \x20   argparse.parse(argv)\n\
            \x20   str n = argparse.get(\"name\")\n\
            \x20   int64 c = argparse.get_int(\"count\")\n\
            \x20   str h = argparse.format_help()\n\
            \x20   if argparse.get_flag(\"verbose\"):\n\
            \x20       print(n)\n\
            \x20   if argparse.help_requested():\n\
            \x20       print(h)\n\
            \x20   print(c)\n\
            \x20   argparse.reset()\n");
    }

    #[test]
    fn unknown_argparse_function_is_an_error() {
        assert!(
            !errors("def main():\n    argparse.frobnicate()\n").is_empty(),
            "expected unknown argparse.* function to error"
        );
    }

    #[test]
    fn on_signal_decorator_type_checks() {
        ok("@on_signal(Signal.SIGTERM)\ndef cleanup():\n    print(1)\n\ndef main():\n    print(0)\n");
    }

    #[test]
    fn on_signal_rejects_non_signal_argument() {
        let e =
            errors("@on_signal(42)\ndef cleanup():\n    print(1)\n\ndef main():\n    print(0)\n");
        assert!(!e.is_empty(), "expected @on_signal(non-Signal) to error");
    }

    #[test]
    fn scope_cancel_on_and_grace_type_check() {
        ok("def f():\n    print(1)\n\ndef main():\n\
            \x20   scope(cancel_on=[Signal.SIGINT, Signal.SIGTERM], grace=5000):\n\
            \x20       spawn f()\n");
    }

    #[test]
    fn scope_cancel_on_rejects_non_signal_list() {
        let e = errors(
            "def f():\n    print(1)\n\ndef main():\n\
            \x20   scope(cancel_on=[1, 2]):\n        spawn f()\n",
        );
        assert!(!e.is_empty(), "expected cancel_on=[non-Signal] to error");
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
        // This covers the uint8/bool dtypes and mixed-dtype promotion (#49) too,
        // since those stay loosely typed at the sema layer (the dtype lives in
        // the ZIR lowering).
        ok(include_str!("../../../tests/golden/array_int.zap"));
        ok(include_str!("../../../tests/golden/array_f64.zap"));
        ok(include_str!("../../../tests/golden/array_promote.zap"));
        ok(include_str!("../../../tests/golden/array_uint8.zap"));
        ok(include_str!("../../../tests/golden/array_bool.zap"));
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

    // --- operator overloading (ticket #52, D-0047) ---

    #[test]
    fn operator_on_struct_requires_the_dunder() {
        // `a + b` on a struct with no `__add__` is a clear compile error that
        // names the dunder and interface (D-0047).
        let e = errors(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    auto c = a + b\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("__add__"), "{e:?}");
        assert!(e[0].message.contains("Add"), "{e:?}");
    }

    #[test]
    fn comparison_on_struct_requires_cmp() {
        // `<` needs `__cmp__`; the error names it and the `Ord` interface.
        let e = errors(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    if a < b:\n        print(1)\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("__cmp__"), "{e:?}");
    }

    #[test]
    fn equality_on_struct_requires_eq() {
        let e = errors(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    if a == b:\n        print(1)\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("__eq__"), "{e:?}");
    }

    #[test]
    fn hand_written_dunders_make_operators_type_check() {
        // A struct defining the dunders satisfies the operator interfaces; the
        // operator's result type is the dunder's return type.
        ok("struct V:\n    int64 x\n\n    \
            def __add__(self, V other) -> V:\n        return V(x=self.x)\n\n    \
            def __eq__(self, V other) -> bool:\n        return True\n\n    \
            def __cmp__(self, V other) -> int64:\n        return 0\n\n\
            def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
            auto c = a + b\n    if a == b:\n        print(1)\n    \
            if a < b:\n        print(2)\n");
    }

    #[test]
    fn aug_assign_accepts_inplace_only_dunder() {
        // A struct that defines only `__iadd__` (no `__add__`) type-checks `+=`:
        // the assignment calls the in-place dunder (D-0047/D-0023).
        ok("struct Acc:\n    int64 total\n\n    \
            def __iadd__(self, Acc other) -> Acc:\n        \
            self.total = self.total + other.total\n        return self\n\n\
            def main():\n    auto a = Acc(total=1)\n    auto b = Acc(total=2)\n    \
            a += b\n    print(a.total)\n");
    }

    #[test]
    fn aug_assign_falls_back_to_binary_dunder() {
        // With only `__add__` defined, `+=` still type-checks by deriving from
        // the binary operator.
        ok("struct V:\n    int64 x\n\n    \
            def __add__(self, V other) -> V:\n        return V(x=self.x + other.x)\n\n\
            def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
            a += b\n    print(a.x)\n");
    }

    #[test]
    fn aug_assign_without_either_dunder_errors() {
        // `+=` on a struct that defines neither `__iadd__` nor `__add__` is a
        // clean error naming the binary dunder it would fall back to.
        let e = errors(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    a += b\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("__add__"), "{e:?}");
    }

    #[test]
    fn derive_ord_and_hash_are_accepted() {
        // @derive(Eq, Ord, Hash, Repr) satisfies ==, <, and makes the type a key.
        ok("@derive(Eq, Ord, Hash, Repr)\nstruct V:\n    int64 x\n\n\
            def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
            if a == b:\n        print(1)\n    if a < b:\n        print(2)\n    \
            auto s = {a, b}\n");
    }

    #[test]
    fn eq_without_hash_key_warns() {
        // A struct with equality but no `__hash__` used as a set key warns
        // (non-fatal) — D-0047 invariant guard.
        let w = warnings_str(
            "@derive(Eq)\nstruct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto s = {a}\n",
        );
        assert_eq!(w.len(), 1, "expected one warning, got {w:?}");
        assert!(w[0].message.contains("__hash__"), "{w:?}");
    }

    #[test]
    fn eq_with_hash_key_does_not_warn() {
        // Deriving Hash alongside Eq silences the dict/set-key warning.
        let w = warnings_str(
            "@derive(Eq, Hash)\nstruct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto d = {a: 1}\n",
        );
        assert!(w.is_empty(), "unexpected warnings: {w:?}");
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

    // --- interfaces (ticket #84, D-0016) ---

    const SHAPE_IFACE: &str =
        "interface Shape:\n    def area(self) -> f64\n    def name(self) -> str\n\n\
        struct Circle:\n    f64 r\n\
        \x20   def area(self) -> f64:\n        return self.r\n\
        \x20   def name(self) -> str:\n        return \"circle\"\n\n";

    #[test]
    fn struct_conforms_to_interface() {
        // A struct whose method set matches the interface is assignable to the
        // interface type (structural conformance, D-0016), and an interface
        // method dispatches with its declared return type.
        ok(&format!(
            "{SHAPE_IFACE}def main():\n    Shape s = Circle(r=2.0)\n    f64 a = s.area()\n    print(a)\n    print(s.name())\n"
        ));
    }

    #[test]
    fn struct_missing_method_is_rejected() {
        // A struct missing an interface method cannot be used where the interface
        // is expected — the typo-class bug D-0016 catches at compile time.
        let m = msgs(
            "interface Shape:\n    def area(self) -> f64\n    def name(self) -> str\n\n\
            struct Square:\n    f64 side\n\
            \x20   def area(self) -> f64:\n        return self.side\n\n\
            def main():\n    Shape s = Square(side=1.0)\n",
        );
        assert!(m.contains("Square") && m.contains("Shape"), "got:\n{m}");
    }

    #[test]
    fn interface_method_typo_is_rejected() {
        // Calling a method the interface does not declare is a compile error.
        let m = msgs(&format!(
            "{SHAPE_IFACE}def main():\n    Shape s = Circle(r=2.0)\n    print(s.aera())\n"
        ));
        assert!(m.contains("no method 'aera'"), "got:\n{m}");
    }

    #[test]
    fn struct_passes_as_interface_argument() {
        // A conforming struct is accepted where an interface-typed parameter is
        // expected (a conversion site, D-0016).
        ok(&format!(
            "{SHAPE_IFACE}def describe(Shape s) -> f64:\n    return s.area()\n\n\
            def main():\n    print(describe(Circle(r=1.0)))\n"
        ));
    }

    #[test]
    fn value_struct_conforms_to_interface() {
        // A `value struct` whose method set matches conforms to the interface
        // (I#75 Part 1): the conversion boxes it to the heap in lowering, so it
        // is accepted here exactly like a reference struct.
        ok("interface Shape:\n    def area(self) -> f64\n\n\
            value struct Vec2:\n    f64 x\n    f64 y\n\
            \x20   def area(self) -> f64:\n        return self.x\n\n\
            def main():\n    Shape s = Vec2(x=1.0, y=2.0)\n    print(s.area())\n");
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

    // --- iteration protocol (D-0048) ---

    /// A comma-ok user iterable: `__iter__` yields an iterator whose `__next__`
    /// returns `(T, bool)`.
    const RANGE_SRC: &str = "\
struct Range:
    int64 lo
    int64 hi
    def __iter__(self) -> RangeIter:
        return RangeIter(cur=self.lo, hi=self.hi)

struct RangeIter:
    int64 cur
    int64 hi
    def __iter__(self) -> RangeIter:
        return self
    def __next__(self) -> (int64, bool):
        if self.cur >= self.hi:
            return (0, False)
        auto v = self.cur
        self.cur = self.cur + 1
        return (v, True)
";

    #[test]
    fn user_iterable_commaok_type_checks() {
        ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto total = 0\n    for x in Range(lo=0, hi=5):\n        total = total + x\n    print(total)\n"
        ));
    }

    #[test]
    fn user_iterable_loop_var_is_typed_from_next() {
        // The loop variable has the element type `__next__` yields (int64 here),
        // so an int-only use type-checks and a wrong-type use would not.
        ok(&format!(
            "{RANGE_SRC}\ndef main():\n    for x in Range(lo=0, hi=5):\n        int64 y = x + 1\n        print(y)\n"
        ));
    }

    #[test]
    fn stopiteration_iterable_type_checks() {
        // The Python form: `__next__ -> T` raising `StopIteration` at exhaustion.
        ok("\
struct Count:
    int64 n
    def __iter__(self) -> CountIter:
        return CountIter(cur=0, n=self.n)

struct CountIter:
    int64 cur
    int64 n
    def __iter__(self) -> CountIter:
        return self
    def __next__(self) -> int64:
        if self.cur >= self.n:
            raise StopIteration
        auto v = self.cur
        self.cur = self.cur + 1
        return v

def main():
    auto total = 0
    for x in Count(n=4):
        total = total + x
    print(total)
");
    }

    #[test]
    fn non_iterable_in_for_is_an_error() {
        let e = errors(
            "struct Point:\n    int64 x\ndef main():\n    for p in Point(x=1):\n        print(1)\n",
        );
        assert_eq!(e.len(), 1, "expected one not-iterable error, got {e:?}");
        assert!(e[0].message.contains("not iterable"), "got {e:?}");
    }

    #[test]
    fn comprehension_over_user_iterable_type_checks() {
        ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto squares = [x * x for x in Range(lo=1, hi=5) if x != 3]\n    print(len(squares))\n"
        ));
    }

    #[test]
    fn comprehension_over_non_iterable_is_an_error() {
        let e = errors("def main():\n    auto xs = [y for y in 7]\n    print(len(xs))\n");
        assert!(
            e.iter().any(|e| e.message.contains("not iterable")),
            "expected a not-iterable error, got {e:?}"
        );
    }

    #[test]
    fn membership_fallback_without_contains_type_checks() {
        // A struct with no `__contains__` but which is iterable supports `in`
        // (membership by iteration, D-0048).
        ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto r = Range(lo=0, hi=5)\n    if 3 in r:\n        print(1)\n"
        ));
    }

    // --- value structs (#65, D-0051) ---

    #[test]
    fn value_struct_type_checks_like_a_struct() {
        // A `value struct` type-checks exactly like a reference struct: methods,
        // construction, and field access all work (the kind only changes value
        // semantics, D-0051).
        ok("value struct Vec2:\n    f64 x\n    f64 y\n\
            \x20   def len2(self) -> f64:\n        return self.x * self.x + self.y * self.y\n\
            def main():\n    auto v = Vec2(x=3.0, y=4.0)\n    auto w = v\n    print(w.len2())\n");
    }

    #[test]
    fn value_struct_operators_and_derive_work() {
        // Operator overloading (D-0047) and `@derive` work identically on a value
        // struct: `==` resolves to the derived `__eq__`, and the value struct can
        // be used as a dict key (Eq + Hash).
        ok("@derive(Eq, Hash)\nvalue struct Key:\n    int64 a\n    int64 b\n\
            def main():\n    auto k = Key(a=1, b=2)\n    auto d = {k: 10}\n    if k == k:\n        print(1)\n");
    }

    #[test]
    fn value_struct_direct_recursion_errors() {
        // A value struct cannot contain itself by value (infinite inline size).
        let e = errors("value struct Node:\n    int64 v\n    Node next\n");
        assert_eq!(e.len(), 1, "expected one recursion error, got {e:?}");
        assert!(
            e[0].message.contains("cannot contain itself by value"),
            "message was: {}",
            e[0].message
        );
    }

    #[test]
    fn value_struct_indirect_recursion_errors() {
        // An indirect cycle through another value struct is also rejected.
        let e = errors("value struct A:\n    B b\n\nvalue struct B:\n    A a\n");
        assert!(
            !e.is_empty()
                && e.iter()
                    .any(|x| x.message.contains("cannot contain itself by value")),
            "expected an indirect-recursion error, got {e:?}"
        );
    }

    #[test]
    fn reference_struct_recursion_is_allowed() {
        // A plain (reference) struct may be recursive — the field is a pointer
        // (D-0023), so no infinite-size problem.
        ok("struct Node:\n    int64 v\n    Node next\n");
    }

    #[test]
    fn value_struct_holding_a_reference_struct_is_fine() {
        // A value struct may hold a reference-struct field (a pointer); that does
        // not make it infinitely sized.
        ok("struct Ref:\n    int64 n\n\nvalue struct Holder:\n    Ref r\n    int64 k\n");
    }

    #[test]
    fn heap_type_checks() {
        // heap[T] (D-0057): construction, push/pop/peek, len, and iteration all
        // type-check. The min-heap and the max-heap-via-comparator forms are both
        // accepted, and the full golden fixture type-checks.
        ok(include_str!("../../../tests/golden/heap.zap"));
    }

    #[test]
    fn heap_element_type_is_tracked() {
        // Iterating a `heap[int64]` yields `int64` loop elements, and `len(h)` is
        // an integer — the heap carries its element type like list/set.
        ok("def main():\n    \
                auto h = heap[int64]()\n    \
                h.push(3)\n    \
                int64 total = 0\n    \
                for x in h:\n        total = total + x\n    \
                print(total)\n    \
                int64 n = len(h)\n    \
                print(n)\n");
    }

    #[test]
    fn heap_bad_comparator_errors() {
        // `cmp` accepts only the string literals "min" / "max".
        let e = errors("def main():\n    auto h = heap[int64](cmp=\"biggest\")\n");
        assert!(
            e.iter().any(|x| x.message.contains("cmp must be")),
            "expected a heap-cmp error, got {e:?}"
        );
    }

    #[test]
    fn heap_unknown_keyword_errors() {
        // Only the `cmp` keyword argument is accepted.
        let e = errors("def main():\n    auto h = heap[int64](dir=\"max\")\n");
        assert!(
            e.iter().any(|x| x.message.contains("no keyword argument")),
            "expected an unknown-keyword error, got {e:?}"
        );
    }

    #[test]
    fn heap_positional_argument_errors() {
        // A positional argument (no `cmp=`) is rejected.
        let e = errors("def main():\n    auto h = heap[int64](3)\n");
        assert!(
            e.iter()
                .any(|x| x.message.contains("only the keyword argument")),
            "expected a positional-argument error, got {e:?}"
        );
    }
}
