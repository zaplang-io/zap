//! Zap's source-level type model (ticket #129).
//!
//! The `Ty` type the checker infers and the `*Info` records it builds for
//! registered enums/structs/interfaces, plus the pure type operations over them
//! (`assignable`/`join`/`num_join`/`default_type` and the operator-dunder
//! tables). Split out of `zapc_sema` so edits to the `Checker` or a per-module
//! type-check twin no longer recompile this stable model. `zapc_sema` re-exports
//! `Ty`, so `zapc_sema::Ty` is unchanged; the records and helpers are `pub` here
//! because the `Checker` (now in a separate crate) builds and reads them.

use std::collections::HashMap;

use zapc_syntax::ast::{BinOp, Type};
use zapc_syntax::span::Span;

/// A resolved type.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Int64,
    Int32,
    /// Unsigned fixed-width integers (#552, D-0019 amendment). Reintroduced for
    /// C FFI (#539) and array dtypes. They are a distinct, non-interchangeable
    /// family: there is **no implicit conversion** between signed and unsigned
    /// (mixing them in arithmetic/comparison is a compile error, ZAP4015), and
    /// narrowing always requires an explicit `uintN(...)` / `intN(...)` cast
    /// (D-0020). Implicit *widening* is allowed only up the unsigned tower
    /// (`uint8`→`uint16`→`uint32`→`uint64`). Overflow wraps (two's-complement
    /// modular arithmetic), matching C.
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    /// Pointer-width unsigned integer (`usize`, 64-bit on the supported
    /// x86_64/aarch64 targets). The single canonical name for a size/count at
    /// the C boundary; `size_t` is only the C *mapping target* (future #542
    /// `@repr(c)`), never a Zap-level alias (D-0019 locks "no type aliases").
    /// `usize` takes part in no implicit conversion — every `usize` conversion
    /// is explicit — because its width is platform-dependent.
    Usize,
    Float64,
    Float32,
    Bool,
    Str,
    Bytes,
    /// A **borrowed, non-owning view** of a `str`'s bytes (#1082, borrowed-bytes
    /// RFC — discussion #1097). A `strview` names an existing string's bytes
    /// `(backing, byte_offset, byte_len)` **without allocating an owned `ZapStr`
    /// or copying**, for read-only consumers (`dict.get`/`in`, `set` membership,
    /// hash, `==`, `startswith`/`endswith`/`contains`, `int()`/`float()`/parse,
    /// write-to-output). It is created explicitly (`s.view()` / `s.view(lo, hi)`,
    /// sub-viewed with `v[lo:hi]`), so `str[a:b]`'s copy semantics (D-0025) are
    /// unchanged — views are opt-in.
    ///
    /// A `strview` is an **affine, non-escaping** value tied to its backing's
    /// lifetime: the checker rejects storing it into a dict/list/field/global,
    /// returning it, or capturing it in a closure/first-class-fn (`ZAP5130`),
    /// reusing the `@scoped` machinery (#876). At the zir level it lowers to a
    /// by-value tuple `(str, int64, int64)`, so the backing pointer is traced and
    /// rewritten on a moving-Immix evacuation via the existing aggregate-root
    /// `(base, desc)` pairs (#586) — the base+offset ("2b") representation the
    /// RFC settled on, with no new GC-root machinery.
    StrView,
    /// A **borrowed, non-owning view** of a `bytes` value's bytes (#1082
    /// follow-up): the `bytes` counterpart of [`Ty::StrView`]. A `bytesview`
    /// names an existing `bytes`'s storage `(backing, byte_offset, byte_len)`
    /// **without allocating or copying**, for the same read-only consumers. It is
    /// created with `b.view()` / `b.view(lo, hi)`; unlike `strview`, its bounds
    /// are **byte** indices (I#337 — `bytes` is byte-indexed, not codepoint-
    /// indexed). It is affine under the identical rule (`ZAP5135`) and lowers to a
    /// by-value tuple `(bytes, int64, int64)`, tracked through evacuation by the
    /// same aggregate-root pairs (#586).
    BytesView,
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
    /// A built-in `deque[T]` double-ended queue (#983, D-0208): O(1) push/pop at
    /// both ends with an optional bounded `maxlen`. A GC-managed reference type
    /// like `list`; any storable element (no capability bound).
    Deque(Box<Ty>),
    /// A built-in `oset[T]` insertion-ordered set (#983, D-0208): membership like
    /// `set` (element must be `Hashable`) but iteration follows first-add order.
    OSet(Box<Ty>),
    /// A fixed-arity, heterogeneous, immutable tuple (D-0019/D-0049) carrying its
    /// element types in positional order. The empty tuple `()` is the unit value.
    Tuple(Vec<Ty>),
    /// A typed channel `channel[T]` (ticket #25, D-0031): a first-class reference
    /// value over which `T` values are sent (`ch <- v`) and received (`<-ch`).
    Chan(Box<Ty>),
    /// A concurrent `AtomicArray[T]` (#844, D-01xx): a fixed-capacity array of
    /// atomically CAS-able reference/word slots — the pointer-array primitive the
    /// field-based `Atomic[T]` (D-0066) cannot express. A GC-managed reference
    /// type; `@shareable` so it may back a mutable module global (D-0113).
    AtomicArray(Box<Ty>),
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
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Ty::Int64
                | Ty::Int32
                | Ty::Uint8
                | Ty::Uint16
                | Ty::Uint32
                | Ty::Uint64
                | Ty::Usize
                | Ty::Float64
                | Ty::Float32
                | Ty::Bigint
                | Ty::Complex
                | Ty::UntypedInt
                | Ty::UntypedFloat
        )
    }

    pub fn is_int(&self) -> bool {
        matches!(
            self,
            Ty::Int64
                | Ty::Int32
                | Ty::Uint8
                | Ty::Uint16
                | Ty::Uint32
                | Ty::Uint64
                | Ty::Usize
                | Ty::Bigint
                | Ty::UntypedInt
        )
    }

    /// An unsigned fixed-width integer or `usize` (#552). `UntypedInt` is not
    /// unsigned — a bare literal is sign-agnostic until it takes a concrete type.
    pub fn is_unsigned(&self) -> bool {
        matches!(
            self,
            Ty::Uint8 | Ty::Uint16 | Ty::Uint32 | Ty::Uint64 | Ty::Usize
        )
    }

    /// A concrete *signed* machine integer (`int32`/`int64`). Excludes `bigint`,
    /// the untyped literal, and every unsigned type. Used to detect mixed-sign
    /// arithmetic (#552).
    pub fn is_signed_int(&self) -> bool {
        matches!(self, Ty::Int64 | Ty::Int32)
    }

    /// Bit width of a fixed-width unsigned type, for implicit-widening checks
    /// (#552). `usize` reports 64 (its width on supported targets) but is kept
    /// out of implicit widening by [`assignable`].
    pub fn uint_width(&self) -> Option<u32> {
        Some(match self {
            Ty::Uint8 => 8,
            Ty::Uint16 => 16,
            Ty::Uint32 => 32,
            Ty::Uint64 => 64,
            Ty::Usize => 64,
            _ => return None,
        })
    }

    pub fn display(&self) -> String {
        match self {
            Ty::Int64 => "int64".into(),
            Ty::Int32 => "int32".into(),
            Ty::Uint8 => "uint8".into(),
            Ty::Uint16 => "uint16".into(),
            Ty::Uint32 => "uint32".into(),
            Ty::Uint64 => "uint64".into(),
            Ty::Usize => "usize".into(),
            Ty::Float64 => "float64".into(),
            Ty::Float32 => "float32".into(),
            Ty::Bool => "bool".into(),
            Ty::Str => "str".into(),
            Ty::Bytes => "bytes".into(),
            Ty::StrView => "strview".into(),
            Ty::BytesView => "bytesview".into(),
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
            Ty::Deque(t) => format!("deque[{}]", t.display()),
            Ty::OSet(t) => format!("oset[{}]", t.display()),
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
            Ty::AtomicArray(t) => format!("AtomicArray[{}]", t.display()),
            Ty::Named(n) => n.clone(),
            Ty::Match => "Match".into(),
            Ty::Py => "py".into(),
            Ty::Unknown => "<unknown>".into(),
        }
    }

    /// Convert this inferred type into the AST [`Type`] the monomorphizer works
    /// with (#1166), so mono can unify sema's inferred argument types against a
    /// generic's parameters and mangle instances — instead of re-deriving the
    /// type with its own partial inferer (epic I#1169).
    ///
    /// Returns `None` for a type with no meaningful concrete AST form as a
    /// generic argument — `Unknown` / `Py` / `Unit` / `Match`, the affine
    /// `strview` / `bytesview` views (which sema forbids storing anyway), and
    /// any composite one of whose elements is itself `None` — so mono leaves
    /// that argument uninferred (fail-closed) rather than mangling a bogus name.
    ///
    /// Untyped literals take their default (`int` → `int64`, `float` →
    /// `float64`, D-0003), matching the type a bare literal pins today. The
    /// named spellings are exactly the source type names (`list`, `dict`,
    /// `channel`, `AtomicArray`, …), so the result mangles identically to a
    /// hand-written annotation. Synthesized nodes carry a default span.
    pub fn to_ast_type(&self) -> Option<Type> {
        fn named(name: &str, args: Vec<Type>) -> Type {
            Type::Named {
                name: name.to_string(),
                args,
                span: Span::default(),
            }
        }
        Some(match self {
            Ty::Int64 | Ty::UntypedInt => named("int64", vec![]),
            Ty::Int32 => named("int32", vec![]),
            Ty::Uint8 => named("uint8", vec![]),
            Ty::Uint16 => named("uint16", vec![]),
            Ty::Uint32 => named("uint32", vec![]),
            Ty::Uint64 => named("uint64", vec![]),
            Ty::Usize => named("usize", vec![]),
            Ty::Float64 | Ty::UntypedFloat => named("float64", vec![]),
            Ty::Float32 => named("float32", vec![]),
            Ty::Bool => named("bool", vec![]),
            Ty::Str => named("str", vec![]),
            Ty::Bytes => named("bytes", vec![]),
            Ty::Char => named("char", vec![]),
            Ty::Bigint => named("bigint", vec![]),
            Ty::Complex => named("complex", vec![]),
            Ty::Range => named("range", vec![]),
            Ty::List(e) => named("list", vec![e.to_ast_type()?]),
            Ty::Set(e) => named("set", vec![e.to_ast_type()?]),
            Ty::Heap(e) => named("heap", vec![e.to_ast_type()?]),
            Ty::Deque(e) => named("deque", vec![e.to_ast_type()?]),
            Ty::OSet(e) => named("oset", vec![e.to_ast_type()?]),
            Ty::Chan(e) => named("channel", vec![e.to_ast_type()?]),
            Ty::AtomicArray(e) => named("AtomicArray", vec![e.to_ast_type()?]),
            Ty::Dict(k, v) => named("dict", vec![k.to_ast_type()?, v.to_ast_type()?]),
            Ty::Tuple(elems) => {
                let mut out = Vec::with_capacity(elems.len());
                for e in elems {
                    out.push(e.to_ast_type()?);
                }
                Type::Tuple {
                    elems: out,
                    span: Span::default(),
                }
            }
            // A `$`-prefixed name is a compiler-synthesized, unspellable type —
            // the shared generator-expression iterator `$genexpr`, a generator
            // function's state type `$gen:<name>`, an interface fat-pointer
            // layout `$iface:<name>` (I#67/D-0048, #596). These are not
            // user-nameable nominal types and must never be bridged into
            // monomorphization: taking one down the struct/enum instance path
            // would mint a bogus `Foo$$genexpr`-style instance over an untested,
            // element-erased layout (a miscompile/ICE risk) instead of the
            // intended fail-closed `ZAP6001`. Returning `None` leaves the
            // generic argument `Unknown`, so it is rejected cleanly — exactly as
            // before the #1167 backfill (#1190).
            Ty::Named(n) if n.starts_with('$') => return None,
            Ty::Named(n) => named(n, vec![]),
            Ty::Unit | Ty::StrView | Ty::BytesView | Ty::Match | Ty::Py | Ty::Unknown => {
                return None
            }
        })
    }
}

/// A span-keyed lookup of the type the checker inferred for every expression it
/// visited (#1165), folded from `zapc_sema`'s per-expression records. It lives
/// in this low-level crate (which `zapc_sema` and `zapc_ir`/`zapc_mono` both
/// depend on) so monomorphization can read sema's inferred types as *data*,
/// without depending on the sema checker itself (epic I#1169). A generic
/// constructor / call argument's type is `type_at(arg_span)`.
#[derive(Clone, Debug, Default)]
pub struct TypeMap {
    by_span: HashMap<Span, Ty>,
}

impl TypeMap {
    /// Build a map from `(span, ty)` records in visitation order. A span that
    /// repeats keeps the **last** entry, so a re-inference wins; with the
    /// built-in prelude prepended, prelude item bodies are visited before the
    /// user's, so a user expression wins on the rare event its byte span
    /// coincides with a prelude expression's — which is what a consumer keyed on
    /// user source (monomorphization) wants. (A span is only locally unique; a
    /// future hardening may key on a stable node id — tracked in the epic.)
    pub fn from_records<I>(records: I) -> Self
    where
        I: IntoIterator<Item = (Span, Ty)>,
    {
        let mut by_span = HashMap::new();
        for (span, ty) in records {
            by_span.insert(span, ty);
        }
        Self { by_span }
    }

    /// The inferred type of the expression at `span`, or `None` if no expression
    /// with that span was typed (e.g. a span from a different source, or a node
    /// the checker never reached because an earlier error cut analysis short).
    pub fn type_at(&self, span: Span) -> Option<&Ty> {
        self.by_span.get(&span)
    }

    /// The number of distinct expression spans recorded.
    pub fn len(&self) -> usize {
        self.by_span.len()
    }

    /// Whether no expression types were recorded (the normal non-analysis path,
    /// or an empty module).
    pub fn is_empty(&self) -> bool {
        self.by_span.is_empty()
    }
}

/// Is a value of type `from` assignable where `to` is expected? Implements
/// implicit widening (D-0020); narrowing is rejected.
pub fn assignable(from: &Ty, to: &Ty) -> bool {
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
        // An untyped integer literal fits any unsigned target too (#552), so
        // `uint8 x = 5` and `usize n = 0` work without a cast. A *negative*
        // literal into an unsigned target is caught at the checker (it has the
        // literal's value/sign); here the literal is already sign-agnostic.
        (Ty::UntypedInt, t) if t.is_unsigned() => true,
        // Implicit widening *within* the unsigned tower only
        // (`uint8`→`uint16`→`uint32`→`uint64`): always value-preserving
        // (zero-extend). `usize` is deliberately excluded — its width is
        // platform-dependent, so every `usize` conversion is explicit (#552).
        (from, to)
            if from.is_unsigned() && to.is_unsigned() && *from != Ty::Usize && *to != Ty::Usize =>
        {
            from.uint_width() <= to.uint_width()
        }
        // Concrete widening.
        (Ty::Int32, Ty::Int64) => true,
        (Ty::Int32 | Ty::Int64, Ty::Float64 | Ty::Float32) => true,
        (Ty::Float32, Ty::Float64) => true,
        (Ty::Int32 | Ty::Int64, Ty::Bigint) => true,
        (Ty::Int32 | Ty::Int64 | Ty::Float32 | Ty::Float64 | Ty::Bigint, Ty::Complex) => true,
        (Ty::List(a), Ty::List(b)) => assignable(a, b),
        (Ty::Set(a), Ty::Set(b)) => assignable(a, b),
        (Ty::Heap(a), Ty::Heap(b)) => assignable(a, b),
        (Ty::Deque(a), Ty::Deque(b)) => assignable(a, b),
        (Ty::OSet(a), Ty::OSet(b)) => assignable(a, b),
        (Ty::AtomicArray(a), Ty::AtomicArray(b)) => assignable(a, b),
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
/// type. Comparison is by exact type identity (no widening) with one exception:
/// an *untyped* interface parameter (`Ty::Unknown`) imposes no constraint and so
/// matches any concrete parameter type. This is what lets the self-referential
/// capability dunders conform — `Equatable`'s `__eq__(self, other)` leaves `other`
/// untyped, and a conforming struct supplies `__eq__(self, other: Self)` whose
/// `other` is its own concrete type (D-0139/#806). An untyped parameter on either
/// side therefore matches; a declared type on both must be identical.
pub fn sigs_match(have: &FnSig, want: &FnSig) -> bool {
    let hp = &have.params.get(1..).unwrap_or(&[]);
    let wp = &want.params.get(1..).unwrap_or(&[]);
    hp.len() == wp.len()
        && hp
            .iter()
            .zip(wp.iter())
            .all(|(a, b)| a.ty == b.ty || a.ty == Ty::Unknown || b.ty == Ty::Unknown)
        && have.ret == want.ret
}

/// A nameless, non-defaulted, non-variadic parameter of the given type — used to
/// synthesize `@derive`d dunder signatures (D-0047).
pub fn param(ty: Ty) -> ParamInfo {
    ParamInfo {
        name: String::new(),
        ty,
        has_default: false,
        variadic: false,
        kwargs: false,
        keyword_only: false,
        positional_only: false,
    }
}

/// The compiler-known structural operator interfaces for arithmetic / bitwise /
/// shift operators (D-0047): each maps to the dunder a user struct defines and
/// the interface name that provides it. Comparison (`Eq`/`Ord`), membership
/// (`Contains`), size (`Len`), truthiness (`Bool`), indexing (`Index`), call
/// (`Call`), and repr (`Str`/`Repr`) are handled separately. The result of one
/// of these operators is typed from the dunder's own return type.
pub fn binary_op_dunder(op: BinOp) -> Option<(&'static str, &'static str)> {
    use BinOp::*;
    Some(match op {
        Add => ("__add__", "Add"),
        Sub => ("__sub__", "Sub"),
        Mul => ("__mul__", "Mul"),
        Div => ("__truediv__", "TrueDiv"),
        FloorDiv => ("__floordiv__", "FloorDiv"),
        Mod => ("__mod__", "Mod"),
        MatMul => ("__matmul__", "MatMul"),
        Pow => ("__pow__", "Pow"),
        BitAnd => ("__and__", "BitAnd"),
        BitOr => ("__or__", "BitOr"),
        BitXor => ("__xor__", "BitXor"),
        Shl => ("__lshift__", "LShift"),
        Shr => ("__rshift__", "RShift"),
        // Equality/relational/membership/identity/logical/send are not
        // arithmetic dunders.
        Eq | Ne | Lt | Le | Gt | Ge | And | Or | In | NotIn | Is | IsNot | Send => return None,
    })
}

/// The in-place augmented-assignment dunder for an operator (D-0047): if the
/// LHS type of `a OP= b` defines this, the assignment calls `a.__iOP__(b)`
/// (mutating in place, D-0023) instead of deriving `a = a.__OP__(b)` from the
/// binary operator. Only the operators that can head an augmented assignment
/// have one. Its interface mirrors the binary one (`Add` -> `IAdd`, etc.).
pub fn inplace_op_dunder(op: BinOp) -> Option<&'static str> {
    use BinOp::*;
    Some(match op {
        Add => "__iadd__",
        Sub => "__isub__",
        Mul => "__imul__",
        Div => "__itruediv__",
        FloorDiv => "__ifloordiv__",
        Mod => "__imod__",
        MatMul => "__imatmul__",
        Pow => "__ipow__",
        BitAnd => "__iand__",
        BitOr => "__ior__",
        BitXor => "__ixor__",
        Shl => "__ilshift__",
        Shr => "__irshift__",
        Eq | Ne | Lt | Le | Gt | Ge | And | Or | In | NotIn | Is | IsNot | Send => return None,
    })
}

/// The *reflected* binary-operator dunder for an operator (D-0047, #1180): when
/// the left operand cannot handle `a OP b` itself, the RIGHT operand's reflected
/// dunder is tried as a fallback (`a + b` -> `b.__radd__(a)`). Only the
/// arithmetic / bitwise / shift / matmul operators — those with a
/// [`binary_op_dunder`] — have a reflected form; comparison / membership /
/// logical / send do not. The reflected method's `self` is the right operand and
/// its first non-`self` parameter is the left operand's type.
pub fn reflected_op_dunder(op: BinOp) -> Option<&'static str> {
    use BinOp::*;
    Some(match op {
        Add => "__radd__",
        Sub => "__rsub__",
        Mul => "__rmul__",
        Div => "__rtruediv__",
        FloorDiv => "__rfloordiv__",
        Mod => "__rmod__",
        MatMul => "__rmatmul__",
        Pow => "__rpow__",
        BitAnd => "__rand__",
        BitOr => "__ror__",
        BitXor => "__rxor__",
        Shl => "__rlshift__",
        Shr => "__rrshift__",
        Eq | Ne | Lt | Le | Gt | Ge | And | Or | In | NotIn | Is | IsNot | Send => return None,
    })
}

/// The source symbol for an operator, for diagnostics.
pub fn binop_symbol(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        FloorDiv => "//",
        Mod => "%",
        MatMul => "@",
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
        Is => "is",
        IsNot => "is not",
        Send => "<-",
    }
}

/// Make an untyped literal type concrete (its default).
pub fn default_type(t: Ty) -> Ty {
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
pub fn num_join(l: &Ty, r: &Ty) -> Ty {
    // Unsigned arithmetic promotes to `uint64` and wraps modulo 2^64, exactly
    // mirroring the signed tower's promotion to `int64` (D-0019): a narrower
    // unsigned result requires an explicit `uintN(...)` cast. Mixing signed and
    // unsigned operands is rejected by the checker (ZAP4015) *before* this runs,
    // so a surviving unsigned operand pairs only with another unsigned or an
    // untyped-int literal. The flag is read from the *original* types, before
    // `default_type` turns an `UntypedInt` into `Int64`, so `uint8 + 5` joins to
    // `uint64`, not `int64`.
    let unsigned = l.is_unsigned() || r.is_unsigned();
    let l = default_type(l.clone());
    let r = default_type(r.clone());
    if matches!(l, Ty::Complex) || matches!(r, Ty::Complex) {
        Ty::Complex
    } else if matches!(l, Ty::Float64 | Ty::Float32) || matches!(r, Ty::Float64 | Ty::Float32) {
        Ty::Float64
    } else if unsigned {
        Ty::Uint64
    } else if matches!(l, Ty::Bigint) || matches!(r, Ty::Bigint) {
        Ty::Bigint
    } else {
        Ty::Int64
    }
}

/// The least type both `a` and `b` are assignable to (best-effort).
pub fn join(a: &Ty, b: &Ty) -> Ty {
    if assignable(a, b) {
        b.clone()
    } else if assignable(b, a) {
        a.clone()
    } else {
        Ty::Unknown
    }
}

#[derive(Clone)]
pub struct ParamInfo {
    pub name: String,
    pub ty: Ty,
    pub has_default: bool,
    pub variadic: bool,
    /// `true` for a `V **name` keyword-collector parameter (#601): extra keyword
    /// arguments are collected into a `dict[str, V]`. `ty` holds the value type
    /// `V` (not the dict), mirroring how a variadic's `ty` holds its element type.
    pub kwargs: bool,
    /// `true` when the parameter follows the bare `*` separator and is therefore
    /// keyword-only (I#496): it may only be supplied by name at a call site.
    pub keyword_only: bool,
    /// `true` when the parameter precedes the bare `/` separator and is therefore
    /// positional-only (#602): it may only be supplied positionally at a call
    /// site, never by name.
    pub positional_only: bool,
}

#[derive(Clone)]
pub struct FnSig {
    pub params: Vec<ParamInfo>,
    pub ret: Ty,
}

/// A registered enum type (D-0018 tier-1 / D-0046 tier-3). `is_sum` is true when
/// any variant carries payload fields — then the whole enum is a tagged value
/// and the two tiers do not mix (D-0046).
#[derive(Clone)]
pub struct EnumInfo {
    pub is_sum: bool,
    pub variants: Vec<EnumVariantInfo>,
    /// `true` when the enum carries `@derive(Json)` (ticket #346, D-0086): an
    /// open string-enum codec. Gates recognition of the `Enum.from_json(s)`
    /// associated decode function, mirroring the struct codec.
    pub derives_json: bool,
    /// Source method name -> signature (`self` is the first parameter, typed as
    /// the enum). Enums can carry methods (D-0046, #429/#449); this records their
    /// resolved signatures so an enum can be checked for structural interface
    /// conformance exactly like a struct (D-0016 extension, #731). Empty for an
    /// enum with no methods.
    pub methods: HashMap<String, FnSig>,
}

#[derive(Clone)]
pub struct EnumVariantInfo {
    pub name: String,
    /// Payload fields in declaration order (`(name, type)`); empty for a
    /// tier-1 constant or a payload-free variant.
    pub fields: Vec<(String, Ty)>,
}

impl EnumInfo {
    pub fn variant(&self, name: &str) -> Option<&EnumVariantInfo> {
        self.variants.iter().find(|v| v.name == name)
    }
}

/// A registered struct type. Carries just enough to check operator overloading
/// (D-0047): the method signatures, including the dunders `@derive(...)`
/// generates, so an operator's result is typed from its dunder and the compiler
/// can warn on an `Eq`-without-`Hash` dict/set key.
#[derive(Clone)]
pub struct StructInfo {
    /// Source method name -> signature (`self` is the first parameter).
    pub methods: HashMap<String, FnSig>,
    /// `true` for a `value struct` (D-0051): value semantics (copied on
    /// assign/pass/return, no shared identity), laid out inline. A plain
    /// `struct` is a reference type (D-0023) with `is_value == false`.
    pub is_value: bool,
    /// Resolved field types in declaration order (D-0139/#808). Used to decide
    /// whether a `value struct` auto-derives `Equatable`+`Hashable` (it does iff
    /// every field is itself hashable). Empty is a legitimate value (a fieldless
    /// struct).
    pub fields: Vec<Ty>,
}

impl StructInfo {
    /// Does this struct provide `dunder`, either hand-written or `@derive`d?
    pub fn has(&self, dunder: &str) -> bool {
        self.methods.contains_key(dunder)
    }
}

/// A registered structural interface (D-0016). The method set is the interface's
/// declared signatures, in declaration order (which is the dispatch-table slot
/// order a conforming struct is checked against and lowered to).
#[derive(Clone)]
pub struct InterfaceInfo {
    /// Interface method name -> signature (`self` is the first parameter, typed
    /// as the interface itself), in declaration order.
    pub methods: Vec<(String, FnSig)>,
    /// Refined supertype interface names (D-0139/#806): the `Equatable` in
    /// `interface Hashable: Equatable`. A type conforming to this interface must
    /// conform to every supertype transitively, and a `[T: this]` bound carries
    /// the supertype method requirements. Empty for a plain interface.
    pub supertypes: Vec<String>,
}

impl InterfaceInfo {
    pub fn method(&self, name: &str) -> Option<&FnSig> {
        self.methods.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_classification() {
        for t in [Ty::Uint8, Ty::Uint16, Ty::Uint32, Ty::Uint64, Ty::Usize] {
            assert!(t.is_unsigned(), "{t:?} should be unsigned");
            assert!(t.is_int(), "{t:?} should be an int");
            assert!(t.is_numeric(), "{t:?} should be numeric");
            assert!(!t.is_signed_int(), "{t:?} is not a signed int");
        }
        assert!(Ty::Int64.is_signed_int() && !Ty::Int64.is_unsigned());
        // An untyped literal is sign-agnostic, not unsigned.
        assert!(!Ty::UntypedInt.is_unsigned());
    }

    #[test]
    fn unsigned_assignability() {
        // Widening up the fixed-width unsigned tower is implicit.
        assert!(assignable(&Ty::Uint8, &Ty::Uint16));
        assert!(assignable(&Ty::Uint8, &Ty::Uint64));
        assert!(assignable(&Ty::Uint32, &Ty::Uint64));
        // Narrowing and sign crossings are not.
        assert!(!assignable(&Ty::Uint64, &Ty::Uint8));
        assert!(!assignable(&Ty::Int64, &Ty::Uint64));
        assert!(!assignable(&Ty::Uint64, &Ty::Int64));
        assert!(!assignable(&Ty::Int32, &Ty::Uint32));
        // `usize` is isolated from implicit conversion in both directions.
        assert!(!assignable(&Ty::Uint64, &Ty::Usize));
        assert!(!assignable(&Ty::Usize, &Ty::Uint64));
        assert!(!assignable(&Ty::Uint8, &Ty::Usize));
        // An untyped literal still fits any unsigned type.
        assert!(assignable(&Ty::UntypedInt, &Ty::Uint8));
        assert!(assignable(&Ty::UntypedInt, &Ty::Usize));
    }

    #[test]
    fn unsigned_num_join_promotes_to_uint64() {
        // Any unsigned operand (incl. with an untyped literal) promotes to u64,
        // wrapping at 2^64 — mirroring the signed tower's promotion to i64.
        assert_eq!(num_join(&Ty::Uint8, &Ty::Uint8), Ty::Uint64);
        assert_eq!(num_join(&Ty::Uint32, &Ty::Uint64), Ty::Uint64);
        assert_eq!(num_join(&Ty::Uint8, &Ty::UntypedInt), Ty::Uint64);
        assert_eq!(num_join(&Ty::UntypedInt, &Ty::Usize), Ty::Uint64);
        // All-signed arithmetic is unchanged.
        assert_eq!(num_join(&Ty::Int32, &Ty::Int64), Ty::Int64);
    }
}
