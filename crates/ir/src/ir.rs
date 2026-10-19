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
    /// An integer/bool element, passed as `i64`.
    I64,
    /// A reference-typed element (`str`, nested collection), passed as a pointer
    /// and — as a *key* — hashed/compared by its bytes (a `ZapStr`).
    Ptr,
    /// A user-struct element (operator overloading, D-0047). Stored as a pointer
    /// word, but as a dict/set *key* it is hashed and compared by the struct's
    /// own `__hash__` / `__eq__` rather than by raw bytes, so it routes to the
    /// function-pointer runtime entry points (key suffix `fn`). The `u32` indexes
    /// [`Module::structs`].
    Struct(u32),
}

impl Elem {
    /// The ZIR type a value of this element kind flows as.
    pub fn ty(self) -> Ty {
        match self {
            Elem::I64 => Ty::I64,
            Elem::Ptr => Ty::Ptr,
            Elem::Struct(sid) => Ty::Struct(sid),
        }
    }
    /// The runtime-entry-point suffix for an element/value of this kind. Struct
    /// *values* are stored as plain pointer words (only *keys* need per-type
    /// hashing), so they share the `ptr` path.
    pub fn val_suffix(self) -> &'static str {
        match self {
            Elem::I64 => "i64",
            Elem::Ptr | Elem::Struct(_) => "ptr",
        }
    }
    /// The runtime-entry-point suffix for a *key* of this kind. Pointer keys are
    /// strings (hashed/compared by contents); struct keys use the `fn` path,
    /// which takes the type's `__hash__`/`__eq__` as function pointers.
    pub fn key_suffix(self) -> &'static str {
        match self {
            Elem::I64 => "i64",
            Elem::Ptr => "str",
            Elem::Struct(_) => "fn",
        }
    }
}

/// The element dtype of a numeric `array[T]` (tickets #24, #49). `f64`/`i64`
/// are 8-byte element words; `uint8`/`bool` are 1-byte words (image data,
/// masks). The dtype selects the typed runtime entry point (`zap_array_*_f64` /
/// `_i64` / `_u8` / `_bool`) and the LLVM *storage* element type of the SIMD
/// element-wise lowering (see [`Dtype::storage_ty`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dtype {
    F64,
    I64,
    /// 8-bit unsigned integer (`uint8`/`u8`), e.g. image pixels. Element-wise
    /// arithmetic wraps mod 256 (D-0005).
    Uint8,
    /// Boolean mask (`bool`), stored as a 0/1 byte. Arithmetic on bools
    /// promotes to `uint8` (see [`Dtype::promote`]).
    Bool,
}

impl Dtype {
    /// The ZIR scalar type of one element as it crosses the runtime ABI
    /// (get/set/sum values). `uint8`/`bool` elements are carried as `i64` words
    /// (the runtime truncates/normalizes on store, zero-extends on load), so
    /// `a[i]` and `sum(a)` are ordinary integers; the compact 1-byte storage is
    /// [`Dtype::storage_ty`].
    pub fn ty(self) -> Ty {
        match self {
            Dtype::F64 => Ty::F64,
            Dtype::I64 | Dtype::Uint8 | Dtype::Bool => Ty::I64,
        }
    }
    /// The LLVM *storage* element type in the contiguous buffer: `uint8`/`bool`
    /// occupy one byte (`i8`), `i64`/`f64` eight. The SIMD element-wise kernel
    /// loads/stores/GEPs with this type.
    pub fn storage_ty(self) -> Ty {
        match self {
            Dtype::F64 => Ty::F64,
            Dtype::I64 => Ty::I64,
            Dtype::Uint8 | Dtype::Bool => Ty::I8,
        }
    }
    /// The size in bytes of one stored element (the SIMD load/store alignment).
    pub fn elem_size(self) -> u32 {
        match self {
            Dtype::F64 | Dtype::I64 => 8,
            Dtype::Uint8 | Dtype::Bool => 1,
        }
    }
    /// The runtime-entry-point suffix for this dtype.
    pub fn suffix(self) -> &'static str {
        match self {
            Dtype::F64 => "f64",
            Dtype::I64 => "i64",
            Dtype::Uint8 => "u8",
            Dtype::Bool => "bool",
        }
    }
    /// The runtime dtype tag passed to `zap_array_new` (matches `zaprt.c`).
    pub fn code(self) -> i64 {
        match self {
            Dtype::F64 => 0,
            Dtype::I64 => 1,
            Dtype::Uint8 => 2,
            Dtype::Bool => 3,
        }
    }
    pub fn is_float(self) -> bool {
        matches!(self, Dtype::F64)
    }
    /// Whether the *stored* integer is unsigned (`uint8`/`bool`). Selects
    /// zero-extend / unsigned-int-to-float in the promotion conversions; `i64`
    /// is signed. (`f64` is not an integer; the result is unused for it.)
    pub fn is_unsigned(self) -> bool {
        matches!(self, Dtype::Uint8 | Dtype::Bool)
    }
    /// The promotion rank: `bool` < `uint8` < `i64` < `f64`. Mixed-dtype
    /// element-wise ops produce the higher-ranked dtype.
    pub fn rank(self) -> u8 {
        match self {
            Dtype::Bool => 0,
            Dtype::Uint8 => 1,
            Dtype::I64 => 2,
            Dtype::F64 => 3,
        }
    }
    fn of_rank(r: u8) -> Dtype {
        match r {
            0 => Dtype::Bool,
            1 => Dtype::Uint8,
            2 => Dtype::I64,
            _ => Dtype::F64,
        }
    }
    /// The result dtype of an element-wise arithmetic op on two array operands
    /// (ticket #49). NumPy-style: the higher-ranked dtype wins, so
    /// `i64 + f64 -> f64`, `uint8 + i64 -> i64`. Arithmetic never *yields* a
    /// `bool` — `bool + bool` promotes to `uint8` (`True + True == 2`, which a
    /// 1-bit result could not represent), matching Python's `bool`-as-`int`.
    pub fn promote(a: Dtype, b: Dtype) -> Dtype {
        let d = Dtype::of_rank(a.rank().max(b.rank()));
        if matches!(d, Dtype::Bool) {
            Dtype::Uint8
        } else {
            d
        }
    }
    /// The result dtype of `array op scalar` broadcast (ticket #49). Scalars are
    /// "weak" (NumPy NEP 50): an integer scalar keeps the array's dtype (so
    /// `uint8_image + 1` stays `uint8`), and only a float scalar forces `f64`.
    /// A `bool` array is promoted to `uint8` first (arithmetic, not a mask).
    pub fn promote_scalar(arr: Dtype, scalar_is_float: bool) -> Dtype {
        if scalar_is_float {
            Dtype::F64
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
    /// A pointer to a numeric `array` object with the given element dtype
    /// (ticket #24).
    Array(Dtype),
    /// A pointer to a runtime channel object (ticket #25, D-0031) whose elements
    /// are stored as uniform 8-byte words of the given kind. The element kind
    /// selects the typed send/receive runtime entry point (`zap_chan_send_i64`
    /// vs `zap_chan_send_ptr`), exactly like [`Ty::List`]. Maps to `ptr` in
    /// codegen.
    Chan(Elem),
    /// A pointer to a regex match object (ticket #23, D-0010). Null means no
    /// match (falsy); owns its captured groups. Distinct from `Ptr`/`Str` so
    /// lowering can route `.group(...)` method dispatch.
    Match,
    /// A pointer to a GC-managed Python object handle (ticket #27, D-0012): the
    /// wrapper owns one CPython reference (decref'd by a finalizer). Distinct
    /// from `Ptr` so lowering can route attribute/call dispatch and marshaling
    /// through the Python runtime (`zap_py_*`). Maps to `ptr` in codegen.
    Py,
    Void,
}

impl Ty {
    pub fn is_float(self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }
    pub fn int_width(self) -> Option<u32> {
        match self {
            Ty::I1 => Some(1),
            Ty::I8 => Some(8),
            Ty::I16 => Some(16),
            Ty::I32 => Some(32),
            Ty::I64 => Some(64),
            _ => None,
        }
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
            Ty::I1 | Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::F32 | Ty::F64 | Ty::Void
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
    Div, // sdiv for ints, fdiv for floats (chosen by result type)
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr, // arithmetic (signed) right shift
}

/// Comparison predicate (signed for ints, ordered for floats).
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
    Trunc,
    SIToFP,
    FPToSI,
    FPExt,
    FPTrunc,
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
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub structs: Vec<StructLayout>,
    pub funcs: Vec<Func>,
}

// --- textual form (for `zapc ir` and tests) ---

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Ty::I1 => "i1",
            Ty::I8 => "i8",
            Ty::I16 => "i16",
            Ty::I32 => "i32",
            Ty::I64 => "i64",
            Ty::F32 => "f32",
            Ty::F64 => "f64",
            Ty::Ptr => "ptr",
            Ty::Match => "match",
            Ty::Py => "py",
            Ty::Void => "void",
            Ty::Struct(id) => return write!(f, "struct#{id}"),
            Ty::List(e) => return write!(f, "list[{}]", e.val_suffix()),
            Ty::Dict(k, v) => return write!(f, "dict[{}, {}]", k.key_suffix(), v.val_suffix()),
            Ty::Set(e) => return write!(f, "set[{}]", e.key_suffix()),
            Ty::Heap(e) => return write!(f, "heap[{}]", e.val_suffix()),
            Ty::Array(d) => return write!(f, "array[{}]", d.suffix()),
            Ty::Chan(e) => return write!(f, "chan[{}]", e.val_suffix()),
        };
        f.write_str(s)
    }
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        Inst::Spawn { func, args, owner } => {
            let a: Vec<String> = args.iter().map(|v| format!("%{v}")).collect();
            let own = match owner {
                Owner::Root => String::new(),
                Owner::Scope(s) => format!(" in scope %{s}"),
                Owner::Detached => " detached".to_string(),
            };
            format!("spawn{own} @{func}({})", a.join(", "))
        }
        Inst::FuncAddr { dest, func } => format!("%{dest} = funcaddr @{func}"),
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
