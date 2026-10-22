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

use zapc_syntax::ast::BinOp;

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
    pub fn is_numeric(&self) -> bool {
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

    pub fn is_int(&self) -> bool {
        matches!(self, Ty::Int64 | Ty::Int32 | Ty::Bigint | Ty::UntypedInt)
    }

    pub fn display(&self) -> String {
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
pub fn sigs_match(have: &FnSig, want: &FnSig) -> bool {
    let hp = &have.params.get(1..).unwrap_or(&[]);
    let wp = &want.params.get(1..).unwrap_or(&[]);
    hp.len() == wp.len()
        && hp.iter().zip(wp.iter()).all(|(a, b)| a.ty == b.ty)
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
pub fn inplace_op_dunder(op: BinOp) -> Option<&'static str> {
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
pub fn binop_symbol(op: BinOp) -> &'static str {
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
}

impl InterfaceInfo {
    pub fn method(&self, name: &str) -> Option<&FnSig> {
        self.methods.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }
}
