//! ZIR data structures — see `design/IR.md`.
//!
//! A [`Module`] holds [`Func`]s. Each function is SSA over numbered [`Value`]s
//! (values `0..params.len()` are the parameters) with a per-value type table,
//! and a list of [`Block`]s each ending in a [`Term`]. Mutable locals are
//! lowered via stack slots (`Alloca`/`Load`/`Store`); control flow is explicit.

use std::fmt;

/// The machine representation of a collection element, key, or value. Elements
/// are stored as uniform 8-byte words by the runtime (see `design/RUNTIME-ABI.md`
/// §"Collection layout"): integers/bools fit directly, reference types are a
/// pointer. This two-way split is all codegen needs; it is what selects the
/// typed runtime entry point (e.g. `zap_list_push_i64` vs `zap_list_push_ptr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Elem {
    /// An integer element, passed as `i64`.
    I64,
    /// A `bool` element (#274, D-0124). Physically the same `i64` word as
    /// [`Elem::I64`] (val/key suffix `i64`, read-back type `i64` — so codegen,
    /// GC, hashing, and read-back are byte-identical to an integer element), but
    /// it retains `bool` identity so the Display protocol can render a `bool`
    /// collection element as `True`/`False` (via `zap_bool_to_str`) rather than
    /// the integer `1`/`0`. `bool` is `Ty::I1`, distinct from every integer, so
    /// this is unambiguous.
    Bool,
    /// A `char` element (ticket #709). Physically the same `i64` word as
    /// [`Elem::I64`] (val/key suffix `i64`, read-back type `i64` — so codegen,
    /// GC, hashing, and read-back are byte-identical to an integer element), but
    /// it retains `char` identity so the Display protocol renders a `char`
    /// collection element / dict key / dict value Python-style (`'a'`, via
    /// `zap_char_to_str`) rather than its decimal codepoint. `char` is the
    /// distinct [`Ty::Char`], so this is unambiguous — an `int32` element stays
    /// [`Elem::I64`] and renders as a decimal.
    Char,
    /// A floating-point (`f64`) element (I#569). Stored in the same uniform
    /// 8-byte word as an `i64` element — the runtime reinterprets the bits — so
    /// it reuses the word storage via the `_f64` entry points
    /// (`zap_list_push_f64` / `zap_list_get_f64`, `zap_dict_*_f64`), which
    /// push/return a `double`. Admitted in every collection position: list
    /// elements (I#569), dict VALUES / heap / channel (#800 Phase 1), and the
    /// hashed dict KEY / set MEMBER positions (#800 Phase 2 / #803). In a hashed
    /// position the runtime canonicalises `-0.0 → +0.0` and rejects NaN with a
    /// catchable `ValueError`, then hashes/compares the canonical bits as a plain
    /// `i64` word (D-0138).
    F64,
    /// A tier-1 enum discriminant, passed as `i64` but retaining enum identity.
    Enum(u32),
    /// A reference-typed element (`str`, nested collection), passed as a pointer
    /// and — as a *key* — hashed/compared by its bytes (a `ZapStr`).
    Ptr,
    /// A user-struct element (operator overloading, D-0047). Stored as a pointer
    /// word, but as a dict/set *key* it is hashed and compared by the struct's
    /// own `__hash__` / `__eq__` rather than by raw bytes, so it routes to the
    /// function-pointer runtime entry points (key suffix `fn`). The `u32` indexes
    /// [`Module::structs`].
    Struct(u32),
    /// A **nested collection** element — a `list`/`dict`/`set`/`heap`/`channel`
    /// stored inside another collection (#625). Physically a pointer word, exactly
    /// like [`Elem::Ptr`], so it pushes/reads through the `_ptr` runtime entry
    /// points and GC-traces as a pointer. The distinction exists purely so
    /// *lowering* can recover the element's full ZIR type for read-back dispatch:
    /// a flat `Ptr` loses whether the element is a `str` or a nested `list[int64]`,
    /// which silently miscompiled `rows[i]`/`len(rows[i])` as string operations.
    /// The `u32` indexes a lowering-only interner of the element's [`Ty`] (it is
    /// never serialized into the module — codegen treats every collection uniformly
    /// as a traced pointer, so it never dereferences the id). [`Elem::ty`] has no
    /// interner, so it falls back to [`Ty::Ptr`]; the lowering resolves the real
    /// nested type through its interner instead.
    Nested(u32),
}

impl Elem {
    /// The ZIR type a value of this element kind flows as.
    pub fn ty(self) -> Ty {
        match self {
            // `Bool` reads back as the `i64` word it is stored as (its `bool`
            // identity is used only by the renderer, which special-cases the
            // `Elem`), so read-back width is unchanged from an integer element.
            Elem::I64 | Elem::Bool | Elem::Char => Ty::I64,
            Elem::F64 => Ty::F64,
            Elem::Enum(id) => Ty::Enum(id),
            Elem::Ptr => Ty::Ptr,
            Elem::Struct(sid) => Ty::Struct(sid),
            // A nested collection flows as a pointer when the interner is out of
            // reach; lowering resolves the precise `list`/`dict`/... type itself.
            Elem::Nested(_) => Ty::Ptr,
        }
    }
    /// The runtime-entry-point suffix for an element/value of this kind. Struct
    /// *values* are stored as plain pointer words (only *keys* need per-type
    /// hashing), so they share the `ptr` path.
    pub fn val_suffix(self) -> &'static str {
        match self {
            Elem::I64 | Elem::Bool | Elem::Char | Elem::Enum(_) => "i64",
            Elem::F64 => "f64",
            Elem::Ptr | Elem::Struct(_) | Elem::Nested(_) => "ptr",
        }
    }
    /// The runtime-entry-point suffix for a *key* of this kind. Pointer keys are
    /// strings (hashed/compared by contents); struct keys use the `fn` path,
    /// which takes the type's `__hash__`/`__eq__` as function pointers.
    pub fn key_suffix(self) -> &'static str {
        match self {
            Elem::I64 | Elem::Bool | Elem::Char | Elem::Enum(_) => "i64",
            Elem::Ptr => "str",
            Elem::Struct(_) => "fn",
            // A float dict KEY / set MEMBER (#803) routes to the `_f64` key entry
            // points (`zap_dict_set_f64_*`, `zap_set_add_f64`, …), which take the
            // key as a `double`, canonicalise `-0.0`, and reject NaN (D-0138).
            Elem::F64 => "f64",
            // A nested collection is unhashable (lists/dicts/sets cannot be dict
            // or set keys), so this is never reached; share the pointer-key path
            // for a defined value rather than panicking.
            Elem::Nested(_) => "str",
        }
    }
}

/// The element dtype of a numeric `array[T]` (tickets #24, #49, #503, #890).
/// The full dense matrix: floats `f32`/`f64`, signed `i8`/`i16`/`i32`/`i64`,
/// unsigned `u8`/`u16`/`u32`/`u64`, and `bool` (a 0/1 byte mask). The dtype
/// selects the typed runtime entry point (`zap_array_*_<suffix>`) and the LLVM
/// *storage* element type of the SIMD element-wise lowering (see
/// [`Dtype::storage_ty`]). Element values cross the runtime ABI as a uniform
/// word — every integer as `i64`, every float as `f64` (see [`Dtype::ty`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dtype {
    F64,
    F32,
    I64,
    I32,
    I16,
    I8,
    /// 8-bit unsigned integer (`uint8`/`u8`), e.g. image pixels. Integer
    /// arithmetic wraps mod 2^width (D-0005).
    Uint8,
    U16,
    U32,
    U64,
    /// Boolean mask (`bool`), stored as a 0/1 byte. Arithmetic on bools
    /// promotes to `uint8` (see [`Dtype::promote`]).
    Bool,
}

impl Dtype {
    /// The ZIR scalar type of one element as it crosses the runtime ABI
    /// (get/set/sum values): every **integer** dtype is carried as an `i64` word
    /// (the runtime truncates/wraps on store, sign/zero-extends on load) and
    /// every **float** dtype as an `f64` word (narrowed on store, widened on
    /// load). So `a[i]` and `sum(a)` are ordinary `int64`/`float64`; the compact
    /// storage width is [`Dtype::storage_ty`].
    pub fn ty(self) -> Ty {
        if self.is_float() {
            Ty::F64
        } else {
            Ty::I64
        }
    }
    /// The LLVM *storage* element type in the contiguous buffer, by width.
    /// Unsigned widths share the signed LLVM integer type (LLVM ints are
    /// sign-agnostic); [`Dtype::is_unsigned`] drives the zext-vs-sext choice.
    /// The SIMD element-wise kernel loads/stores/GEPs with this type.
    pub fn storage_ty(self) -> Ty {
        match self {
            Dtype::F64 => Ty::F64,
            Dtype::F32 => Ty::F32,
            Dtype::I64 | Dtype::U64 => Ty::I64,
            Dtype::I32 | Dtype::U32 => Ty::I32,
            Dtype::I16 | Dtype::U16 => Ty::I16,
            Dtype::I8 | Dtype::Uint8 | Dtype::Bool => Ty::I8,
        }
    }
    /// The size in bytes of one stored element (the SIMD load/store alignment).
    pub fn elem_size(self) -> u32 {
        match self {
            Dtype::F64 | Dtype::I64 | Dtype::U64 => 8,
            Dtype::F32 | Dtype::I32 | Dtype::U32 => 4,
            Dtype::I16 | Dtype::U16 => 2,
            Dtype::I8 | Dtype::Uint8 | Dtype::Bool => 1,
        }
    }
    /// The runtime-entry-point suffix for this dtype.
    pub fn suffix(self) -> &'static str {
        match self {
            Dtype::F64 => "f64",
            Dtype::F32 => "f32",
            Dtype::I64 => "i64",
            Dtype::I32 => "i32",
            Dtype::I16 => "i16",
            Dtype::I8 => "i8",
            Dtype::Uint8 => "u8",
            Dtype::U16 => "u16",
            Dtype::U32 => "u32",
            Dtype::U64 => "u64",
            Dtype::Bool => "bool",
        }
    }
    /// The runtime dtype tag passed to `zap_array_new`/`_new_nd` (matches the
    /// `DTYPE_*` enum in `zaprt.c`). Existing codes (0..=3) are preserved so the
    /// ABI is stable; the wider dtypes take fresh codes.
    pub fn code(self) -> i64 {
        match self {
            Dtype::F64 => 0,
            Dtype::I64 => 1,
            Dtype::Uint8 => 2,
            Dtype::Bool => 3,
            Dtype::F32 => 4,
            Dtype::I32 => 5,
            Dtype::I8 => 6,
            Dtype::I16 => 7,
            Dtype::U16 => 8,
            Dtype::U32 => 9,
            Dtype::U64 => 10,
        }
    }
    pub fn is_float(self) -> bool {
        matches!(self, Dtype::F32 | Dtype::F64)
    }
    /// Whether the *stored* integer is unsigned (`uint8`/`u16`/`u32`/`u64`, and
    /// `bool` as a 0/1 byte). Selects zero-extend / unsigned-int-to-float in the
    /// promotion conversions; the signed `iN` dtypes use sext/sitofp. (A float
    /// dtype is not an integer; the result is unused for it.)
    pub fn is_unsigned(self) -> bool {
        matches!(
            self,
            Dtype::Uint8 | Dtype::U16 | Dtype::U32 | Dtype::U64 | Dtype::Bool
        )
    }
    /// Whether this is a signed integer dtype (`i8`/`i16`/`i32`/`i64`).
    fn is_signed_int(self) -> bool {
        matches!(self, Dtype::I8 | Dtype::I16 | Dtype::I32 | Dtype::I64)
    }
    /// The logical width in bits of an **integer** dtype (`bool` counts as 1).
    /// Not meaningful for a float dtype.
    fn int_bits(self) -> u32 {
        match self {
            Dtype::Bool => 1,
            Dtype::I8 | Dtype::Uint8 => 8,
            Dtype::I16 | Dtype::U16 => 16,
            Dtype::I32 | Dtype::U32 => 32,
            Dtype::I64 | Dtype::U64 => 64,
            Dtype::F32 => 32,
            Dtype::F64 => 64,
        }
    }
    /// The integer dtype of a given signedness and width (8/16/32/64 bits).
    fn int_dtype(signed: bool, bits: u32) -> Dtype {
        match (signed, bits) {
            (true, 8) => Dtype::I8,
            (true, 16) => Dtype::I16,
            (true, 32) => Dtype::I32,
            (true, _) => Dtype::I64,
            (false, 8) => Dtype::Uint8,
            (false, 16) => Dtype::U16,
            (false, 32) => Dtype::U32,
            (false, _) => Dtype::U64,
        }
    }
    /// The result dtype of an element-wise arithmetic op on two array operands
    /// (NumPy NEP-50 type promotion, D-0166). A float operand forces a float
    /// result of the **widest float** among the float operands (an integer never
    /// bumps `f32` to `f64`: `i64 + f32 -> f32`). Two integers of the **same
    /// signedness** promote to the wider; **mixed** signed/unsigned promote to
    /// the signed type that holds both — the signed operand's width if it is
    /// strictly wider, else the next signed width up (`u8 + i8 -> i16`,
    /// `u32 + i32 -> i64`), and `u64` against any signed falls back to `f64`
    /// (no 128-bit int). Arithmetic never *yields* `bool`: `bool + bool -> u8`
    /// (`True + True == 2`), and `bool` otherwise defers to the other operand.
    pub fn promote(a: Dtype, b: Dtype) -> Dtype {
        use Dtype::*;
        // `bool` is the weakest; arithmetic never yields it.
        match (a, b) {
            (Bool, Bool) => return Uint8,
            (Bool, other) | (other, Bool) => return other,
            _ => {}
        }
        // A float operand dominates; the result float width is the widest float
        // operand only (an int operand does not widen f32 -> f64).
        if a.is_float() || b.is_float() {
            let aw = if a.is_float() { a.int_bits() } else { 0 };
            let bw = if b.is_float() { b.int_bits() } else { 0 };
            return if aw.max(bw) >= 64 { F64 } else { F32 };
        }
        // Both integers.
        let (asig, bsig) = (a.is_signed_int(), b.is_signed_int());
        let (aw, bw) = (a.int_bits(), b.int_bits());
        if asig == bsig {
            return Dtype::int_dtype(asig, aw.max(bw));
        }
        // Mixed signed/unsigned: the unsigned width `uw`, the signed width `sw`.
        let (uw, sw) = if asig { (bw, aw) } else { (aw, bw) };
        if uw < sw {
            // The signed type already covers the unsigned operand's range.
            Dtype::int_dtype(true, sw)
        } else {
            // Need a signed type one size up to hold the unsigned's range;
            // `u64` has none, so NumPy falls back to `f64`.
            match uw {
                8 => I16,
                16 => I32,
                32 => I64,
                _ => F64,
            }
        }
    }
    /// The result dtype of `array op scalar` broadcast. Scalars are **weak**
    /// (NEP-50): an integer scalar keeps the array's dtype (`uint8_image + 1`
    /// stays `uint8`, `f32_buf + 2` stays `f32`); a float scalar keeps a float
    /// array's width (`f32_buf + 1.5 -> f32`) and otherwise forces `f64`. A
    /// `bool` array promotes to `uint8` first (arithmetic, not a mask).
    pub fn promote_scalar(arr: Dtype, scalar_is_float: bool) -> Dtype {
        if scalar_is_float {
            if arr.is_float() {
                arr
            } else {
                Dtype::F64
            }
        } else if matches!(arr, Dtype::Bool) {
            Dtype::Uint8
        } else {
            arr
        }
    }
}

/// A ZIR type (lower-level than a Zap type; reference types become `Ptr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    I1,
    I8,
    I16,
    I32,
    I64,
    /// Unsigned fixed-width integers (#552, D-0019 amendment). Physically the
    /// same LLVM `i8`/`i16`/`i32`/`i64` as the signed widths — LLVM integers are
    /// sign-agnostic — but kept as distinct ZIR types so lowering and codegen can
    /// select the *unsigned* operations: `udiv`/`urem`, the unsigned `icmp`
    /// predicates, a logical (not arithmetic) right shift, zero-extension on
    /// widening, and unsigned decimal printing. Arithmetic wraps modulo 2^width
    /// (two's complement), matching C. The source `usize` lowers to [`Ty::U64`]
    /// (pointer width on the supported 64-bit targets); it is not a separate ZIR
    /// type because it is byte-identical to `uint64` in codegen.
    U8,
    U16,
    U32,
    U64,
    /// A Unicode scalar `char` (ticket #709). Physically an `i32` codepoint —
    /// codegen maps it to the LLVM `i32` type and every arithmetic / comparison /
    /// coercion / hash path treats it exactly like [`Ty::I32`] (same
    /// [`int_width`](Ty::int_width), same word). It is a *distinct* IR type only
    /// so the value layer can tell a `char` apart from an `int32`: that is what
    /// lets the Display protocol render a `char` Python-style (`'a'`, via
    /// `zap_char_to_str`) while an `int32` renders as its decimal value, in every
    /// position (scalar, struct field, tuple/collection element). `char` arises
    /// only from `s[i]`, `chr()`/`char()`, and string iteration — there is no
    /// `char` literal.
    Char,
    /// A tier-1 enum value carried as its `i64` discriminant. The id preserves
    /// the source enum identity through lowering without changing its ABI.
    Enum(u32),
    F32,
    F64,
    Ptr,
    /// A pointer to a struct instance; the `u32` indexes [`Module::structs`].
    Struct(u32),
    /// A pointer to a `list` object with the given element representation.
    List(Elem),
    /// A pointer to a `dict` object with the given key and value representations.
    Dict(Elem, Elem),
    /// A pointer to a `set` object with the given element representation. Elements
    /// are hashed/compared like dict keys, so they use [`Elem::key_suffix`].
    Set(Elem),
    /// A pointer to a `heap` object (D-0057): a binary min-heap priority queue
    /// over the given element representation. Elements are *ordered* (not hashed),
    /// so the element kind selects the `_i64` / `_ptr` runtime entry point and the
    /// runtime compares `i64` words numerically and pointer words as `ZapStr`
    /// bytes.
    Heap(Elem),
    /// A pointer to a numeric `array` object with the given element dtype and
    /// **static rank** (`ndim`, ticket #24 / #500, D-0164). The rank is part of
    /// the type so indexing can be typed statically: a full index (one per axis)
    /// yields a scalar, a partial index (fewer than `ndim`) yields a lower-rank
    /// `array` view, and element-wise ops require equal rank. The runtime
    /// `ZapArray` header carries the concrete per-axis `dims`; the rank here is
    /// always `>= 1`. A bare `array[T]` type annotation denotes rank 1 (the
    /// historical default); N-D values arise through inference (`auto`), and an
    /// explicit N-D annotation syntax is a documented follow-up.
    Array(Dtype, u8),
    /// A pointer to a runtime channel object (ticket #25, D-0031) whose elements
    /// are stored as uniform 8-byte words of the given kind. The element kind
    /// selects the typed send/receive runtime entry point (`zap_chan_send_i64`
    /// vs `zap_chan_send_ptr`), exactly like [`Ty::List`]. Maps to `ptr` in
    /// codegen.
    Chan(Elem),
    /// A pointer to a runtime `AtomicArray[T]` object (#844): a fixed-capacity
    /// array of atomically CAS-able 8-byte word slots. The element kind selects
    /// how a stored word is interpreted (a reference is ptrtoint/inttoptr'd at the
    /// lowering boundary, like the field-based [`Inst::AtomicCas`]) and whether the
    /// collector traces the slots. Maps to `ptr` in codegen, like [`Ty::Chan`].
    AtomicArray(Elem),
    /// A pointer to an immutable `bytes` object (ticket #337): physically the
    /// same `ZapStr {len, data}` length-prefixed byte buffer as `str`, so it maps
    /// to `ptr` in codegen and coerces to/from `Ptr` as a no-op bitcast. Kept
    /// distinct from `Ptr`/`Str` purely so lowering can route the byte-wise
    /// operations (`b[i]` -> an `int64` byte, byte-based slicing, `str(b)` UTF-8
    /// decode) that differ from `str`'s codepoint semantics (D-0025).
    Bytes,
    /// A pointer to a regex match object (ticket #23, D-0010). Null means no
    /// match (falsy); owns its captured groups. Distinct from `Ptr`/`Str` so
    /// lowering can route `.group(...)` method dispatch.
    Match,
    /// A pointer to a GC-managed Python object handle (ticket #27, D-0012): the
    /// wrapper owns one CPython reference (decref'd by a finalizer). Distinct
    /// from `Ptr` so lowering can route attribute/call dispatch and marshaling
    /// through the Python runtime (`zap_py_*`). Maps to `ptr` in codegen.
    Py,
    /// A GC-managed arbitrary-precision signed integer (D-0019), represented
    /// by an immutable runtime bignum object.
    Bigint,
    /// A GC-managed IEEE-754 complex number (D-0019), represented by an
    /// immutable runtime object containing real and imaginary components.
    Complex,
    /// A **first-class function value** `fn(T, U) -> R` (I#163). Physically a
    /// pointer to a heap *closure record* `{code_ptr, captures…}` (code at
    /// offset 0); a bare top-level function used as a value is wrapped so it has
    /// the same shape. The `u32` indexes [`Module::fn_sigs`] for the call
    /// signature (params + return), which lowering needs to build the indirect
    /// call. Maps to `ptr` in codegen and is a GC-traced reference (`is_ptr`).
    Func(u32),
    Void,
}

impl Ty {
    pub fn is_float(self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }
    pub fn int_width(self) -> Option<u32> {
        match self {
            Ty::I1 => Some(1),
            Ty::I8 | Ty::U8 => Some(8),
            Ty::I16 | Ty::U16 => Some(16),
            // `char` is a 32-bit codepoint: same width as `int32`, so every
            // width-driven path (arithmetic, comparison, coercion) treats it
            // identically (#709).
            Ty::I32 | Ty::Char | Ty::U32 => Some(32),
            Ty::I64 | Ty::Enum(_) | Ty::U64 => Some(64),
            _ => None,
        }
    }

    /// Whether this ZIR type is an **unsigned** integer (#552). Drives op
    /// selection in lowering/codegen: unsigned `div`/`rem`, the unsigned `icmp`
    /// predicates, a logical right shift, zero-extension on widening, and
    /// unsigned decimal printing.
    pub fn is_unsigned(self) -> bool {
        matches!(self, Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64)
    }

    /// Whether this ZIR type is a **GC-managed heap pointer** in the physical
    /// layout codegen emits. Every reference type (`str`, collections, `array`,
    /// channels, regex matches, Python handles, and struct instances — including
    /// `value struct` fields, which are still stored as a pointer *inside* a
    /// containing object, D-0051) lowers to a pointer; the scalars and `Void` do
    /// not. The precise collector (ticket #48) uses this to decide which fields
    /// and SSA temps are roots / traceable children. `Ptr` is the untyped
    /// pointer (e.g. a GC-allocated closure / function address) and counts too.
    pub fn is_ptr(self) -> bool {
        !matches!(
            self,
            Ty::I1
                | Ty::I8
                | Ty::I16
                | Ty::I32
                | Ty::Char
                | Ty::I64
                | Ty::U8
                | Ty::U16
                | Ty::U32
                | Ty::U64
                | Ty::Enum(_)
                | Ty::F32
                | Ty::F64
                | Ty::Void
        )
    }
}

/// An SSA value id (index into [`Func::value_types`]).
pub type Value = u32;
/// A basic-block id (index into [`Func::blocks`]).
pub type BlockId = u32;

/// Which scope owns a spawned coroutine (D-0044, structured concurrency). See
/// [`Inst::Spawn`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// Bare `spawn`: the implicit root scope (joined at program exit).
    Root,
    /// `spawn` inside a `scope:` block; the value is that scope's handle.
    Scope(Value),
    /// `spawn detached`: owned by nothing, never awaited.
    Detached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div, // sdiv/udiv for ints, fdiv for floats (signed/unsigned by result type, #552)
    Rem, // srem/urem (signed/unsigned chosen by result type, #552)
    And,
    Or,
    Xor,
    Shl,
    Shr, // arithmetic (signed, ashr) or logical (unsigned, lshr) right shift, by result type (#552)
}

/// Element-wise unary math over a numeric `array` (#505, D-0168). Each maps to a
/// float LLVM intrinsic in codegen (`llvm.sqrt`/`exp`/`log`/`sin`/`cos`/`fabs`);
/// `tan` has no direct intrinsic, so codegen emits `sin/cos`. Carried by
/// [`Inst::ArrayUnaryOp`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrayMathOp {
    Sqrt,
    Exp,
    Log,
    Sin,
    Cos,
    Tan,
    Fabs,
}

/// Comparison predicate. Codegen picks the signed or unsigned integer `icmp`
/// predicate from the *operand* type (#552) and the ordered predicate for
/// floats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpPred {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastKind {
    SExt,
    /// Zero-extend a narrower *unsigned* integer to a wider one (#552). The
    /// unsigned counterpart of [`CastKind::SExt`]: codegen emits `zext`, so
    /// `uint8(200)` widened to `uint64` stays `200`, not a sign-extended value.
    ZExt,
    Trunc,
    SIToFP,
    /// Convert an *unsigned* integer to a float (#552): codegen emits `uitofp`
    /// so a `uint64` above `INT64_MAX` converts to the correct magnitude.
    UIToFP,
    FPToSI,
    /// Convert a float to an *unsigned* integer (#552): codegen emits `fptoui`.
    FPToUI,
    FPExt,
    FPTrunc,
}

/// Memory-ordering for an atomic operation (`Atomic[T]`, I#176). Only the
/// strongest ordering is emitted today — every `Atomic[T]` method is
/// **sequentially consistent** by default — but the orderings carried by the
/// atomic ZIR nodes are an explicit enum so relaxed / acquire-release can be
/// added later (e.g. a `load_relaxed()` method or an ordering argument) without
/// changing the node shapes or any existing caller. Codegen maps each variant to
/// the corresponding inkwell `AtomicOrdering`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomicOrd {
    /// `seq_cst` — a single total order across all `SeqCst` operations.
    SeqCst,
}

/// The read-modify-write operation of an [`Inst::AtomicRmw`] (`Atomic[T]`,
/// I#176). `Add`/`Sub` are the integer fetch-and-op primitives (the atomic node
/// yields the **new** value, matching Go's `atomic.Int64.Add`); `Xchg` is the
/// unconditional swap (yields the **old** value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomicRmwOp {
    Add,
    Sub,
    Xchg,
}

#[derive(Clone, Debug)]
pub enum Inst {
    IConst {
        dest: Value,
        val: i64,
    },
    FConst {
        dest: Value,
        val: f64,
    },
    BConst {
        dest: Value,
        val: bool,
    },
    /// A null pointer constant (`dest`'s value type is the pointer type). Used by
    /// the niche `Optional[T]` layout (I#154, D-0064 #4): a niche optional is a
    /// single nullable pointer, and `None` is the null value. Codegen emits the
    /// `const_null` of `dest`'s pointer type.
    NullConst {
        dest: Value,
    },
    /// `dest : i1` is true iff the pointer `val` is null. Backs the niche-optional
    /// `match` tests (`None` = null, `Some` = non-null) without a tag field.
    IsNull {
        dest: Value,
        val: Value,
    },
    Bin {
        dest: Value,
        op: BinOp,
        lhs: Value,
        rhs: Value,
    },
    ICmp {
        dest: Value,
        pred: CmpPred,
        lhs: Value,
        rhs: Value,
    },
    FCmp {
        dest: Value,
        pred: CmpPred,
        lhs: Value,
        rhs: Value,
    },
    Un {
        dest: Value,
        op: UnOp,
        val: Value,
    },
    Cast {
        dest: Value,
        kind: CastKind,
        val: Value,
    },
    /// `dest : Ptr` is a stack slot holding a `pointee`.
    Alloca {
        dest: Value,
        pointee: Ty,
    },
    /// Load `value_types[dest]` from `ptr`.
    Load {
        dest: Value,
        ptr: Value,
    },
    Store {
        ptr: Value,
        val: Value,
    },
    Select {
        dest: Value,
        cond: Value,
        a: Value,
        b: Value,
    },
    Call {
        dest: Option<Value>,
        func: String,
        args: Vec<Value>,
    },
    /// `print(...)` — lowered to runtime output; args printed space-separated.
    Print {
        args: Vec<Value>,
    },
    /// A string literal: `dest : Ptr` is a freshly built string object holding
    /// `bytes` (interned via the runtime).
    StrConst {
        dest: Value,
        bytes: Vec<u8>,
    },
    /// Allocate a zeroed struct instance; `dest : Struct(sid)`.
    StructNew {
        dest: Value,
        sid: u32,
    },
    /// A zeroed struct instance allocated in a **scope-bound arena** (#553, D-0158):
    /// the struct's leaf (pointer-free) bytes are bump-allocated from `arena` (the
    /// `ZapArena*` handle bound by the enclosing `with arena(...) as a:`) into a
    /// non-moving, never-reclaimed region, and freed as a unit at block exit.
    /// Codegen emits `zap_arena_alloc_atomic(arena, size_of(sid))` and the result
    /// `dest : Struct(sid)` is GC-rooted like any heap pointer (it must appear in
    /// codegen's `inst_dest`, #839) — the precise marker marks it in place (the
    /// region is pinned) and never traces into it (leaf-only). On a NULL (cap /
    /// heap-OOM) return the lowering raises via `zap_arena_oom`.
    ArenaNew {
        dest: Value,
        sid: u32,
        arena: Value,
    },
    /// A **stack-allocated** zeroed struct instance (escape analysis, #839): the
    /// value is a `StructNew` that the escape pass (`crates/ir/src/escape.rs`)
    /// proved does not escape its function AND whose layout is leaf / pointer-free
    /// / finalizer-free. Codegen lowers it to an entry-block `alloca` + zero-init
    /// instead of a GC heap allocation. `dest : Struct(sid)` (a pointer to the
    /// stack slot), so `FieldGet`/`FieldSet` are byte-identical to the heap case.
    /// The slot holds no outgoing heap pointers, so it needs no GC-root tracking
    /// (keeps the D-0144/#568 frame-skip) and is never relocated by moving Immix.
    /// Only the pass constructs this; `lower` always emits `StructNew`.
    StackStruct {
        dest: Value,
        sid: u32,
    },
    /// Build one string by joining `parts` in order (#839, folds in the #837
    /// lever-3(b) single-alloc f-string join). `lower_fstring` emits this in place
    /// of a left-folded `zap_str_concat` chain; `dest : Ptr` is the joined string.
    /// When `stack` is true (set by the escape pass when the result is proven
    /// non-escaping) codegen builds into an entry-block inline buffer of `cap`
    /// bytes with a runtime heap spill when the summed length exceeds `cap`;
    /// otherwise it is one heap allocation. `cap` is meaningful only when `stack`.
    StrBuild {
        dest: Value,
        parts: Vec<Value>,
        stack: bool,
        cap: u32,
    },
    /// Load field `idx` of struct `sid` from `base`; `dest` gets the field type.
    FieldGet {
        dest: Value,
        base: Value,
        sid: u32,
        idx: u32,
    },
    /// Store `val` into field `idx` of struct `sid` at `base`.
    FieldSet {
        base: Value,
        sid: u32,
        idx: u32,
        val: Value,
    },
    /// A call that may raise (exception edge, see `design/IR.md` "Reserved" and
    /// `design/RUNTIME-ABI.md` §"Exception / unwinding ABI"). Lowers to an LLVM
    /// `invoke`: control continues straight-line on normal return, or transfers
    /// to the landing-pad block `unwind` if the callee unwinds. `unwind` must be
    /// a block whose first instruction is [`Inst::LandingPad`]. The normal
    /// continuation is implicit (codegen splits the block after the invoke).
    Invoke {
        dest: Option<Value>,
        func: String,
        args: Vec<Value>,
        unwind: BlockId,
    },
    /// The entry of a landing-pad block: receives a thrown exception during
    /// unwinding. `dest : Ptr` is the in-flight exception object (the message
    /// value passed to `zap_panic`). Must be the first instruction in its block,
    /// and that block must be the `unwind` target of at least one [`Inst::Invoke`].
    LandingPad {
        dest: Value,
    },
    /// Element-wise binary op on numeric arrays (tickets #24, #49): `dest` is a
    /// fresh array holding `lhs[i] <op> rhs[i]` (or `lhs[i] <op> scalar` when
    /// `scalar`). `op` is one of `Add`/`Sub`/`Mul`. Codegen lowers this to an
    /// explicit LLVM vector loop (SIMD) with a scalar remainder — the one
    /// non-mechanical ZIR node for arrays, mirroring how `Invoke`/`LandingPad`
    /// carry the exception-edge shape.
    ///
    /// Mixed-dtype promotion (#49): the operands may have different dtypes, so
    /// each carries its own — `lhs_dtype`/`rhs_dtype` — and `out` is the
    /// promoted result dtype ([`Dtype::promote`] / [`Dtype::promote_scalar`]).
    /// Codegen loads each operand at its own storage type, widens it to `out`,
    /// does the arithmetic in `out`, and stores `out`. `lhs` is always an array;
    /// `rhs` is an array (same length) when `!scalar`, or a scalar of
    /// `rhs_dtype`'s [`Dtype::ty`] when `scalar`.
    ArrayBinOp {
        dest: Value,
        op: BinOp,
        out: Dtype,
        lhs: Value,
        lhs_dtype: Dtype,
        rhs: Value,
        rhs_dtype: Dtype,
        scalar: bool,
    },
    /// Element-wise unary math over a numeric `array` (#505, D-0168): the result
    /// is a shape-preserving array of `out` dtype, `out[i] = op(src[i])`. Codegen
    /// loads each `src` element (`src_dtype`), converts to the float result dtype
    /// `out`, applies the LLVM math intrinsic for `op`, and stores (a vectorized
    /// body with a scalar tail, like [`Inst::ArrayBinOp`]). The result is always
    /// a float dtype (`f32` for an `f32` source, else `f64`).
    ArrayUnaryOp {
        dest: Value,
        op: ArrayMathOp,
        out: Dtype,
        src: Value,
        src_dtype: Dtype,
    },
    /// Launch a coroutine running `func(args...)` (ticket #25, D-0013): a
    /// fire-and-forget `spawn`. Arguments are evaluated eagerly in the spawning
    /// coroutine (the `args` values), so argument evaluation happens-before the
    /// child starts. Codegen packages the evaluated arguments into a GC-allocated
    /// closure and hands it, with a per-target trampoline, to `zap_spawn`, which
    /// starts a GC-registered OS thread (see `crates/codegen/src/zaprt.c`). This
    /// is a non-mechanical ZIR node (codegen synthesizes the trampoline), like
    /// [`Inst::ArrayBinOp`]; the language-visible semantics are the M:N
    /// scheduler's, with the OS-thread backing a documented v1 implementation
    /// choice (the user-space work-stealing scheduler is a deferred optimization).
    Spawn {
        func: String,
        args: Vec<Value>,
        /// A human-readable identity for this coroutine, reported by
        /// `ScopeTimeout`'s pending set (D-0094/#475) in place of the possibly
        /// synthetic `func`. `None` means "use `func`" — the common case for a
        /// `spawn named_fn(...)`. A method spawn carries `Struct.method`; a
        /// first-class-function-value spawn (lowered through a generated
        /// `$spawnthunk$…`) carries the source callee's name so the report
        /// names the user's coroutine, not the internal thunk.
        name: Option<String>,
        /// Structured-concurrency ownership (D-0044, `design/0005`):
        /// - `Owner::Root` — a bare `spawn`, owned by the implicit root scope and
        ///   joined at program exit (`zap_spawn`);
        /// - `Owner::Scope(s)` — owned by the enclosing `scope:` whose handle is
        ///   the value `s`; awaited/cancelled at its dedent (`zap_spawn_scoped`);
        /// - `Owner::Detached` — `spawn detached`, owned by nothing, never
        ///   awaited, not joined at exit (`zap_spawn_detached`).
        owner: Owner,
    },
    /// The address of a top-level function, as a pointer value (`dest : Ptr`).
    /// Used to hand a struct type's `__hash__` / `__eq__` to the collection
    /// runtime so a struct can be a `dict`/`set` key (operator overloading,
    /// D-0047). Codegen lowers it to the function's global pointer.
    FuncAddr {
        dest: Value,
        func: String,
    },
    /// The address of a module-level global slot (`dest : Ptr`), indexing
    /// [`Module::globals`]. A module-level `const` (D-0112, #658 Phase 1) is a
    /// global slot initialized once before `main` by `@__zap_init_globals`; a
    /// reference to the binding lowers to this address followed by an
    /// [`Inst::Load`], exactly as a stack local lowers to its `alloca` + load.
    GlobalAddr {
        dest: Value,
        idx: u32,
    },
    /// Build a **first-class function value** (I#163): a closure record whose
    /// code pointer is the top-level function `func` (a wrapper for a bare
    /// function, or a lifted lambda body — both take the env record as their
    /// implicit param 0) and whose captured free variables are `captures`, in
    /// order. `env_sid` is the record's struct layout `{code_ptr, cap_tys…}`
    /// (field 0 is the code pointer, captures follow). `dest : Func(sig)` is a
    /// pointer to the freshly heap-allocated record. A captureless value (a bare
    /// function, or a non-capturing lambda) has `captures` empty — the record is
    /// just `{code_ptr}`. The record is GC-managed; the precise collector scans
    /// its pointer captures via the record's type descriptor.
    ClosureMake {
        dest: Value,
        func: String,
        env_sid: u32,
        captures: Vec<Value>,
    },
    /// **Indirect call** through a function value (I#163): load the code pointer
    /// from `callee`'s record (field 0) and call it, passing the record itself
    /// as the implicit env (`self`) followed by `args`. `params`/`ret` are the
    /// value's signature (excluding the env) — codegen builds the callee type
    /// `abi(ret) (ptr env, abi(params)…)`. When `unwind` is set the call is on an
    /// exception edge (inside a `try`): it lowers to an `invoke` transferring to
    /// that landing pad, mirroring [`Inst::Call`] vs [`Inst::Invoke`].
    CallClosure {
        dest: Option<Value>,
        callee: Value,
        args: Vec<Value>,
        params: Vec<Ty>,
        ret: Ty,
        unwind: Option<BlockId>,
    },
    /// Materialize an **interface value** (the `{data, itable}` fat pointer,
    /// D-0016, `design/0006` §2.4): wrap a concrete instance `data` as a value of
    /// interface `iid`. `dest : Struct(fat_sid)` is the interface's synthetic
    /// inline `{ptr data, ptr itable}` aggregate (see [`Module::interfaces`]);
    /// `itable` is the static dispatch table for the `(concrete_sid, iid)` pair
    /// that codegen builds once. `data` is the underlying instance pointer (a
    /// reference struct — a `value struct` is boxed to the heap first, I#75).
    IfaceMake {
        dest: Value,
        data: Value,
        iid: u32,
        concrete_sid: u32,
    },
    /// **Dynamic dispatch** through an interface value (D-0016): load method slot
    /// `method` (interface-declaration order) from `iface`'s itable and call it
    /// with the fat pointer's `data` as `self`. The callee signature is the
    /// interface method's (`Module::interfaces[iid].methods[method]`). The plain
    /// form; the exception-edge form is [`Inst::IfaceInvoke`].
    IfaceCall {
        dest: Option<Value>,
        iface: Value,
        iid: u32,
        method: u32,
        args: Vec<Value>,
    },
    /// Like [`Inst::IfaceCall`] but on the exception edge (inside a `try`): an
    /// interface method that unwinds transfers to the landing-pad block `unwind`,
    /// mirroring [`Inst::Invoke`] vs [`Inst::Call`].
    IfaceInvoke {
        dest: Option<Value>,
        iface: Value,
        iid: u32,
        method: u32,
        args: Vec<Value>,
        unwind: BlockId,
    },
    /// **Render an interface value** through the Display protocol (#707, D-0124):
    /// load the *renderer* slot — the slot past the method slots — from `iface`'s
    /// itable and call it with the fat pointer's `data` as `self`, returning the
    /// rendered `str`. The renderer is the concrete type's own renderer (an
    /// explicit `__str__`/`__repr__`, else the compiler default field-wise /
    /// tier-3-enum structural form), which the lowerer synthesizes and records in
    /// [`Module::iface_renderers`] so codegen can place it in the itable. This is
    /// the dynamic-dispatch counterpart to [`FnLower::struct_to_str`] for a value
    /// whose concrete type is erased behind an interface. `dest : str` always (a
    /// renderer always produces a string). The plain form; the exception-edge
    /// form is [`Inst::IfaceRenderInvoke`] (the renderer may raise, e.g. a
    /// user `__str__` or a `bytes` field decode).
    IfaceRender {
        dest: Value,
        iface: Value,
        iid: u32,
    },
    /// Like [`Inst::IfaceRender`] but on the exception edge (inside a `try`): the
    /// concrete type's renderer may raise, and transfers to the landing-pad block
    /// `unwind`, mirroring [`Inst::IfaceInvoke`] vs [`Inst::IfaceCall`].
    IfaceRenderInvoke {
        dest: Value,
        iface: Value,
        iid: u32,
        unwind: BlockId,
    },
    /// Atomic load of the cell field `idx` of the `Atomic[T]` box `base`
    /// (`Atomic[T]`, I#176). `dest` is the loaded value, typed `elem` (the cell's
    /// lowered type `T`). Codegen GEPs field `idx` of struct `sid` — the cell sits
    /// at offset 0, so this is the box pointer — and emits an LLVM atomic `load`
    /// with `order`. A reference payload is accessed as a machine word
    /// (ptrtoint/inttoptr); `i1` (`bool`) is accessed as `i8`.
    AtomicLoad {
        dest: Value,
        base: Value,
        sid: u32,
        idx: u32,
        elem: Ty,
        order: AtomicOrd,
    },
    /// Atomic store of `val` into the cell field `idx` of the `Atomic[T]` box
    /// `base` (I#176). No result. Emits an LLVM atomic `store` with `order`.
    AtomicStore {
        base: Value,
        sid: u32,
        idx: u32,
        val: Value,
        elem: Ty,
        order: AtomicOrd,
    },
    /// Atomic read-modify-write on the cell field `idx` of the `Atomic[T]` box
    /// `base` (I#176): `atomicrmw <op>`. For `Add`/`Sub`, `dest` is the **new**
    /// value (Go semantics); for `Xchg`, `dest` is the **old** value. `dest` is
    /// typed `elem`. `Add`/`Sub` are integer-only (rejected in lowering for a
    /// `bool` / reference cell).
    AtomicRmw {
        dest: Value,
        base: Value,
        sid: u32,
        idx: u32,
        op: AtomicRmwOp,
        val: Value,
        elem: Ty,
        order: AtomicOrd,
    },
    /// Atomic compare-and-swap on the cell field `idx` of the `Atomic[T]` box
    /// `base` (I#176): `cmpxchg`. Stores `desired` iff the cell currently equals
    /// `expected`; `dest : i1` is whether the swap happened. `success`/`failure`
    /// are the orderings of the successful / failed paths (`cmpxchg` takes both).
    AtomicCas {
        dest: Value,
        base: Value,
        sid: u32,
        idx: u32,
        expected: Value,
        desired: Value,
        elem: Ty,
        success: AtomicOrd,
        failure: AtomicOrd,
    },
}

#[derive(Clone, Debug)]
pub enum Term {
    Ret(Option<Value>),
    Br(BlockId),
    BrIf {
        cond: Value,
        then_bb: BlockId,
        else_bb: BlockId,
    },
    Unreachable,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: BlockId,
    pub insts: Vec<Inst>,
    pub term: Term,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub is_main: bool,
    /// `true` when parameter 0 is a by-reference method receiver (`self`). A
    /// `value struct` `self` is passed **by reference** (a pointer), Swift-style,
    /// so a mutating method acts on the receiver variable — unlike ordinary
    /// value-struct arguments, which are passed by value (D-0051). Codegen uses
    /// this to keep `self` a pointer even when its type would otherwise be
    /// [`StructLayout::inline`]. `false` for free functions.
    pub self_param: bool,
    /// Type of every value; `value_types[0..params.len()]` are the parameters.
    pub value_types: Vec<Ty>,
    pub blocks: Vec<Block>,
}

impl Func {
    pub fn value_ty(&self, v: Value) -> Ty {
        self.value_types[v as usize]
    }
}

/// A struct layout: a name and its field types, in declaration order.
#[derive(Clone, Debug)]
pub struct StructLayout {
    pub name: String,
    pub fields: Vec<Ty>,
    /// `true` for a `value struct` (D-0051): a value type with value semantics
    /// (copied on assign/pass/return, no shared identity). A plain `struct`
    /// (reference type, D-0023) and an anonymous tuple layout have
    /// `is_value == false`. The physical storage is a GC-heap aggregate behind a
    /// pointer; value semantics are applied at lowering by explicit copies.
    pub is_value: bool,
    /// `true` when this aggregate uses the **physical inline / register / sret**
    /// calling convention (D-0051): it is passed and returned **by value** (an
    /// LLVM aggregate — small ones land in registers, larger ones return via an
    /// `sret` pointer the backend inserts) rather than as a heap pointer. Set for
    /// a `value struct` *and* for a tuple (both are value-semantic aggregates
    /// with no shared identity), but only when **no field is itself a value
    /// aggregate** (value struct or tuple): a by-value aggregate copy is shallow,
    /// so for a flat aggregate it is exactly the field-wise value-struct copy,
    /// whereas a nested value-aggregate field would be shared by a shallow copy
    /// and so keeps the heap representation. Reference structs (D-0023) and
    /// tier-3 enum payload structs are never inline — they have shared identity.
    pub inline: bool,
    /// C-style bitfield packing (I#173, D-0068). `Some` iff this is a packed
    /// `value struct`: **every** field is a bitfield and the whole aggregate is a
    /// single scalar backing integer (holding no pointers, so the precise GC
    /// never traces it). `None` for an ordinary aggregate. When `Some`, `fields`
    /// still lists the logical field types (so a `FieldGet` yields the right
    /// type), while [`PackedLayout`] says how each one maps to a bit slice of the
    /// backing integer.
    pub packed: Option<PackedLayout>,
}

/// How a packed `value struct`'s fields pack into one backing integer (I#173).
/// Bits are assigned least-significant-first in declaration order.
#[derive(Clone, Debug)]
pub struct PackedLayout {
    /// The backing unsigned-integer unit holding every bitfield: the smallest of
    /// `I8`/`I16`/`I32`/`I64` whose width is at least the total of all field
    /// widths (which is at most 64).
    pub backing: Ty,
    /// One entry per declared field, 1:1 and in the same order as
    /// [`StructLayout::fields`].
    pub fields: Vec<BitField>,
}

/// One field's bit slice within a [`PackedLayout`]'s backing integer (I#173).
#[derive(Clone, Copy, Debug)]
pub struct BitField {
    /// Bit offset of the field's least-significant bit from the backing integer's
    /// bit 0.
    pub offset: u32,
    /// Field width in bits (`1..=64`).
    pub width: u32,
    /// `true` for a signed integer field (read back **sign-extended**); `false`
    /// for `bool` / tier-1 enum (read back zero-extended).
    pub signed: bool,
}

/// One method slot of an interface's dispatch table (D-0016), in interface
/// declaration order. The signature is the **interface's** declared signature
/// (every implementor matches it structurally), used by codegen to build the
/// indirect-call function type for dynamic dispatch. `params` excludes the
/// implicit `self` (which is always the leading `ptr`).
#[derive(Clone, Debug)]
pub struct IfaceMethod {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
}

/// A structural interface's runtime representation (D-0016, `design/0006` §2.4).
/// An interface *value* is a `{data, itable}` fat pointer, realized as the
/// synthetic inline `{ptr, ptr}` struct layout `fat_sid` (indexes
/// [`Module::structs`]); `methods` is the dispatch-table slot order.
#[derive(Clone, Debug)]
pub struct InterfaceLayout {
    pub name: String,
    pub methods: Vec<IfaceMethod>,
    /// The `{ptr data, ptr itable}` inline struct layout that an interface value
    /// of this interface has as its ZIR type.
    pub fat_sid: u32,
}

/// The call signature of a first-class function value (I#163): the parameter
/// types and return type of a `fn(T, U) -> R`, **excluding** the implicit
/// closure-environment pointer (ZIR param 0 of the code pointer). Interned in
/// [`Module::fn_sigs`] and referenced by [`Ty::Func`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnSig {
    pub params: Vec<Ty>,
    pub ret: Ty,
}

/// A module-level global slot (D-0112, #658 Phase 1): a named, statically
/// allocated binding initialized once before `main`. Phase 1 materializes only
/// module-level `const` declarations; the slot is a permanent GC root (codegen
/// emits `zap_gc_add_root` for a pointer-typed one, so both the conservative
/// Boehm collector and the precise Immix collector keep its referent live).
#[derive(Clone, Debug)]
pub struct GlobalVar {
    /// The (already module-qualified, D-0081) binding name.
    pub name: String,
    /// The slot's value type.
    pub ty: Ty,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub structs: Vec<StructLayout>,
    pub funcs: Vec<Func>,
    /// Module-level global slots (D-0112, #658 Phase 1), indexed by the `u32` in
    /// [`Inst::GlobalAddr`]. Empty when the program declares no module-level
    /// state. Initialized in this order (import-topological + source order) by
    /// the synthetic `@__zap_init_globals` function, called once at the top of
    /// `main`.
    pub globals: Vec<GlobalVar>,
    /// Interned first-class function-value signatures (I#163), indexed by the
    /// `u32` in [`Ty::Func`]. Empty when the program has no function values.
    pub fn_sigs: Vec<FnSig>,
    /// Structural interfaces (D-0016); indexed by interface id (`iid`). Empty
    /// when the program declares no interfaces.
    pub interfaces: Vec<InterfaceLayout>,
    /// Display-protocol renderer per concrete type that is used behind an
    /// interface (#707, D-0124): concrete struct id (`concrete_sid` of an
    /// [`Inst::IfaceMake`]) -> the symbol of the function that renders a value of
    /// that type to a `str`. The lowerer synthesizes the renderer at the
    /// concrete->interface coercion and records it here; codegen appends it as the
    /// *renderer slot* (past the method slots) of that type's `(concrete, iid)`
    /// itable, so an [`Inst::IfaceRender`] dynamically dispatches to it. A concrete
    /// type with no entry (its renderer could not be synthesized) gets a null
    /// renderer slot — not reachable for the field types D-0124 supports. Empty
    /// when no interface value is ever constructed.
    pub iface_renderers: std::collections::HashMap<u32, String>,
}

// --- textual form (for `zapc ir` and tests) ---

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Ty::I1 => "i1",
            Ty::I8 => "i8",
            Ty::I16 => "i16",
            Ty::I32 => "i32",
            Ty::Char => "char",
            Ty::I64 => "i64",
            Ty::U8 => "u8",
            Ty::U16 => "u16",
            Ty::U32 => "u32",
            Ty::U64 => "u64",
            Ty::Enum(id) => return write!(f, "enum#{id}"),
            Ty::F32 => "f32",
            Ty::F64 => "f64",
            Ty::Ptr => "ptr",
            Ty::Bytes => "bytes",
            Ty::Match => "match",
            Ty::Py => "py",
            Ty::Bigint => "bigint",
            Ty::Complex => "complex",
            Ty::Void => "void",
            Ty::Struct(id) => return write!(f, "struct#{id}"),
            Ty::List(e) => return write!(f, "list[{}]", e.val_suffix()),
            Ty::Dict(k, v) => return write!(f, "dict[{}, {}]", k.key_suffix(), v.val_suffix()),
            Ty::Set(e) => return write!(f, "set[{}]", e.key_suffix()),
            Ty::Heap(e) => return write!(f, "heap[{}]", e.val_suffix()),
            Ty::Array(d, n) => {
                return if *n <= 1 {
                    write!(f, "array[{}]", d.suffix())
                } else {
                    write!(f, "array[{}; {}]", d.suffix(), n)
                }
            }
            Ty::Chan(e) => return write!(f, "chan[{}]", e.val_suffix()),
            Ty::AtomicArray(e) => return write!(f, "AtomicArray[{}]", e.val_suffix()),
            Ty::Func(idx) => return write!(f, "fn#{idx}"),
        };
        f.write_str(s)
    }
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, g) in self.globals.iter().enumerate() {
            writeln!(f, "global @g{i}: {} ; {}", g.ty, g.name)?;
        }
        for func in &self.funcs {
            let params: Vec<String> = func
                .params
                .iter()
                .enumerate()
                .map(|(i, t)| format!("%{i}: {t}"))
                .collect();
            writeln!(
                f,
                "func @{}({}) -> {} {{",
                func.name,
                params.join(", "),
                func.ret
            )?;
            for b in &func.blocks {
                writeln!(f, "  block{}:", b.id)?;
                for inst in &b.insts {
                    writeln!(f, "    {}", fmt_inst(inst))?;
                }
                writeln!(f, "    {}", fmt_term(&b.term))?;
            }
            writeln!(f, "}}")?;
        }
        Ok(())
    }
}

fn fmt_inst(i: &Inst) -> String {
    match i {
        Inst::IConst { dest, val } => format!("%{dest} = iconst {val}"),
        Inst::FConst { dest, val } => format!("%{dest} = fconst {val}"),
        Inst::BConst { dest, val } => format!("%{dest} = bconst {val}"),
        Inst::NullConst { dest } => format!("%{dest} = nullconst"),
        Inst::IsNull { dest, val } => format!("%{dest} = isnull %{val}"),
        Inst::Bin { dest, op, lhs, rhs } => format!("%{dest} = {op:?} %{lhs}, %{rhs}"),
        Inst::ICmp {
            dest,
            pred,
            lhs,
            rhs,
        } => format!("%{dest} = icmp.{pred:?} %{lhs}, %{rhs}"),
        Inst::FCmp {
            dest,
            pred,
            lhs,
            rhs,
        } => format!("%{dest} = fcmp.{pred:?} %{lhs}, %{rhs}"),
        Inst::Un { dest, op, val } => format!("%{dest} = {op:?} %{val}"),
        Inst::Cast { dest, kind, val } => format!("%{dest} = {kind:?} %{val}"),
        Inst::Alloca { dest, pointee } => format!("%{dest} = alloca {pointee}"),
        Inst::Load { dest, ptr } => format!("%{dest} = load %{ptr}"),
        Inst::Store { ptr, val } => format!("store %{ptr}, %{val}"),
        Inst::Select { dest, cond, a, b } => format!("%{dest} = select %{cond}, %{a}, %{b}"),
        Inst::Call { dest, func, args } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            match dest {
                Some(d) => format!("%{d} = call @{func}({})", a.join(", ")),
                None => format!("call @{func}({})", a.join(", ")),
            }
        }
        Inst::Print { args } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            format!("print({})", a.join(", "))
        }
        Inst::StrConst { dest, bytes } => {
            format!("%{dest} = strconst {:?}", String::from_utf8_lossy(bytes))
        }
        Inst::StructNew { dest, sid } => format!("%{dest} = structnew struct#{sid}"),
        Inst::ArenaNew { dest, sid, arena } => {
            format!("%{dest} = arenanew struct#{sid} in %{arena}")
        }
        Inst::StackStruct { dest, sid } => format!("%{dest} = stackstruct struct#{sid}"),
        Inst::StrBuild {
            dest,
            parts,
            stack,
            cap,
        } => {
            let a: Vec<String> = parts.iter().map(|v| format!("%{v}")).collect();
            if *stack {
                format!("%{dest} = strbuild.stack[{cap}] {}", a.join(", "))
            } else {
                format!("%{dest} = strbuild {}", a.join(", "))
            }
        }
        Inst::FieldGet {
            dest,
            base,
            sid,
            idx,
        } => {
            format!("%{dest} = fieldget struct#{sid} %{base}.{idx}")
        }
        Inst::FieldSet {
            base,
            sid,
            idx,
            val,
        } => {
            format!("fieldset struct#{sid} %{base}.{idx} = %{val}")
        }
        Inst::Invoke {
            dest,
            func,
            args,
            unwind,
        } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            match dest {
                Some(d) => format!(
                    "%{d} = invoke @{func}({}) unwind block{unwind}",
                    a.join(", ")
                ),
                None => format!("invoke @{func}({}) unwind block{unwind}", a.join(", ")),
            }
        }
        Inst::LandingPad { dest } => format!("%{dest} = landingpad"),
        Inst::ArrayBinOp {
            dest,
            op,
            out,
            lhs,
            lhs_dtype,
            rhs,
            rhs_dtype,
            scalar,
        } => {
            let kind = if *scalar { "scalar" } else { "array" };
            format!(
                "%{dest} = array.{op:?}.{} %{lhs}:{}, %{rhs}:{} ({kind})",
                out.suffix(),
                lhs_dtype.suffix(),
                rhs_dtype.suffix(),
            )
        }
        Inst::ArrayUnaryOp {
            dest,
            op,
            out,
            src,
            src_dtype,
        } => format!(
            "%{dest} = array.{op:?}.{} %{src}:{}",
            out.suffix(),
            src_dtype.suffix(),
        ),
        Inst::Spawn {
            func, args, owner, ..
        } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            let own = match owner {
                Owner::Root => String::new(),
                Owner::Scope(s) => format!(" in scope %{s}"),
                Owner::Detached => " detached".to_string(),
            };
            format!("spawn{own} @{func}({})", a.join(", "))
        }
        Inst::FuncAddr { dest, func } => format!("%{dest} = funcaddr @{func}"),
        Inst::GlobalAddr { dest, idx } => format!("%{dest} = globaladdr @g{idx}"),
        Inst::ClosureMake {
            dest,
            func,
            env_sid,
            captures,
        } => {
            let c: Vec<String> = captures.iter().map(|v| format!("%{v}")).collect();
            format!(
                "%{dest} = closure @{func} struct#{env_sid} [{}]",
                c.join(", ")
            )
        }
        Inst::CallClosure {
            dest,
            callee,
            args,
            unwind,
            ..
        } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            let uw = match unwind {
                Some(bb) => format!(" unwind bb{bb}"),
                None => String::new(),
            };
            match dest {
                Some(d) => format!("%{d} = callclosure %{callee}({}){uw}", a.join(", ")),
                None => format!("callclosure %{callee}({}){uw}", a.join(", ")),
            }
        }
        Inst::IfaceMake {
            dest,
            data,
            iid,
            concrete_sid,
        } => {
            format!("%{dest} = ifacemake iface#{iid} struct#{concrete_sid} %{data}")
        }
        Inst::IfaceCall {
            dest,
            iface,
            iid,
            method,
            args,
        } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            let call = format!("ifacecall iface#{iid}.{method} %{iface}({})", a.join(", "));
            match dest {
                Some(d) => format!("%{d} = {call}"),
                None => call,
            }
        }
        Inst::IfaceInvoke {
            dest,
            iface,
            iid,
            method,
            args,
            unwind,
        } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            let call = format!(
                "ifaceinvoke iface#{iid}.{method} %{iface}({}) unwind block{unwind}",
                a.join(", ")
            );
            match dest {
                Some(d) => format!("%{d} = {call}"),
                None => call,
            }
        }
        Inst::IfaceRender { dest, iface, iid } => {
            format!("%{dest} = ifacerender iface#{iid} %{iface}")
        }
        Inst::IfaceRenderInvoke {
            dest,
            iface,
            iid,
            unwind,
        } => {
            format!("%{dest} = ifacerenderinvoke iface#{iid} %{iface} unwind block{unwind}")
        }
        Inst::AtomicLoad {
            dest,
            base,
            sid,
            idx,
            elem,
            order,
        } => {
            format!("%{dest} = atomic.load.{order:?} struct#{sid} %{base}.{idx} : {elem}")
        }
        Inst::AtomicStore {
            base,
            sid,
            idx,
            val,
            elem,
            order,
        } => {
            format!("atomic.store.{order:?} struct#{sid} %{base}.{idx} = %{val} : {elem}")
        }
        Inst::AtomicRmw {
            dest,
            base,
            sid,
            idx,
            op,
            val,
            elem,
            order,
        } => {
            format!("%{dest} = atomic.{op:?}.{order:?} struct#{sid} %{base}.{idx}, %{val} : {elem}")
        }
        Inst::AtomicCas {
            dest,
            base,
            sid,
            idx,
            expected,
            desired,
            elem,
            success,
            failure,
        } => {
            format!(
                "%{dest} = atomic.cas.{success:?}.{failure:?} struct#{sid} %{base}.{idx}, %{expected}, %{desired} : {elem}"
            )
        }
    }
}

fn fmt_term(t: &Term) -> String {
    match t {
        Term::Ret(Some(v)) => format!("ret %{v}"),
        Term::Ret(None) => "ret".to_string(),
        Term::Br(b) => format!("br block{b}"),
        Term::BrIf {
            cond,
            then_bb,
            else_bb,
        } => {
            format!("br_if %{cond}, block{then_bb}, block{else_bb}")
        }
        Term::Unreachable => "unreachable".to_string(),
    }
}

#[cfg(test)]
mod dtype_tests {
    //! The dtype matrix (#503/#890) and NEP-50 promotion (D-0166). These are pure
    //! Rust, so they run on every host (no LLVM) and pin the promotion table as a
    //! walled garden — a wrong entry is a user-visible semantics regression.
    use super::{Dtype, Ty};

    const ALL: [Dtype; 11] = [
        Dtype::Bool,
        Dtype::Uint8,
        Dtype::U16,
        Dtype::U32,
        Dtype::U64,
        Dtype::I8,
        Dtype::I16,
        Dtype::I32,
        Dtype::I64,
        Dtype::F32,
        Dtype::F64,
    ];

    #[test]
    fn promotion_is_commutative_and_never_bool() {
        for &a in &ALL {
            for &b in &ALL {
                assert_eq!(
                    Dtype::promote(a, b),
                    Dtype::promote(b, a),
                    "promote not commutative for {a:?}, {b:?}"
                );
                assert_ne!(
                    Dtype::promote(a, b),
                    Dtype::Bool,
                    "arithmetic must never yield bool ({a:?}, {b:?})"
                );
            }
        }
    }

    #[test]
    fn promotion_matrix_matches_nep50() {
        use Dtype::*;
        let cases = [
            // same kind -> wider
            (I32, I64, I64),
            (I8, I16, I16),
            (Uint8, U32, U32),
            (U16, U64, U64),
            (F32, F64, F64),
            // bool defers to the other operand; bool+bool -> uint8
            (Bool, Bool, Uint8),
            (Bool, I32, I32),
            (Bool, Uint8, Uint8),
            (Bool, F32, F32),
            // int + float -> float; an int never widens f32 -> f64
            (I64, F32, F32),
            (U64, F32, F32),
            (I64, F64, F64),
            (Uint8, F64, F64),
            // mixed signed/unsigned: signed that holds both
            (Uint8, I8, I16),
            (U16, I16, I32),
            (U32, I32, I64),
            (U64, I64, F64),   // no 128-bit int -> f64
            (Uint8, I32, I32), // signed already wider
            (U32, I8, I64),    // unsigned wider -> next signed up
            // legacy 4-dtype behavior preserved
            (Uint8, I64, I64),
            (Uint8, Uint8, Uint8),
            (I64, I64, I64),
            (F64, I64, F64),
        ];
        for (a, b, want) in cases {
            assert_eq!(Dtype::promote(a, b), want, "promote({a:?}, {b:?})");
        }
    }

    #[test]
    fn weak_scalar_promotion() {
        use Dtype::*;
        // int scalar keeps the array dtype; float scalar keeps a float array's
        // width, else forces f64; bool array promotes to uint8.
        assert_eq!(Dtype::promote_scalar(Uint8, false), Uint8);
        assert_eq!(Dtype::promote_scalar(F32, false), F32);
        assert_eq!(Dtype::promote_scalar(I32, false), I32);
        assert_eq!(Dtype::promote_scalar(Bool, false), Uint8);
        assert_eq!(Dtype::promote_scalar(F32, true), F32);
        assert_eq!(Dtype::promote_scalar(F64, true), F64);
        assert_eq!(Dtype::promote_scalar(I32, true), F64);
        assert_eq!(Dtype::promote_scalar(Uint8, true), F64);
    }

    #[test]
    fn dtype_metadata_is_consistent() {
        // Distinct codes, and storage/ABI widths agree per dtype.
        let mut codes: Vec<i64> = ALL.iter().map(|d| d.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ALL.len(), "dtype codes must be distinct");
        for &d in &ALL {
            let want = match d.storage_ty() {
                Ty::F64 | Ty::I64 => 8,
                Ty::F32 | Ty::I32 => 4,
                Ty::I16 => 2,
                Ty::I8 => 1,
                other => panic!("unexpected storage {other:?} for {d:?}"),
            };
            assert_eq!(d.elem_size(), want, "elem_size vs storage for {d:?}");
            // The ABI word is i64 for ints, f64 for floats.
            assert_eq!(d.ty(), if d.is_float() { Ty::F64 } else { Ty::I64 });
        }
    }
}
