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
    /// The prelude generic enum `Optional[T]` (I#154) with its payload type `T`
    /// preserved (#1452). Previously sema erased the payload — every `Optional[T]`
    /// resolved to the opaque `Named("Optional")` — so a payload-type mismatch was
    /// invisible to the type checker and only caught in IR lowering
    /// (`check_optional_payload`, #1407; `check_numeric_payload`, #1379). Carrying
    /// the payload (like `List`/`Dict`) lets `assignable`/`coercible` be
    /// payload-aware — `Optional[U]` is assignable to `Optional[T]` iff `U == T`
    /// (reject on mismatch, **no new widening**; locked by the #1379/#1407 ADRs) —
    /// so the mismatch surfaces through the sema-only `check_source`/LSP path. A
    /// bare, un-parameterized `Optional` (or an `Optional.None` with no recoverable
    /// payload) carries [`Ty::Unknown`], which is a wildcard on both sides. The
    /// name mangles identically to the old `Named("Optional")`-plus-argument form
    /// (`to_ast_type` → `Optional[T]`), so monomorphization is byte-for-byte
    /// unchanged.
    Optional(Box<Ty>),
    /// A fixed-arity, heterogeneous, immutable tuple (D-0019/D-0049) carrying its
    /// element types in positional order. The empty tuple `()` is the unit value.
    Tuple(Vec<Ty>),
    /// A variadic `tuple[T]` (#1461, RFC #1472): a dynamic-length, immutable,
    /// homogeneous, hashable sequence — Python's `tuple(iterable)` result. The
    /// one-type-argument sibling of the fixed heterogeneous [`Ty::Tuple`]:
    /// `(A, B)` stays fixed, `tuple[T]` / `tuple(iterable)` is this variadic form.
    /// Like `list`/`set` it is a GC-managed reference type, but immutable and
    /// hashable-iff-`T`-is (so usable as a `dict`/`set` key).
    VarTuple(Box<Ty>),
    /// A typed channel `channel[T]` (ticket #25, D-0031): a first-class reference
    /// value over which `T` values are sent (`ch <- v`) and received (`<-ch`).
    Chan(Box<Ty>),
    /// A concurrent `AtomicArray[T]` (#844, D-01xx): a fixed-capacity array of
    /// atomically CAS-able reference/word slots — the pointer-array primitive the
    /// field-based `Atomic[T]` (D-0066) cannot express. A GC-managed reference
    /// type; `@shareable` so it may back a mutable module global (D-0113).
    AtomicArray(Box<Ty>),
    /// A coroutine-local execution-context variable `ContextVar[T]` (#1237,
    /// PROVISIONAL): a key under which a typed value is bound for the *current*
    /// coroutine and read by deep callees across suspension/resumption and
    /// worker migration. Isolated per coroutine; a `spawn`ed child inherits a
    /// snapshot of the parent's bindings. A GC-managed reference type (the object
    /// identity is the lookup key); `@shareable` so one `ContextVar` can be a
    /// module global shared across coroutines (D-0113). Zap's `contextvars`.
    ContextVar(Box<Ty>),
    /// The opaque token returned by `ContextVar.set(v)` and consumed by
    /// `reset(token)` to restore the context to the state just before that `set`
    /// (#1237, PROVISIONAL). A GC-managed reference, deliberately non-generic and
    /// opaque so it cannot be forged or inspected.
    CtxToken,
    /// A **first-class type value** `Type[T]` (#1527): the value a bare primitive
    /// type name denotes in value position, so a concrete type can be passed as a
    /// library-decorator argument (`@cli.option("--count", int64, default=1)`).
    /// `T` is the represented type, carried so two type values are **statically
    /// distinguishable** (`Type[int64]` != `Type[str]`) and a non-type argument in
    /// a `Type[T]` position is a precise error. Scoped to the primitive scalar
    /// types in v1 (see [`primitive_type_value_ty`]); user struct/enum type values
    /// are deferred (the type-vs-constructor ambiguity — ADR
    /// `decisions/lang/1527-type-args-to-decorators.md`).
    ///
    /// It bridges to the AST annotation `Type[T]` (`to_ast_type`), so
    /// monomorphization's `unify` binds `T` across the `Named("Type", [..])`
    /// arguments it already recurses. At the IR level it **erases to a plain
    /// `int64` type-id constant** ([`type_value_id`]): no runtime reflection graph
    /// (that is E2, #1250), no new IR type, and no GC root — a scalar token the
    /// library can carry (and, in a later phase, inspect).
    TypeVal(Box<Ty>),
    /// A user-defined type (struct/enum), treated opaquely for now.
    Named(String),
    /// A **generic interface instantiated at value positions** (#1428):
    /// `SomeIface[Arg, ...]` as a parameter / return / typed-binding type,
    /// carrying the interface name and its concrete type arguments. A bare
    /// (non-generic) interface stays a plain [`Ty::Named`]; only an interface
    /// *with* type arguments resolves here, so the value-position conformance
    /// check can recover `[Arg]` and call `conforms_with_args` with the real
    /// instantiation (otherwise every generic-interface annotation checked
    /// conformance with **no** type arguments, dropping `[Arg]` at resolution —
    /// so a struct that conforms at the right instantiation was wrongly
    /// rejected). An interface-typed *value* then carries its element type
    /// onward, which `next()` / `map` / `filter` need (#1382/#1390). It erases
    /// to the same `{data, itable}` fat pointer a bare `Ty::Named(iface)` does,
    /// so IR lowering / codegen need nothing new (the interface registry lookup
    /// in `resolve_ty` already ignores the arguments). The decision to add a
    /// dedicated variant (vs. extending the pervasive `Ty::Named`) is recorded
    /// in `decisions/lang/1428-iface-value-position-conformance.md`.
    Iface(String, Vec<Ty>),
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

    /// Bit width of *any* concrete fixed-width integer type, signed or unsigned
    /// (#1654). Unlike [`Ty::uint_width`], this also covers the signed family
    /// (`int32`→32, `int64`→64). Used by the width-stable [`num_join`] to pick
    /// the result width ("widen to the larger operand"). `usize` reports 64 (its
    /// width on supported targets). Returns `None` for `bigint`, the untyped
    /// literal, and every non-integer type.
    pub fn int_width(&self) -> Option<u32> {
        Some(match self {
            Ty::Int32 => 32,
            Ty::Int64 => 64,
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
            Ty::Optional(t) => format!("Optional[{}]", t.display()),
            Ty::TypeVal(t) => format!("Type[{}]", t.display()),
            Ty::VarTuple(t) => format!("tuple[{}]", t.display()),
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
            Ty::ContextVar(t) => format!("ContextVar[{}]", t.display()),
            Ty::CtxToken => "ContextToken".into(),
            Ty::Named(n) => n.clone(),
            // A generic interface instantiation prints `Iface[A, B]` (#1428),
            // the spelling a diagnostic names so a conformance failure points at
            // the exact instantiation that was expected.
            Ty::Iface(n, args) => {
                let inner = args
                    .iter()
                    .map(|a| a.display())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{n}[{inner}]")
            }
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
            // The payload must bridge, so a bare `Optional[Unknown]` (`None` /
            // un-parameterized) is left uninferred (fail-closed) exactly like any
            // other composite with an un-bridgeable element. A concrete payload
            // mangles to `Optional$T`, identical to the pre-#1452 `Named("Optional")`
            // + type-argument form, so monomorph names are byte-for-byte unchanged.
            Ty::Optional(e) => named("Optional", vec![e.to_ast_type()?]),
            // A type value bridges to the annotation `Type[T]` so mono's `unify`
            // binds `T` across its argument (#1527). A bare `Type[Unknown]` (a
            // `Type[T]` whose parameter is unresolved) keeps `Unknown` as its
            // argument, matching every other composite's un-bridgeable element.
            Ty::TypeVal(e) => named("Type", vec![e.to_ast_type()?]),
            Ty::VarTuple(e) => named("tuple", vec![e.to_ast_type()?]),
            Ty::Chan(e) => named("channel", vec![e.to_ast_type()?]),
            Ty::AtomicArray(e) => named("AtomicArray", vec![e.to_ast_type()?]),
            Ty::ContextVar(e) => named("ContextVar", vec![e.to_ast_type()?]),
            Ty::CtxToken => named("ContextToken", vec![]),
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
            // A generic interface instantiation bridges back to the AST
            // annotation `Iface[A, ...]` (#1428). IR `resolve_ty` erases an
            // interface annotation to its fat-pointer `Ty::Struct(fat_sid)`
            // regardless of the arguments (the registry lookup ignores them), so
            // the arguments are carried only for fidelity; a non-bridgeable
            // argument fails closed exactly like any other composite element.
            Ty::Iface(n, args) => {
                let mut out = Vec::with_capacity(args.len());
                for a in args {
                    out.push(a.to_ast_type()?);
                }
                named(n, out)
            }
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
        (Ty::VarTuple(a), Ty::VarTuple(b)) => assignable(a, b),
        (Ty::AtomicArray(a), Ty::AtomicArray(b)) => assignable(a, b),
        // `Optional[U]` is assignable to `Optional[T]` iff the payloads are
        // identical — a reject-on-mismatch rule, NOT a widening (#1452; locked by
        // the #1379/#1407 ADRs, which the IR `check_optional_payload` enforces on
        // monomorphized payloads). An `Unknown` payload on either side is a
        // wildcard: a bare `Optional` annotation and an `Optional.None` carry
        // `Unknown`, so `Optional[int64] x = None`-style flows stay accepted. The
        // `from == to` fast path above already admits identical payloads; this arm
        // exists to REJECT a differing concrete payload (which would otherwise fall
        // through to `false` — the intended behaviour — but is spelled out so the
        // `Unknown` wildcard and the intent are explicit).
        (Ty::Optional(a), Ty::Optional(b)) => {
            **a == **b || matches!(**a, Ty::Unknown) || matches!(**b, Ty::Unknown)
        }
        // A type value `Type[U]` is assignable to `Type[T]` iff the represented
        // types are identical — type values are **invariant** (no widening, so
        // `Type[int32]` does NOT fit `Type[int64]`), the distinguishability #1527
        // relies on. An `Unknown` argument on either side is a wildcard: a
        // `Type[T]` parameter resolves to `Type[Unknown]`, so any concrete type
        // value binds to it (and the non-type reject still holds — a non-`TypeVal`
        // `from` never reaches this arm). (#1527)
        (Ty::TypeVal(a), Ty::TypeVal(b)) => {
            **a == **b || matches!(**a, Ty::Unknown) || matches!(**b, Ty::Unknown)
        }
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

/// The primitive **type value** a bare type name denotes in value position
/// (#1527), or `None` if `name` is not one of the v1 primitive scalar types.
///
/// This is the single source of the canonical type-value name set: every spelling
/// of a primitive scalar (including the Python-facing `int`/`float` and the
/// short `i64`/`u8`/`f32` aliases) maps to its canonical [`Ty`], so `int64` and
/// `int` denote the *same* `Type[int64]`. The composite containers (`list`,
/// `dict`, `set`, …) and user struct/enum names are deliberately excluded in v1
/// — they are a documented follow-up (the type-vs-constructor ambiguity for user
/// types; the container element question). Keeping the set here, rather than in
/// the checker, lets IR lowering agree on exactly which names are type values.
pub fn primitive_type_value_ty(name: &str) -> Option<Ty> {
    Some(match name {
        "int" | "int64" | "i64" => Ty::Int64,
        "int32" | "i32" => Ty::Int32,
        "uint8" | "u8" => Ty::Uint8,
        "uint16" | "u16" => Ty::Uint16,
        "uint32" | "u32" => Ty::Uint32,
        "uint64" | "u64" => Ty::Uint64,
        "usize" => Ty::Usize,
        "float" | "float64" | "f64" => Ty::Float64,
        "float32" | "f32" => Ty::Float32,
        "bool" => Ty::Bool,
        "str" => Ty::Str,
        "bytes" => Ty::Bytes,
        "char" => Ty::Char,
        "bigint" => Ty::Bigint,
        "complex" => Ty::Complex,
        _ => return None,
    })
}

/// The **stable runtime type-id** a type value lowers to (#1527) — the `int64`
/// token a bare primitive type name becomes at the IR level. Stable and
/// documented (`decisions/lang/1527-type-args-to-decorators.md`) so a later phase
/// can expose it to libraries; v1 carries it without a user-facing reader. `None`
/// for any non-primitive `Ty` (a `Type[T]` whose parameter is still `Unknown`, a
/// container, a user type): such a type value has no concrete id to emit.
pub fn type_value_id(ty: &Ty) -> Option<i64> {
    Some(match ty {
        Ty::Int64 => 1,
        Ty::Int32 => 2,
        Ty::Uint8 => 3,
        Ty::Uint16 => 4,
        Ty::Uint32 => 5,
        Ty::Uint64 => 6,
        Ty::Usize => 7,
        Ty::Float64 => 8,
        Ty::Float32 => 9,
        Ty::Bool => 10,
        Ty::Str => 11,
        Ty::Bytes => 12,
        Ty::Char => 13,
        Ty::Bigint => 14,
        Ty::Complex => 15,
        _ => return None,
    })
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
        // Defaulting is element-wise for an `Optional` payload too, so the payload
        // of `Optional.Some(5)` concretizes to `int64` (never a lingering untyped
        // literal), which is what the payload-aware assignability compares (#1452).
        Ty::Optional(t) => Ty::Optional(Box::new(default_type(*t))),
        Ty::VarTuple(e) => Ty::VarTuple(Box::new(default_type(*e))),
        // Defaulting is element-wise for an interface instantiation's arguments
        // too (#1428), so `Container[1-typed-literal]` never carries a lingering
        // untyped literal into the conformance check.
        Ty::Iface(n, args) => Ty::Iface(n, args.into_iter().map(default_type).collect()),
        other => other,
    }
}

/// The result type of a numeric binary operation.
///
/// Fixed-width integer arithmetic is **width-stable** and wraps at the operand
/// width (two's-complement), superseding the promote-to-64 clause of D-0019 /
/// #552 (ADR `decisions/lang/1654-fixed-width-integer-semantics.md`). The result
/// is a fixed-width type of a definite width, not a silent promotion to 64-bit:
///
/// 1. **Same-width typed operands → that width.** `uint8 + uint8 → uint8`,
///    `int32 + int32 → int32` (each wraps mod 2^width at codegen).
/// 2. **Mixed-width typed operands → the larger operand's width.**
///    `uint8 + uint16 → uint16`, `int32 + int64 → int64`. (Signed↔unsigned
///    mixing is rejected by the checker, ZAP4015, *before* this runs, so a
///    surviving concrete operand pairs only with the same signedness or an
///    untyped-int literal.)
/// 3. **An untyped literal adopts the concrete operand's type.**
///    `uint8 + 5 → uint8`, `int32 + 5 → int32` — the literal infers to its
///    context instead of defaulting to `int64`. The checker range-checks the
///    literal against the adopted type (ZAP4016).
/// 4. **No typed context → `int64` default.** `5 + 3 → int64` (both untyped).
///
/// `float`/`complex`/`bigint` operands still win over the integer families, as
/// before. Untyped operands are made concrete for those non-integer cases, so
/// arithmetic always yields a concrete type.
pub fn num_join(l: &Ty, r: &Ty) -> Ty {
    // Non-integer numeric families take precedence and are unaffected by the
    // width-stable rule. `default_type` collapses `UntypedFloat`→`Float64` so a
    // float literal is detected here; the integer path below reads the
    // *original* types so it can tell an `UntypedInt` literal (rule 3) apart
    // from a concrete `int64`.
    let ld = default_type(l.clone());
    let rd = default_type(r.clone());
    if matches!(ld, Ty::Complex) || matches!(rd, Ty::Complex) {
        return Ty::Complex;
    }
    if matches!(ld, Ty::Float64 | Ty::Float32) || matches!(rd, Ty::Float64 | Ty::Float32) {
        return Ty::Float64;
    }
    if matches!(ld, Ty::Bigint) || matches!(rd, Ty::Bigint) {
        return Ty::Bigint;
    }
    // Both operands are now a fixed-width integer or an `UntypedInt` literal.
    match (l, r) {
        // Rule 4: both untyped → the `int64` default (context-free literal).
        (Ty::UntypedInt, Ty::UntypedInt) => Ty::Int64,
        // Rule 3: an untyped literal adopts the concrete operand's type. `t` is a
        // concrete fixed-width int here — every non-integer case returned above.
        (Ty::UntypedInt, t) | (t, Ty::UntypedInt) => t.clone(),
        // Rules 1 & 2: two concrete fixed-width ints stay width-stable, widening
        // to the larger operand.
        _ => int_widen(l, r),
    }
}

/// The width-stable join of two *concrete* fixed-width integer types (#1654):
/// identical types stay put, otherwise the result is the wider operand's type
/// ("widen to the larger operand, wrap there"). Signed↔unsigned mixing never
/// reaches here (the checker rejects it with ZAP4015 first); the only
/// equal-width-but-distinct pair in the shipped family is `usize` vs `uint64`,
/// both 64-bit unsigned, for which the concrete-width `uint64` is returned.
fn int_widen(l: &Ty, r: &Ty) -> Ty {
    if l == r {
        return l.clone();
    }
    let lw = l.int_width().unwrap_or(64);
    let rw = r.int_width().unwrap_or(64);
    match lw.cmp(&rw) {
        std::cmp::Ordering::Greater => l.clone(),
        std::cmp::Ordering::Less => r.clone(),
        // Equal width, distinct types: `usize`/`uint64` → `uint64`.
        std::cmp::Ordering::Equal => Ty::Uint64,
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
    /// Payload fields in declaration order; empty for a tier-1 constant or a
    /// payload-free variant. Enriched from bare `(name, type)` pairs to full
    /// [`FieldInfo`] records (#1257, E1) so compile-time reflection can report a
    /// variant payload with the same shape as a struct field (name, resolved and
    /// declared type, positional index, span). Enum variant payload fields carry
    /// no attribute surface in the grammar today, so `FieldInfo::attrs` is always
    /// empty here.
    pub fields: Vec<FieldInfo>,
    /// Source span of the whole variant declaration (#1257): the reflection
    /// anchor for a per-variant diagnostic once the comptime evaluator (E2/#1250)
    /// can drive `type_info` over variants.
    pub span: Span,
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
    /// Declared data fields in declaration order (D-0139/#808, D-0066 order
    /// preserved per #1066). Enriched from a bare `Vec<Ty>` to full [`FieldInfo`]
    /// records (#1257, E1): each field carries its name, resolved type, declared
    /// (syntactic) type, 0-based positional index, attribute list, and span —
    /// the data compile-time reflection (`reflect.type_info`) reports. The
    /// `value struct` auto-derive check (`Equatable`+`Hashable`, iff every field
    /// is itself hashable) reads `field.ty`. Empty is a legitimate value (a
    /// fieldless struct). Compiler-synthesized types (e.g. a generator state
    /// `$gen:…`) register with no fields.
    pub fields: Vec<FieldInfo>,
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
    /// The interface's own generic type-parameter names in declaration order
    /// (#1205): the `T` in `interface Container[T]`. Empty for a plain,
    /// non-generic interface. A method signature that names one of these keeps
    /// it as a `Ty::Named(param)` type variable so structural conformance can
    /// substitute the concrete type arguments (`T := A`) before matching.
    pub generics: Vec<String>,
    /// The interface's associated/output type names in declaration order
    /// (#1207): the `Out` in `interface Producer[In]: type Out`. A method that
    /// names one keeps it as a `Ty::Named(assoc)` type variable; a conformer
    /// binds it structurally, and bounded code projects it as `T.Out`. Empty for
    /// an interface with no associated types.
    pub assoc_types: Vec<String>,
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

// ---------------------------------------------------------------------------
// Compile-time type introspection — the read-only `reflect.type_info(T)` model
// (#1257, epic #1248 phase E1; design resolved in Discussion #1243).
//
// E1 is the smallest shippable slice of the reflection epic: a read-only
// description of a type's *shape*, assert-tested host-side. There is **no
// comptime evaluator, no codegen, and no runtime materialization** here — the
// comptime-unrolled `for`, typed `reflect.field`, the kind-switch and `@derive`
// are later phases (#1250/#1251/#1252/#1253, E3). This module defines only the
// value model; the resolver that maps a `Ty`/`Type` onto it lives in `zapc_sema`
// (it needs the struct/enum registries and `resolve_type`).
// ---------------------------------------------------------------------------

/// The broad category a type falls into for compile-time introspection
/// (Discussion #1243). Nominal kinds (`Struct`/`Enum`) carry a type name;
/// `Primitive` carries its canonical spelling; the structural container kinds
/// (`Tuple`/`List`/`Dict`/`Optional`) carry element/key/value types but no
/// nominal name. The set is deliberately the one E1 describes; later phases
/// extend it if a consumer needs more (e.g. `set`/`heap`), which is why a
/// container outside this set resolves to the "not available for introspection"
/// [`ReflectError`] rather than silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeKind {
    Struct,
    Enum,
    Tuple,
    Primitive,
    List,
    Dict,
    Optional,
}

impl TypeKind {
    /// The lowercase spelling used in diagnostics and (eventually) the comptime
    /// `TypeKind` enum surface.
    pub fn as_str(&self) -> &'static str {
        match self {
            TypeKind::Struct => "struct",
            TypeKind::Enum => "enum",
            TypeKind::Tuple => "tuple",
            TypeKind::Primitive => "primitive",
            TypeKind::List => "list",
            TypeKind::Dict => "dict",
            TypeKind::Optional => "optional",
        }
    }
}

/// One argument of a reflected attribute (#1257, HQ9). `name` is set for a
/// keyword argument (`rename="id"` → `name = Some("rename")`), `None` for a bare
/// positional argument. `value` is the argument rendered to a canonical string
/// (a string literal without its quotes, a numeric lexeme, `true`/`false`, a
/// bare name, or `…` for a form E1 does not render). E1 captures attributes as
/// *structured, comptime-queryable data* only — there is no per-attribute
/// compiler semantics; a library layer (E3) decides what an attribute means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttrArg {
    pub name: Option<String>,
    pub value: String,
}

/// A reflected attribute (decorator) recorded on a struct field (#1257, HQ9):
/// the `@field(rename="id")` / `@default(0)` / `@validate(max_len=3)` metadata
/// already parsed on the field. Name plus its argument list, in source order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttrInfo {
    pub name: String,
    pub args: Vec<AttrArg>,
    pub span: Span,
}

/// A reflected data field — a struct field or an enum-variant payload field
/// (#1257). Carries everything the `@derive`/serde layer (E3) and its
/// diagnostics (HQ4) will need: the field name, its resolved canonical [`Ty`],
/// the declared (syntactic) [`Type`] exactly as written (which, unlike the
/// resolved `Ty`, preserves an `Optional`/container element type), its 0-based
/// positional index, the attribute list, and the source span.
#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub name: String,
    /// The field's resolved, canonical type. This is what the rest of the
    /// compiler (mono/codegen, the auto-hashable check) reads.
    pub ty: Ty,
    /// The declared type exactly as written in source. Retained alongside `ty`
    /// because the resolved `Ty` erases an `Optional[T]`'s element (sema tracks
    /// `T?` opaquely as `Named("Optional")`); the declared `Type` keeps it, so a
    /// later phase can introspect an optional/container field's element.
    pub decl_ty: Type,
    pub index: usize,
    pub attrs: Vec<AttrInfo>,
    pub span: Span,
}

/// A reflected enum variant (#1257, HQ8): its name, 0-based declaration index,
/// payload fields (empty for a unit/tier-1 variant), attributes (always empty in
/// E1 — the grammar has no variant-level attribute surface yet), and span.
#[derive(Clone, Debug)]
pub struct VariantInfo {
    pub name: String,
    pub index: usize,
    pub fields: Vec<FieldInfo>,
    pub attrs: Vec<AttrInfo>,
    pub span: Span,
}

/// The kind-specific payload of a [`TypeInfo`] (#1257). An enum rather than a
/// bag of optional fields so each kind carries exactly — and only — the data it
/// can have, and a `match` over it is exhaustive.
#[derive(Clone, Debug)]
pub enum TypeShape {
    /// A user struct: nominal, with its declared data fields in declaration
    /// order (#1066).
    Struct {
        name: String,
        fields: Vec<FieldInfo>,
    },
    /// A user enum: nominal, with its variants in declaration order.
    Enum {
        name: String,
        variants: Vec<VariantInfo>,
    },
    /// A tuple: structural, positional element types (the empty tuple `()` has
    /// no elements).
    Tuple { elems: Vec<Ty> },
    /// A primitive scalar/string type, carrying its canonical spelling
    /// (`int64`, `bool`, `str`, `char`, `float64`, …).
    Primitive { name: String },
    /// `list[T]`: structural, element type `T`.
    List { elem: Ty },
    /// `dict[K, V]`: structural, key type `K` and value type `V`.
    Dict { key: Ty, value: Ty },
    /// `T?` / `Optional[T]`: structural, element type `T`. `elem` is
    /// [`Ty::Unknown`] only for a bare, un-parameterized `Optional`.
    Optional { elem: Ty },
}

/// The read-only introspection record `reflect.type_info(T)` yields (#1257): a
/// type's shape as a compile-time value. Produced by the resolver in
/// `zapc_sema`; consumed (indexed / iterated) only by the later comptime
/// evaluator (E2). The thin accessors mirror the eventual comptime surface
/// (`ti.kind`, `ti.name`, `ti.fields`, `ti.variants`).
#[derive(Clone, Debug)]
pub struct TypeInfo {
    pub shape: TypeShape,
}

impl TypeInfo {
    pub fn kind(&self) -> TypeKind {
        match &self.shape {
            TypeShape::Struct { .. } => TypeKind::Struct,
            TypeShape::Enum { .. } => TypeKind::Enum,
            TypeShape::Tuple { .. } => TypeKind::Tuple,
            TypeShape::Primitive { .. } => TypeKind::Primitive,
            TypeShape::List { .. } => TypeKind::List,
            TypeShape::Dict { .. } => TypeKind::Dict,
            TypeShape::Optional { .. } => TypeKind::Optional,
        }
    }

    /// The nominal name — `Some` for a struct/enum (the type name) and for a
    /// primitive (its canonical spelling); `None` for the structural container
    /// kinds (tuple/list/dict/optional).
    pub fn name(&self) -> Option<&str> {
        match &self.shape {
            TypeShape::Struct { name, .. } | TypeShape::Enum { name, .. } => Some(name),
            TypeShape::Primitive { name } => Some(name),
            TypeShape::Tuple { .. }
            | TypeShape::List { .. }
            | TypeShape::Dict { .. }
            | TypeShape::Optional { .. } => None,
        }
    }

    /// The struct's declared data fields, in declaration order; empty for any
    /// non-struct kind.
    pub fn fields(&self) -> &[FieldInfo] {
        match &self.shape {
            TypeShape::Struct { fields, .. } => fields,
            _ => &[],
        }
    }

    /// The enum's variants, in declaration order; empty for any non-enum kind.
    pub fn variants(&self) -> &[VariantInfo] {
        match &self.shape {
            TypeShape::Enum { variants, .. } => variants,
            _ => &[],
        }
    }
}

/// The failure of a `type_info(T)` resolution (#1257): `T` is a type E1 does not
/// describe — an unsupported container (`set`/`heap`/`channel`/…), a function
/// type, a Python handle, or an un-inferable/opaque type. `message` is the exact
/// "not available for introspection" diagnostic and `span` points at the type as
/// written, so a later phase (E2's comptime call site) can surface it as a real,
/// span-attributed compile error instead of a panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReflectError {
    pub message: String,
    pub span: Span,
}

impl ReflectError {
    /// Build the canonical "not available for introspection" error for a type
    /// rendered as `display`, anchored at `span`.
    pub fn unsupported(display: &str, span: Span) -> Self {
        ReflectError {
            message: format!("`{display}` is not available for introspection"),
            span,
        }
    }
}

/// Classify a resolved [`Ty`] as an introspectable primitive, returning its
/// canonical spelling (#1257). These are the leaf scalar/string value types that
/// have no further sub-structure to reflect. Collections, views, user types,
/// and un-inferable types are not primitives (they are other kinds, or
/// unsupported). An untyped literal reports its concrete default.
pub fn primitive_name(ty: &Ty) -> Option<&'static str> {
    Some(match ty {
        Ty::Int64 | Ty::UntypedInt => "int64",
        Ty::Int32 => "int32",
        Ty::Uint8 => "uint8",
        Ty::Uint16 => "uint16",
        Ty::Uint32 => "uint32",
        Ty::Uint64 => "uint64",
        Ty::Usize => "usize",
        Ty::Float64 | Ty::UntypedFloat => "float64",
        Ty::Float32 => "float32",
        Ty::Bool => "bool",
        Ty::Str => "str",
        Ty::Bytes => "bytes",
        Ty::Char => "char",
        Ty::Bigint => "bigint",
        Ty::Complex => "complex",
        _ => return None,
    })
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
    fn reflect_primitive_name_classification() {
        // Scalars / string value types are primitives; untyped literals report
        // their concrete default.
        assert_eq!(primitive_name(&Ty::Int64), Some("int64"));
        assert_eq!(primitive_name(&Ty::UntypedInt), Some("int64"));
        assert_eq!(primitive_name(&Ty::UntypedFloat), Some("float64"));
        assert_eq!(primitive_name(&Ty::Char), Some("char"));
        assert_eq!(primitive_name(&Ty::Bigint), Some("bigint"));
        assert_eq!(primitive_name(&Ty::Bytes), Some("bytes"));
        // Collections, user types, and un-inferable types are not primitives.
        assert_eq!(primitive_name(&Ty::List(Box::new(Ty::Int64))), None);
        assert_eq!(primitive_name(&Ty::Named("Point".into())), None);
        assert_eq!(primitive_name(&Ty::Unknown), None);
        assert_eq!(primitive_name(&Ty::Unit), None);
    }

    #[test]
    fn reflect_typeinfo_accessors_per_shape() {
        let s = TypeInfo {
            shape: TypeShape::Struct {
                name: "Point".into(),
                fields: vec![FieldInfo {
                    name: "x".into(),
                    ty: Ty::Int64,
                    decl_ty: Type::Named {
                        name: "int64".into(),
                        args: vec![],
                        span: Span::default(),
                    },
                    index: 0,
                    attrs: vec![],
                    span: Span::default(),
                }],
            },
        };
        assert_eq!(s.kind(), TypeKind::Struct);
        assert_eq!(s.name(), Some("Point"));
        assert_eq!(s.fields().len(), 1);
        assert!(s.variants().is_empty());

        let list = TypeInfo {
            shape: TypeShape::List { elem: Ty::Str },
        };
        assert_eq!(list.kind(), TypeKind::List);
        assert_eq!(list.name(), None, "a container is structural");
        assert!(list.fields().is_empty());

        let prim = TypeInfo {
            shape: TypeShape::Primitive {
                name: "bool".into(),
            },
        };
        assert_eq!(prim.kind(), TypeKind::Primitive);
        assert_eq!(prim.name(), Some("bool"));
    }

    #[test]
    fn reflect_error_message_is_the_exact_phrase() {
        let e = ReflectError::unsupported("set[int64]", Span::new(3, 9));
        assert_eq!(e.message, "`set[int64]` is not available for introspection");
        assert_eq!(e.span, Span::new(3, 9));
    }

    #[test]
    fn num_join_is_width_stable() {
        // #1654 / ADR `decisions/lang/1654-fixed-width-integer-semantics.md`:
        // fixed-width integer arithmetic is width-stable and wraps at the
        // operand width, superseding the promote-to-64 clause of D-0019 / #552.

        // Rule 1: same-width typed operands stay that width (no promote-to-64).
        assert_eq!(num_join(&Ty::Uint8, &Ty::Uint8), Ty::Uint8);
        assert_eq!(num_join(&Ty::Uint16, &Ty::Uint16), Ty::Uint16);
        assert_eq!(num_join(&Ty::Uint32, &Ty::Uint32), Ty::Uint32);
        assert_eq!(num_join(&Ty::Uint64, &Ty::Uint64), Ty::Uint64);
        assert_eq!(num_join(&Ty::Int32, &Ty::Int32), Ty::Int32);
        assert_eq!(num_join(&Ty::Int64, &Ty::Int64), Ty::Int64);
        assert_eq!(num_join(&Ty::Usize, &Ty::Usize), Ty::Usize);

        // Rule 2: mixed-width typed operands widen to the larger operand.
        assert_eq!(num_join(&Ty::Uint8, &Ty::Uint16), Ty::Uint16);
        assert_eq!(num_join(&Ty::Uint16, &Ty::Uint8), Ty::Uint16);
        assert_eq!(num_join(&Ty::Uint8, &Ty::Uint32), Ty::Uint32);
        assert_eq!(num_join(&Ty::Uint16, &Ty::Uint64), Ty::Uint64);
        assert_eq!(num_join(&Ty::Uint32, &Ty::Uint64), Ty::Uint64);
        assert_eq!(num_join(&Ty::Int32, &Ty::Int64), Ty::Int64);
        assert_eq!(num_join(&Ty::Int64, &Ty::Int32), Ty::Int64);
        // `usize` widens the narrow unsigned types just like `uint64` does, and
        // the equal-width `usize`/`uint64` pair resolves to the concrete width.
        assert_eq!(num_join(&Ty::Uint8, &Ty::Usize), Ty::Usize);
        assert_eq!(num_join(&Ty::Usize, &Ty::Uint64), Ty::Uint64);
        assert_eq!(num_join(&Ty::Uint64, &Ty::Usize), Ty::Uint64);

        // Rule 3: an untyped literal adopts the concrete operand's type instead
        // of defaulting to `int64`/`uint64` (order-independent).
        assert_eq!(num_join(&Ty::Uint8, &Ty::UntypedInt), Ty::Uint8);
        assert_eq!(num_join(&Ty::UntypedInt, &Ty::Uint8), Ty::Uint8);
        assert_eq!(num_join(&Ty::Uint32, &Ty::UntypedInt), Ty::Uint32);
        assert_eq!(num_join(&Ty::Int32, &Ty::UntypedInt), Ty::Int32);
        assert_eq!(num_join(&Ty::UntypedInt, &Ty::Int32), Ty::Int32);
        assert_eq!(num_join(&Ty::Uint64, &Ty::UntypedInt), Ty::Uint64);
        assert_eq!(num_join(&Ty::UntypedInt, &Ty::Usize), Ty::Usize);
        assert_eq!(num_join(&Ty::Int64, &Ty::UntypedInt), Ty::Int64);

        // Rule 4: no typed context → the `int64` default (unchanged).
        assert_eq!(num_join(&Ty::UntypedInt, &Ty::UntypedInt), Ty::Int64);

        // The non-integer families still win and are unaffected by rules 1–4.
        assert_eq!(num_join(&Ty::Uint8, &Ty::Float64), Ty::Float64);
        assert_eq!(num_join(&Ty::Int32, &Ty::Float32), Ty::Float64);
        assert_eq!(num_join(&Ty::UntypedInt, &Ty::UntypedFloat), Ty::Float64);
        assert_eq!(num_join(&Ty::Int64, &Ty::Complex), Ty::Complex);
        assert_eq!(num_join(&Ty::Int32, &Ty::Bigint), Ty::Bigint);
        assert_eq!(num_join(&Ty::UntypedInt, &Ty::Bigint), Ty::Bigint);
    }

    #[test]
    fn int_width_covers_the_whole_fixed_width_family() {
        // The width-stable join keys off `int_width`, which (unlike
        // `uint_width`) must report the signed family too (#1654).
        assert_eq!(Ty::Int32.int_width(), Some(32));
        assert_eq!(Ty::Int64.int_width(), Some(64));
        assert_eq!(Ty::Uint8.int_width(), Some(8));
        assert_eq!(Ty::Uint16.int_width(), Some(16));
        assert_eq!(Ty::Uint32.int_width(), Some(32));
        assert_eq!(Ty::Uint64.int_width(), Some(64));
        assert_eq!(Ty::Usize.int_width(), Some(64));
        // Not a fixed-width machine integer.
        assert_eq!(Ty::Bigint.int_width(), None);
        assert_eq!(Ty::UntypedInt.int_width(), None);
        assert_eq!(Ty::Float64.int_width(), None);
    }
}
