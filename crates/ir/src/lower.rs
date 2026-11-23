//! AST -> ZIR lowering.
//!
//! Mirrors the vertical slice the backend supports: `int64`/`float64`/`bool`,
//! functions (incl. recursion), `let`/assign/aug-assign, `if`/`while`/
//! `do-while`/`for`-over-`range`, `return`, calls, arithmetic/comparison, and
//! `print`. Type-directed decisions (int vs float ops, widening casts,
//! truthiness) happen here, so the ZIR -> LLVM step is mechanical. Unsupported
//! constructs produce a [`LowerError`].

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use zapc_syntax::ast::{
    self, BinOp as AstBin, BindTarget, CompClause, DictItem, Expr, PatArg, Pattern, Stmt,
    UnOp as AstUn,
};
use zapc_syntax::fstring::{self, Align, FStringPart, FormatSpec};
use zapc_syntax::span::Span;

use crate::ir::*;

/// The synthetic function that runs every module-level global initializer once
/// before `main` (D-0112). Its body is the single-threaded init pre-pass, which
/// is exempt from the `--race` sharing-access instrumentation (#677): no
/// coroutine exists yet, so an init-time read/write cannot race.
const INIT_GLOBALS_FN: &str = "__zap_init_globals";
// `LowerError` lives in `zapc_mono` with monomorphization (#129). Re-export it
// at `crate::lower::LowerError` so the stdlib module lowerers (`modules/*.rs`,
// `use crate::lower::LowerError`) and `zapc_ir::lower::LowerError` are unchanged.
pub use crate::mono::LowerError;

/// Built-in stdlib module registry (ticket #121): per-module lowering lives in
/// `lower/modules/<name>.rs`, dispatched by a data table instead of a hand-
/// written `match (module, method)` block, so new modules are conflict-free
/// parallel-addable. See [`modules`] for the seam contract.
mod modules;

/// Python interop (import python) lowering lives in lower/pyinterop.rs (#1123).
mod pyinterop;

/// Statement + control-flow + exception-handling lowering lives in lower/stmts.rs (#1125).
mod stmts;

/// Enum + match/pattern lowering lives in lower/enums.rs (#1126).
mod enums;

/// First-class function value lowering lives in lower/fnvalues.rs (#1126).
mod fnvalues;

/// Regex lowering lives in lower/regex.rs (#1126).
mod regex;

/// Value → string conversions and f-strings live in lower/strings.rs (#1127).
mod strings;

/// Decorator/reflection emission + query lowering lives in lower/decorators.rs (#1127).
mod decorators;

/// Coercions / numeric helpers live in lower/coerce.rs (#1127).
mod coerce;

/// Collection-key hash/eq machinery lives in lower/keys.rs (#1134).
mod keys;

/// Value → repr / display rendering lives in lower/repr.rs (#1134).
mod repr;

/// Binary-operator lowering lives in lower/operators.rs (#1135).
mod operators;

/// String-method + str/bytes-view lowering lives in lower/strmethods.rs (#1151).
mod strmethods;

/// Interns anonymous tuple layouts (D-0049). A tuple is a by-value aggregate laid
/// out like an anonymous struct (positional order), so it reuses the struct
/// machinery: each distinct element-type sequence gets one [`Ty::Struct`] id,
/// allocated after the named structs (`base = number of named structs`). The
/// interned layouts are appended to [`Module::structs`] once lowering finishes,
/// so their ids line up with `base + index`.
#[derive(Default)]
struct TupleInterner {
    base: u32,
    layouts: Vec<Vec<Ty>>,
    map: HashMap<Vec<Ty>, u32>,
    /// The reserved id for the borrowed `strview` aggregate `(backing: str,
    /// byte_off: int64, byte_len: int64)` (#1082), allocated once on first use.
    /// It is a `push_layout` id — NOT deduplicated — so it can never equal the
    /// id a genuine `(str, int64, int64)` tuple interns to, which keeps a
    /// `strview` distinct from such a tuple for `==` / `print` / method dispatch.
    strview_sid: Option<u32>,
    /// The reserved id for the borrowed `bytesview` aggregate `(backing: bytes,
    /// byte_off: int64, byte_len: int64)` (#1082 follow-up), like `strview_sid`.
    bytesview_sid: Option<u32>,
}

impl TupleInterner {
    fn new(base: u32) -> TupleInterner {
        TupleInterner {
            base,
            layouts: Vec::new(),
            map: HashMap::new(),
            strview_sid: None,
            bytesview_sid: None,
        }
    }

    /// The reserved `strview` layout id (#1082), allocated on first use. Uses
    /// [`Self::push_layout`] (no dedup) so it is distinct from any natural
    /// `(str, int64, int64)` tuple; appended to [`Module::structs`] like any
    /// tuple, so the backing pointer is traced via the aggregate-root pairs.
    fn strview_sid(&mut self) -> u32 {
        if let Some(sid) = self.strview_sid {
            return sid;
        }
        let sid = self.push_layout(vec![Ty::Ptr, Ty::I64, Ty::I64]);
        self.strview_sid = Some(sid);
        sid
    }

    /// Whether `sid` is the reserved `strview` layout. A pure read — it never
    /// allocates the layout, so probing an arbitrary struct id (e.g. during
    /// method dispatch) does not materialize a stray `strview` in a module that
    /// has none.
    fn is_strview(&self, sid: u32) -> bool {
        self.strview_sid == Some(sid)
    }

    /// The reserved `bytesview` layout id (#1082 follow-up), allocated on first
    /// use — the `bytes` counterpart of [`Self::strview_sid`]. Field 0 is `bytes`.
    fn bytesview_sid(&mut self) -> u32 {
        if let Some(sid) = self.bytesview_sid {
            return sid;
        }
        let sid = self.push_layout(vec![Ty::Bytes, Ty::I64, Ty::I64]);
        self.bytesview_sid = Some(sid);
        sid
    }

    /// Whether `sid` is the reserved `bytesview` layout (pure read).
    fn is_bytesview(&self, sid: u32) -> bool {
        self.bytesview_sid == Some(sid)
    }

    /// The struct id for a tuple with these element types (deduplicated).
    fn intern(&mut self, elems: Vec<Ty>) -> u32 {
        if let Some(&sid) = self.map.get(&elems) {
            return sid;
        }
        let sid = self.base + self.layouts.len() as u32;
        self.map.insert(elems.clone(), sid);
        self.layouts.push(elems);
        sid
    }

    /// Append a fresh anonymous aggregate layout **without deduplication**
    /// (I#163): used for closure-environment records, which must never share an
    /// id with a genuine tuple — an env is forced to a non-inline reference
    /// struct, and sharing its id with a value-semantic tuple would corrupt the
    /// tuple's ABI. Not inserted into the dedup map, so [`Self::intern`] never
    /// returns this id for a matching tuple.
    fn push_layout(&mut self, elems: Vec<Ty>) -> u32 {
        let sid = self.base + self.layouts.len() as u32;
        self.layouts.push(elems);
        sid
    }

    /// Whether `sid` names a tuple layout (vs a user struct / tier-3 enum).
    fn is_tuple(&self, sid: u32) -> bool {
        sid >= self.base && ((sid - self.base) as usize) < self.layouts.len()
    }

    /// The element types of tuple `sid`, if it is one.
    fn fields(&self, sid: u32) -> Option<Vec<Ty>> {
        if sid < self.base {
            return None;
        }
        self.layouts.get((sid - self.base) as usize).cloned()
    }
}

/// Interner for first-class function-value signatures (I#163). Maps a
/// `(params, ret)` signature to the `u32` carried by [`Ty::Func`], deduplicated,
/// and materialized into [`Module::fn_sigs`] after lowering.
#[derive(Default)]
struct FnSigInterner {
    sigs: Vec<FnSig>,
    map: HashMap<FnSig, u32>,
}

impl FnSigInterner {
    fn intern(&mut self, params: Vec<Ty>, ret: Ty) -> u32 {
        let sig = FnSig { params, ret };
        if let Some(&idx) = self.map.get(&sig) {
            return idx;
        }
        let idx = self.sigs.len() as u32;
        self.map.insert(sig.clone(), idx);
        self.sigs.push(sig);
        idx
    }

    fn get(&self, idx: u32) -> &FnSig {
        &self.sigs[idx as usize]
    }
}

/// Interner for **nested-collection element types** (#625). A collection stored
/// inside another collection (`list[list[int64]]`, `dict[str, list[int64]]`, …)
/// is physically a pointer, but lowering must recover its full element type for
/// read-back dispatch (`rows[i]`, `len(rows[i])`, iteration): a bare pointer
/// cannot say whether an element is a `str` or a nested `list[int64]`, which
/// silently miscompiled the nested reads as string operations. Each distinct
/// nested [`Ty`] interns to the `u32` carried by [`Elem::Nested`], deduplicated so
/// structurally-equal nestings share an id (making the element-kind equality
/// checks in collection literals compare correctly). The table lives only for the
/// duration of lowering — codegen treats every collection uniformly as a traced
/// pointer and never reads the id — so it is not materialized into the module.
#[derive(Default)]
struct NestedInterner {
    tys: Vec<Ty>,
    map: HashMap<Ty, u32>,
}

impl NestedInterner {
    fn intern(&mut self, t: Ty) -> u32 {
        if let Some(&id) = self.map.get(&t) {
            return id;
        }
        let id = self.tys.len() as u32;
        self.map.insert(t, id);
        self.tys.push(t);
        id
    }

    fn get(&self, id: u32) -> Ty {
        self.tys[id as usize]
    }
}

/// One data-carrying variant of a tier-3 enum (D-0046), resolved for lowering.
#[derive(Clone, Debug)]
struct VariantLow {
    name: String,
    /// The discriminant stored in the tag slot (declaration order).
    tag: i64,
    /// Payload fields: `(name, struct-field index in the tagged struct, type)`.
    /// The index points into the tagged struct, whose field 0 is the tag.
    fields: Vec<(String, u32, Ty)>,
}

/// A lowered enum (D-0018 tier-1 / D-0046 tier-3).
///
/// A **tier-1** enum (`is_sum == false`) is an unboxed backing integer (`i64`):
/// members carry explicit or auto-incremented discriminants. A **tier-3** enum
/// (`is_sum == true`) is a GC-allocated **tagged struct** `{ tag:i32, payload…
/// }`: `sid` indexes [`Module::structs`]. Reusing the struct machinery means a
/// recursive variant field (whose type is the enum itself) is already a pointer
/// to a heap enum value, which breaks the cycle for free (D-0046).
#[derive(Clone, Debug)]
struct EnumLow {
    /// Stable index assigned in declaration order; carried by tier-1 ZIR values.
    enum_id: u32,
    enum_name: String,
    is_sum: bool,
    /// Tagged-struct id (tier-3 only).
    sid: Option<u32>,
    /// Members in declaration order: `(name, discriminant)` (tier-1).
    members: Vec<(String, i64)>,
    /// Variants in declaration order (tier-3).
    variants: Vec<VariantLow>,
    /// The niche optimization (I#154, D-0064 #4), set for a tier-3 enum of the
    /// `Optional` shape — exactly two variants, one payload-free and one with a
    /// single non-nullable-pointer field. Such an enum is represented as that
    /// single nullable pointer: the payload-free variant is the null pointer, the
    /// one-field variant *is* the payload pointer — one word, no tag, no
    /// allocation. `None` for a tagged enum (value-type or multi-field payloads,
    /// or a nullable/enum payload that would collide with the null niche).
    niche: Option<NicheLow>,
}

/// The niche layout of an `Optional`-shaped tier-3 enum (I#154, D-0064 #4). The
/// value's ZIR type stays `Struct(sid)` (so method dispatch still finds the
/// enum's methods), but it is physically the payload pointer, with `None` the
/// null pointer. Codegen treats every `Struct(sid)` as an opaque pointer, so no
/// codegen type change is needed; the precise collector reads each live object's
/// descriptor from its own header (dynamic) and skips null slots, so GC stays
/// correct with no new mechanism.
#[derive(Clone, Debug)]
struct NicheLow {
    /// The one-field (`Some`-like) variant's name.
    some_variant: String,
    /// The payload-free (`None`-like) variant's name.
    none_variant: String,
    /// The payload field's ZIR type — the niche's physical representation.
    payload: Ty,
}

impl EnumLow {
    fn member_value(&self, name: &str) -> Option<i64> {
        self.members
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
    }
    fn variant(&self, name: &str) -> Option<&VariantLow> {
        self.variants.iter().find(|v| v.name == name)
    }
}

/// The information `resolve_ty` needs to map a named type to a ZIR type: struct
/// / tier-3-enum ids, plus the enum registry (so a tier-1 enum resolves to its
/// backing integer and a tier-3 enum to its tagged struct).
struct TyEnv<'a> {
    ids: &'a HashMap<String, u32>,
    enums: &'a HashMap<String, EnumLow>,
    /// Structural interfaces (D-0016): interface name -> the synthetic inline
    /// `{ptr data, ptr itable}` struct id (`fat_sid`) that an interface value has
    /// as its ZIR type. An interface-typed annotation resolves to this.
    ifaces: &'a HashMap<String, u32>,
    /// Interner for anonymous tuple layouts (D-0049), shared so every
    /// `(T, U, ...)` type resolves to the same struct id across the module.
    tuples: &'a RefCell<TupleInterner>,
    /// Interner for first-class function-value signatures (I#163): a
    /// `fn(T, U) -> R` annotation resolves to `Ty::Func(idx)` where `idx` is the
    /// interned signature.
    fn_sigs: &'a RefCell<FnSigInterner>,
    /// Interner for nested-collection element types (#625): a collection element
    /// that is itself a collection resolves to `Elem::Nested(id)`, preserving its
    /// full type for read-back dispatch.
    nested: &'a RefCell<NestedInterner>,
}

/// Lower a type-checked module to ZIR.
/// Struct layouts/ids collected before lowering function bodies.
struct StructCtx {
    ids: HashMap<String, u32>,
    names: Vec<String>,
    fields: Vec<Vec<(String, Ty)>>,
    /// Struct ids that are `value struct`s (value types, D-0051). A value struct
    /// has value semantics: copied on assign/pass/return (no shared identity).
    value_ids: HashSet<u32>,
    /// Struct ids that are `@scoped` (#876, D-0159), explicit or closed under
    /// contagion (a struct owning a `@scoped` field is itself `@scoped`). Drives
    /// the `with`-lowering auto-teardown drop-glue: an owner's owned `@scoped`
    /// fields have their `__exit__` run (reverse declaration order) at every
    /// `with`-exit. Sema enforces the static `@scoped` rules (`crates/sema/src/scoped.rs`).
    scoped_structs: HashSet<u32>,
    /// `@exception(Parent)` family edges (#1278), as resolved `(member_type_id,
    /// parent_type_id)` pairs (the same name-hash ids `raise`/`except` use). Each
    /// becomes a `zap_exception_register_parent(member, parent)` call emitted once
    /// at startup (in `@__zap_init_globals`), so the runtime's parent-chain walk
    /// (`zap_exception_is_a`) threads a member through its declared family instead
    /// of straight to the built-in `Exception`. Empty when no `@exception(Parent)`
    /// edge exists.
    exception_parents: Vec<(i64, i64)>,
    /// Registered enums (tier-1 and tier-3), keyed by type name.
    enums: HashMap<String, EnumLow>,
    /// Python import aliases (ticket #27, D-0012): the in-scope name
    /// (`import python "numpy" as np` → `np`) mapped to the CPython module name
    /// (`numpy`). A reference to one of these names lowers to a `zap_py_import`.
    py: HashMap<String, String>,
    /// Receiver name → canonical `arch.*` module registry name (I#139 Tier 2,
    /// D-0199), from `import arch.x86_64 [as name]` / `import arch.aarch64 [as
    /// name]`. A call `x86.crc32c_u64(..)` lowers by resolving its receiver
    /// (`x86`) through this map to the registry name (`arch.x86_64`), matching
    /// the sema dispatch. Empty unless the file imports an `arch.*` module.
    arch_imports: HashMap<String, &'static str>,
    /// Interned anonymous tuple layouts (D-0049), appended to [`Module::structs`]
    /// after lowering.
    tuples: RefCell<TupleInterner>,
    /// Structural interfaces (D-0016): interface name -> `fat_sid` (its synthetic
    /// inline `{ptr data, ptr itable}` struct layout id). An interface value's
    /// ZIR type is `Ty::Struct(fat_sid)`.
    ifaces: HashMap<String, u32>,
    /// Interface metadata, indexed by interface id (`iid`); `iid` is the position
    /// in this vector. Used to map a `fat_sid` back to its interface for dynamic
    /// dispatch and to build the per-method dispatch-table slots.
    interfaces: Vec<InterfaceLow>,
    /// Reverse map `fat_sid -> iid`, so a method call on an interface-typed
    /// receiver resolves to the owning interface.
    fat_to_iid: HashMap<u32, u32>,
    /// Generator functions (I#67, D-0026/D-0048), keyed by source function name.
    /// A call `gen(args)` constructs the function's synthetic state struct
    /// instead of calling a function; see [`GeneratorInfo`].
    generators: HashMap<String, GeneratorInfo>,
    /// Interned first-class function-value signatures (I#163), materialized into
    /// [`Module::fn_sigs`] after lowering.
    fn_sigs: RefCell<FnSigInterner>,
    /// Interned nested-collection element types (#625). Lowering-only (not
    /// materialized into the module); resolves an `Elem::Nested(id)` back to the
    /// element's full [`Ty`] for read-back dispatch.
    nested: RefCell<NestedInterner>,
    /// Closure records (I#163): compiler-generated lifted-lambda and
    /// bare-function-wrapper ZIR functions, collected while lowering bodies and
    /// appended to [`Module::funcs`] afterward.
    generated: RefCell<Vec<Func>>,
    /// Tuple-interner ids that are closure-environment records (I#163): forced
    /// to non-inline reference structs in the inline post-pass (an env outlives
    /// its creating frame and has shared identity, like a reference struct).
    closure_env_sids: RefCell<Vec<u32>>,
    /// Hands out distinct names for compiler-generated closure functions (I#163).
    closure_counter: RefCell<u32>,
    /// The shared `{code_ptr}` env-record struct id for captureless function
    /// values (I#163), interned once and reused.
    captureless_env: RefCell<Option<u32>>,
    /// Bare top-level functions already wrapped as function values (I#163):
    /// source name -> generated wrapper name, so `f` taken as a value reuses one
    /// wrapper `f$fnval`.
    fnval_wrappers: RefCell<HashMap<String, String>>,
    /// First-class-function-value `spawn` thunks (I#475), keyed by the interned
    /// call signature: a generated `$spawnthunk$<sig>(closure, args…)` whose body
    /// is a single indirect [`Inst::CallClosure`], reused across every
    /// `spawn fval(…)` of that signature. The thunk is a normal top-level
    /// function, so the existing named-function [`Inst::Spawn`] trampoline runs
    /// it as the coroutine body (its catch-all pad routes a child failure to the
    /// scope, exactly as for a named spawn).
    spawn_thunks: RefCell<HashMap<u32, String>>,
    /// Synthesized structural key functions (#680 tuples, D-0049; #704
    /// `Optional[T]`, D-0122), keyed by the value's interned struct id: the
    /// `(__hash__, __eq__)` symbol names of the structural hash/equality
    /// functions generated on demand the first time that type is used as a dict
    /// key or set element. Tuples and `Optional[T]` already lower as value
    /// aggregates whose keys route through the fn-key path, so each is just an
    /// anonymous `@derive(Hash, Eq)`: a tuple's hash folds the per-element hashes
    /// with `zap_hash_combine` and its eq ANDs the per-element comparisons; an
    /// Optional hashes a fixed sentinel for `None` and delegates to `T` for
    /// `Some`. Generated once per struct id and reused (tuple and Optional ids are
    /// distinct, so one map serves both), exactly like [`Self::spawn_thunks`].
    synth_keyfns: RefCell<HashMap<u32, (String, String)>>,
    /// Synthesized default value-rendering functions (#274, D-0124), keyed by the
    /// struct id: the symbol name of the `{sid}$repr(self) -> str` function that
    /// renders a user type with no hand-written `__str__`/`__repr__` and no
    /// `@derive(Repr)` in the default field-wise form `TypeName(field=value, …)`,
    /// recursing through every field via the shared value renderer
    /// ([`FnLower::render_str`]). Generated on demand the first time such a type is
    /// rendered (by `print`/`str`/f-string/REPL) and reused; a nested user-type
    /// field renders by *calling* that field type's own repr function, so a
    /// mutually recursive type graph terminates at compile time (one function per
    /// struct) and only an actual runtime object cycle recurses without bound —
    /// the documented D-0124 limitation, matching a hand-written recursive
    /// `__repr__`. Tuples (anonymous `(tuple)` layouts) reuse the same map, keyed
    /// by their interned id, rendering the Python `(v0, v1)` form instead.
    synth_reprfns: RefCell<HashMap<u32, String>>,
    /// Synthesized native container (`list`/`dict`/`set`) rendering functions
    /// (#274, D-0124), keyed by the container's ZIR type signature (`{ty:?}`): the
    /// symbol name of a `$crepr$N(c) -> str` function that renders the container
    /// in Python form (`[v0, v1]`, `{k0: v0}`, `{v0, v1}`; empty `[]`/`{}`/`set()`)
    /// with each element/key/value rendered in `repr` mode via the shared value
    /// renderer. Generated on demand and reused per container type; a container
    /// type cannot contain itself, so nested containers resolve to distinct
    /// functions and synthesis terminates.
    synth_container_reprfns: RefCell<HashMap<String, String>>,
    /// Display-protocol renderer per concrete type used behind an interface
    /// (#707, D-0124), keyed by `concrete_sid`: the symbol of the function that
    /// renders a value of that type to a `str`. Populated at each concrete->
    /// interface coercion (where [`Inst::IfaceMake`] is emitted) by synthesizing
    /// the concrete type's renderer via [`FnLower::struct_repr_fn`] — the same
    /// resolution a direct render uses (`__str__`/`__repr__`/`@derive(Repr)`, the
    /// tier-3-enum structural form, or the default field-wise form). Drained into
    /// [`zir::ir::Module::iface_renderers`] so codegen can append the renderer as
    /// the itable's renderer slot, letting [`Inst::IfaceRender`] dispatch to the
    /// concrete type's renderer through a type-erased interface value.
    iface_renderers: RefCell<HashMap<u32, String>>,
    /// Generator-expression iterators (#596), keyed by their tuple-interned
    /// state-struct id. Populated on demand while lowering a `(x for x in xs)`;
    /// the iteration protocol hooks consult it so a genexpr value flows through
    /// `for`/`sum`/etc. exactly like a generator's result.
    genexpr_iters: RefCell<HashMap<u32, GenExprIter>>,
    /// Hands out distinct names for compiler-generated generator-expression
    /// functions (#596), so each `(x for x in xs)` gets a unique state struct.
    genexpr_counter: RefCell<u32>,
    /// C-style bitfield packing (I#173, D-0068), keyed by struct id. A value
    /// struct whose fields all carry a bit width is laid out as one backing
    /// integer with shift/mask field accessors; absent for ordinary structs.
    packed_layouts: HashMap<u32, PackedLayout>,
    /// Literal field defaults (D-0038), parallel to [`StructCtx::fields`]: for
    /// struct id `sid`, `field_defaults[sid][i]` is the default expression of
    /// field `i`, or `None` if the field is undefaulted. A caller that omits a
    /// defaulted field at construction (`Box()` vs `Box(xs=[])`) has that
    /// default lowered fresh in the constructor's scope (#461). Entries for
    /// enum/interface synthetic layouts are empty (they are never constructed
    /// through [`FnLower::construct`]).
    field_defaults: Vec<Vec<Option<Expr>>>,
    /// Call-site argument-assembly metadata for default and keyword arguments
    /// (D-0038, I#298), keyed by free-function name (the same key as `sigs`) and
    /// parallel to that signature's `Ty` vector. A call that omits a defaulted
    /// parameter or passes keyword arguments out of declaration order is
    /// reordered and filled against this before lowering; see [`FnLower::call`].
    call_params: HashMap<String, Vec<ParamInfo>>,
    /// Per-struct-id `@json(...)` field options (ticket #346, D-0086): the inner
    /// vec is parallel to `fields[sid]`. Absent sid = no json field options on
    /// that struct; only `@derive(Json)` structs ever read it.
    json_field_opts: HashMap<u32, Vec<JsonFieldOpts>>,
    /// Per-struct-id field validation constraints from `@validate(...)` /
    /// `@range(...)` field decorators (D-0101), parallel to `fields[sid]`: for
    /// field `i`, the checks run at construction time. An all-empty inner vec (or
    /// an absent sid) means the struct has no generated validator. Sema has
    /// already checked argument shapes and field-type suitability.
    field_checks: HashMap<u32, Vec<Vec<FieldCheck>>>,
    /// Module-level global slots (D-0112, #658 Phase 1), in declaration /
    /// init order. Populated by the init pre-pass (`lower_init_globals`) before
    /// any user function is lowered, so a reference to a module `const` resolves
    /// to its slot ([`FnLower::expr`]'s `Expr::Name` global fallback). The index
    /// into this vec is the `idx` carried by [`ir_core::Inst::GlobalAddr`] and
    /// [`ir_core::Module::globals`].
    globals: RefCell<Vec<GlobalVar>>,
    /// Parallel to [`globals`]: whether each module global is *race-instrumented*
    /// under `--race` (#677, epic #665, D-0114) — i.e. a MUTABLE, non-`@shareable`
    /// module global (scalar or reference). This is exactly the ZAP5125
    /// sharing-lint set (#664): a `const` global cannot race, and a `@shareable`
    /// cell (user struct or a [`BUILTIN_SHAREABLE_TYPES`] type) is asserted safe,
    /// so neither carries a `true` here. Computed once, at slot registration in
    /// [`lower_init_globals`], mirroring sema's `declared_type_is_shareable`.
    global_racy: RefCell<Vec<bool>>,
    /// User `struct`s marked `@shareable` (#664, D-0113), by type name. The race
    /// instrumentation (like the sharing lint) treats a mutable module global of
    /// such a type as an asserted-safe cell and never instruments it. Built-in
    /// shareable types (`Atomic`/`Mutex`/… in [`BUILTIN_SHAREABLE_TYPES`]) carry
    /// the marker implicitly and are recognized by [`StructCtx::name_is_shareable`].
    shareable_types: HashSet<String>,
    /// Whether the runtime data-race detector (`--race`, #665, D-0114) is on for
    /// this lowering. When `false` (the default) lowering emits ZERO race hooks,
    /// so the output is byte-identical to a non-`--race` build (golden-safe).
    race: bool,
    /// Mangled names (`Type__func`) of user-defined **associated functions**
    /// (receiverless `def`s on a struct/enum, #1260). A call `Type.func(args)`
    /// lowers like a namespaced free function with no receiver, so the call site
    /// dispatches here; registered alongside `sigs`/`call_params`, whose entries
    /// for these names carry the full parameter list (no implicit `self` dropped).
    associated_fns: HashSet<String>,
}

/// One classified axis of an array subscript (#510). `Slice`'s pairs are the
/// `(value, present_flag)` results of `array_slice_bound`; `Int` carries the
/// evaluated index value and its type; `Adv` carries an index array value with
/// its dtype and rank. Built with one evaluation per part.
enum IdxPart {
    Slice {
        lo: (Value, Value),
        hi: (Value, Value),
        step: (Value, Value),
    },
    Int(Value, Ty),
    Adv(Value, Dtype, u8),
}

/// One construction-time field validation constraint (`@validate`/`@range`,
/// D-0101). Sema validated the argument shapes; lowering realizes each as a
/// guard in the generated `{Struct}____validate__` method that raises a
/// catchable `ValueError` (D-0007) when violated.
#[derive(Clone, Copy)]
enum FieldCheck {
    /// `@validate(max_len=n)` on a `str`/`bytes` field: byte length must be `<= n`.
    MaxLen(i64),
    /// `@validate(min_len=n)` on a `str`/`bytes` field: byte length must be `>= n`.
    MinLen(i64),
    /// `@range(min=v)` on a numeric field: the value must be `>= v`.
    RangeMin(NumLit),
    /// `@range(max=v)` on a numeric field: the value must be `<= v`.
    RangeMax(NumLit),
}

/// A numeric bound literal for `@range` (D-0101), carried as both an integer and
/// a float so the generated guard can emit a constant of the field's own type.
#[derive(Clone, Copy)]
struct NumLit {
    int: i64,
    float: f64,
}

/// The wire form of a `DateTime` field under `@derive(Json)` (ticket #346,
/// D-0086), chosen per field via `@json(timestamp=...)`.
#[derive(Clone, Copy, PartialEq)]
enum TsFormat {
    /// JSON number — Unix epoch seconds (UTC). The default when a `DateTime`
    /// field carries no `@json(timestamp=...)` (matches the AWS awsJson default).
    Epoch,
    /// JSON string — ISO-8601 (`2023-11-14T22:13:20`, UTC).
    Iso8601,
}

/// Per-field `@json(...)` options for a `@derive(Json)` struct (ticket #346,
/// D-0086). One entry per struct field, parallel to `StructCtx.fields[sid]`.
#[derive(Clone, Default)]
struct JsonFieldOpts {
    /// `Some` on a `DateTime` field with `@json(timestamp=...)`; `None` means the
    /// default form (epoch) applies. Ignored on non-`DateTime` fields.
    timestamp: Option<TsFormat>,
}

/// Parse a struct field's `@json(...)` decorators into [`JsonFieldOpts`]. Sema
/// has already validated the option names/values (ticket #346); this mirrors
/// that for lowering and is lenient on anything it does not recognize.
fn parse_json_field_opts(fld: &ast::Field) -> JsonFieldOpts {
    let mut opts = JsonFieldOpts::default();
    for d in &fld.decorators {
        if d.name != "json" {
            continue;
        }
        for a in &d.args {
            if a.name.as_deref() == Some("timestamp") {
                if let ast::Expr::Str { value, .. } = &a.value {
                    opts.timestamp = match value.as_str() {
                        "iso8601" => Some(TsFormat::Iso8601),
                        _ => Some(TsFormat::Epoch),
                    };
                }
            }
        }
    }
    opts
}

/// Per-parameter metadata used to assemble a call's arguments in declaration
/// order (D-0038, I#298). One entry per declared parameter, in order, parallel
/// to the parameter `Ty` vector held in `sigs`.
#[derive(Clone)]
struct ParamInfo {
    /// The parameter's name, for matching a keyword argument (`name=value`).
    name: String,
    /// The default-value expression, if any. It is lowered fresh in the
    /// caller's context at each call site so the value is recomputed per call
    /// (D-0038: mutable defaults like `[]` are therefore safe).
    default: Option<Expr>,
    /// `true` for a `TYPE... name` variadic parameter (D-0038). A function with
    /// a variadic parameter falls back to positional lowering (reorder/fill is
    /// not attempted), preserving existing behavior.
    variadic: bool,
    /// `true` for a `V **name` keyword-collector parameter (#601): the caller
    /// assembles a `dict[str, V]` of the extra keyword arguments to pass here.
    kwargs: bool,
}

/// Where one assembled call argument comes from (I#298): an expression written
/// at the call site, or a parameter's default expression supplied for an
/// omitted argument.
enum ArgSlot {
    /// Index into the call's argument list.
    Given(usize),
    /// Index of the parameter whose default fills this (omitted) argument.
    Default(usize),
}

/// A positional-argument source for an assembled (`*`/`**` or `**kwargs`) call
/// (#597). Either a call-site expression, or a value already extracted from a
/// `*tuple` spread (a tuple is statically sized, so its elements are pulled out
/// with `FieldGet` and bound positionally like ordinary arguments).
enum PosSrc<'e> {
    Expr(&'e Expr),
    Val(Value, Ty),
}

/// Per-parameter metadata for a struct/enum method's call sites (I#481),
/// dropping the implicit `self` receiver (always the first declared parameter).
/// The result lines up one-to-one with a method call's explicit arguments, so
/// [`plan_call_args`] can fill omitted defaults and reorder keyword arguments
/// exactly as it does for free functions; the method-call lowering re-adds
/// `self` when it emits the call.
fn method_param_info(m: &ast::Function) -> Vec<ParamInfo> {
    fn_param_info(m, true)
}

/// True when `m` is a **receiverless** `def` — an associated function (#1260):
/// its first parameter is not `self`, or it declares no parameters at all. An
/// instance method's first parameter is always the `self` receiver.
fn is_receiverless(m: &ast::Function) -> bool {
    !m.params.first().is_some_and(|p| p.name == "self")
}

/// Per-parameter call-site metadata for a `def`. For an instance method,
/// `skip_self` drops the implicit `self` receiver so the result lines up with the
/// call site's explicit arguments (the method-call lowering re-adds `self`). For
/// an associated function (#1260) `skip_self` is `false`: every parameter is a
/// real argument, exactly like a free function.
fn fn_param_info(m: &ast::Function, skip_self: bool) -> Vec<ParamInfo> {
    m.params
        .iter()
        .skip(usize::from(skip_self))
        .map(|p| ParamInfo {
            name: p.name.clone(),
            default: p.default.clone(),
            variadic: p.variadic,
            kwargs: p.kwargs,
        })
        .collect()
}

/// Assemble a call's arguments into declaration order (D-0038, I#298).
///
/// Positional arguments (`name == None`) bind to parameters left to right;
/// keyword arguments (`name == Some`) bind to the parameter they name. Any
/// parameter left unbound is filled from its default, or reported as missing.
/// The result has exactly one [`ArgSlot`] per parameter, in declaration order,
/// so the caller can lower each against the matching parameter type.
///
/// Sema (`check_user_call`) has already validated arities, keyword names, and
/// types; the errors here are a defensive backstop that keeps a malformed call
/// from reaching codegen as an invalid module.
fn plan_call_args(
    fname: &str,
    params: &[ParamInfo],
    args: &[ast::Arg],
) -> Result<Vec<ArgSlot>, LowerError> {
    let n = params.len();
    let mut chosen: Vec<Option<ArgSlot>> = (0..n).map(|_| None).collect();
    let mut next_pos = 0usize;
    for (j, a) in args.iter().enumerate() {
        match &a.name {
            None => {
                if next_pos >= n {
                    return Err(err(format!("too many positional arguments to '{fname}'")));
                }
                chosen[next_pos] = Some(ArgSlot::Given(j));
                next_pos += 1;
            }
            Some(nm) => {
                let Some(pi) = params.iter().position(|p| &p.name == nm) else {
                    return Err(err(format!("'{fname}' has no parameter named '{nm}'")));
                };
                if chosen[pi].is_some() {
                    return Err(err(format!(
                        "'{fname}' got multiple values for parameter '{nm}'"
                    )));
                }
                chosen[pi] = Some(ArgSlot::Given(j));
            }
        }
    }
    let mut plan = Vec::with_capacity(n);
    for (i, slot) in chosen.into_iter().enumerate() {
        match slot {
            Some(s) => plan.push(s),
            None if params[i].default.is_some() => plan.push(ArgSlot::Default(i)),
            None => {
                return Err(err(format!(
                    "missing required argument '{}' to '{fname}'",
                    params[i].name
                )));
            }
        }
    }
    Ok(plan)
}

/// A compiler-generated generator (I#67, D-0026/D-0048). A free `def` whose body
/// contains `yield` is lowered to a heap **state struct** that structurally
/// implements the `Iterator` protocol (D-0048): `__iter__(self) -> self` and the
/// comma-ok `__next__(self) -> (T, bool)`. Calling the generator constructs an
/// instance of this struct (capturing the arguments); each `__next__` resumes
/// the body to the next `yield` (or signals exhaustion).
#[derive(Clone)]
struct GeneratorInfo {
    /// Source function name (also the key in [`StructCtx::generators`]).
    name: String,
    /// The synthetic state-struct id (indexes `Module::structs`).
    sid: u32,
    /// Element type `T` — the join of the `yield` operand types.
    elem: Ty,
    /// The `(T, bool)` tuple layout id that `__next__` returns.
    next_tuple_sid: u32,
    /// The state-struct fields that back the generator's live values, in field
    /// order: `(name, type)`. The first `n_params` are the generator's
    /// parameters (captured at construction); the rest are body locals. A var at
    /// position `p` lives in field `p + 1` — field `0` is the `$state`
    /// discriminant (`i64`), set to `0` at construction, to the resume id at each
    /// `yield`, and to `-1` at exhaustion.
    vars: Vec<(String, Ty)>,
    /// How many leading `vars` are the generator's parameters.
    n_params: usize,
}

/// Field index of generator var at position `p` (slot `0` is `$state`).
fn gen_var_field(p: usize) -> u32 {
    (p + 1) as u32
}

/// A lowered bare generator expression `(x for x in xs)` (#596, epic #614 D4).
/// Unlike a free generator (I#67), a genexpr's state struct is built on demand
/// while lowering an expression — its captured free-variable types are only
/// known in the enclosing scope — so it cannot be a named struct registered in
/// the module pre-pass. Instead it is a tuple-interned, non-inline (heap) state
/// struct, and this side table lets the D-0048 iteration hooks
/// ([`FnLower::setup_iter`]/[`FnLower::next_form`]/[`FnLower::drive_iteration`])
/// recognize that struct id as its own iterator and find its `__next__`. The
/// state machine itself is emitted by the shared [`lower_generator_next`].
#[derive(Clone)]
struct GenExprIter {
    /// The emitted comma-ok `__next__(self) -> (T, bool)` function's name.
    next_fn: String,
    /// The `(T, bool)` tuple layout id its `__next__` returns.
    next_tuple_sid: u32,
    /// Element type `T` (the type of the yielded expression).
    elem: Ty,
}

/// Lowered interface metadata (D-0016). `fat_sid` is the synthetic inline
/// `{ptr data, ptr itable}` struct layout; `methods` is the dispatch-table slot
/// order — the interface's own methods (declaration order) followed by every
/// method inherited from its transitive refined supertypes (#830, D-0139). A
/// conforming type's itable is built slot-by-slot from this list by name, and a
/// method call resolves its dynamic slot index by searching it, so a
/// supertype-inherited method (e.g. `Named.name` on a `Ranked` value) dispatches
/// to the same slot the itable laid out for it.
#[derive(Clone)]
struct InterfaceLow {
    name: String,
    fat_sid: u32,
    methods: Vec<IfaceMethodLow>,
    /// Names of the refined supertypes this interface declares (D-0139/#806):
    /// the `Named` in `interface Ranked: Named`. Used once, in the post-build
    /// expansion pass, to fold each supertype's methods into `methods`.
    supertypes: Vec<String>,
}

/// One interface method slot (D-0016): its name (used to resolve a call and to
/// name the implementor's `Struct__method` in the itable) and its ZIR signature
/// (excluding `self`), used to coerce arguments and to type the dispatch.
#[derive(Clone)]
struct IfaceMethodLow {
    name: String,
    params: Vec<Ty>,
    ret: Ty,
}

impl StructCtx {
    /// Owned `@scoped` fields of struct `sid`, in declaration order (#876,
    /// D-0159): `(field_idx, field_sid, field___exit___symbol)` for each field
    /// whose type is a `@scoped` struct. Empty for every struct that owns none
    /// (all structs today), so the drop-glue is a no-op for existing `with`s.
    fn scoped_field_exits(&self, sid: u32) -> Vec<(u32, u32, String)> {
        let Some(fs) = self.fields.get(sid as usize) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (idx, (_name, ty)) in fs.iter().enumerate() {
            if let Ty::Struct(fsid) = ty {
                if self.scoped_structs.contains(fsid) {
                    let exit_fn = format!("{}__{}", self.names[*fsid as usize], "__exit__");
                    out.push((idx as u32, *fsid, exit_fn));
                }
            }
        }
        out
    }

    /// Index and type of `name` in struct `sid`.
    fn field(&self, sid: u32, name: &str) -> Option<(u32, Ty)> {
        self.fields[sid as usize]
            .iter()
            .enumerate()
            .find(|(_, (n, _))| n == name)
            .map(|(i, (_, t))| (i as u32, *t))
    }

    fn tyenv(&self) -> TyEnv<'_> {
        TyEnv {
            ids: &self.ids,
            enums: &self.enums,
            ifaces: &self.ifaces,
            tuples: &self.tuples,
            fn_sigs: &self.fn_sigs,
            nested: &self.nested,
        }
    }

    /// If `sid` is an interface's synthetic fat-pointer layout, its interface id.
    fn iid_of_fat_sid(&self, sid: u32) -> Option<u32> {
        self.fat_to_iid.get(&sid).copied()
    }

    /// Is `sid` a `value struct` (value type, D-0051)?
    fn is_value_struct(&self, sid: u32) -> bool {
        self.value_ids.contains(&sid)
    }

    /// Register a module-level global slot (D-0112, #658 Phase 1), returning its
    /// index. Called by the init pre-pass in declaration order. `racy` records
    /// whether the slot is race-instrumented under `--race` (#677): a mutable,
    /// non-`@shareable` global — the ZAP5125 set (#664) — and `false` otherwise.
    fn add_global(&self, name: &str, ty: Ty, racy: bool) -> u32 {
        let mut g = self.globals.borrow_mut();
        let idx = g.len() as u32;
        g.push(GlobalVar {
            name: name.to_string(),
            ty,
        });
        self.global_racy.borrow_mut().push(racy);
        idx
    }

    /// Is a type named `name` `@shareable` (#664, D-0113)? Mirrors sema's
    /// `name_is_shareable`: a user `struct` marked `@shareable`, or one of the
    /// built-in concurrency primitives / concurrent collections that carry the
    /// marker implicitly ([`BUILTIN_SHAREABLE_TYPES`]). The one rule the race
    /// instrumentation and the sharing lint both consult, so their scopes match.
    fn name_is_shareable(&self, name: &str) -> bool {
        self.shareable_types.contains(name) || zapc_syntax::BUILTIN_SHAREABLE_TYPES.contains(&name)
    }

    /// The slot index of `name` iff it is a module global that `--race` should
    /// instrument (#677): it exists AND its [`global_racy`] bit is set (mutable +
    /// non-`@shareable`). Shadowing by a local is a separate check the caller
    /// makes (a local wins), so this consults only the module-global frame.
    fn racy_global_idx(&self, name: &str) -> Option<u32> {
        let g = self.globals.borrow();
        g.iter()
            .position(|v| v.name == name)
            .filter(|&i| self.global_racy.borrow()[i])
            .map(|i| i as u32)
    }

    /// Resolve a module-level global binding to its `(slot index, type)`, if any.
    /// A reference to a module `const` lowers to a [`Inst::GlobalAddr`] + load.
    fn global(&self, name: &str) -> Option<(u32, Ty)> {
        self.globals
            .borrow()
            .iter()
            .position(|g| g.name == name)
            .map(|i| (i as u32, self.globals.borrow()[i].ty))
    }

    /// Whether the program declares any module-level global (D-0112). `main`
    /// calls `@__zap_init_globals` once at its top exactly when this is true.
    fn has_globals(&self) -> bool {
        !self.globals.borrow().is_empty()
    }

    /// Whether `@__zap_init_globals` has any work and so must be emitted and
    /// called from `main`'s prologue: module globals to initialize (D-0112) or
    /// `@exception(Parent)` family edges to register at startup (#1278).
    fn has_init_work(&self) -> bool {
        self.has_globals() || !self.exception_parents.is_empty()
    }

    /// Whether `sid` uses the physical inline / register / sret layout (D-0051):
    /// a value struct or a tuple, all of whose fields are non-aggregate (no field
    /// is itself a value struct or tuple). This is the single source of truth for
    /// [`StructLayout::inline`]; lowering consults it to decide where the
    /// by-value calling convention already supplies the value-semantics copy, so
    /// an explicit copy can be dropped. See [`StructLayout::inline`] for why a
    /// nested value-aggregate field keeps the heap representation.
    fn inline_eligible(&self, sid: u32) -> bool {
        let is_tuple = self.tuples.borrow().is_tuple(sid);
        if !(self.is_value_struct(sid) || is_tuple) {
            return false;
        }
        let blocking = |fty: &Ty| -> bool {
            matches!(fty, Ty::Struct(fsid)
                if self.is_value_struct(*fsid) || self.tuples.borrow().is_tuple(*fsid))
        };
        if is_tuple {
            match self.tuples.borrow().fields(sid) {
                Some(fields) => !fields.iter().any(&blocking),
                None => false,
            }
        } else {
            !self.fields[sid as usize].iter().any(|(_, f)| blocking(f))
        }
    }
}

/// An expression whose value is an *existing object* (an lvalue read), so a
/// value struct obtained from it must be copied to preserve value semantics
/// (D-0051). A freshly produced value — a constructor call, function/method
/// call, or operator result — is already uniquely owned and needs no copy.
fn is_lvalue_read(e: &ast::Expr) -> bool {
    matches!(
        e,
        ast::Expr::Name(..) | ast::Expr::Attr { .. } | ast::Expr::Index { .. }
    )
}

/// The in-place augmented-assignment dunder for an operator (D-0047): `a OP= b`
/// calls `a.__iOP__(b)` when the LHS struct type defines it, mutating in place
/// and avoiding the fresh allocation the binary `__OP__` form needs. Only the
/// arithmetic / bitwise / shift operators that can appear in an augmented
/// assignment have one; everything else returns `None` (and falls back to the
/// derive-from-binary form).
fn inplace_op_dunder(op: AstBin) -> Option<&'static str> {
    use AstBin::*;
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
        _ => return None,
    })
}

/// The *reflected* binary-operator dunder for an operator (D-0047, #1180): `a OP
/// b` falls back to `b.__rOP__(a)` when the left operand cannot handle the
/// operator itself. Mirrors sema's `reflected_op_dunder`; only the arithmetic /
/// bitwise / shift / matmul operators have a reflected form.
fn reflected_op_dunder(op: AstBin) -> Option<&'static str> {
    use AstBin::*;
    Some(match op {
        Add => "__radd__",
        Sub => "__rsub__",
        Mul => "__rmul__",
        Div => "__rtruediv__",
        FloorDiv => "__rfloordiv__",
        Mod => "__rmod__",
        Pow => "__rpow__",
        MatMul => "__rmatmul__",
        BitAnd => "__rand__",
        BitOr => "__ror__",
        BitXor => "__rxor__",
        Shl => "__rlshift__",
        Shr => "__rrshift__",
        _ => return None,
    })
}

pub fn lower_module(module: &ast::Module) -> (Module, Vec<LowerError>) {
    // The default path lowers WITHOUT race instrumentation, so existing callers
    // and goldens are byte-for-byte unchanged (#677, epic #665): `--race` opts in
    // through [`lower_module_opts`].
    lower_module_opts(module, false)
}

/// Like [`lower_module`], but with the runtime data-race detector (`--race`,
/// #665, D-0114; epic #661) controllable: when `race` is `true`, AST→ZIR
/// lowering emits a `__zap_race_read` / `__zap_race_write` hook at every
/// instrumented shared-memory access (the ZAP5125 set, #664). When `false`,
/// emission is identical to today — no race hooks — so the `main` prologue and
/// every golden stay byte-identical.
pub fn lower_module_opts(module: &ast::Module, race: bool) -> (Module, Vec<LowerError>) {
    lower_module_opts_with_types(module, race, &crate::mono::TypeMap::default())
}

/// Like [`lower_module_opts`], but given sema's inferred types (#1167): the
/// monomorphizer consults `types` to pin a generic argument whose type its own
/// best-effort inferer cannot derive (built-in call returns #1132, module
/// constants #1162, …). The driver produces `types` with
/// `zapc_sema::analyze_types` and passes it here; `zapc_ir` cannot build it (it
/// does not depend on the sema checker). An empty map reproduces
/// [`lower_module_opts`] exactly.
pub fn lower_module_opts_with_types(
    module: &ast::Module,
    race: bool,
    types: &crate::mono::TypeMap,
) -> (Module, Vec<LowerError>) {
    // Prepend the built-in prelude (`Signal` enum, `MemStats` struct) so the
    // signals surface lowers through the ordinary enum/struct paths. Must match
    // the set sema sees (both call the same helper), ticket #77. `with_prelude`
    // preserves user-expression spans, so they still key into `types` (which
    // sema recorded over its own `with_prelude` module).
    let module = zapc_syntax::parser::with_prelude(module);
    // The library-registered derivable traits (#1290, E3a) must be collected
    // from the PRE-monomorphization module: a `@derivable(Trait)` generator is a
    // generic free function, which mono drops when it has no instantiation. We
    // keep only the trait names, so a `@derive(Trait)` can be reported as
    // deferred-to-E3b (#1291) rather than "unknown derivable trait".
    let library_derivables = library_derivable_traits(&module);
    // Monomorphize user generics to concrete declarations (I#154, D-0017/D-0064)
    // before any id/type collection runs, so the rest of lowering and all of
    // codegen only ever see concrete types.
    let module = match crate::mono::monomorphize_with_types(&module, types) {
        Ok(m) => m,
        Err(e) => return (Module::default(), vec![e]),
    };
    let module = &module;
    let mut errors = Vec::new();

    // Collect struct ids and tier-3-enum tagged-struct ids (a tier-3 enum is
    // lowered as a struct, D-0046), then resolve field types. Multiple passes so
    // fields may reference types declared later, and so a recursive enum variant
    // (whose field type is the enum itself) resolves to a pointer.
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    let mut value_ids: HashSet<u32> = HashSet::new();
    for item in &module.items {
        match item {
            ast::Item::Struct(s) => {
                let sid = names.len() as u32;
                ids.insert(s.name.clone(), sid);
                names.push(s.name.clone());
                // Record the value-vs-reference kind (D-0051); a `value struct`
                // gets value semantics at lowering.
                if s.is_value {
                    value_ids.insert(sid);
                }
                // A struct method generator (I#429): same synthetic state struct
                // as a free generator, keyed on the mangled `Struct__method` so
                // it is unique across types. `self` becomes one of its captured
                // fields. Its layout is filled once the signatures are known.
                for m in &s.methods {
                    if zapc_syntax::genanalysis::is_generator(m) {
                        let name = zapc_syntax::genanalysis::gen_state_name(&format!(
                            "{}__{}",
                            s.name, m.name
                        ));
                        ids.insert(name.clone(), names.len() as u32);
                        names.push(name);
                    }
                }
            }
            // A data-carrying enum needs a struct id reserved up front so its
            // own (possibly recursive) variant fields resolve to a pointer.
            ast::Item::Enum(e) if e.variants.iter().any(|v| !v.fields.is_empty()) => {
                ids.insert(e.name.clone(), names.len() as u32);
                names.push(e.name.clone());
                // An enum method generator (I#449): same synthetic state struct
                // as a struct method generator (I#429) / free generator (I#67),
                // keyed on the mangled `Enum__method`. `self` (the tagged-struct
                // receiver) becomes one of its captured fields. Layout filled once
                // signatures are known. Only data-carrying (tier-3) enum methods
                // lower — tier-1 enum method generators are rejected in sema
                // (ZAP5060), so no state struct is reserved for them here.
                for m in &e.methods {
                    if zapc_syntax::genanalysis::is_generator(m) {
                        let name = zapc_syntax::genanalysis::gen_state_name(&format!(
                            "{}__{}",
                            e.name, m.name
                        ));
                        ids.insert(name.clone(), names.len() as u32);
                        names.push(name);
                    }
                }
            }
            // A tier-1 (unboxed-integer) enum that carries methods gets a small
            // box struct `{Enum}$box = { $tag:i64 }` reserved up front (#731,
            // D-0016). An unboxed enum has no pointer identity, so to put it behind
            // an interface's `{data, itable}` fat pointer its scalar tag is stored
            // in this heap cell and that cell's pointer becomes `data`; the
            // interface's itable slots point at per-method thunks `{Enum}$box__m`
            // that reload the tag and tail-call the real `{Enum}__m(self: i64)`.
            // Reserved only when the enum has at least one method (nothing to
            // dispatch otherwise), so a plain tier-1 enum costs nothing.
            ast::Item::Enum(e)
                if !e.variants.iter().any(|v| !v.fields.is_empty()) && !e.methods.is_empty() =>
            {
                let box_name = format!("{}$box", e.name);
                ids.insert(box_name.clone(), names.len() as u32);
                names.push(box_name);
            }
            // A generator (a free `def` with `yield`, I#67) gets a synthetic
            // state-struct id reserved up front — its layout is filled after the
            // function signatures are known (its live locals need type inference).
            ast::Item::Function(f)
                if zapc_syntax::genanalysis::is_generator(f) && f.name != "main" =>
            {
                let name = zapc_syntax::genanalysis::gen_state_name(&f.name);
                ids.insert(name.clone(), names.len() as u32);
                names.push(name);
            }
            _ => {}
        }
    }

    // Interfaces (D-0016): each gets a synthetic inline `{ptr data, ptr itable}`
    // struct layout (`fat_sid`) — the ZIR type of an interface value is
    // `Ty::Struct(fat_sid)`. Reserve the ids up front (like structs/enums) so an
    // interface-typed field/parameter resolves, and so these ids are claimed
    // BEFORE the tuple interner captures `names.len()` as its base. The layout
    // name is prefixed with `$iface:` so it never collides with a user type.
    let mut ifaces: HashMap<String, u32> = HashMap::new();
    for item in &module.items {
        if let ast::Item::Interface(i) = item {
            let fat_sid = names.len() as u32;
            ifaces.insert(i.name.clone(), fat_sid);
            names.push(format!("$iface:{}", i.name));
        }
    }

    // Anonymous tuple layouts (D-0049) are interned after the named structs, so
    // their struct ids start at `names.len()`. The interner is shared across the
    // field/signature pre-passes and all body lowering, then its layouts are
    // appended to `Module::structs`.
    let tuples = RefCell::new(TupleInterner::new(names.len() as u32));
    // First-class function-value signatures (I#163) interned during lowering.
    let fn_sigs = RefCell::new(FnSigInterner::default());
    // Nested-collection element types (#625) interned during lowering.
    let nested = RefCell::new(NestedInterner::default());

    // Build the enum registry. For tier-1 enums, compute member discriminants
    // (explicit, else previous + 1 from 0). For tier-3 enums, lay out the tagged
    // struct `{ tag:i32, <flattened payload fields> }` and record each variant's
    // tag and its fields' struct indices.
    let mut enums: HashMap<String, EnumLow> = HashMap::new();
    // Sids reserved for tier-3 (data-carrying) enums: a payload of one of these is
    // itself a nullable/tagged value, so it can never be a niche's non-null
    // payload (I#154, D-0064 #4).
    let enum_sids: HashSet<u32> = module
        .items
        .iter()
        .filter_map(|it| match it {
            ast::Item::Enum(e) if e.variants.iter().any(|v| !v.fields.is_empty()) => {
                Some(ids[&e.name])
            }
            _ => None,
        })
        .collect();
    {
        // A borrow of `ids`/`enums` is needed to resolve payload field types,
        // but enum entries are inserted as they are built, so recursive and
        // forward references resolve against `ids` directly.
        let mut next_enum_id = 0u32;
        for item in &module.items {
            if let ast::Item::Enum(e) = item {
                let enum_id = next_enum_id;
                next_enum_id += 1;
                let is_sum = e.variants.iter().any(|v| !v.fields.is_empty());
                if !is_sum {
                    let mut members = Vec::new();
                    let mut next = 0i64;
                    for v in &e.variants {
                        let value = v.value.as_ref().and_then(const_int_value).unwrap_or(next);
                        members.push((v.name.clone(), value));
                        next = value + 1;
                    }
                    enums.insert(
                        e.name.clone(),
                        EnumLow {
                            enum_id,
                            enum_name: e.name.clone(),
                            is_sum: false,
                            sid: None,
                            members,
                            variants: Vec::new(),
                            niche: None,
                        },
                    );
                } else {
                    let sid = ids[&e.name];
                    let mut variants = Vec::new();
                    // Struct field 0 is the tag; payload fields follow, flattened
                    // across variants (each variant's fields occupy their own
                    // slots — a space-optimal overlapping union is a follow-up).
                    let mut next_idx = 1u32;
                    for (tag, v) in e.variants.iter().enumerate() {
                        let mut vfields = Vec::new();
                        for f in &v.fields {
                            let ft = resolve_ty(
                                &f.ty,
                                &TyEnv {
                                    ids: &ids,
                                    enums: &enums,
                                    ifaces: &ifaces,
                                    tuples: &tuples,
                                    fn_sigs: &fn_sigs,
                                    nested: &nested,
                                },
                            )
                            .unwrap_or(Ty::Ptr);
                            vfields.push((f.name.clone(), next_idx, ft));
                            next_idx += 1;
                        }
                        variants.push(VariantLow {
                            name: v.name.clone(),
                            tag: tag as i64,
                            fields: vfields,
                        });
                    }
                    // Niche optimization (I#154, D-0064 #4): an `Optional`-shaped
                    // enum — exactly two variants, one payload-free and one with a
                    // single field whose type is a non-nullable pointer (a
                    // reference type that is neither a value struct nor another
                    // tier-3 enum) — is represented as that single nullable
                    // pointer. Value-type payloads (scalars, value structs) and
                    // enum/nullable payloads keep the tagged representation.
                    let niche = if variants.len() == 2 {
                        let empty = variants.iter().find(|v| v.fields.is_empty());
                        let single = variants.iter().find(|v| v.fields.len() == 1);
                        match (empty, single) {
                            (Some(none_v), Some(some_v)) if none_v.name != some_v.name => {
                                let payload = some_v.fields[0].2;
                                let tagged_payload = matches!(payload, Ty::Struct(psid)
                                    if enum_sids.contains(&psid) || value_ids.contains(&psid));
                                if payload.is_ptr() && !tagged_payload {
                                    Some(NicheLow {
                                        some_variant: some_v.name.clone(),
                                        none_variant: none_v.name.clone(),
                                        payload,
                                    })
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    enums.insert(
                        e.name.clone(),
                        EnumLow {
                            enum_id,
                            enum_name: e.name.clone(),
                            is_sum: true,
                            sid: Some(sid),
                            members: Vec::new(),
                            variants,
                            niche,
                        },
                    );
                }
            }
        }
    }

    let mut fields: Vec<Vec<(String, Ty)>> = vec![Vec::new(); names.len()];
    // Literal field defaults (D-0038), parallel to `fields` (#461). Filled only
    // for named-struct fields; enum/interface synthetic layouts stay empty.
    let mut field_defaults: Vec<Vec<Option<Expr>>> = vec![Vec::new(); names.len()];
    // C-style bitfield packing (I#173, D-0068), keyed by struct id. A value
    // struct whose fields all carry a bit width packs into one backing integer.
    let mut packed_layouts: HashMap<u32, PackedLayout> = HashMap::new();
    // Per-field `@json(...)` options (ticket #346, D-0086), keyed by struct id.
    let mut json_field_opts: HashMap<u32, Vec<JsonFieldOpts>> = HashMap::new();
    // Per-field `@validate(...)` / `@range(...)` constraints (D-0101), keyed by
    // struct id and parallel to `fields[sid]`.
    let mut field_checks: HashMap<u32, Vec<Vec<FieldCheck>>> = HashMap::new();
    {
        let tyenv = TyEnv {
            ids: &ids,
            enums: &enums,
            ifaces: &ifaces,
            tuples: &tuples,
            fn_sigs: &fn_sigs,
            nested: &nested,
        };
        for item in &module.items {
            if let ast::Item::Struct(s) = item {
                let sid = ids[&s.name];
                let mut opts: Vec<JsonFieldOpts> = Vec::with_capacity(s.fields.len());
                let mut checks: Vec<Vec<FieldCheck>> = Vec::with_capacity(s.fields.len());
                for fld in &s.fields {
                    let ft = resolve_ty(&fld.ty, &tyenv).unwrap_or(Ty::Ptr);
                    fields[sid as usize].push((fld.name.clone(), ft));
                    // `@default(expr)` (D-0101) supplies a per-instance
                    // construction-time default, reusing the literal-default slot
                    // (#461). Sema has rejected a field carrying both forms.
                    let default = fld.default.clone().or_else(|| field_default_decorator(fld));
                    field_defaults[sid as usize].push(default);
                    opts.push(parse_json_field_opts(fld));
                    checks.push(collect_field_checks(fld, &mut errors));
                }
                if opts.iter().any(|o| o.timestamp.is_some()) {
                    json_field_opts.insert(sid, opts);
                }
                if checks.iter().any(|c| !c.is_empty()) {
                    field_checks.insert(sid, checks);
                }
                if let Some(pl) = packed_layout_of(s) {
                    packed_layouts.insert(sid, pl);
                }
            }
        }
        // Fill in each tier-3 enum's tagged-struct layout: tag + payload slots.
        for e in enums.values() {
            if let (true, Some(sid)) = (e.is_sum, e.sid) {
                let slot = &mut fields[sid as usize];
                slot.push(("$tag".to_string(), Ty::I32));
                for v in &e.variants {
                    for (fname, _idx, ft) in &v.fields {
                        slot.push((format!("{}${}", v.name, fname), *ft));
                    }
                }
            }
        }
        // Fill in each tier-1 enum box layout (#731): a single `i64` tag slot. The
        // box is a reference struct (not in `value_ids`), so `StructNew` heap-
        // allocates it and GC traces it as a pointer-free leaf. Its field type is
        // `i64` to match the enum's unboxed backing integer (`Ty::Enum` lowers to
        // `i64`).
        for item in &module.items {
            if let ast::Item::Enum(e) = item {
                let is_tier1 = !e.variants.iter().any(|v| !v.fields.is_empty());
                if is_tier1 && !e.methods.is_empty() {
                    let box_sid = ids[&format!("{}$box", e.name)];
                    fields[box_sid as usize].push(("$tag".to_string(), Ty::I64));
                }
            }
        }
        // Lay out each interface's synthetic fat-pointer struct as
        // `{ ptr data, ptr itable }` (D-0016, design/0006 §2.4).
        for &fat_sid in ifaces.values() {
            fields[fat_sid as usize] = vec![
                ("$data".to_string(), Ty::Ptr),
                ("$itable".to_string(), Ty::Ptr),
            ];
        }
    }

    // Build the interface metadata (D-0016): per interface, resolve each method's
    // ZIR signature (excluding `self`) in declaration order — the dispatch-table
    // slot order. `iid` is the index into `interfaces`.
    let mut interfaces: Vec<InterfaceLow> = Vec::new();
    let mut fat_to_iid: HashMap<u32, u32> = HashMap::new();
    {
        let tyenv = TyEnv {
            ids: &ids,
            enums: &enums,
            ifaces: &ifaces,
            tuples: &tuples,
            fn_sigs: &fn_sigs,
            nested: &nested,
        };
        for item in &module.items {
            if let ast::Item::Interface(i) = item {
                let fat_sid = ifaces[&i.name];
                let iid = interfaces.len() as u32;
                fat_to_iid.insert(fat_sid, iid);
                let methods = i
                    .methods
                    .iter()
                    .map(|m| {
                        // Parameter types excluding the leading `self`.
                        let params = m
                            .params
                            .iter()
                            .filter(|p| p.name != "self")
                            .map(|p| {
                                p.ty.as_ref()
                                    .map(|t| resolve_ty(t, &tyenv).unwrap_or(Ty::Ptr))
                                    .unwrap_or(Ty::Ptr)
                            })
                            .collect();
                        let ret = m
                            .ret
                            .as_ref()
                            .map(|t| resolve_ty(t, &tyenv).unwrap_or(Ty::Void))
                            .unwrap_or(Ty::Void);
                        IfaceMethodLow {
                            name: m.name.clone(),
                            params,
                            ret,
                        }
                    })
                    .collect();
                // Refined supertypes (D-0139/#806): record each named supertype
                // so the post-build pass can fold its methods into this
                // interface's dispatch-table (#830). A supertype is always a
                // plain interface name; anything else is rejected by sema.
                let supertypes = i
                    .supertypes
                    .iter()
                    .filter_map(|t| match t {
                        ast::Type::Named { name, .. } => Some(name.clone()),
                        _ => None,
                    })
                    .collect();
                interfaces.push(InterfaceLow {
                    name: i.name.clone(),
                    fat_sid,
                    methods,
                    supertypes,
                });
            }
        }
    }

    // Fold each interface's transitive refined-supertype methods into its
    // dispatch-table (#830, D-0139). The slot order is the interface's own
    // methods first (declaration order), then each supertype's *effective*
    // methods (its own plus its supertypes', recursively) in supertype
    // declaration order, skipping any method name already present — so an
    // interface that re-declares a supertype method keeps its own slot and an
    // inherited method appears exactly once. This single list is the source of
    // truth for both the itable layout (codegen builds slot k from
    // `interfaces[iid].methods[k]` by name -> `Concrete__name`) and dynamic
    // dispatch (`lower_iface_call` resolves a method's index by searching it),
    // so the two always agree. Cycle-safe via a visited set; a malformed cyclic
    // refinement is reported separately by sema (ZAP4082).
    {
        let by_name: HashMap<String, usize> = interfaces
            .iter()
            .enumerate()
            .map(|(idx, i)| (i.name.clone(), idx))
            .collect();
        // Depth-first effective-method list for interface `idx`, appending to
        // `out` and tracking seen method names in `seen`.
        fn collect_effective(
            idx: usize,
            interfaces: &[InterfaceLow],
            by_name: &HashMap<String, usize>,
            visited: &mut HashSet<usize>,
            seen: &mut HashSet<String>,
            out: &mut Vec<IfaceMethodLow>,
        ) {
            if !visited.insert(idx) {
                return;
            }
            for m in &interfaces[idx].methods {
                if seen.insert(m.name.clone()) {
                    out.push(m.clone());
                }
            }
            for sup in &interfaces[idx].supertypes {
                if let Some(&sidx) = by_name.get(sup) {
                    collect_effective(sidx, interfaces, by_name, visited, seen, out);
                }
            }
        }
        let expanded: Vec<Vec<IfaceMethodLow>> = (0..interfaces.len())
            .map(|idx| {
                let mut out = Vec::new();
                let mut visited = HashSet::new();
                let mut seen = HashSet::new();
                collect_effective(
                    idx,
                    &interfaces,
                    &by_name,
                    &mut visited,
                    &mut seen,
                    &mut out,
                );
                out
            })
            .collect();
        for (iface, methods) in interfaces.iter_mut().zip(expanded) {
            iface.methods = methods;
        }
    }
    // Python imports (ticket #27, D-0012): map each imported module's in-scope
    // name to its CPython module name. The alias defaults to the module string
    // when `as` is omitted.
    let mut py: HashMap<String, String> = HashMap::new();
    for item in &module.items {
        if let ast::Item::Import(ast::Import::Python { module, alias, .. }) = item {
            let name = alias.clone().unwrap_or_else(|| module.clone());
            py.insert(name, module.clone());
        }
    }
    // Architecture-specific intrinsics imports (I#139 Tier 2, D-0199): receiver
    // name → canonical `arch.*` registry name, mirroring the sema dispatch.
    let arch_imports: HashMap<String, &'static str> =
        zapc_syntax::arch::collect_arch_imports(module)
            .into_iter()
            .map(|(name, arch)| (name, zapc_syntax::arch::arch_module_name(arch)))
            .collect();

    // User `@shareable` struct names (#664, D-0113), for the `--race`
    // instrumentation predicate (#677): a mutable module global of such a type is
    // an asserted-safe cell and is never instrumented, matching the ZAP5125 lint.
    // Built-in shareable types are handled by `StructCtx::name_is_shareable`.
    let shareable_types: HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| match item {
            ast::Item::Struct(s)
                if s.decorators
                    .iter()
                    .any(|d| d.name == zapc_syntax::SHAREABLE_MARKER) =>
            {
                Some(s.name.clone())
            }
            _ => None,
        })
        .collect();

    // `@scoped` struct ids (#876, D-0159): explicitly marked, closed under
    // contagion (a struct owning a `@scoped` field is itself `@scoped`). Drives
    // the with-lowering auto-teardown drop-glue. Computed before `names`/`fields`
    // are moved into `ctx`.
    let scoped_structs: HashSet<u32> = {
        let mut s: HashSet<u32> = module
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Struct(st)
                    if st
                        .decorators
                        .iter()
                        .any(|d| d.name == zapc_syntax::SCOPED_MARKER) =>
                {
                    ids.get(&st.name).copied()
                }
                _ => None,
            })
            .collect();
        loop {
            let mut added: Vec<u32> = Vec::new();
            for sid in 0..names.len() as u32 {
                if s.contains(&sid) {
                    continue;
                }
                let owns_scoped = fields[sid as usize]
                    .iter()
                    .any(|(_, t)| matches!(t, Ty::Struct(f) if s.contains(f)));
                if owns_scoped {
                    added.push(sid);
                }
            }
            if added.is_empty() {
                break;
            }
            s.extend(added);
        }
        s
    };

    // `@exception(Parent)` family edges (#1278), resolved to `(member_id,
    // parent_id)` name-hash pairs — the same ids `raise`/`except` compute. sema
    // has already validated that each parent is an `@exception` struct and that
    // the forest is acyclic, so this is a straight read of the declarations.
    let exception_parents: Vec<(i64, i64)> = module
        .items
        .iter()
        .filter_map(|item| match item {
            ast::Item::Struct(st) => st
                .decorators
                .iter()
                .find(|d| d.name == zapc_syntax::EXCEPTION_MARKER)
                .and_then(|d| d.args.first())
                .and_then(|a| match &a.value {
                    ast::Expr::Name(parent, _) => Some(parent.clone()),
                    _ => None,
                })
                .map(|parent| {
                    let member_id = custom_exception_type_id(&st.name);
                    let parent_id = exception_type_id_name(&parent)
                        .unwrap_or_else(|| custom_exception_type_id(&parent));
                    (member_id, parent_id)
                }),
            _ => None,
        })
        .collect();

    let mut ctx = StructCtx {
        ids,
        names,
        fields,
        value_ids,
        scoped_structs,
        exception_parents,
        enums,
        py,
        arch_imports,
        tuples,
        ifaces,
        interfaces,
        fat_to_iid,
        generators: HashMap::new(),
        fn_sigs,
        nested,
        generated: RefCell::new(Vec::new()),
        genexpr_iters: RefCell::new(HashMap::new()),
        genexpr_counter: RefCell::new(0),
        closure_env_sids: RefCell::new(Vec::new()),
        closure_counter: RefCell::new(0),
        captureless_env: RefCell::new(None),
        fnval_wrappers: RefCell::new(HashMap::new()),
        spawn_thunks: RefCell::new(HashMap::new()),
        synth_keyfns: RefCell::new(HashMap::new()),
        synth_reprfns: RefCell::new(HashMap::new()),
        synth_container_reprfns: RefCell::new(HashMap::new()),
        iface_renderers: RefCell::new(HashMap::new()),
        packed_layouts,
        field_defaults,
        call_params: HashMap::new(),
        json_field_opts,
        field_checks,
        globals: RefCell::new(Vec::new()),
        global_racy: RefCell::new(Vec::new()),
        shareable_types,
        race,
        associated_fns: HashSet::new(),
    };

    // Signatures: top-level functions and struct methods (mangled `S__m`).
    let mut sigs: HashMap<String, (Vec<Ty>, Ty)> = HashMap::new();
    // Per-function parameter metadata for default/keyword argument assembly
    // (D-0038, I#298). Collected for free functions, whose calls go through the
    // name-dispatch path that honors defaults and keyword reordering.
    let mut call_params: HashMap<String, Vec<ParamInfo>> = HashMap::new();
    // Mangled names of receiverless associated functions (#1260), so the call
    // site can dispatch `Type.func(args)` as a no-receiver free call.
    let mut associated_fns: HashSet<String> = HashSet::new();
    for item in &module.items {
        match item {
            ast::Item::Function(f) => {
                let params = f.params.iter().map(|p| param_ty(p, None, &ctx)).collect();
                call_params.insert(
                    f.name.clone(),
                    f.params
                        .iter()
                        .map(|p| ParamInfo {
                            name: p.name.clone(),
                            default: p.default.clone(),
                            variadic: p.variadic,
                            kwargs: p.kwargs,
                        })
                        .collect(),
                );
                // A generator's call returns its synthetic iterator state struct
                // (I#67), not the declared/`void` type; the body is compiled into
                // the struct's `__next__`, not a function named `f`.
                let ret = if zapc_syntax::genanalysis::is_generator(f) && f.name != "main" {
                    Ty::Struct(ctx.ids[&zapc_syntax::genanalysis::gen_state_name(&f.name)])
                } else {
                    f.ret
                        .as_ref()
                        .map(|t| resolve_ty(t, &ctx.tyenv()).unwrap_or(Ty::Void))
                        .unwrap_or(Ty::Void)
                };
                sigs.insert(f.name.clone(), (params, ret));
            }
            ast::Item::Struct(s) => {
                let sid = ctx.ids[&s.name];
                for m in &s.methods {
                    let params = m
                        .params
                        .iter()
                        .map(|p| param_ty(p, Some(sid), &ctx))
                        .collect();
                    let mangled = format!("{}__{}", s.name, m.name);
                    // A generator method (I#429): calling it returns its synthetic
                    // iterator state struct (like a free generator, I#67), not the
                    // declared/`void` type — its body is compiled into the state
                    // struct's `__next__`, not a function named `Struct__method`.
                    let ret = if zapc_syntax::genanalysis::is_generator(m) {
                        Ty::Struct(ctx.ids[&zapc_syntax::genanalysis::gen_state_name(&mangled)])
                    } else {
                        m.ret
                            .as_ref()
                            .map(|t| resolve_ty(t, &ctx.tyenv()).unwrap_or(Ty::Void))
                            .unwrap_or(Ty::Void)
                    };
                    // Default/keyword argument metadata for method calls (I#481,
                    // parity with free functions I#298). The implicit `self`
                    // receiver is dropped so the recorded params line up with the
                    // call site's explicit arguments; the method-call path re-adds
                    // `self` when it assembles the call. A receiverless `def` is an
                    // associated function (#1260): it keeps its full parameter list
                    // and is recorded so the call site dispatches it with no
                    // receiver.
                    if is_receiverless(m) {
                        associated_fns.insert(mangled.clone());
                        call_params.insert(mangled.clone(), fn_param_info(m, false));
                    } else {
                        call_params.insert(mangled.clone(), method_param_info(m));
                    }
                    sigs.insert(mangled, (params, ret));
                }
            }
            // Tier-3 enum methods (D-0046; used by generic `Optional[T]` etc.,
            // I#154). A tier-3 enum lowers to a tagged struct, so `self` is that
            // struct and a method call dispatches through the ordinary
            // struct-method path. Tier-1 enum methods (#731) take `self` as the
            // unboxed backing integer (`Ty::Enum`); see the `else` branch.
            ast::Item::Enum(e) => {
                if let Some(sid) = ctx.enums.get(&e.name).and_then(|i| i.sid) {
                    for m in &e.methods {
                        let params = m
                            .params
                            .iter()
                            .map(|p| param_ty(p, Some(sid), &ctx))
                            .collect();
                        let mangled = format!("{}__{}", e.name, m.name);
                        // An enum method generator (I#449): calling it returns its
                        // synthetic iterator state struct (like a struct method
                        // generator, I#429), not the declared/`void` type — its
                        // body is compiled into the state struct's `__next__`.
                        let ret = if zapc_syntax::genanalysis::is_generator(m) {
                            Ty::Struct(ctx.ids[&zapc_syntax::genanalysis::gen_state_name(&mangled)])
                        } else {
                            m.ret
                                .as_ref()
                                .map(|t| resolve_ty(t, &ctx.tyenv()).unwrap_or(Ty::Void))
                                .unwrap_or(Ty::Void)
                        };
                        // Method default/keyword argument metadata (I#481), as
                        // for struct methods above — `self` dropped, unless this is
                        // a receiverless associated function (#1260).
                        if is_receiverless(m) {
                            associated_fns.insert(mangled.clone());
                            call_params.insert(mangled.clone(), fn_param_info(m, false));
                        } else {
                            call_params.insert(mangled.clone(), method_param_info(m));
                        }
                        sigs.insert(mangled, (params, ret));
                    }
                } else if !e.methods.is_empty() {
                    // Tier-1 (unboxed-integer) enum methods (#731). `self` is the
                    // backing integer `Ty::Enum(enum_id)` (a value, by-value). Two
                    // symbols per method: the real `{Enum}__m(self: enum, ...)`
                    // (also the target of a direct `value.m()` call), and — when the
                    // enum has a box (it carries methods) — a thunk
                    // `{Enum}$box__m(self: box_ptr, ...)` placed in interface
                    // itables, which reloads the tag and tail-calls the real method.
                    let enum_id = ctx.enums[&e.name].enum_id;
                    let box_sid = ctx.ids.get(&format!("{}$box", e.name)).copied();
                    for m in &e.methods {
                        let mangled = format!("{}__{}", e.name, m.name);
                        let ret = m
                            .ret
                            .as_ref()
                            .map(|t| resolve_ty(t, &ctx.tyenv()).unwrap_or(Ty::Void))
                            .unwrap_or(Ty::Void);
                        let params: Vec<Ty> = m
                            .params
                            .iter()
                            .map(|p| {
                                if p.name == "self" {
                                    Ty::Enum(enum_id)
                                } else {
                                    param_ty(p, None, &ctx)
                                }
                            })
                            .collect();
                        // A receiverless associated function (#1260) keeps its full
                        // parameter list and is never placed in an itable (it has no
                        // `self`), so it needs no box thunk.
                        if is_receiverless(m) {
                            associated_fns.insert(mangled.clone());
                            call_params.insert(mangled.clone(), fn_param_info(m, false));
                        } else {
                            call_params.insert(mangled.clone(), method_param_info(m));
                            if let Some(bsid) = box_sid {
                                let mut tparams = vec![Ty::Struct(bsid)];
                                tparams.extend_from_slice(&params[1..]);
                                sigs.insert(format!("{}$box__{}", e.name, m.name), (tparams, ret));
                            }
                        }
                        sigs.insert(mangled, (params, ret));
                    }
                }
            }
            _ => {}
        }
    }

    // Decorators & annotations (ticket #16, D-0036). Register the signatures of
    // the methods `@derive(...)` will generate, so user calls (`u.eq(v)`) resolve
    // during body lowering, and collect the runtime-reflection metadata.
    let deco = DecoratorPlan::collect(module, &ctx, &library_derivables, &mut errors);
    for (sid, traits) in &deco.derived {
        for t in traits {
            for (mname, psig, ret) in t.signatures(*sid) {
                sigs.insert(
                    format!("{}__{}", ctx.names[*sid as usize], mname),
                    (psig, ret),
                );
            }
        }
    }
    // Field-validator signatures (D-0101): a struct carrying `@validate`/`@range`
    // field decorators gets a generated `{Struct}____validate__(self)` guard that
    // `construct` invokes after it fills every field.
    for &sid in ctx.field_checks.keys() {
        sigs.insert(
            format!("{}____validate__", ctx.names[sid as usize]),
            (vec![Ty::Struct(sid)], Ty::Void),
        );
    }
    // Open string-enum JSON codec signatures (ticket #346, D-0086): `to_json`
    // (self -> str), `from_json` (str -> Self), and the DOM bridge
    // `from_json_value` (JsonValue -> Self) so a nested enum field decodes.
    if let Some(jv_sid) = ctx.ids.get("JsonValue").copied() {
        for &sid in &deco.json_enums {
            let name = ctx.names[sid as usize].clone();
            sigs.insert(format!("{name}__to_json"), (vec![Ty::Struct(sid)], Ty::Ptr));
            sigs.insert(
                format!("{name}__from_json"),
                (vec![Ty::Ptr], Ty::Struct(sid)),
            );
            sigs.insert(
                format!("{name}__from_json_value"),
                (vec![Ty::Struct(jv_sid)], Ty::Struct(sid)),
            );
        }
    }

    // Generators (I#67, I#429): discover each generator's live locals + element
    // type, lay out its state struct, and register the synthetic
    // `__iter__`/`__next__` signatures — all before body lowering, so a
    // `for x in gen()` / `for x in obj.gen()` and the emitted methods resolve
    // through the ordinary struct/method machinery.
    //
    // A generator's **callable key** in `sigs` is a free generator's own name or
    // a struct method generator's mangled `Struct__method` (I#429); the key is
    // also the base of the synthetic state-struct name (`$gen:<key>`). The two
    // cases share all the layout/registration logic below, keyed only on this
    // string and the generator's AST `&Function` (its `self`, when present, is
    // simply the leading captured parameter).
    let mut gen_funcs: Vec<(String, &ast::Function)> = Vec::new();
    for item in &module.items {
        match item {
            ast::Item::Function(f)
                if zapc_syntax::genanalysis::is_generator(f) && f.name != "main" =>
            {
                gen_funcs.push((f.name.clone(), f));
            }
            ast::Item::Struct(s) => {
                for m in &s.methods {
                    if zapc_syntax::genanalysis::is_generator(m) {
                        gen_funcs.push((format!("{}__{}", s.name, m.name), m));
                    }
                }
            }
            // Tier-3 enum method generators (I#449): collected exactly like
            // struct method generators — keyed on the mangled `Enum__method`, the
            // captured `self` being the tagged-struct receiver. Gated on the
            // tagged-struct id (data-carrying enums only); tier-1 enum method
            // generators never reach here (rejected in sema, ZAP5060).
            ast::Item::Enum(e) if ctx.enums.get(&e.name).and_then(|i| i.sid).is_some() => {
                for m in &e.methods {
                    if zapc_syntax::genanalysis::is_generator(m) {
                        gen_funcs.push((format!("{}__{}", e.name, m.name), m));
                    }
                }
            }
            _ => {}
        }
    }
    let mut gen_map: HashMap<String, GeneratorInfo> = HashMap::new();
    for (key, f) in gen_funcs {
        let (locals, elem) = match discover_generator(f, &key, &sigs, &ctx) {
            Ok(v) => v,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let sname = zapc_syntax::genanalysis::gen_state_name(&key);
        let sid = ctx.ids[&sname];
        // State-struct vars: the parameters (captured at construction; `self` is
        // simply the leading one for a method generator) then the discovered body
        // locals; field index is `1 + position` (`$state` is 0).
        let param_tys = sigs[&key].0.clone();
        let mut vars: Vec<(String, Ty)> = f
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), param_tys[i]))
            .collect();
        let n_params = vars.len();
        vars.extend(locals);
        let next_tuple_sid = ctx.tuples.borrow_mut().intern(vec![elem, Ty::I1]);
        // Fill the state struct's layout: `$state` discriminant + the var fields.
        let mut layout: Vec<(String, Ty)> = vec![("$state".to_string(), Ty::I64)];
        layout.extend(vars.iter().cloned());
        ctx.fields[sid as usize] = layout;
        // Register the iterator dunders so the for-loop/comprehension driver and
        // `setup_iter`/`next_form` find them.
        sigs.insert(
            format!("{sname}____iter__"),
            (vec![Ty::Struct(sid)], Ty::Struct(sid)),
        );
        sigs.insert(
            format!("{sname}____next__"),
            (vec![Ty::Struct(sid)], Ty::Struct(next_tuple_sid)),
        );
        gen_map.insert(
            key.clone(),
            GeneratorInfo {
                name: key,
                sid,
                elem,
                next_tuple_sid,
                vars,
                n_params,
            },
        );
    }
    ctx.generators = gen_map;
    ctx.call_params = call_params;
    ctx.associated_fns = associated_fns;

    let mut out = Module::default();
    for sid in 0..ctx.names.len() {
        out.structs.push(StructLayout {
            name: ctx.names[sid].clone(),
            fields: ctx.fields[sid].iter().map(|(_, t)| *t).collect(),
            is_value: ctx.is_value_struct(sid as u32),
            inline: false, // set in the post-pass below, once all layouts exist
            packed: ctx.packed_layouts.get(&(sid as u32)).cloned(),
        });
    }
    // Module-level `const` state (D-0112, #658 Phase 1): lower the const
    // initializers into `@__zap_init_globals` BEFORE any user function, so the
    // global slots are registered in `ctx.globals` and a reference to one
    // resolves (see `FnLower::expr`'s `Expr::Name` global fallback). `main`
    // calls `@__zap_init_globals` once at its top (see `lower_fn`). The module
    // linker has already ordered the items import-topologically (D-0081), so the
    // consts initialize in dependency-then-source order.
    match lower_init_globals(module, &sigs, &ctx) {
        Ok(func) => {
            if ctx.has_init_work() {
                out.funcs.push(func);
            }
        }
        Err(e) => errors.push(e),
    }
    out.globals = ctx.globals.borrow().clone();
    for item in &module.items {
        match item {
            // A generator is lowered to its state struct's `__iter__`/`__next__`
            // (I#67), not a function named `f`.
            ast::Item::Function(f) if ctx.generators.contains_key(&f.name) => {
                let gi = ctx.generators[&f.name].clone();
                out.funcs.push(lower_generator_iter(&gi, &sigs, &ctx));
                match lower_generator_next(f, &gi, &sigs, &ctx) {
                    Ok(func) => out.funcs.push(func),
                    Err(e) => errors.push(e),
                }
            }
            ast::Item::Function(f) => {
                match lower_fn(
                    f,
                    f.name.clone(),
                    None,
                    &sigs,
                    &ctx,
                    &deco.reflect_meta,
                    &deco.param_meta,
                    &deco.on_signal_meta,
                ) {
                    // `@multiversion` (#944, epic #139): emit the clone set + the
                    // group metadata instead of the bare function; codegen builds
                    // the dispatcher under the public name. Other functions emit
                    // unchanged.
                    Ok(mut func) => match f.decorators.iter().find(|d| d.name == "multiversion") {
                        Some(d) => match expand_multiversion(func, d) {
                            Ok((clones, group)) => {
                                out.funcs.extend(clones);
                                out.multiversion_groups.push(group);
                            }
                            Err(e) => errors.push(e),
                        },
                        None => {
                            // `@target_feature("feat")` (#963, epic #139, D-0200):
                            // stamp the single feature so codegen compiles the body
                            // with it enabled and preserves it through LTO (reusing
                            // the `Func.target_feature` channel a `@multiversion`
                            // clone uses). sema has rejected any unguarded call.
                            if let Some(feat) = target_feature_of(&f.decorators) {
                                func.target_feature = Some(feat);
                            }
                            out.funcs.push(func);
                        }
                    },
                    Err(e) => errors.push(e),
                }
            }
            ast::Item::Struct(s) => {
                let sid = ctx.ids[&s.name];
                // `Atomic[T]` (I#176): its methods are compiler intrinsics lowered
                // inline at the call site (see `lower_atomic_method`), so the
                // prelude's placeholder bodies are never emitted — skip them rather
                // than emit dead `Atomic$T__load`/… functions.
                if is_atomic_struct(&ctx, sid) {
                    continue;
                }
                for m in &s.methods {
                    let name = format!("{}__{}", s.name, m.name);
                    // A generator method (I#429) is lowered to its state struct's
                    // `__iter__`/`__next__`, not to a function named
                    // `Struct__method` — exactly like a free generator (I#67). Its
                    // captured `self` is the leading state-struct field.
                    if let Some(gi) = ctx.generators.get(&name) {
                        let gi = gi.clone();
                        out.funcs.push(lower_generator_iter(&gi, &sigs, &ctx));
                        match lower_generator_next(m, &gi, &sigs, &ctx) {
                            Ok(func) => out.funcs.push(func),
                            Err(e) => errors.push(e),
                        }
                        continue;
                    }
                    // A receiverless associated function (#1260) has no `self`
                    // receiver, so it lowers with `self_param = false` (`self_sid`
                    // `None`): its first parameter is an ordinary by-value argument,
                    // not a by-reference receiver — the ABI must match the no-
                    // receiver call site.
                    let recv_sid = if is_receiverless(m) { None } else { Some(sid) };
                    match lower_fn(m, name, recv_sid, &sigs, &ctx, &[], &[], &[]) {
                        Ok(func) => out.funcs.push(func),
                        Err(e) => errors.push(e),
                    }
                }
            }
            // Tier-3 enum methods (D-0046 / I#154): lowered like struct methods
            // on the tagged-struct `self`.
            ast::Item::Enum(e) => {
                if let Some(sid) = ctx.enums.get(&e.name).and_then(|i| i.sid) {
                    for m in &e.methods {
                        let name = format!("{}__{}", e.name, m.name);
                        // An enum method generator (I#449) is lowered to its state
                        // struct's `__iter__`/`__next__` — exactly like a struct
                        // method generator (I#429). Its captured `self` (the
                        // tagged-struct receiver) is the leading state-struct field.
                        if let Some(gi) = ctx.generators.get(&name) {
                            let gi = gi.clone();
                            out.funcs.push(lower_generator_iter(&gi, &sigs, &ctx));
                            match lower_generator_next(m, &gi, &sigs, &ctx) {
                                Ok(func) => out.funcs.push(func),
                                Err(e) => errors.push(e),
                            }
                            continue;
                        }
                        // A receiverless associated function (#1260) lowers with
                        // `self_param = false` — no by-reference `self` receiver.
                        let recv_sid = if is_receiverless(m) { None } else { Some(sid) };
                        match lower_fn(m, name, recv_sid, &sigs, &ctx, &[], &[], &[]) {
                            Ok(func) => out.funcs.push(func),
                            Err(e) => errors.push(e),
                        }
                    }
                } else if !e.methods.is_empty() {
                    // Tier-1 (unboxed-integer) enum methods (#731). The real method
                    // body lowers like a free function whose `self` is the backing
                    // integer (`Ty::Enum`, supplied by the registered sig, so
                    // `self_sid` is `None` — `self` is by value, not a struct
                    // receiver). Alongside it, emit the itable thunk
                    // `{Enum}$box__m` that reloads the tag from the heap box and
                    // tail-calls the real method, so dynamic dispatch through an
                    // interface value reaches the same code a direct call does.
                    let box_sid = ctx.ids.get(&format!("{}$box", e.name)).copied();
                    for m in &e.methods {
                        let name = format!("{}__{}", e.name, m.name);
                        match lower_fn(m, name, None, &sigs, &ctx, &[], &[], &[]) {
                            Ok(func) => out.funcs.push(func),
                            Err(e) => errors.push(e),
                        }
                        // A receiverless associated function (#1260) is never in an
                        // itable (it has no `self`), so it needs no box thunk — and
                        // the thunk builder assumes a `self` receiver, so skip it.
                        if let Some(bsid) = box_sid {
                            if !is_receiverless(m) {
                                match build_tier1_box_thunk(&e.name, &m.name, bsid, &sigs, &ctx) {
                                    Ok(func) => out.funcs.push(func),
                                    Err(e) => errors.push(e),
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Generate the bodies of the derived methods (compile-time expansion of the
    // built-in `@derive(...)` decorator, kept isolated from expression lowering).
    // `@derive(Json)` and `@derive(Yaml)` share the DOM decode core
    // (`{Struct}__from_json_value`); when a struct derives both, it is emitted by
    // each, so dedup generated functions by name (keep the first).
    let mut gen_names: HashSet<String> = HashSet::new();
    for (sid, traits) in &deco.derived {
        for t in traits {
            match t.generate(*sid, &sigs, &ctx) {
                Ok(funcs) => {
                    for f in funcs {
                        if gen_names.insert(f.name.clone()) {
                            out.funcs.push(f);
                        }
                    }
                }
                Err(e) => errors.push(e),
            }
        }
    }
    // Open string-enum JSON codec functions (ticket #346, D-0086).
    for &sid in &deco.json_enums {
        match gen_open_enum_json(sid, &sigs, &ctx) {
            Ok(funcs) => {
                for f in funcs {
                    if gen_names.insert(f.name.clone()) {
                        out.funcs.push(f);
                    }
                }
            }
            Err(e) => errors.push(e),
        }
    }
    // Field-validator methods (D-0101): one `{Struct}____validate__(self)` per
    // struct that carries `@validate`/`@range` field decorators. Rendered in the
    // ZIR dump, so `zap expand` shows the generated validation, and invoked from
    // `construct` so every construction is checked.
    for &sid in ctx.field_checks.keys() {
        match gen_validate_method(sid, &sigs, &ctx) {
            Ok(func) => {
                if gen_names.insert(func.name.clone()) {
                    out.funcs.push(func);
                }
            }
            Err(e) => errors.push(e),
        }
    }

    // Append the compiler-generated closure functions (I#163): lifted lambda
    // bodies and bare-function value wrappers, produced while lowering the bodies
    // above. They are already fully lowered ZIR functions.
    out.funcs.append(&mut ctx.generated.borrow_mut());

    // Append the interned tuple layouts (D-0049) after the named structs. Their
    // ids were handed out as `base + index` with `base == out.structs.len()`
    // here, so they line up positionally. A tuple is laid out exactly like an
    // anonymous struct, so codegen needs no tuple-specific path.
    for elems in ctx.tuples.borrow().layouts.iter() {
        out.structs.push(StructLayout {
            name: "(tuple)".to_string(),
            fields: elems.clone(),
            is_value: false,
            inline: false, // set in the post-pass below
            packed: None,
        });
    }

    // Physical inline-layout pass (D-0051). A value struct or a tuple is a
    // value-semantic aggregate with no shared identity, so it can be laid out
    // inline and passed/returned by value (register/sret) instead of as a heap
    // pointer — but only when none of its fields is *itself* a value aggregate
    // (see `StructCtx::inline_eligible`, the shared predicate).
    for sid in 0..out.structs.len() {
        out.structs[sid].inline = ctx.inline_eligible(sid as u32);
    }

    // Interface fat pointers are passed/returned by value exactly like an inline
    // value aggregate (D-0016, design/0006 §2.4: "the one reference type wider
    // than a single word"), so force their synthetic `{ptr, ptr}` layout inline.
    // Emit the interface table so codegen can build dispatch itables.
    for i in &ctx.interfaces {
        out.structs[i.fat_sid as usize].inline = true;
        out.interfaces.push(InterfaceLayout {
            name: i.name.clone(),
            fat_sid: i.fat_sid,
            methods: i
                .methods
                .iter()
                .map(|m| IfaceMethod {
                    name: m.name.clone(),
                    params: m.params.clone(),
                    ret: m.ret,
                })
                .collect(),
        });
    }

    // Closure-environment records (I#163) are reference structs with shared
    // identity that outlive their creating frame, so force them non-inline
    // regardless of what the inline-eligibility pass decided (a one-word
    // `{code_ptr}` env would otherwise be inlined and dangle).
    for &sid in ctx.closure_env_sids.borrow().iter() {
        out.structs[sid as usize].inline = false;
    }

    // Materialize the interned first-class function-value signatures (I#163).
    out.fn_sigs = std::mem::take(&mut ctx.fn_sigs.borrow_mut().sigs);

    // Hand codegen the per-concrete-type Display renderers recorded at each
    // concrete->interface coercion (#707, D-0124), so it can place each one in the
    // renderer slot of that type's itable for `Inst::IfaceRender` dispatch.
    out.iface_renderers = std::mem::take(&mut ctx.iface_renderers.borrow_mut());

    // Escape analysis (#839, discussion #849): lower non-escaping leaf /
    // pointer-free / finalizer-free `StructNew` and f-string `StrBuild` nodes to
    // the stack. Conservative — it only ever turns a heap allocation into a stack
    // one, so a well-formed module stays well-formed and semantics are preserved;
    // run it only when lowering produced a complete module (no errors).
    if errors.is_empty() {
        crate::escape::run(&mut out);
    }

    (out, errors)
}

fn resolve_ty(t: &ast::Type, env: &TyEnv) -> Result<Ty, LowerError> {
    match t {
        ast::Type::Named { name, args, .. } => match name.as_str() {
            "int" | "int64" | "i64" => Ok(Ty::I64),
            "int32" | "i32" => Ok(Ty::I32),
            // `float` is the Python-facing name for the 64-bit IEEE double
            // (I#569); `float64`/`f64` are the explicit-width spellings.
            "float" | "float64" | "f64" => Ok(Ty::F64),
            "float32" | "f32" => Ok(Ty::F32),
            // Unsigned scalar integers (#552, D-0019 amendment). `usize` is
            // pointer-width — 64-bit on the supported targets — so it lowers to
            // `u64`; it stays a distinct *source* type (sema `Ty::Usize`) only so
            // diagnostics and the C mapping (`size_t`) can tell them apart.
            "uint8" | "u8" => Ok(Ty::U8),
            "uint16" | "u16" => Ok(Ty::U16),
            "uint32" | "u32" => Ok(Ty::U32),
            "uint64" | "u64" => Ok(Ty::U64),
            "usize" => Ok(Ty::U64),
            "bool" => Ok(Ty::I1),
            // `char` is a distinct IR type (#709): physically an `i32` codepoint,
            // but kept apart from `int32` so the value layer can render it
            // Python-style.
            "char" => Ok(Ty::Char),
            "str" => Ok(Ty::Ptr),
            "bytes" => Ok(Ty::Bytes),
            // A borrowed `strview` (#1082) lowers to a by-value tuple
            // `(backing: str, byte_off: int64, byte_len: int64)`. Reusing the
            // tuple/value-aggregate machinery means the `backing` pointer is
            // traced and rewritten on a moving-GC evacuation via the existing
            // aggregate-root `(base, desc)` pairs (#586) — the "2b" base+offset
            // representation — with no new GC-root instruction. Sema (`Ty::StrView`)
            // keeps it a distinct, affine type; only its physical layout coincides
            // with a `(str, int64, int64)` tuple, which has no methods, so the
            // method dispatch never confuses the two.
            "strview" => Ok(Ty::Struct(env.tuples.borrow_mut().strview_sid())),
            // Borrowed `bytesview` (#1082 follow-up): the `bytes` counterpart,
            // lowering to the reserved `(bytes, int64, int64)` aggregate.
            "bytesview" => Ok(Ty::Struct(env.tuples.borrow_mut().bytesview_sid())),
            "bigint" => Ok(Ty::Bigint),
            "complex" => Ok(Ty::Complex),
            "pyobject" => Ok(Ty::Py),
            "list" => {
                let e = match args.first() {
                    Some(a) => list_elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::List(e))
            }
            "array" => {
                let d = match args.first() {
                    Some(a) => dtype_of(a, env)?,
                    None => Dtype::F64,
                };
                // A bare `array[T]` annotation denotes a rank-1 array (#500);
                // N-D values arise through inference. Explicit N-D annotation
                // syntax is a documented follow-up.
                Ok(Ty::Array(d, 1))
            }
            "dict" => {
                let k = match args.first() {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                // The dict *value* is a value position, so a `float`/`f64` value
                // is admitted (#800); the *key* above keeps `elem_of`, which still
                // rejects a float key (Phase 2, pending the hash ruling).
                let v = match args.get(1) {
                    Some(a) => value_elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Dict(k, v))
            }
            "set" => {
                let e = match args.first() {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Set(e))
            }
            "heap" => {
                let e = match args.first() {
                    Some(a) => heap_elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Heap(e))
            }
            "channel" => {
                // A channel's element uses the same uniform-word representation
                // as a collection element (ticket #25, D-0031): the element kind
                // picks the typed send/recv runtime entry point. A channel element
                // is a value position (just a stored word), so a `float`/`f64`
                // element is admitted (#800).
                let e = match args.first() {
                    Some(a) => value_elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Chan(e))
            }
            // `deque[T]` (#983): a value position (any storable element, like
            // `list`), so `f64` is admitted.
            "deque" => {
                let e = match args.first() {
                    Some(a) => value_elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Deque(e))
            }
            // `oset[T]` (#983): a hashed position (element must be `Hashable`,
            // like `set`), so `elem_of` rejects a float element.
            "oset" => {
                let e = match args.first() {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::OSet(e))
            }
            "AtomicArray" => {
                // AtomicArray[T] (#844): a reference element lowers to a pointer
                // slot, `int64` to a word slot. The element kind picks the typed
                // runtime entry point (`zap_atomicarray_*_ptr` vs `_i64`), the same
                // split channels use.
                let e = match args.first() {
                    Some(a) => value_elem_of(a, env)?,
                    None => Elem::Ptr,
                };
                Ok(Ty::AtomicArray(e))
            }
            // Coroutine-local execution context (#1237). `ContextVar[T]` carries
            // its value element (reference -> pointer slot, `int64` -> word slot,
            // picking `zap_ctxvar_*_ptr` vs `_i64`); `ContextToken` is opaque.
            "ContextVar" => {
                let e = match args.first() {
                    Some(a) => value_elem_of(a, env)?,
                    None => Elem::Ptr,
                };
                Ok(Ty::ContextVar(e))
            }
            "ContextToken" => Ok(Ty::CtxToken),
            other => {
                // A tier-1 enum is its unboxed backing integer; a tier-3 enum is
                // its tagged struct; a struct is its layout (D-0018/D-0046).
                if let Some(e) = env.enums.get(other) {
                    if e.is_sum {
                        return Ok(Ty::Struct(e.sid.expect("tier-3 enum has a struct id")));
                    }
                    return Ok(Ty::Enum(e.enum_id));
                }
                // An interface value is its synthetic `{ptr data, ptr itable}`
                // fat-pointer layout (D-0016).
                if let Some(fat_sid) = env.ifaces.get(other) {
                    return Ok(Ty::Struct(*fat_sid));
                }
                match env.ids.get(other) {
                    Some(id) => Ok(Ty::Struct(*id)),
                    None => Err(err(format!(
                        "type '{other}' is not supported by codegen yet"
                    ))),
                }
            }
        },
        // A tuple type `(T, U, ...)` is an anonymous struct laid out positionally
        // (D-0049); it interns to a struct id reused by the struct machinery.
        ast::Type::Tuple { elems, .. } => {
            let tys = elems
                .iter()
                .map(|e| resolve_ty(e, env))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Ty::Struct(env.tuples.borrow_mut().intern(tys)))
        }
        // A first-class function value `fn(T, U) -> R` (I#163): a pointer to a
        // closure record, carrying its interned call signature so the indirect
        // call can be built. A missing `-> R` is a `Void`-returning function.
        ast::Type::Func { params, ret, .. } => {
            let ptys = params
                .iter()
                .map(|p| resolve_ty(p, env))
                .collect::<Result<Vec<_>, _>>()?;
            let rty = match ret {
                Some(r) => resolve_ty(r, env)?,
                None => Ty::Void,
            };
            Ok(Ty::Func(env.fn_sigs.borrow_mut().intern(ptys, rty)))
        }
    }
}

/// Convert a type written in *expression* position into the [`ast::Type`] the
/// resolver expects (#703). A collection **constructor** is parsed as a call of
/// a subscript expression, so its element/key type arrives as an [`Expr`], not
/// an [`ast::Type`]: `set[(int64, int64)]()` carries `(int64, int64)` as an
/// `Expr::Tuple`, `set[list[int64]]()` carries `list[int64]` as an
/// `Expr::Index`, and `set[int64]()` carries `int64` as an `Expr::Name`. The
/// collection *literal* and the `dict[K, V]` / `set[T]` type-*annotation* forms
/// already accept composite element types through [`resolve_ty`]; this bridges
/// the constructor forms to that same path so they are consistent (previously
/// only a bare name was accepted, rejecting a tuple with `ZAP6001 "expects an
/// element type name"`). Only shapes that denote a type are accepted:
///   * a bare name `int64`            → `Named { int64 }`
///   * a tuple `(int64, str)`         → `Tuple { [int64, str] }`
///   * a generic `list[int64]` / `dict[int64, str]` → `Named { list, [int64] }`
///
/// A monomorphized generic (e.g. `Optional[int64]` → `Optional$int64`) already
/// arrives as an `Expr::Name` and keeps flowing through the name branch.
fn expr_to_type(e: &Expr) -> Result<ast::Type, LowerError> {
    match e {
        Expr::Name(name, span) => Ok(ast::Type::Named {
            name: name.clone(),
            args: Vec::new(),
            span: *span,
        }),
        Expr::Tuple(elems, span) => {
            let elems = elems
                .iter()
                .map(expr_to_type)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ast::Type::Tuple { elems, span: *span })
        }
        Expr::Index { base, index, span } => {
            let Expr::Name(name, _) = base.as_ref() else {
                return Err(err(
                    "not a valid element type: a generic type's base must be a name, \
                     e.g. list[int64]",
                ));
            };
            // `list[int64]` has a single type argument; `dict[int64, str]`'s two
            // arguments arrive as a tuple in the subscript.
            let args = match index.as_ref() {
                Expr::Tuple(items, _) => items
                    .iter()
                    .map(expr_to_type)
                    .collect::<Result<Vec<_>, _>>()?,
                other => vec![expr_to_type(other)?],
            };
            Ok(ast::Type::Named {
                name: name.clone(),
                args,
                span: *span,
            })
        }
        _ => Err(err(
            "not a valid collection element type (expected a type such as int64, \
             (int64, str), or list[int64])",
        )),
    }
}

/// How one element/payload of a structural key is hashed and compared — the
/// shared classification behind a tuple's per-field key handling (#680, D-0049)
/// and an `Optional[T]`'s payload handling (#704, D-0122). See
/// [`FnLower::elem_key_plan`].
enum ElemKey {
    /// Integer-word element (int/bool/char/enum): hash via `zap_i64_hash`,
    /// compare with `ICmp Eq`.
    Int,
    /// Reference byte-buffer element (`str`/`bytes`): hash via `zap_str_hash`,
    /// compare with `zap_str_eq`.
    Str,
    /// A `float`/`float64` (or `float32`) element (D-0139/#808): hash via the
    /// #803 canonical key word (`zap_f64_value_key_word` — `-0.0` canonicalized,
    /// `NaN` rejected at runtime) then `zap_i64_hash`, compare with `FCmp Eq`
    /// (so `-0.0 == 0.0` and `NaN != NaN`). The carried `Ty` is the field width
    /// (`F32` widens to `F64` before hashing). This makes a float a hashable
    /// element of a value aggregate / tuple key, inheriting bare-float-key
    /// semantics (#800/#803).
    Float(Ty),
    /// A nested hashable aggregate (tuple, `Optional[T]`, or a `@derive(Hash,
    /// Eq)` struct): call its `(__hash__, __eq__)` by symbol name.
    Struct(String, String),
}

/// The runtime representation of a collection element/key/value given its
/// source type.
fn elem_of(t: &ast::Type, env: &TyEnv) -> Result<Elem, LowerError> {
    elem_kind(resolve_ty(t, env)?, env.nested)
}

/// The element kind of a `list[T]` element type. Identical to [`elem_of`] except
/// that an `f64` element is admitted (I#569): a list stores uniform 8-byte words
/// and a `double` fits one, so `list[float]` reuses the word storage through the
/// `_f64` runtime entry points. Only *lists* take this path — the hashed/ordered
/// element positions (dict/set keys, heaps) still reject floats via [`elem_kind`].
fn list_elem_of(t: &ast::Type, env: &TyEnv) -> Result<Elem, LowerError> {
    list_elem_kind(resolve_ty(t, env)?, env.nested)
}

/// The element kind of a *value*-position element type — a dict **value** or a
/// **channel** element — that admits an `f64` element (#800). Mirrors
/// [`list_elem_of`]; a float dict *key*, set member, or heap element is a
/// key/ordered position and still rejects floats.
fn value_elem_of(t: &ast::Type, env: &TyEnv) -> Result<Elem, LowerError> {
    value_elem_kind(resolve_ty(t, env)?, env.nested)
}

/// The element kind of a `heap[T]` element type (D-0057). A heap orders its
/// elements, so — unlike a general collection — only the *comparable* element
/// kinds are supported in this increment: integers/bools/chars (compared as
/// signed `i64`) and strings (compared by `ZapStr` bytes). User-struct elements
/// (ordered via a comparator / `__cmp__`) and floats are a clean follow-up.
fn heap_elem_of(t: &ast::Type, env: &TyEnv) -> Result<Elem, LowerError> {
    match resolve_ty(t, env)? {
        // `char` is ordered/compared as its `i64` codepoint word, exactly like an
        // integer heap element (#709); it was accepted here as `Ty::I32` before
        // the IR split, so keep it accepted.
        Ty::I1 | Ty::I8 | Ty::I16 | Ty::I32 | Ty::Char | Ty::I64 => Ok(Elem::I64),
        // A `heap[float64]` orders its elements by IEEE value through a dedicated
        // `f64` heap kind (#800); NaN sorts as greatest under a documented total
        // order, so the heap invariant holds. `f32` heaps stay deferred (#49).
        Ty::F64 => Ok(Elem::F64),
        Ty::Enum(id) => Ok(Elem::Enum(id)),
        Ty::Ptr => Ok(Elem::Ptr),
        other => Err(err(format!(
            "heap[{other}] is not supported yet (only integer, float, and str elements)"
        ))),
    }
}

/// The array dtype for a `array[T]` element type (tickets #24, #49). Supported
/// dtypes: `f64`, `i64`, `uint8` (image data), and `bool` (masks). Other
/// numeric dtypes (`f32`, `i32`) are deferred.
fn dtype_of(t: &ast::Type, env: &TyEnv) -> Result<Dtype, LowerError> {
    match resolve_ty(t, env)? {
        Ty::F64 => Ok(Dtype::F64),
        Ty::F32 => Ok(Dtype::F32),
        Ty::I64 => Ok(Dtype::I64),
        Ty::I32 => Ok(Dtype::I32),
        Ty::I16 => Ok(Dtype::I16),
        Ty::I8 => Ok(Dtype::I8),
        // The unsigned scalar `Ty::U8`..`U64` map to the unsigned array dtypes
        // (the full dtype matrix, #503/#890; `uint8` is the image-byte dtype).
        Ty::U8 => Ok(Dtype::Uint8),
        Ty::U16 => Ok(Dtype::U16),
        Ty::U32 => Ok(Dtype::U32),
        Ty::U64 => Ok(Dtype::U64),
        Ty::I1 => Ok(Dtype::Bool),
        // A complex array (#938): dense NumPy `complex128` (two f64 lanes).
        Ty::Complex => Ok(Dtype::Complex),
        other => Err(err(format!(
            "array[{other}] is not a numeric dtype (f32/f64, i8/i16/i32/i64, u8/u16/u32/u64, bool, complex)"
        ))),
    }
}

/// The array dtype named by a construction subscript like `array[f64]`, where
/// the dtype appears as a bare name in expression position.
/// Which shaped-fill constructor is being lowered (#504): `zeros`/`ones` fill a
/// constant 0/1, `full` fills a caller-supplied value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArrayFill {
    Zeros,
    Ones,
    Full,
}

fn dtype_from_name(name: &str) -> Result<Dtype, LowerError> {
    match name {
        "float" | "float64" | "f64" => Ok(Dtype::F64),
        "float32" | "f32" => Ok(Dtype::F32),
        "int" | "int64" | "i64" => Ok(Dtype::I64),
        "int32" | "i32" => Ok(Dtype::I32),
        "int16" | "i16" => Ok(Dtype::I16),
        "int8" | "i8" => Ok(Dtype::I8),
        "uint8" | "u8" => Ok(Dtype::Uint8),
        "uint16" | "u16" => Ok(Dtype::U16),
        "uint32" | "u32" => Ok(Dtype::U32),
        "uint64" | "u64" => Ok(Dtype::U64),
        "bool" => Ok(Dtype::Bool),
        "complex" => Ok(Dtype::Complex),
        other => Err(err(format!(
            "array[{other}] is not a numeric dtype (f32/f64, i8/i16/i32/i64, u8/u16/u32/u64, bool, complex)"
        ))),
    }
}

/// Validate one subtree of an N-D array literal against the remaining `shape`
/// (the dims below this level, row-major) and append its scalar leaves to `flat`
/// in row-major order (#500). An empty `shape` means this node must be a scalar
/// leaf; otherwise it must be a nested list of exactly `shape[0]` children, each
/// recursively matching `shape[1..]`. Any departure is a *ragged* (non-
/// rectangular / inconsistently nested) literal and is rejected at compile time,
/// since a dense `array` has one fixed shape.
fn flatten_array_literal<'e>(
    e: &'e ast::Expr,
    shape: &[i64],
    flat: &mut Vec<&'e ast::Expr>,
) -> Result<(), LowerError> {
    match shape.split_first() {
        None => {
            if matches!(e, ast::Expr::List(..)) {
                return Err(err(
                    "ragged array literal: a row is nested deeper than the array's shape \
                     (every row must nest to the same depth)",
                ));
            }
            flat.push(e);
            Ok(())
        }
        Some((&want, rest)) => {
            let ast::Expr::List(elems, _) = e else {
                return Err(err(format!(
                    "ragged array literal: expected a nested list of {want} elements, \
                     found a scalar (every row must nest to the same depth)"
                )));
            };
            if elems.len() as i64 != want {
                return Err(err(format!(
                    "ragged array literal: expected {want} elements in this row, found {}",
                    elems.len()
                )));
            }
            for c in elems {
                flatten_array_literal(c, rest, flat)?;
            }
            Ok(())
        }
    }
}

/// Classify a ZIR type into the element word kind collections store. Elements
/// are uniform 8-byte words (RUNTIME-ABI §"Collection layout"): integers/bools
/// go in as `i64`, reference types as pointers, and an `f64` as the reinterpreted
/// `double` word ([`Elem::F64`]). `f64` is admitted in *every* collection position
/// — value positions (list element I#569, dict VALUE / heap / channel #800
/// Phase 1) and the hashed/ordered positions (dict KEY, set MEMBER #800 Phase 2 /
/// #803): after `-0.0` canonicalisation and NaN rejection a float key hashes and
/// compares as a plain `i64` word (D-0138). Only an `f32` element (ticket #49) and
/// bigint/complex (`ZAP4024`) stay unsupported.
fn elem_kind(t: Ty, nested: &RefCell<NestedInterner>) -> Result<Elem, LowerError> {
    match t {
        // `bool` (`Ty::I1`) and `char` (`Ty::Char`) each keep their own element
        // kind so the Display protocol renders a `bool` element as `True`/`False`
        // and a `char` element Python-style (`'a'`) (#274/#709); both are
        // otherwise identical to an `i64` element (same word, suffix, and
        // read-back word). An `int32` element stays `Elem::I64` and renders as a
        // decimal — the `char`/`int32` IR types are now distinct, so this is
        // unambiguous.
        Ty::I1 => Ok(Elem::Bool),
        Ty::Char => Ok(Elem::Char),
        Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 => Ok(Elem::I64),
        Ty::Enum(id) => Ok(Elem::Enum(id)),
        // A struct element carries its id so a struct *key* hashes/compares via
        // its own `__hash__`/`__eq__` (operator overloading, D-0047).
        Ty::Struct(sid) => Ok(Elem::Struct(sid)),
        // A nested collection (#625) is physically a pointer, but its full type is
        // interned so read-back (`rows[i]`, iteration) recovers the precise
        // `list`/`dict`/`set`/... type instead of defaulting a bare pointer to
        // `str`. Dedup makes structurally-equal nestings compare equal.
        Ty::List(_)
        | Ty::Dict(..)
        | Ty::Set(_)
        | Ty::Heap(_)
        | Ty::Chan(_)
        | Ty::AtomicArray(_)
        | Ty::ContextVar(_)
        | Ty::Deque(_)
        | Ty::OSet(_) => Ok(Elem::Nested(nested.borrow_mut().intern(t))),
        // A `bytes` element is a `ZapStr` pointer word — the same storage, GC
        // tracing, and byte-wise key hashing as a `str` (`Elem::Ptr`) — but it is
        // a dedicated `Elem::Bytes` (#1074, D-0241) so read-back (`rows[i]`,
        // `for row in rows`) recovers `Ty::Bytes` instead of defaulting the bare
        // pointer to `str`; without it, `len(rows[i])` counted codepoints
        // (`zap_str_len_cp`) rather than bytes (`zap_str_len`).
        Ty::Bytes => Ok(Elem::Bytes),
        // A first-class function value (I#163) is physically a closure-record
        // pointer word, but its signature is retained in a dedicated `Elem::Func`
        // (#1348) so read-back (`callbacks[i]`, `for cb in callbacks`, a
        // list/dict/set/channel field) recovers `Ty::Func` and the call site
        // lowers to an indirect closure call instead of a failed top-level name
        // lookup (`ZAP6001`). The `u32` is the signature index, stable module-wide.
        Ty::Func(idx) => Ok(Elem::Func(idx)),
        Ty::Ptr | Ty::Array(..) | Ty::Match | Ty::Py | Ty::CtxToken => Ok(Elem::Ptr),
        Ty::Bigint | Ty::Complex => Err(err(
            "collections of bigint or complex values are not supported yet",
        )),
        // An `f64` element shares the uniform 8-byte word (a bit reinterpret, like
        // `list[float]`); in a hashed/ordered position (#803) the key is
        // canonicalised and NaN-rejected by the runtime. An `f32` element stays
        // deferred (ticket #49).
        Ty::F64 => Ok(Elem::F64),
        Ty::F32 => Err(err(
            "collections of 32-bit float (`f32`) elements are not supported yet (ticket #49)",
        )),
        // Scalar unsigned integers (#552) are supported as values, parameters,
        // returns, and C-style bitfields, but a collection/channel *element* of
        // an unsigned type is a clean follow-up: the uniform element word is
        // read back and rendered as a signed `i64`, so storing an unsigned there
        // would mis-render `uint64` values above `INT64_MAX`. Dense unsigned byte
        // data has `array[uint8]`.
        Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 => Err(err(
            "collections of unsigned-integer elements are not supported yet (use array[uint8] for dense byte data)",
        )),
        Ty::Void => Err(err("a collection element cannot be void")),
    }
}

/// The element kind of a *value*-position element type — a list element (I#569),
/// a dict **value**, or a **channel** element (#800). Now that [`elem_kind`]
/// itself admits an `f64` in every position (#803), this is an alias of
/// [`elem_kind`]; it is kept as a named value-position call site for the Phase 1
/// code paths, and so the intent at those sites stays legible.
fn value_elem_kind(t: Ty, nested: &RefCell<NestedInterner>) -> Result<Elem, LowerError> {
    // A `complex` value element (#928) is admitted in value positions (list element,
    // dict value, channel element) as a dedicated `Elem::Complex` — a `ZapComplex`
    // pointer word that retains complex identity for read-back / Display. It stays
    // rejected as a hashed key by [`elem_kind`] (complex-as-key is a follow-up), so
    // this split is the value-position counterpart of the `f64`-in-lists admission.
    if t == Ty::Complex {
        return Ok(Elem::Complex);
    }
    elem_kind(t, nested)
}

/// The element kind of a `list[T]` element type (I#569): a *value* position, so a
/// `float`/`f64` element is admitted. An alias of [`value_elem_kind`].
fn list_elem_kind(t: Ty, nested: &RefCell<NestedInterner>) -> Result<Elem, LowerError> {
    value_elem_kind(t, nested)
}

/// The `DEQUE_ELEM_*` runtime tag for a deque element kind (#983): 0 = i64 word
/// (integers/bool/char/enum), 1 = pointer (str), 2 = f64. Mirrors the `heap`
/// element-kind tag; it drives both the typed entry points and GC tracing.
fn deque_elem_tag(elem: Elem) -> i64 {
    match elem {
        Elem::I64 | Elem::Bool | Elem::Char | Elem::Enum(_) => 0,
        Elem::F64 => 2,
        _ => 1,
    }
}

/// Whether the heap can impose a total order on this element kind at runtime
/// (ticket #978): an `i64` word (integer/bool/char/enum), an `f64` (total-order
/// comparator), or a `str` (byte-wise). A struct, nested collection, or complex
/// element has no runtime ordering — `heap.nlargest`/`nsmallest` must be given a
/// `key=` that maps it to an orderable key, and `heap[T]` rejects it outright.
fn heap_orderable(elem: Elem) -> bool {
    // `str` and `bytes` are both byte-wise-orderable `ZapStr` words (#1074).
    matches!(
        elem,
        Elem::I64 | Elem::Bool | Elem::Char | Elem::Enum(_) | Elem::F64 | Elem::Ptr | Elem::Bytes
    )
}

/// The `OSET_ELEM_*` runtime tag for an oset element kind (#983): 0 = i64 word,
/// 1 = pointer (str). v1 admits only these (floats are not `Hashable`; structs
/// are a follow-up), enforced by [`FnLower::reject_nonprimitive_elem`].
fn oset_elem_tag(elem: Elem) -> i64 {
    match elem {
        Elem::I64 | Elem::Bool | Elem::Char | Elem::Enum(_) => 0,
        _ => 1,
    }
}

/// The integer-word element family: `i64`, `bool` (`Elem::Bool`), and `char`
/// (`Elem::Char`) elements all occupy one physical cell — an `i64` word, the
/// `i64` val/key suffix, and an `i64` read-back type (`Elem::ty()`). They are
/// distinct `Elem` kinds only so the Display protocol can render them
/// differently (a decimal, `True`/`False`, or `'a'`) (#274/#709); they are
/// interchangeable for storage.
///
/// An indexed store coerces its value word to the destination `elem.ty()` and
/// emits the `i64`-suffixed setter regardless of which of these the value and
/// destination are, so a value whose kind is any of these may be stored into a
/// destination whose kind is any of these — e.g. `want[i] = flags[j]`, where the
/// `list[bool]` element `flags[j]` reads back as a bare `i64` word yet the
/// destination `want` is also a `list[bool]`. Requiring exact `Elem` identity in
/// [`FnLower::check_index_value`] wrongly rejected that with the
/// self-contradictory "indexed assignment expects `i64`, found `i64`" (the #727
/// regression, introduced by #708's `Elem::Bool` tag and widened to `char` by
/// #712's `Elem::Char`). `Elem::Enum` shares the `i64` suffix but is NOT in this
/// family — an enum store keeps its own identity check.
fn is_int_word_elem(e: Elem) -> bool {
    matches!(e, Elem::I64 | Elem::Bool | Elem::Char)
}

fn param_ty(p: &ast::Param, self_sid: Option<u32>, ctx: &StructCtx) -> Ty {
    if p.name == "self" {
        if let Some(sid) = self_sid {
            return Ty::Struct(sid);
        }
    }
    let base =
        p.ty.as_ref()
            .map(|t| resolve_ty(t, &ctx.tyenv()).unwrap_or(Ty::Ptr))
            .unwrap_or(Ty::Ptr);
    // A `TYPE... name` variadic parameter is "received as a `list`" (SPEC §7,
    // D-0038): the callee iterates / indexes / `len`s it like any other list,
    // and the caller packs its trailing arguments into that list (see the
    // variadic branch of `call`). This matches how sema types the body binding.
    // A float/void element has no list representation yet, so such a variadic
    // degrades to its base type (already unsupported by the collection runtime).
    if p.variadic {
        match elem_kind(base, &ctx.nested) {
            Ok(elem) => Ty::List(elem),
            Err(_) => base,
        }
    } else if p.kwargs {
        // A `V **name` keyword-collector is received as `dict[str, V]` (#601):
        // str keys lower to pointer keys, the value is V's element kind. A value
        // kind the dict runtime cannot hold degrades to the base type (rejected
        // downstream, like a float variadic).
        match elem_kind(base, &ctx.nested) {
            Ok(vk) => Ty::Dict(Elem::Ptr, vk),
            Err(_) => base,
        }
    } else {
        base
    }
}

fn err(m: impl Into<String>) -> LowerError {
    LowerError {
        code: zapc_diag::ErrorCode::ZAP6001,
        message: m.into(),
        derive: None,
    }
}

/// A lowering error carrying a specific diagnostic code (most lowering errors
/// use the generic [`err`] / ZAP6001 umbrella; a few, like the bare-`raise`
/// re-raise-outside-handler case, ZAP6002, have a dedicated code).
fn err_code(code: zapc_diag::ErrorCode, m: impl Into<String>) -> LowerError {
    LowerError {
        code,
        message: m.into(),
        derive: None,
    }
}

/// The emitted clone symbol for a `@multiversion` tier: `foo` + `avx2` ->
/// `foo$mv$avx2` (#944, epic #139). Dots in a tier name (`sse4.2`) become `_`
/// so the symbol stays a valid identifier for every backend.
fn mv_clone_name(public: &str, tier: &str) -> String {
    format!("{public}$mv${}", tier.replace('.', "_"))
}

/// The feature name of a `@target_feature("feat")` decorator, if present (#963,
/// epic #139, D-0200). sema has already validated it (one string-literal feature
/// name from the taxonomy, not `"default"`).
fn target_feature_of(decorators: &[ast::Decorator]) -> Option<String> {
    let d = decorators.iter().find(|d| d.name == "target_feature")?;
    match d.args.first().map(|a| &a.value) {
        Some(ast::Expr::Str { value, .. }) => Some(value.clone()),
        _ => None,
    }
}

/// Expand a `@multiversion(...)` function into its clone set (#944, epic #139).
/// `base` is the already-lowered body under the public name; this returns one
/// clone [`Func`] per tier (a copy tagged with its `target_feature`) plus the
/// [`MvGroup`] codegen turns into a resolver + pointer-once-init dispatcher. The
/// base is NOT emitted under the public name — codegen emits the dispatcher
/// there, so every caller dispatches with no call-site change. Sema has already
/// validated the decorator (string tiers, a mandatory `"default"`); the checks
/// here are a defensive backstop that keeps a malformed decorator from reaching
/// codegen as a half-formed group.
fn expand_multiversion(
    base: Func,
    deco: &ast::Decorator,
) -> Result<(Vec<Func>, MvGroup), LowerError> {
    let public = base.name.clone();
    let mut tiers = Vec::new();
    let mut clones = Vec::new();
    for a in &deco.args {
        let ast::Expr::Str { value, .. } = &a.value else {
            return Err(err("@multiversion tiers must be string literals, e.g. \
                 @multiversion(\"avx2\", \"default\")"));
        };
        let tier = value.clone();
        let clone_name = mv_clone_name(&public, &tier);
        let mut clone = base.clone();
        clone.name = clone_name.clone();
        clone.target_feature = Some(tier.clone());
        clones.push(clone);
        tiers.push(MvTier {
            clone_name,
            is_default: tier == "default",
            tier,
        });
    }
    if !tiers.iter().any(|t| t.is_default) {
        return Err(err(
            "@multiversion requires a \"default\" tier (the mandatory portable fallback)",
        ));
    }
    Ok((
        clones,
        MvGroup {
            public_name: public,
            tiers,
        },
    ))
}

// ----------------------------------------------------------------------------
// Decorators & annotations (ticket #16, D-0036)
//
// This is the item-level compile-time expansion of the built-in decorator set.
// The v1 set is `@derive(Eq, Ord, Json, Repr, Hash)`, `@default`, `@validate`,
// `@range`, and `@deprecated`, plus runtime reflection over annotations. This
// pass implements:
//   * `@derive(Eq)` / `@derive(Repr)` — generate a structural-equality method
//     and a debug-string method directly as ZIR (zero runtime cost).
//   * runtime reflection — struct-level annotations/decorators are recorded in a
//     registry at startup and queried with `reflect_has` / `reflect_names`.
//   * `@deprecated` — a sema-only use-site warning (ZAP5124, D-0109); it emits
//     no ZIR, so it is a recognized no-op here (see `DECORATED_NO_CODE`).
// The field group `@default`/`@validate`/`@range` is realized at field targets
// (D-0101); a *non-field* application of those names stays a clean "not
// implemented yet" lowering error (`DEFERRED_BUILTINS`). `@cache` (I#666,
// D-0115) is expanded *before* sema into ordinary Zap over a module-level dict
// (crates/driver's behavioral-decorator pass), so it is consumed and never
// reaches this pass; `@inject` stays reclassified/declined and rejected at sema
// (ZAP5055, D-0109). The user-defined compile-time macro API stays deferred
// (OPEN-QUESTIONS.md).
// ----------------------------------------------------------------------------

/// The built-in field decorators whose non-field application is deferred. Sema
/// accepts the names (they are part of the locked set) and realizes them at
/// field targets (`@default`/`@validate`/`@range`, D-0101); any *other* target
/// is reported here as a clean "not implemented yet" lowering error.
///
/// `@cache`/`@inject` are not here: `@cache` (I#666/D-0115) is expanded to real
/// Zap before sema (so it is gone by lowering); `@inject` stays rejected at sema
/// (ZAP5055, I#472/D-0109). `@deprecated` is likewise gone: a sema-only use-site
/// warning (ZAP5124) and generates no code, so lowering treats it as a no-op
/// (see [`DECORATED_NO_CODE`]).
const DEFERRED_BUILTINS: &[&str] = &["default", "validate", "range"];

/// Built-in decorators that are fully handled before lowering and emit no ZIR,
/// so the lowering decorator scan must silently ignore them rather than fall
/// through to a "not implemented" error. `@deprecated` (I#472, D-0109) is a
/// sema-only use-site warning; the decorated function/method body lowers
/// unchanged. `@request_source` (#959, D-0198) is a pure marker consumed by the
/// pre-sema handler-adapter desugar; the resolver function it marks lowers
/// unchanged, so there is likewise nothing to generate here. `@derivable(Trait)`
/// (#1290, E3a) is a compile-time registration marker on a generic generator: it
/// is collected by sema and consumed by the `@derive` forwarding-stub synthesis
/// (E3b, #1291); the generator body lowers unchanged, so it too emits no ZIR.
const DECORATED_NO_CODE: &[&str] = &["deprecated", "request_source", "derivable"];

/// A trait requested via `@derive(...)`.
#[derive(Clone, Copy, PartialEq)]
enum DerivedTrait {
    Eq,
    Repr,
    Ord,
    Json,
    Csv,
    Yaml,
    Hash,
}

impl DerivedTrait {
    fn parse(name: &str) -> Option<DerivedTrait> {
        match name {
            "Eq" => Some(DerivedTrait::Eq),
            "Repr" => Some(DerivedTrait::Repr),
            "Ord" => Some(DerivedTrait::Ord),
            "Json" => Some(DerivedTrait::Json),
            "Csv" => Some(DerivedTrait::Csv),
            "Yaml" => Some(DerivedTrait::Yaml),
            "Hash" => Some(DerivedTrait::Hash),
            _ => None,
        }
    }

    /// The dunder method the generated code is reachable through and that the
    /// corresponding operator desugars to (D-0047): `Eq` → `__eq__` (`==`/`!=`),
    /// `Ord` → `__cmp__` (`< <= > >=`), `Hash` → `__hash__` (dict/set keys),
    /// `Repr` → `__repr__` (`print` / `str`).
    fn method_name(self) -> &'static str {
        match self {
            DerivedTrait::Eq => "__eq__",
            DerivedTrait::Repr => "__repr__",
            DerivedTrait::Ord => "__cmp__",
            DerivedTrait::Json => "to_json",
            DerivedTrait::Csv => "to_csv",
            DerivedTrait::Yaml => "to_yaml",
            DerivedTrait::Hash => "__hash__",
        }
    }

    /// The ZIR signature of the generated method, if this trait is implemented.
    /// `__cmp__` returns an `i64` three-way result (−1 / 0 / +1); `__hash__`
    /// returns the combined `i64` hash.
    fn signatures(self, sid: u32) -> Vec<(&'static str, Vec<Ty>, Ty)> {
        match self {
            DerivedTrait::Eq => vec![("__eq__", vec![Ty::Struct(sid), Ty::Struct(sid)], Ty::I1)],
            DerivedTrait::Repr => vec![("__repr__", vec![Ty::Struct(sid)], Ty::Ptr)],
            DerivedTrait::Ord => vec![("__cmp__", vec![Ty::Struct(sid), Ty::Struct(sid)], Ty::I64)],
            DerivedTrait::Hash => vec![("__hash__", vec![Ty::Struct(sid)], Ty::I64)],
            // Compile-time JSON codec (D-0059, I#100): the encode method
            // `to_json(self) -> str` and the decode associated function
            // `from_json(str) -> Self`.
            DerivedTrait::Json => vec![
                ("to_json", vec![Ty::Struct(sid)], Ty::Ptr),
                ("from_json", vec![Ty::Ptr], Ty::Struct(sid)),
            ],
            // Compile-time CSV record codec (D-0059 §4b, I#132): the encode
            // method `to_csv(self) -> str` and the decode associated function
            // `from_csv(str) -> Self`.
            DerivedTrait::Csv => vec![
                ("to_csv", vec![Ty::Struct(sid)], Ty::Ptr),
                ("from_csv", vec![Ty::Ptr], Ty::Struct(sid)),
            ],
            // Compile-time YAML codec (D-0063, I#149): the encode method
            // `to_yaml(self) -> str` and the decode associated function
            // `from_yaml(str) -> Self`. Reuses the shared DOM decode core.
            DerivedTrait::Yaml => vec![
                ("to_yaml", vec![Ty::Struct(sid)], Ty::Ptr),
                ("from_yaml", vec![Ty::Ptr], Ty::Struct(sid)),
            ],
        }
    }

    /// Generate the method body as a ZIR function, or `Ok(None)` if the trait
    /// produces no standalone function. Deferred traits return an error.
    fn generate<'a>(
        self,
        sid: u32,
        sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
        ctx: &'a StructCtx,
    ) -> Result<Vec<Func>, LowerError> {
        Ok(match self {
            DerivedTrait::Eq => vec![gen_eq_method(sid, sigs, ctx)?],
            DerivedTrait::Repr => vec![gen_repr_method(sid, sigs, ctx)?],
            DerivedTrait::Ord => vec![gen_cmp_method(sid, sigs, ctx)?],
            DerivedTrait::Hash => vec![gen_hash_method(sid, sigs, ctx)?],
            DerivedTrait::Json => vec![
                gen_json_to_method(sid, sigs, ctx)?,
                gen_json_from_method(sid, sigs, ctx)?,
                gen_json_from_value_method(sid, sigs, ctx)?,
            ],
            DerivedTrait::Csv => vec![
                gen_csv_to_method(sid, sigs, ctx)?,
                gen_csv_from_method(sid, sigs, ctx)?,
            ],
            // `from_yaml` parses to the shared `JsonValue` DOM and reuses the
            // same `from_json_value` decode core (generated here too, deduped
            // against `@derive(Json)` by name when a struct derives both).
            DerivedTrait::Yaml => vec![
                gen_yaml_to_method(sid, sigs, ctx)?,
                gen_yaml_from_method(sid, sigs, ctx)?,
                gen_json_from_value_method(sid, sigs, ctx)?,
            ],
        })
    }

    fn name(self) -> &'static str {
        match self {
            DerivedTrait::Eq => "Eq",
            DerivedTrait::Repr => "Repr",
            DerivedTrait::Ord => "Ord",
            DerivedTrait::Json => "Json",
            DerivedTrait::Csv => "Csv",
            DerivedTrait::Yaml => "Yaml",
            DerivedTrait::Hash => "Hash",
        }
    }
}

/// A reflectable parameter of a function/method (I#458, D-0089). `self` is
/// excluded by the collector.
struct ParamReflect {
    name: String,
    /// Rendered declared type (`int`, `list[str]`, `fn(Request) -> Response`),
    /// or `""` when the parameter is untyped.
    type_disp: String,
    has_default: bool,
    /// Declared annotation/decorator names on the parameter, in source order.
    annos: Vec<String>,
}

/// Reflectable signature metadata for one function/method (I#458, D-0089). The
/// `owner` is the key callers pass to `reflect_params`/`reflect_sig`: a function
/// name for a top-level `def`, or `"Type.method"` for a method.
struct FnReflect {
    owner: String,
    params: Vec<ParamReflect>,
    /// Readable one-line signature, built at compile time (e.g.
    /// `(id: int, q: str = ...) -> int64`).
    sig: String,
}

/// Render an `ast::Type` to its source-like display (mirrors `analysis::render_type`,
/// duplicated here to avoid an `ir -> analysis` dependency).
fn render_ast_type(t: &ast::Type) -> String {
    match t {
        ast::Type::Named { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = args.iter().map(render_ast_type).collect();
                format!("{}[{}]", name, inner.join(", "))
            }
        }
        ast::Type::Tuple { elems, .. } => {
            let inner: Vec<String> = elems.iter().map(render_ast_type).collect();
            format!("({})", inner.join(", "))
        }
        ast::Type::Func { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(render_ast_type).collect();
            match ret {
                Some(r) => format!("fn({}) -> {}", ps.join(", "), render_ast_type(r)),
                None => format!("fn({})", ps.join(", ")),
            }
        }
    }
}

/// Collect the reflectable signature metadata for one function/method body
/// (I#458, D-0089). `owner` is the reflection key; `meta_names` is the set of
/// declared annotation/decorator names (the only param decorators recorded —
/// everything else stays deferred/rejected, mirroring the fn-level model).
/// Returns `None` when the function has no reflectable parameters AND no return
/// type (nothing worth a registry entry — a bare `def f(): ...`).
fn collect_fn_reflect(f: &ast::Function, owner: String, meta_names: &HashSet<String>) -> FnReflect {
    let mut params = Vec::new();
    let mut parts = Vec::new();
    for p in &f.params {
        // The implicit receiver is not part of the reflectable signature.
        if p.name == "self" {
            continue;
        }
        let type_disp = p.ty.as_ref().map(render_ast_type).unwrap_or_default();
        let annos: Vec<String> = p
            .decorators
            .iter()
            .filter(|d| meta_names.contains(&d.name))
            .map(|d| d.name.clone())
            .collect();
        // Readable rendering for `reflect_sig`: `@anno name: type = ...`.
        let mut part = String::new();
        for a in &annos {
            part.push('@');
            part.push_str(a);
            part.push(' ');
        }
        part.push_str(&p.name);
        if !type_disp.is_empty() {
            part.push_str(": ");
            part.push_str(&type_disp);
        }
        if p.default.is_some() {
            part.push_str(" = ...");
        }
        parts.push(part);
        params.push(ParamReflect {
            name: p.name.clone(),
            type_disp,
            has_default: p.default.is_some(),
            annos,
        });
    }
    let mut sig = format!("({})", parts.join(", "));
    if let Some(r) = &f.ret {
        sig.push_str(" -> ");
        sig.push_str(&render_ast_type(r));
    }
    FnReflect { owner, params, sig }
}

/// The result of scanning a module's decorators/annotations.
struct DecoratorPlan {
    /// Per struct (by id), the traits to derive.
    derived: Vec<(u32, Vec<DerivedTrait>)>,
    /// `(type_name, decorator_or_annotation_name)` pairs for runtime reflection.
    reflect_meta: Vec<(String, String)>,
    /// Per function/method, the reflectable signature metadata (I#458, D-0089).
    param_meta: Vec<FnReflect>,
    /// `@on_signal(Signal.X)` handlers (ticket #77): `(signal_number, fn_name)`
    /// pairs, registered at the top of `main` via `zap_signal_on`.
    on_signal_meta: Vec<(i64, String)>,
    /// Open string enums carrying `@derive(Json)` + `@json(open)` (ticket #346,
    /// D-0086): the enum's tagged-struct id. Each gets `to_json` / `from_json` /
    /// `from_json_value` generated by the enum JSON path.
    json_enums: Vec<u32>,
}

/// Whether struct `sid` *declares* (hand-writes or `@derive`s) `__eq__` and
/// `__hash__` — returned as `(has_eq, has_hash)`. Used to decide hashability of a
/// struct-typed field (D-0139/#808): a struct that declares both is hashable
/// outright; a `value struct` that declares NEITHER may still auto-derive the
/// pair (if all its fields are hashable); one that declares exactly one took over
/// the pair all-or-nothing and gets no synthesized counterpart, so it is not
/// hashable. A tuple interner id has no AST entry → `(false, false)`.
fn struct_decls_eq_hash(ctx: &StructCtx, module: &ast::Module, sid: u32) -> (bool, bool) {
    let Some(name) = ctx.names.get(sid as usize) else {
        return (false, false);
    };
    let Some(s) = module.items.iter().find_map(|it| match it {
        ast::Item::Struct(s) if &s.name == name => Some(s),
        _ => None,
    }) else {
        return (false, false);
    };
    let has_method = |m: &str| s.methods.iter().any(|f| f.name == m);
    let derives = |t: &str| {
        s.decorators.iter().any(|d| {
            d.name == "derive"
                && d.args
                    .iter()
                    .any(|a| matches!(&a.value, ast::Expr::Name(n, _) if n == t))
        })
    };
    (
        has_method("__eq__") || derives("Eq"),
        has_method("__hash__") || derives("Hash"),
    )
}

/// Is `fty` hashable as a field / element of an auto-derived value aggregate
/// (D-0139/#808)? Mirrors [`FnLower::elem_key_plan`]'s acceptance set without
/// synthesizing anything: integers / bool / char / enum, `str` / `bytes`, and —
/// new in #808 — `float`/`float64`/`float32` (inheriting #803 key semantics); a
/// tuple or an `Optional[T]` is hashable iff all its elements/payload are. A named
/// struct is hashable iff it declares both `__eq__` and `__hash__`, or — for a
/// `value struct` that declares neither — all its fields are hashable (so it will
/// itself auto-derive); a partial override (exactly one of the pair) is NOT
/// hashable, matching the all-or-nothing rule. Mutable collections
/// (`list`/`dict`/`set`/`heap`/`channel`) are never hashable. Cycle-safe via
/// `visited` (value structs cannot actually cycle — a separate hard error — so
/// breaking a cycle optimistically is sound).
fn field_type_hashable(
    ctx: &StructCtx,
    module: &ast::Module,
    fty: Ty,
    visited: &mut HashSet<u32>,
) -> bool {
    match fty {
        Ty::Ptr | Ty::Bytes => true,
        Ty::F32 | Ty::F64 => true,
        t if t.int_width().is_some() => true,
        Ty::Struct(sid) => {
            if !visited.insert(sid) {
                return true; // cycle (already in-progress) — broken optimistically
            }
            let is_tuple = ctx.tuples.borrow().is_tuple(sid);
            let tuple_fields = ctx.tuples.borrow().fields(sid);
            let ok = if is_tuple {
                tuple_fields
                    .map(|fs| {
                        fs.iter()
                            .all(|f| field_type_hashable(ctx, module, *f, visited))
                    })
                    .unwrap_or(false)
            } else if let Some(opt) = optional_layout(ctx, Ty::Struct(sid)) {
                field_type_hashable(ctx, module, opt.payload_ty, visited)
            } else {
                let (has_eq, has_hash) = struct_decls_eq_hash(ctx, module, sid);
                if has_eq && has_hash {
                    true
                } else if ctx.is_value_struct(sid) && !has_eq && !has_hash {
                    ctx.fields[sid as usize]
                        .clone()
                        .iter()
                        .all(|(_, f)| field_type_hashable(ctx, module, *f, visited))
                } else {
                    false
                }
            };
            visited.remove(&sid);
            ok
        }
        _ => false,
    }
}

/// The library-registered derivable trait names in a module (#1290, E3a): the
/// `Trait` of each `@derivable(Trait)` on a free function. Collected from the
/// **pre-monomorphization** AST, because mono drops an uninstantiated generic
/// generator before the decorator pass runs.
fn library_derivable_traits(module: &ast::Module) -> HashSet<String> {
    let mut set = HashSet::new();
    for item in &module.items {
        if let ast::Item::Function(f) = item {
            for d in &f.decorators {
                if d.name == "derivable" {
                    if let Some(ast::Expr::Name(t, _)) = d.args.first().map(|a| &a.value) {
                        set.insert(t.clone());
                    }
                }
            }
        }
    }
    set
}

impl DecoratorPlan {
    fn collect(
        module: &ast::Module,
        ctx: &StructCtx,
        library_derivables: &HashSet<String>,
        errors: &mut Vec<LowerError>,
    ) -> DecoratorPlan {
        // Names declared as metadata (annotations) or behavioral decorators.
        // These are what the reflection registry records; the built-in
        // compile-time decorators (`@derive` etc.) leave no runtime trace.
        //
        // `library_derivables` (the names a `@derivable(Trait)` generator
        // registered, #1290, E3a) is collected by the caller from the PRE-mono
        // module, because mono drops an uninstantiated generic generator; a
        // `@derive` of one of these is reported as deferred-to-E3b (#1291), not
        // "unknown derivable trait" (which would contradict sema).
        let mut meta_names: HashSet<String> = HashSet::new();
        for item in &module.items {
            match item {
                ast::Item::Annotation(a) => {
                    meta_names.insert(a.name.clone());
                }
                ast::Item::Decorator(d) => {
                    meta_names.insert(d.name.clone());
                }
                _ => {}
            }
        }

        let mut derived: Vec<(u32, Vec<DerivedTrait>)> = Vec::new();
        let mut reflect_meta: Vec<(String, String)> = Vec::new();
        let mut on_signal_meta: Vec<(i64, String)> = Vec::new();
        let mut json_enums: Vec<u32> = Vec::new();
        // Signature reflection (I#458, D-0089): every top-level function and every
        // struct/enum method in the user module is reflectable by name.
        let mut param_meta: Vec<FnReflect> = Vec::new();

        for item in &module.items {
            match item {
                ast::Item::Struct(s) => {
                    let sid = ctx.ids[&s.name];
                    let mut traits: Vec<DerivedTrait> = Vec::new();
                    for d in &s.decorators {
                        if d.name == "derive" {
                            for a in &d.args {
                                if let ast::Expr::Name(t, _) = &a.value {
                                    match DerivedTrait::parse(t) {
                                        Some(tr) => {
                                            // A user method of the same name wins;
                                            // deriving it too would duplicate it.
                                            if s.methods.iter().any(|m| m.name == tr.method_name())
                                            {
                                                errors.push(err(format!(
                                                    "struct '{}' both defines method '{}' and \
                                                     derives {} (remove one)",
                                                    s.name,
                                                    tr.method_name(),
                                                    tr.name()
                                                )));
                                            } else if !traits.contains(&tr) {
                                                traits.push(tr);
                                            }
                                        }
                                        None if library_derivables.contains(t) => {
                                            // A library `@derivable(T)` trait
                                            // (#1290): resolved by sema, but its
                                            // forwarding-impl synthesis is E3b
                                            // (#1291). Report it as deferred, not
                                            // "unknown", so nothing half-generates.
                                            errors.push(err(format!(
                                                "@derive({t}) on struct '{}' uses a \
                                                 library-registered derivable trait, \
                                                 whose code generation is not \
                                                 implemented yet (deferred to E3b, \
                                                 #1291)",
                                                s.name
                                            )));
                                        }
                                        None => errors.push(err(format!(
                                            "unknown derivable trait '{t}' on struct '{}'",
                                            s.name
                                        ))),
                                    }
                                }
                            }
                        } else if DEFERRED_BUILTINS.contains(&d.name.as_str()) {
                            errors.push(err(format!(
                                "built-in decorator '@{}' is not implemented yet (deferred)",
                                d.name
                            )));
                        } else if meta_names.contains(&d.name)
                            || d.name == zapc_syntax::SHAREABLE_MARKER
                        {
                            // A user `annotation`/`decorator`, or the built-in
                            // `@shareable` concurrency marker (#664, D-0113):
                            // record it so `reflect_has(T, "<name>")` sees it.
                            reflect_meta.push((s.name.clone(), d.name.clone()));
                        }
                        // Unknown names are already rejected by sema.
                    }
                    // Auto-derive Equatable + Hashable for a `value struct`
                    // (D-0139/#808): a value struct is copied-on-insert and has no
                    // identity (D-0023/D-0051), so by-value equality is its only
                    // coherent meaning. The pair is synthesized iff EVERY field is
                    // hashable (conditional synthesis, Swift-style) and the user
                    // has not taken over either half — a hand-written or explicitly
                    // derived `__eq__`/`__hash__` (`Eq`/`Hash`) suppresses BOTH, so
                    // the override stays all-or-nothing and the Eq-without-Hash
                    // guard (sema ZAP5120) still applies. A non-hashable field
                    // (e.g. a `list`) simply leaves the aggregate non-Hashable —
                    // surfaced at key use (never here), matching Python's
                    // unhashable-tuple-with-a-list. `Comparable`/`Ord` stays
                    // explicit (ordering is a choice), as do reference structs.
                    if s.is_value {
                        let user_eq = s.methods.iter().any(|m| m.name == "__eq__")
                            || traits.contains(&DerivedTrait::Eq);
                        let user_hash = s.methods.iter().any(|m| m.name == "__hash__")
                            || traits.contains(&DerivedTrait::Hash);
                        if !user_eq && !user_hash {
                            let mut visited = HashSet::new();
                            let all_hashable = ctx.fields[sid as usize]
                                .clone()
                                .iter()
                                .all(|(_, f)| field_type_hashable(ctx, module, *f, &mut visited));
                            if all_hashable {
                                traits.push(DerivedTrait::Eq);
                                traits.push(DerivedTrait::Hash);
                            }
                        }
                    }
                    if !traits.is_empty() {
                        derived.push((sid, traits));
                    }
                    // Field decorators realized elsewhere: `@json(...)` by the
                    // JSON codec (ticket #346, `StructCtx.json_field_opts`);
                    // `@default`/`@validate`/`@range` at construction time
                    // (D-0101, `StructCtx.field_defaults`/`field_checks`). Any
                    // other field decorator stays deferred.
                    for fld in &s.fields {
                        for d in &fld.decorators {
                            if !matches!(d.name.as_str(), "json" | "default" | "validate" | "range")
                            {
                                errors.push(err(format!(
                                    "field decorators are not implemented yet (deferred): \
                                     '@{}' on field '{}'",
                                    d.name, fld.name
                                )));
                            }
                        }
                    }
                    for m in &s.methods {
                        defer_fn_decorators(m, &meta_names, errors);
                        param_meta.push(collect_fn_reflect(
                            m,
                            format!("{}.{}", s.name, m.name),
                            &meta_names,
                        ));
                    }
                }
                ast::Item::Function(f) => {
                    // `@on_signal(Signal.X)` (ticket #77): register `f` as a
                    // handler for signal X; a detached receiver coroutine runs it
                    // on each delivery.
                    for d in &f.decorators {
                        if d.name == "on_signal" {
                            if !f.params.is_empty() {
                                errors.push(err(format!(
                                    "@on_signal handler '{}' must take no parameters",
                                    f.name
                                )));
                            }
                            match on_signal_number(d, ctx) {
                                Ok(signo) => on_signal_meta.push((signo, f.name.clone())),
                                Err(e) => errors.push(e),
                            }
                        } else if meta_names.contains(&d.name) {
                            // A declared `annotation`/`decorator` on a function
                            // (I#457, D-0087): attach reflection metadata keyed by
                            // the function name, mirroring the struct-level model
                            // (D-0036). The function body lowers unchanged; its
                            // decorator is queryable via reflect_has/reflect_names.
                            // Behavioral wrap/before/after codegen stays deferred.
                            reflect_meta.push((f.name.clone(), d.name.clone()));
                        } else if DECORATED_NO_CODE.contains(&d.name.as_str()) {
                            // `@deprecated` (I#472, D-0109): a sema-only use-site
                            // warning; the function body lowers unchanged, so there
                            // is nothing to generate here.
                        } else if d.name == "multiversion" {
                            // `@multiversion(...)` (#944, epic #139): the function
                            // body lowers unchanged here; the post-lowering pass
                            // (`expand_multiversion`) clones it per CPU-feature tier
                            // and records the dispatch group. Nothing to collect in
                            // this reflection-metadata scan.
                        } else if d.name == "target_feature" {
                            // `@target_feature(...)` (#963, epic #139, D-0200): the
                            // body lowers unchanged; the single feature is stamped
                            // onto the `Func` after lowering (`target_feature_of`),
                            // so codegen compiles it with that feature. Nothing to
                            // collect here.
                        } else if DEFERRED_BUILTINS.contains(&d.name.as_str()) {
                            errors.push(err(format!(
                                "built-in decorator '@{}' is not implemented yet (deferred)",
                                d.name
                            )));
                        } else {
                            // Dotted registering targets (`@app.get(...)`, D-0087)
                            // are desugared to a registration call before sema, so
                            // they never reach here. A bare name that is neither a
                            // declared decorator/annotation nor a known built-in was
                            // already rejected by sema (ZAP5032); this is defensive.
                            errors.push(err(format!(
                                "function decorators are not implemented yet (deferred): \
                                 '@{}' on '{}'",
                                d.name, f.name
                            )));
                        }
                    }
                    for p in &f.params {
                        for d in &p.decorators {
                            // A *declared* annotation/decorator on a parameter is
                            // recorded as reflectable metadata (I#458, D-0089),
                            // mirroring the fn-level model (D-0087): the parameter
                            // binds unchanged and the annotation is queryable via
                            // `reflect_param_annotations`. Behavioral wrap and any
                            // unknown name stay deferred (the latter already caught
                            // by sema's ZAP5032; this is defensive).
                            if !meta_names.contains(&d.name) {
                                errors.push(err(format!(
                                    "parameter decorators are not implemented yet (deferred): \
                                     '@{}' on '{}'",
                                    d.name, p.name
                                )));
                            }
                        }
                    }
                    param_meta.push(collect_fn_reflect(f, f.name.clone(), &meta_names));
                }
                ast::Item::Enum(e) => {
                    if !e.decorators.is_empty() {
                        // `@derive(Json)` + `@json(open)` on an enum (ticket #346,
                        // D-0086): an open string-enum codec. Any other enum
                        // decorator stays deferred.
                        let derives_json = e.decorators.iter().any(|d| {
                            d.name == "derive"
                                && d.args.iter().any(
                                    |a| matches!(&a.value, ast::Expr::Name(t, _) if t == "Json"),
                                )
                        });
                        let is_open = e.decorators.iter().any(|d| {
                            d.name == "json"
                                && d.args.iter().any(
                                    |a| matches!(&a.value, ast::Expr::Name(n, _) if n == "open"),
                                )
                        });
                        for d in &e.decorators {
                            if d.name != "derive" && d.name != "json" {
                                errors.push(err(format!(
                                    "enum decorators are not implemented yet (deferred): '@{}'",
                                    d.name
                                )));
                            }
                        }
                        if derives_json {
                            match validate_open_enum(e, ctx, is_open) {
                                Ok(sid) => json_enums.push(sid),
                                Err(e) => errors.push(e),
                            }
                        } else if is_open {
                            errors.push(err(format!(
                                "enum '{}' has @json(open) but no @derive(Json) (ticket #346)",
                                e.name
                            )));
                        }
                    }
                    // Tier-3 enum methods (D-0046) are reflectable like struct
                    // methods (I#458, D-0089).
                    for m in &e.methods {
                        param_meta.push(collect_fn_reflect(
                            m,
                            format!("{}.{}", e.name, m.name),
                            &meta_names,
                        ));
                    }
                }
                _ => {}
            }
        }

        DecoratorPlan {
            derived,
            reflect_meta,
            param_meta,
            json_enums,
            on_signal_meta,
        }
    }
}

/// Resolve the single `Signal.X` argument of an `@on_signal(...)` decorator to
/// its OS signal number (ticket #77). The argument must be a `Signal` enum
/// member (a tier-1 discriminant that IS the signal number). Sema has already
/// validated the shape; this mirrors it for lowering and errors defensively.
fn on_signal_number(d: &ast::Decorator, ctx: &StructCtx) -> Result<i64, LowerError> {
    if d.args.len() != 1 {
        return Err(err(
            "@on_signal(Signal.X) takes exactly one Signal argument".to_string(),
        ));
    }
    if let Expr::Attr { base, name, .. } = &d.args[0].value {
        if let Expr::Name(tyname, _) = base.as_ref() {
            if tyname == "Signal" {
                if let Some(v) = ctx.enums.get("Signal").and_then(|e| e.member_value(name)) {
                    return Ok(v);
                }
            }
        }
    }
    Err(err(
        "@on_signal expects a Signal member, e.g. @on_signal(Signal.SIGINT)".to_string(),
    ))
}

/// The expression of a field's `@default(expr)` decorator (D-0101), if present.
/// Sema has checked the single-positional shape and that the field carries no
/// literal default too.
fn field_default_decorator(fld: &ast::Field) -> Option<Expr> {
    fld.decorators
        .iter()
        .find(|d| d.name == "default")
        .and_then(|d| d.args.first())
        .map(|a| a.value.clone())
}

/// Parse a numeric literal expression (int/float, optional unary `+`/`-`) into a
/// [`NumLit`] carrying both an integer and a float form. Mirrors sema's
/// `numeric_literal`; used for `@range` bounds (D-0101). Returns `None` for a
/// non-literal (sema has already rejected those).
fn num_lit(e: &Expr) -> Option<NumLit> {
    match e {
        Expr::Int(s, _) => {
            let int = parse_int_literal(s)?;
            Some(NumLit {
                int,
                float: int as f64,
            })
        }
        Expr::Float(s, _) => {
            let float = s.replace('_', "").parse::<f64>().ok()?;
            Some(NumLit {
                int: float as i64,
                float,
            })
        }
        Expr::Unary {
            op: ast::UnOp::Neg,
            expr,
            ..
        } => num_lit(expr).map(|v| NumLit {
            int: -v.int,
            float: -v.float,
        }),
        Expr::Unary {
            op: ast::UnOp::Pos,
            expr,
            ..
        } => num_lit(expr),
        _ => None,
    }
}

/// Parse an integer literal string (decimal/hex/octal/binary, `_` separators).
fn parse_int_literal(s: &str) -> Option<i64> {
    let t = s.replace('_', "");
    let (radix, digits) = if let Some(r) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, r)
    } else if let Some(r) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        (8, r)
    } else if let Some(r) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        (2, r)
    } else {
        (10, t.as_str())
    };
    i64::from_str_radix(digits, radix).ok()
}

/// Collect the `@validate(...)` / `@range(...)` construction-time checks on one
/// field (D-0101). Sema has validated the argument shapes and the field type, so
/// a literal that fails to parse here is defensive (reported as a lowering
/// error). `@default`/`@json` field decorators are handled elsewhere.
fn collect_field_checks(fld: &ast::Field, errors: &mut Vec<LowerError>) -> Vec<FieldCheck> {
    let mut checks = Vec::new();
    for d in &fld.decorators {
        match d.name.as_str() {
            "validate" => {
                for a in &d.args {
                    let Some(name) = a.name.as_deref() else {
                        continue;
                    };
                    match name {
                        "max_len" | "min_len" => match num_lit(&a.value) {
                            Some(v) if v.int >= 0 => {
                                if name == "max_len" {
                                    checks.push(FieldCheck::MaxLen(v.int));
                                } else {
                                    checks.push(FieldCheck::MinLen(v.int));
                                }
                            }
                            _ => errors.push(err(format!(
                                "@validate({name}=...) needs a non-negative integer literal \
                                 on field '{}'",
                                fld.name
                            ))),
                        },
                        _ => {}
                    }
                }
            }
            "range" => {
                for a in &d.args {
                    let Some(name) = a.name.as_deref() else {
                        continue;
                    };
                    let bound = match num_lit(&a.value) {
                        Some(v) => v,
                        None => {
                            errors.push(err(format!(
                                "@range {name} must be a numeric literal on field '{}'",
                                fld.name
                            )));
                            continue;
                        }
                    };
                    match name {
                        "min" => checks.push(FieldCheck::RangeMin(bound)),
                        "max" => checks.push(FieldCheck::RangeMax(bound)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    checks
}

/// Validate an `@derive(Json)` enum (ticket #346, D-0086) and return its
/// tagged-struct id. The enum must be an **open** string enum: it carries
/// `@json(open)`, is a tier-3 tagged enum, every known variant is payload-free,
/// and exactly one fallback variant `Unknown(str)` carries the raw wire string.
/// At least two known variants are required so the value keeps its tag (a lone
/// known variant beside `Unknown(str)` would collapse into the `Optional`
/// niche, which has no tag to switch on).
fn validate_open_enum(e: &ast::EnumDef, ctx: &StructCtx, is_open: bool) -> Result<u32, LowerError> {
    if !is_open {
        return Err(err(format!(
            "@derive(Json) on enum '{}' requires @json(open) with an Unknown(str) \
             fallback variant (closed string enums are a follow-up)",
            e.name
        )));
    }
    let info = ctx
        .enums
        .get(&e.name)
        .ok_or_else(|| err(format!("enum '{}' not registered", e.name)))?;
    let sid = match (info.is_sum, info.sid) {
        (true, Some(sid)) => sid,
        _ => {
            return Err(err(format!(
                "@json(open) enum '{}' must have an Unknown(str) payload variant \
                 (ticket #346)",
                e.name
            )))
        }
    };
    if info.niche.is_some() {
        return Err(err(format!(
            "@json(open) enum '{}' needs at least two known variants besides \
             Unknown(str) (ticket #346)",
            e.name
        )));
    }
    let unknown = info.variant("Unknown").ok_or_else(|| {
        err(format!(
            "@json(open) enum '{}' must declare a fallback variant 'Unknown(str value)' \
             (ticket #346)",
            e.name
        ))
    })?;
    if unknown.fields.len() != 1 || unknown.fields[0].2 != Ty::Ptr {
        return Err(err(format!(
            "@json(open) enum '{}': the Unknown variant must carry exactly one str \
             field, e.g. Unknown(str value) (ticket #346)",
            e.name
        )));
    }
    for v in &info.variants {
        if v.name != "Unknown" && !v.fields.is_empty() {
            return Err(err(format!(
                "@json(open) enum '{}': known variant '{}' must be payload-free \
                 (open string enums map each variant to its name; ticket #346)",
                e.name, v.name
            )));
        }
    }
    Ok(sid)
}

/// Report function/parameter decorators as deferred (not implemented yet).
fn defer_fn_decorators(
    f: &ast::Function,
    meta_names: &HashSet<String>,
    errors: &mut Vec<LowerError>,
) {
    // `@deprecated` on a method (I#472, D-0109) is a sema-only use-site warning
    // and generates no code, so it lowers as a no-op; any other method decorator
    // stays deferred.
    if let Some(d) = f
        .decorators
        .iter()
        .find(|d| !DECORATED_NO_CODE.contains(&d.name.as_str()))
    {
        errors.push(err(format!(
            "function decorators are not implemented yet (deferred): '@{}' on '{}'",
            d.name, f.name
        )));
    }
    for p in &f.params {
        for d in &p.decorators {
            // A declared annotation/decorator on a method parameter is reflectable
            // metadata (I#458, D-0089), like top-level fn params; anything else
            // stays deferred.
            if !meta_names.contains(&d.name) {
                errors.push(err(format!(
                    "parameter decorators are not implemented yet (deferred): '@{}' on '{}'",
                    d.name, p.name
                )));
            }
        }
    }
}

/// `@validate(...)` / `@range(...)` — a construction-time validator method
/// `Struct____validate__(self)` (D-0101). `construct` invokes it after filling
/// every field; each failing constraint raises a catchable `ValueError`
/// (D-0007), matching the index/divide faults (#383). Generated as ordinary ZIR,
/// so `zap expand` shows it and it is type-checked like any method.
fn gen_validate_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = format!("{}____validate__", ctx.names[sid as usize]);
    let mut b = FnLower::new(
        name,
        vec![Ty::Struct(sid)],
        Ty::Void,
        false,
        true, // self_param: the receiver is by-reference, like a derived method
        sigs,
        ctx,
    );
    let fields = ctx.fields[sid as usize].clone();
    let per_field = ctx
        .field_checks
        .get(&sid)
        .cloned()
        .unwrap_or_else(|| vec![Vec::new(); fields.len()]);
    for (idx, checks) in per_field.iter().enumerate() {
        if checks.is_empty() {
            continue;
        }
        let (fname, fty) = fields[idx].clone();
        for chk in checks {
            gen_field_check(&mut b, sid, idx as u32, &fname, fty, *chk)?;
        }
    }
    b.set_term(Term::Ret(None));
    Ok(b.func)
}

/// Emit one field-validation guard (D-0101): load the field, compare against the
/// bound, and on violation raise a catchable `ValueError` (tag 3), continuing on
/// the success path.
fn gen_field_check(
    b: &mut FnLower,
    sid: u32,
    idx: u32,
    fname: &str,
    fty: Ty,
    chk: FieldCheck,
) -> Result<(), LowerError> {
    // The predicate `fail` is true when the constraint is violated.
    let (fail, msg) = match chk {
        FieldCheck::MaxLen(n) | FieldCheck::MinLen(n) => {
            if !matches!(fty, Ty::Ptr | Ty::Bytes) {
                return Err(err(format!(
                    "@validate(max_len/min_len) on field '{fname}' requires a str or bytes field"
                )));
            }
            let fv = b.new_value(fty);
            b.emit(Inst::FieldGet {
                dest: fv,
                base: 0,
                sid,
                idx,
            });
            let len = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(len),
                func: "zap_str_len".to_string(),
                args: vec![fv],
            });
            let bound = b.iconst(Ty::I64, n);
            let fail = b.new_value(Ty::I1);
            let (pred, msg) = match chk {
                FieldCheck::MaxLen(n) => (
                    CmpPred::Gt,
                    format!("field '{fname}' exceeds @validate(max_len={n})"),
                ),
                _ => (
                    CmpPred::Lt,
                    format!("field '{fname}' is shorter than @validate(min_len={n})"),
                ),
            };
            b.emit(Inst::ICmp {
                dest: fail,
                pred,
                lhs: len,
                rhs: bound,
            });
            (fail, msg)
        }
        FieldCheck::RangeMin(v) | FieldCheck::RangeMax(v) => {
            let fv = b.new_value(fty);
            b.emit(Inst::FieldGet {
                dest: fv,
                base: 0,
                sid,
                idx,
            });
            let fail = b.new_value(Ty::I1);
            let is_max = matches!(chk, FieldCheck::RangeMax(_));
            // Render the bound as written: a float field shows the float form,
            // an integer field the integer form.
            let bound_str = if matches!(fty, Ty::F32 | Ty::F64) {
                format!("{}", v.float)
            } else {
                format!("{}", v.int)
            };
            // Violation: value < min, or value > max.
            let msg = if is_max {
                format!("field '{fname}' is above @range(max={bound_str})")
            } else {
                format!("field '{fname}' is below @range(min={bound_str})")
            };
            match fty {
                Ty::F32 | Ty::F64 => {
                    let bound = b.fconst(fty, v.float);
                    b.emit(Inst::FCmp {
                        dest: fail,
                        pred: if is_max { CmpPred::Gt } else { CmpPred::Lt },
                        lhs: fv,
                        rhs: bound,
                    });
                }
                t if t.int_width().is_some() => {
                    let bound = b.iconst(fty, v.int);
                    b.emit(Inst::ICmp {
                        dest: fail,
                        pred: if is_max { CmpPred::Gt } else { CmpPred::Lt },
                        lhs: fv,
                        rhs: bound,
                    });
                }
                other => {
                    return Err(err(format!(
                        "@range on field '{fname}' requires a numeric field, found {other}"
                    )));
                }
            }
            (fail, msg)
        }
    };
    // `if fail: raise ValueError(msg)` — otherwise fall through to the next check.
    let throw_bb = b.new_block();
    let cont_bb = b.new_block();
    b.set_term(Term::BrIf {
        cond: fail,
        then_bb: throw_bb,
        else_bb: cont_bb,
    });
    b.position(throw_bb);
    let m = b.new_value(Ty::Ptr);
    b.emit(Inst::StrConst {
        dest: m,
        bytes: msg.into_bytes(),
    });
    let value_error = b.iconst(Ty::I64, 3); // ZAP_EXC_VALUE_ERROR (see zaprt.c)
    b.throwing_call(None, "zap_panic_typed", vec![value_error, m]);
    if !b.terminated() {
        b.set_term(Term::Unreachable);
    }
    b.position(cont_bb);
    Ok(())
}

/// `@derive(Eq)` — a structural-equality method `Struct____eq__(self, other) ->
/// bool` (the `__eq__` dunder, D-0047) that compares every field (ints/bools by
/// value, floats by ordered compare, strings by content) and ANDs the results.
fn gen_eq_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = format!("{}__{}", ctx.names[sid as usize], "__eq__");
    let mut b = FnLower::new(
        name,
        vec![Ty::Struct(sid), Ty::Struct(sid)],
        Ty::I1,
        false,
        true, // self_param: a derived method's receiver is by-reference
        sigs,
        ctx,
    );
    // Params: value 0 = self, value 1 = other (used directly as struct bases).
    let mut acc = b.iconst(Ty::I1, 1);
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let idx = idx as u32;
        let a = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: a,
            base: 0,
            sid,
            idx,
        });
        let c = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: c,
            base: 1,
            sid,
            idx,
        });
        // Classify + compare the field through the shared structural-key plan
        // (D-0139/#808), so a derived `__eq__` supports float and nested
        // aggregate fields (and rejects an unhashable field with the same clean
        // message), matching the tuple/Optional key paths.
        let key = b.elem_key_plan(*fty)?;
        let bit = b.emit_elem_eq(a, c, &key);
        let next = b.new_value(Ty::I1);
        b.emit(Inst::Bin {
            dest: next,
            op: BinOp::And,
            lhs: acc,
            rhs: bit,
        });
        acc = next;
    }
    b.set_term(Term::Ret(Some(acc)));
    Ok(b.func)
}

/// The Python `str`/`repr` distinction for the Display protocol (#274, D-0124).
/// It affects ONLY leaf string/char/bytes values: a bare string at the top level
/// of `print`/`str` renders unquoted (`Str`), while the same string nested inside
/// a struct field, container element, or tuple element renders Python-quoted
/// (`Repr`, `'a'`). Every other value type renders identically in both modes.
#[derive(Clone, Copy, PartialEq)]
enum RenderMode {
    /// Top-level `str(x)` / `print(x)` / f-string field: bare strings unquoted.
    Str,
    /// Nested (field / element): Python `repr` — strings quoted, `bytes` `b'…'`.
    Repr,
}

/// `@derive(Repr)` — a debug-string method `Struct____repr__(self) -> str` (the
/// `__repr__` dunder, D-0047) producing the default field-wise form
/// `Name(f0=v0, f1=v1, ...)`.
///
/// Since #274 (D-0124) this shares one body generator with the *default* value
/// renderer synthesized for a type that defines no `__str__`/`__repr__`: every
/// field renders through [`FnLower::render_str`], the single value-rendering
/// protocol, so a field of ANY type works — integers, `bool` (`True`/`False`,
/// not the old `-1` bitpattern), `float`, `str`, `char`, `bytes`, `Optional`,
/// **nested user structs** (rendered by *calling* that type's own renderer, so a
/// recursive type graph terminates at compile time), tuples, tier-1 enums,
/// `list`/`dict`/`set`, and — since #706 — tier-3 data-carrying enums (rendered
/// `Enum.Variant(field=value, …)`). The only field type still rejected is the one
/// the shared renderer defers (an interface value, the #707 follow-up) — a clean
/// coded error, never an ICE.
fn gen_repr_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__{}", "__repr__");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::Ptr, false, true, sigs, ctx);
    gen_struct_repr_body(&mut b, sid)?;
    Ok(b.func)
}

/// Emit, into the already-constructed renderer function `b` (whose first value is
/// the `self` struct), the default field-wise rendering of a *named* struct
/// `sid`: `Name(field0=<v0>, field1=<v1>, …)` (#274, D-0124). Each field value is
/// rendered through [`FnLower::render_str`] — the one value-rendering protocol —
/// so the output is identical whether a type opts in via `@derive(Repr)` or gets
/// the compiler-synthesized default. Shared by [`gen_repr_method`] and
/// [`FnLower::struct_repr_fn`]'s on-demand default synthesis.
fn gen_struct_repr_body(b: &mut FnLower, sid: u32) -> Result<(), LowerError> {
    let sname = b.ctx.names[sid as usize].clone();
    let mut acc = b.strconst(&format!("{sname}("));
    let fields = b.ctx.fields[sid as usize].clone();
    for (idx, (fname, fty)) in fields.iter().enumerate() {
        if idx > 0 {
            let sep = b.strconst(", ");
            acc = gen_concat(b, acc, sep);
        }
        let label = b.strconst(&format!("{fname}="));
        acc = gen_concat(b, acc, label);
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        // A field is a NESTED position: render in `repr` mode, so a `str` field
        // shows quoted (`name='a'`), matching Python (#274, D-0124).
        let vstr = b.render_repr(fv, *fty)?;
        acc = gen_concat(b, acc, vstr);
    }
    let close = b.strconst(")");
    acc = gen_concat(b, acc, close);
    b.set_term(Term::Ret(Some(acc)));
    Ok(())
}

/// Emit, into the already-constructed renderer function `b` (whose first value is
/// the tier-3 enum `self`), the structural rendering of a data-carrying enum
/// (#706, the Display protocol #274/D-0124 extended to tier-3 enums, D-0046):
/// `EnumName.Variant(field=<v>, …)` for a data-carrying variant and
/// `EnumName.Variant` for a payload-free one. Each payload field renders through
/// [`FnLower::render_repr`] — the one value-rendering protocol, in `repr` mode so
/// a nested `str`/`char`/`bytes` payload quotes — so a recursive enum
/// (`enum Expr: Add(Expr l, Expr r)`) renders recursively by *calling* this same
/// memoized renderer through the pointer child, terminating at compile time.
///
/// Two layouts (D-0064 #4):
/// - the general tagged struct `{ $tag:i32, payload… }` — switch on the tag,
///   mirroring the tier-1 `.name` discriminant→name chain but building the
///   structural form per variant;
/// - a niche `Optional`-shaped enum — no physical tag, the value IS the single
///   payload pointer (null == the payload-free variant). Rendered by the null
///   test, reusing the tested niche reinterpret ([`gen_opt_is_none`] /
///   [`gen_opt_payload`]). The `Optional`-named ones are intercepted earlier by
///   [`optional_layout`] in [`FnLower::render_value`]; this keeps a *user* niche
///   enum (not named `Optional`) correct too, rather than reading a bogus tag out
///   of a pointer.
fn gen_enum_repr_body(b: &mut FnLower, sid: u32) -> Result<(), LowerError> {
    let info = b
        .ctx
        .enums
        .values()
        .find(|e| e.is_sum && e.sid == Some(sid))
        .cloned()
        .ok_or_else(|| err("tier-3 enum identity is not registered for rendering"))?;
    let ename = info.enum_name.clone();

    // Niche `Optional`-shaped layout: no tag slot; the `self` value (value 0) IS
    // the payload pointer, null for the payload-free variant.
    if let Some(n) = &info.niche {
        let opt = OptionalLayout {
            sid,
            payload_ty: n.payload,
            niche: true,
            some_tag: 0,
            none_tag: 0,
            payload_idx: 0,
        };
        // The single `Some`-like variant's payload field name (for `field=value`).
        let some_field = info
            .variant(&n.some_variant)
            .and_then(|v| v.fields.first())
            .map(|(fname, _, _)| fname.clone())
            .unwrap_or_default();
        let slot = b.alloca(Ty::Ptr);
        let is_none = gen_opt_is_none(b, &opt, 0);
        let none_bb = b.new_block();
        let some_bb = b.new_block();
        let join_bb = b.new_block();
        b.set_term(Term::BrIf {
            cond: is_none,
            then_bb: none_bb,
            else_bb: some_bb,
        });
        b.position(none_bb);
        let none_str = b.strconst(&format!("{ename}.{}", n.none_variant));
        b.store(slot, none_str);
        b.set_term(Term::Br(join_bb));
        b.position(some_bb);
        let payload = gen_opt_payload(b, &opt, 0);
        let pstr = b.render_repr(payload, n.payload)?;
        let head = b.strconst(&format!("{ename}.{}({some_field}=", n.some_variant));
        let acc = gen_concat(b, head, pstr);
        let close = b.strconst(")");
        let acc = gen_concat(b, acc, close);
        b.store(slot, acc);
        b.set_term(Term::Br(join_bb));
        b.position(join_bb);
        let r = b.load(slot, Ty::Ptr);
        b.set_term(Term::Ret(Some(r)));
        return Ok(());
    }

    // General tagged layout: a discriminant→structural-render chain. Field 0 of
    // the tagged struct is the `i32` tag.
    let result_slot = b.alloca(Ty::Ptr);
    let merge = b.new_block();
    let mut check = b.cur;
    for v in &info.variants {
        b.position(check);
        let tag = b.field_get(0, sid, 0, Ty::I32);
        let expected = b.iconst(Ty::I32, v.tag);
        let matches = b.new_value(Ty::I1);
        b.emit(Inst::ICmp {
            dest: matches,
            pred: CmpPred::Eq,
            lhs: tag,
            rhs: expected,
        });
        let found = b.new_block();
        let next = b.new_block();
        b.set_term(Term::BrIf {
            cond: matches,
            then_bb: found,
            else_bb: next,
        });
        b.position(found);
        let mut acc = b.strconst(&format!("{ename}.{}", v.name));
        if !v.fields.is_empty() {
            let open = b.strconst("(");
            acc = gen_concat(b, acc, open);
            for (i, (fname, idx, fty)) in v.fields.iter().enumerate() {
                if i > 0 {
                    let sep = b.strconst(", ");
                    acc = gen_concat(b, acc, sep);
                }
                let label = b.strconst(&format!("{fname}="));
                acc = gen_concat(b, acc, label);
                let fv = b.field_get(0, sid, *idx, *fty);
                // A payload field is a NESTED position: render in `repr` mode so a
                // `str`/`char`/`bytes` payload quotes, matching Python (#274).
                let vstr = b.render_repr(fv, *fty)?;
                acc = gen_concat(b, acc, vstr);
            }
            let close = b.strconst(")");
            acc = gen_concat(b, acc, close);
        }
        b.store(result_slot, acc);
        b.ensure_br(merge);
        check = next;
    }
    // Fall-through: an out-of-range tag is unreachable for a well-formed value,
    // but panic defensively (mirroring the tier-1 `.name` chain's fall-through)
    // rather than returning an uninitialized slot.
    b.position(check);
    let message = b.strconst(&format!("invalid tag for enum {ename}"));
    b.throwing_call(None, "zap_panic", vec![message]);
    if !b.terminated() {
        b.set_term(Term::Unreachable);
    }
    b.position(merge);
    let r = b.load(result_slot, Ty::Ptr);
    b.set_term(Term::Ret(Some(r)));
    Ok(())
}

/// Emit, into the renderer function `b`, the default rendering of a tuple
/// (`(tuple)` anonymous layout, #274/D-0124) in the Python form `(v0, v1, …)` —
/// NOT the `TypeName(field=value)` struct form, since a tuple has positional
/// elements and no field names. A one-element tuple keeps the trailing comma
/// (`(v0,)`), matching CPython and Zap's tuple literal syntax (D-0049). Each
/// element renders through [`FnLower::render_str`], so tuples nest and carry
/// structs/enums/scalars uniformly.
fn gen_tuple_repr_body(b: &mut FnLower, sid: u32) -> Result<(), LowerError> {
    let fields = b
        .ctx
        .tuples
        .borrow()
        .fields(sid)
        .expect("gen_tuple_repr_body called on a non-tuple struct id");
    let mut acc = b.strconst("(");
    let arity = fields.len();
    for (idx, fty) in fields.iter().enumerate() {
        if idx > 0 {
            let sep = b.strconst(", ");
            acc = gen_concat(b, acc, sep);
        }
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        // Tuple elements are nested positions: render in `repr` mode (quoted
        // strings), matching Python's `(1, 'a')` (#274, D-0124).
        let vstr = b.render_repr(fv, *fty)?;
        acc = gen_concat(b, acc, vstr);
    }
    // A single-element tuple renders `(v0,)` (D-0049 / CPython), disambiguating it
    // from a parenthesized expression.
    let close = if arity == 1 {
        b.strconst(",)")
    } else {
        b.strconst(")")
    };
    acc = gen_concat(b, acc, close);
    b.set_term(Term::Ret(Some(acc)));
    Ok(())
}

/// Emit the comma-separator step of a container renderer (#274, D-0124): on the
/// first element (the `first` i64 flag is non-zero) clear the flag and emit
/// nothing; on every later element append `", "` to the accumulator at `acc`.
/// Leaves `b` positioned at the join block so the caller appends the element
/// next.
/// Render one collection element/key/value `e` (already fetched) of element kind
/// `elem` in `repr` mode for a container renderer (#274, D-0124). A `bool`
/// element (`Elem::Bool`) is rendered `True`/`False` directly from its `i64`
/// word via `zap_bool_to_str` — the `Elem::Bool` tag is the only thing that
/// distinguishes it from an integer element, which `render_repr` (keyed on the
/// read-back `Ty`, `i64` for a bool) could not. A `char` element (`Elem::Char`)
/// is likewise distinguished only by its tag: its `i64` word is truncated back
/// to the `i32` codepoint (`Ty::Char`) and rendered Python-style (`'a'`) through
/// the shared `render_repr`, rather than as its decimal codepoint (#709). Every
/// other element kind renders through the shared `render_repr`.
fn render_elem_repr(b: &mut FnLower, e: Value, elem: Elem) -> Result<Value, LowerError> {
    if elem == Elem::Bool {
        let d = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(d),
            func: "zap_bool_to_str".to_string(),
            args: vec![e],
        });
        return Ok(d);
    }
    if elem == Elem::Char {
        // The element word is an `i64`; narrow it to the `i32` codepoint so the
        // `Ty::Char` renderer emits the Python-quoted one-char string.
        let c = b.coerce(e, Elem::Char.ty(), Ty::Char);
        return b.render_repr(c, Ty::Char);
    }
    let ty = b.elem_ty(elem);
    b.render_repr(e, ty)
}

fn gen_seq_sep(b: &mut FnLower, acc: Value, first: Value) {
    let fv = b.load(first, Ty::I64);
    let zero = b.iconst(Ty::I64, 0);
    let is_first = b.new_value(Ty::I1);
    b.emit(Inst::ICmp {
        dest: is_first,
        pred: CmpPred::Ne,
        lhs: fv,
        rhs: zero,
    });
    let first_bb = b.new_block();
    let sep_bb = b.new_block();
    let after_bb = b.new_block();
    b.set_term(Term::BrIf {
        cond: is_first,
        then_bb: first_bb,
        else_bb: sep_bb,
    });
    // First element: just lower the flag, no separator.
    b.position(first_bb);
    let z = b.iconst(Ty::I64, 0);
    b.store(first, z);
    b.set_term(Term::Br(after_bb));
    // Later element: append ", ".
    b.position(sep_bb);
    let a = b.load(acc, Ty::Ptr);
    let sep = b.strconst(", ");
    let a2 = gen_concat(b, a, sep);
    b.store(acc, a2);
    b.set_term(Term::Br(after_bb));
    b.position(after_bb);
}

/// `zap_str_concat(a, b)` as a ZIR call returning the new string.
fn gen_concat(b: &mut FnLower, a: Value, c: Value) -> Value {
    let d = b.new_value(Ty::Ptr);
    b.emit(Inst::Call {
        dest: Some(d),
        func: "zap_str_concat".to_string(),
        args: vec![a, c],
    });
    d
}

/// Format-neutral classification of a derived struct field's type — the shared
/// "derive-walk core" (D-0059 §4b). The `@derive(Json)` encode/decode codegen
/// dispatches on these kinds, and `@derive(Csv)` (PR3) reuses the same
/// classifier, each mapping a kind to its own wire form. A field type with no
/// kind is not yet supported by a derived codec.
enum DeriveFieldKind {
    /// A boolean (`i1`). Classified before `Int` (an `i1` has an integer width).
    Bool,
    /// A float field; carries the exact width (`F32`/`F64`).
    Float(Ty),
    /// An integer field; carries the exact width.
    Int(Ty),
    /// A `str` (reference string, lowered to `Ptr`).
    Str,
    /// A nested struct field; carries its struct id (must itself derive the
    /// codec).
    Nested(u32),
}

/// Classify a derived field's type for the shared codec walk, or `None` if no
/// derived codec supports it yet (collections, enums, tuples, …).
fn derive_field_kind(fty: Ty) -> Option<DeriveFieldKind> {
    match fty {
        Ty::I1 => Some(DeriveFieldKind::Bool),
        Ty::F32 | Ty::F64 => Some(DeriveFieldKind::Float(fty)),
        t if t.int_width().is_some() => Some(DeriveFieldKind::Int(t)),
        Ty::Ptr => Some(DeriveFieldKind::Str),
        Ty::Struct(s) => Some(DeriveFieldKind::Nested(s)),
        _ => None,
    }
}

/// Layout facts for a monomorphized `Optional[T]` instance used as a
/// `@derive(Json)` field (I#165, D-0065). Monomorphization has already
/// specialized `Optional[T]` to a concrete enum (`Optional$int64`, `Optional$str`,
/// …) before lowering, so the derive sees a plain `Ty::Struct(sid)`; this records
/// how to test/construct/extract its `Some`/`None` for the two possible layouts
/// (niche for a reference payload, tagged otherwise — D-0064 #4).
struct OptionalLayout {
    /// The concrete `Optional` instance's struct id.
    sid: u32,
    /// The payload (`T` lowered) — what the inner codec encodes/decodes.
    payload_ty: Ty,
    /// Niche layout: `Some` IS the payload pointer, `None` the null pointer, one
    /// word, no tag. `false` = the tagged `{tag, payload}` layout.
    niche: bool,
    /// Tagged layout only: the `Some` discriminant.
    some_tag: i64,
    /// Tagged layout only: the `None` discriminant.
    none_tag: i64,
    /// Tagged layout only: the payload field's index in the tagged struct.
    payload_idx: u32,
}

/// Recognize a monomorphized `Atomic[T]` box by its mangled struct name
/// (`Atomic` — never instantiated bare — or `Atomic$int64`, `Atomic$str`, …),
/// the same name-prefix test the niche `Optional` layout uses (I#176). The box is
/// the prelude `struct Atomic[T]: T value`; its sole cell field is accessed with
/// inline LLVM atomics instead of ordinary struct-method dispatch.
fn is_atomic_struct(ctx: &StructCtx, sid: u32) -> bool {
    match ctx.names.get(sid as usize) {
        Some(n) => n == "Atomic" || n.starts_with("Atomic$"),
        None => false,
    }
}

/// Is `sid` the built-in scope-bound `arena` struct (#553, D-0158)? Its handle
/// value is an opaque `ZapArena*`; `a.alloc`/`a.new` on it lower to runtime region
/// calls rather than ordinary struct methods.
fn is_arena_struct(ctx: &StructCtx, sid: u32) -> bool {
    ctx.names
        .get(sid as usize)
        .map(|n| n == "arena")
        .unwrap_or(false)
}

/// Is `e` a fresh `arena(...)` construction — the head of a `with arena(...) as a:`
/// (#553)? Recognized by the callee name, mirroring the `expect_raises` special
/// case; sema's `@scoped` rule guarantees it appears only as a `with` head.
fn is_arena_call(e: &Expr) -> bool {
    matches!(e, Expr::Call { callee, .. } if is_name(callee, "arena"))
}

/// Recognize a field type that is a monomorphized `Optional[T]` instance (by its
/// mangled name `Optional` / `Optional$…`) and return its layout, else `None`.
fn optional_layout(ctx: &StructCtx, fty: Ty) -> Option<OptionalLayout> {
    let Ty::Struct(sid) = fty else {
        return None;
    };
    let name = ctx.names.get(sid as usize)?;
    if name != "Optional" && !name.starts_with("Optional$") {
        return None;
    }
    let e = ctx.enums.get(name)?;
    if !e.is_sum {
        return None;
    }
    if let Some(n) = &e.niche {
        return Some(OptionalLayout {
            sid,
            payload_ty: n.payload,
            niche: true,
            some_tag: 0,
            none_tag: 0,
            payload_idx: 0,
        });
    }
    let some = e.variant("Some")?;
    let none = e.variant("None")?;
    let (_, pidx, pty) = some.fields.first()?;
    Some(OptionalLayout {
        sid,
        payload_ty: *pty,
        niche: false,
        some_tag: some.tag,
        none_tag: none.tag,
        payload_idx: *pidx,
    })
}

/// Emit IR constructing `Optional.None` for `opt`.
fn gen_opt_none(b: &mut FnLower, opt: &OptionalLayout) -> Value {
    let dest = b.new_value(Ty::Struct(opt.sid));
    if opt.niche {
        b.emit(Inst::NullConst { dest });
    } else {
        b.emit(Inst::StructNew { dest, sid: opt.sid });
        let tag = b.iconst(Ty::I32, opt.none_tag);
        b.emit(Inst::FieldSet {
            base: dest,
            sid: opt.sid,
            idx: 0,
            val: tag,
        });
    }
    dest
}

/// Emit IR constructing `Optional.Some(inner)` (inner already typed
/// `opt.payload_ty`).
fn gen_opt_some(b: &mut FnLower, opt: &OptionalLayout, inner: Value) -> Value {
    if opt.niche {
        // The enum value IS the payload pointer; reinterpret via a slot so the
        // result carries the enum's ZIR type.
        let slot = b.alloca(Ty::Struct(opt.sid));
        b.store(slot, inner);
        return b.load(slot, Ty::Struct(opt.sid));
    }
    let dest = b.new_value(Ty::Struct(opt.sid));
    b.emit(Inst::StructNew { dest, sid: opt.sid });
    let tag = b.iconst(Ty::I32, opt.some_tag);
    b.emit(Inst::FieldSet {
        base: dest,
        sid: opt.sid,
        idx: 0,
        val: tag,
    });
    b.emit(Inst::FieldSet {
        base: dest,
        sid: opt.sid,
        idx: opt.payload_idx,
        val: inner,
    });
    dest
}

/// Emit IR extracting the `Some` payload (typed `opt.payload_ty`) from a
/// known-`Some` Optional value `fv`.
fn gen_opt_payload(b: &mut FnLower, opt: &OptionalLayout, fv: Value) -> Value {
    if opt.niche {
        // `fv` physically IS the payload pointer; reinterpret via a slot.
        let slot = b.alloca(opt.payload_ty);
        b.store(slot, fv);
        return b.load(slot, opt.payload_ty);
    }
    let d = b.new_value(opt.payload_ty);
    b.emit(Inst::FieldGet {
        dest: d,
        base: fv,
        sid: opt.sid,
        idx: opt.payload_idx,
    });
    d
}

/// Emit IR computing `i1` = whether Optional value `fv` is `None`.
fn gen_opt_is_none(b: &mut FnLower, opt: &OptionalLayout, fv: Value) -> Value {
    if opt.niche {
        let d = b.new_value(Ty::I1);
        b.emit(Inst::IsNull { dest: d, val: fv });
        return d;
    }
    let tag = b.new_value(Ty::I32);
    b.emit(Inst::FieldGet {
        dest: tag,
        base: fv,
        sid: opt.sid,
        idx: 0,
    });
    let k = b.iconst(Ty::I32, opt.none_tag);
    b.binop(AstBin::Eq, tag, Ty::I32, k, Ty::I32)
        .expect("i32 equality is always valid")
        .0
}

/// `@derive(Json)` encode direction (D-0059, I#100): a `Struct__to_json(self)
/// -> str` method producing the struct's compact JSON object,
/// `{"f0":v0,"f1":v1,...}`. Field names are identifiers (no escaping needed);
/// values are encoded per field type. Reuses the DOM runtime helpers
/// (`zap_json_escape` for strings, `zap_i64_to_str`/`zap_f64_to_str` for
/// numbers, `zap_json_bool_str` for booleans); a nested struct field recurses
/// through its own derived `to_json` (so nested records must also
/// `@derive(Json)`). Collections / enum fields are a follow-up slice. This is
/// the string-building encoder; the `__to_json_value__` DOM bridge and the
/// `from_json` decode direction land with the shared derive-walk core.
fn gen_json_to_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__to_json");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::Ptr, false, true, sigs, ctx);
    // Collect each *present* field's `"name":value` fragment into a list, then
    // comma-join and brace-wrap. A list (rather than a static `idx>0` comma rule)
    // lets an `Optional` field omit itself when `None` with no dangling comma
    // (I#165, D-0065). `zap_csv_join` is a plain comma-join over `list[str]`.
    let parts = b.new_value(Ty::List(Elem::Ptr));
    b.emit(Inst::Call {
        dest: Some(parts),
        func: "zap_list_new".to_string(),
        args: vec![],
    });
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (fname, fty)) in fields.iter().enumerate() {
        // A field name is a Zap identifier, so it needs no JSON escaping.
        let label = format!("\"{fname}\":");
        let fopts = field_json_opts(ctx, sid, idx);
        if let Some(opt) = optional_layout(ctx, *fty) {
            // `Optional[T]` field: emit `"name":<inner>` only when `Some`; a
            // `None` omits the key entirely (D-0065 dumps rule).
            let fv = b.new_value(*fty);
            b.emit(Inst::FieldGet {
                dest: fv,
                base: 0,
                sid,
                idx: idx as u32,
            });
            let is_none = gen_opt_is_none(&mut b, &opt, fv);
            let write_bb = b.new_block();
            let cont_bb = b.new_block();
            b.set_term(Term::BrIf {
                cond: is_none,
                then_bb: cont_bb,
                else_bb: write_bb,
            });
            b.position(write_bb);
            let payload = gen_opt_payload(&mut b, &opt, fv);
            let vstr = gen_json_encode_field(&mut b, payload, opt.payload_ty, &fopts, ctx)?;
            let lbl = b.strconst(&label);
            let frag = gen_concat(&mut b, lbl, vstr);
            b.emit(Inst::Call {
                dest: None,
                func: "zap_list_push_ptr".to_string(),
                args: vec![parts, frag],
            });
            b.set_term(Term::Br(cont_bb));
            b.position(cont_bb);
        } else {
            let fv = b.new_value(*fty);
            b.emit(Inst::FieldGet {
                dest: fv,
                base: 0,
                sid,
                idx: idx as u32,
            });
            let vstr = gen_json_encode_field(&mut b, fv, *fty, &fopts, ctx)?;
            let lbl = b.strconst(&label);
            let frag = gen_concat(&mut b, lbl, vstr);
            b.emit(Inst::Call {
                dest: None,
                func: "zap_list_push_ptr".to_string(),
                args: vec![parts, frag],
            });
        }
    }
    let body = b.new_value(Ty::Ptr);
    b.emit(Inst::Call {
        dest: Some(body),
        func: "zap_csv_join".to_string(),
        args: vec![parts],
    });
    let open = b.strconst("{");
    let acc = gen_concat(&mut b, open, body);
    let close = b.strconst("}");
    let acc = gen_concat(&mut b, acc, close);
    b.set_term(Term::Ret(Some(acc)));
    Ok(b.func)
}

/// Encode one `@derive(Json)` field value to its JSON text. `opts` carries the
/// field's `@json(...)` options (AWS codec hooks, ticket #346): the timestamp
/// wire form of a `DateTime` field; a `bytes` field is always base64.
fn gen_json_encode_field(
    b: &mut FnLower,
    fv: Value,
    fty: Ty,
    opts: &JsonFieldOpts,
    ctx: &StructCtx,
) -> Result<Value, LowerError> {
    // An `Optional[T]` is handled one level up (object-field omit on `None`); a
    // nested `Optional` *payload* (`Optional[Optional[T]]`) has no value-position
    // JSON form here yet — reject it clearly instead of miscompiling into a call
    // to the Optional instance's non-existent encoder.
    if optional_layout(ctx, fty).is_some() {
        return Err(err(
            "@derive(Json) does not support a nested Optional[...] payload yet \
             (an Optional[T] json field's T must be a scalar, str, or nested \
             @derive(Json) struct)",
        ));
    }
    let call1 = |b: &mut FnLower, func: &str, arg: Value| -> Value {
        let d = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        d
    };
    // AWS codec hooks (ticket #346, D-0086), checked before the scalar/nested
    // walk since both `bytes` and `DateTime` would otherwise mis-route.
    // `bytes` → base64 JSON string; exact round-trip via encoding/base64.
    if fty == Ty::Bytes {
        let urlsafe = b.iconst(Ty::I64, 0);
        let pad = b.iconst(Ty::I64, 1);
        let b64 = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(b64),
            func: "zap_base64_encode".to_string(),
            args: vec![fv, urlsafe, pad],
        });
        return Ok(call1(b, "zap_json_escape", b64));
    }
    // A `DateTime` field: epoch seconds (JSON number, default) or ISO-8601
    // (JSON string), per `@json(timestamp=...)`.
    if let Ty::Struct(s) = fty {
        if Some(s) == datetime_sid_opt(ctx) {
            let comps = gen_dt_read_components(b, s, fv);
            return Ok(match opts.timestamp.unwrap_or(TsFormat::Epoch) {
                TsFormat::Epoch => {
                    let ts = b.new_value(Ty::I64);
                    b.emit(Inst::Call {
                        dest: Some(ts),
                        func: "zap_datetime_to_unix".to_string(),
                        args: comps,
                    });
                    call1(b, "zap_i64_to_str", ts)
                }
                TsFormat::Iso8601 => {
                    let iso = b.new_value(Ty::Ptr);
                    b.emit(Inst::Call {
                        dest: Some(iso),
                        func: "zap_datetime_format_iso".to_string(),
                        args: comps,
                    });
                    call1(b, "zap_json_escape", iso)
                }
            });
        }
    }
    let r = match derive_field_kind(fty) {
        Some(DeriveFieldKind::Bool) => {
            let w = b.coerce(fv, Ty::I1, Ty::I64);
            call1(b, "zap_json_bool_str", w)
        }
        Some(DeriveFieldKind::Float(w)) => {
            let f = b.coerce(fv, w, Ty::F64);
            call1(b, "zap_f64_to_str", f)
        }
        Some(DeriveFieldKind::Int(w)) => {
            let i = b.coerce(fv, w, Ty::I64);
            call1(b, "zap_i64_to_str", i)
        }
        // A `str` field — escape + quote.
        Some(DeriveFieldKind::Str) => call1(b, "zap_json_escape", fv),
        // A nested struct field — recurse through its own derived encoder.
        Some(DeriveFieldKind::Nested(fsid)) => {
            let fname = ctx.names[fsid as usize].clone();
            call1(b, &format!("{fname}__to_json"), fv)
        }
        None => {
            return Err(err(format!(
                "@derive(Json) does not support field type {fty} yet \
                 (scalars, str, and nested @derive(Json) structs are; \
                 collections and enums are a follow-up)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Json)` decode direction (D-0059, I#100): a `Struct__from_json(s:
/// str) -> Struct` associated function: parse `s` to the DOM (`__json_loads`),
/// then hand off to the recursive `Struct__from_json_value` core.
fn gen_json_from_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__from_json");
    let jv_sid = json_value_sid(ctx)?;
    // One parameter: the source string (value 0); no receiver.
    let mut b = FnLower::new(
        name,
        vec![Ty::Ptr],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    let jv = b.new_value(Ty::Struct(jv_sid));
    b.emit(Inst::Call {
        dest: Some(jv),
        func: "__json_loads".to_string(),
        args: vec![0],
    });
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::Call {
        dest: Some(dest),
        func: format!("{sname}__from_json_value"),
        args: vec![jv],
    });
    b.set_term(Term::Ret(Some(dest)));
    Ok(b.func)
}

/// `@derive(Json)` decode core (D-0059, I#100): `Struct__from_json_value(v:
/// JsonValue) -> Struct`, populating each field from the object by name via the
/// prelude typed accessors (`__json_field` / `__json_as_*`). A nested struct
/// field recurses through the nested type's own `from_json_value`, so nested
/// records decode (they must also `@derive(Json)`). Strict typing is lenient for
/// v1 (a missing/mismatched field yields the accessor's default); strict-error
/// semantics are a follow-up.
fn gen_json_from_value_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__from_json_value");
    let jv_sid = json_value_sid(ctx)?;
    // One parameter: the object JsonValue (value 0); no receiver.
    let mut b = FnLower::new(
        name,
        vec![Ty::Struct(jv_sid)],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::StructNew { dest, sid });
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (fname, fty)) in fields.iter().enumerate() {
        // jf = __json_field(v, "fname")
        let key = b.strconst(fname);
        let jf = b.new_value(Ty::Struct(jv_sid));
        b.emit(Inst::Call {
            dest: Some(jf),
            func: "__json_field".to_string(),
            args: vec![0, key],
        });
        let fopts = field_json_opts(ctx, sid, idx);
        let val = if let Some(opt) = optional_layout(ctx, *fty) {
            gen_json_decode_optional(&mut b, jf, jv_sid, &opt, &fopts, ctx)?
        } else {
            gen_json_decode_field(&mut b, jf, *fty, &fopts, ctx)?
        };
        b.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val,
        });
    }
    b.set_term(Term::Ret(Some(dest)));
    Ok(b.func)
}

/// Decode an `Optional[T]` field (I#165, D-0065): an **absent key** (`__json_field`
/// yields a null pointer on a dict miss) or an **explicit JSON `null`** (a present
/// `JsonValue.Null`, tag 0) decodes to `Optional.None`; any other present value
/// decodes to `Optional.Some(decode(v))`. The `IsNull` test is done first so the
/// tag is never read off the absent-key null pointer.
fn gen_json_decode_optional(
    b: &mut FnLower,
    jf: Value,
    jv_sid: u32,
    opt: &OptionalLayout,
    fopts: &JsonFieldOpts,
    ctx: &StructCtx,
) -> Result<Value, LowerError> {
    let slot = b.alloca(Ty::Struct(opt.sid));
    let isnull = b.new_value(Ty::I1);
    b.emit(Inst::IsNull {
        dest: isnull,
        val: jf,
    });
    let none_bb = b.new_block();
    let checktag_bb = b.new_block();
    let some_bb = b.new_block();
    let join_bb = b.new_block();
    // Absent key (null pointer) → None; otherwise inspect the present value.
    b.set_term(Term::BrIf {
        cond: isnull,
        then_bb: none_bb,
        else_bb: checktag_bb,
    });
    // Present value: explicit JSON `null` is the Null variant (tag 0).
    b.position(checktag_bb);
    let tag = b.new_value(Ty::I32);
    b.emit(Inst::FieldGet {
        dest: tag,
        base: jf,
        sid: jv_sid,
        idx: 0,
    });
    let zero = b.iconst(Ty::I32, 0);
    let (istag0, _) = b.binop(AstBin::Eq, tag, Ty::I32, zero, Ty::I32)?;
    b.set_term(Term::BrIf {
        cond: istag0,
        then_bb: none_bb,
        else_bb: some_bb,
    });
    b.position(none_bb);
    let noneval = gen_opt_none(b, opt);
    b.store(slot, noneval);
    b.set_term(Term::Br(join_bb));
    b.position(some_bb);
    let inner = gen_json_decode_field(b, jf, opt.payload_ty, fopts, ctx)?;
    let someval = gen_opt_some(b, opt, inner);
    b.store(slot, someval);
    b.set_term(Term::Br(join_bb));
    b.position(join_bb);
    Ok(b.load(slot, Ty::Struct(opt.sid)))
}

/// The prelude `JsonValue` enum's struct id (its tagged-struct layout).
fn json_value_sid(ctx: &StructCtx) -> Result<u32, LowerError> {
    ctx.ids
        .get("JsonValue")
        .copied()
        .ok_or_else(|| err("JsonValue unavailable (built-in prelude missing)"))
}

/// The per-field `@json(...)` options for struct `sid`'s field `idx` (ticket
/// #346), or the default (no options) when the struct has none.
fn field_json_opts(ctx: &StructCtx, sid: u32, idx: usize) -> JsonFieldOpts {
    ctx.json_field_opts
        .get(&sid)
        .and_then(|v| v.get(idx))
        .cloned()
        .unwrap_or_default()
}

/// The prelude `DateTime` value-struct id, if the prelude provides it (ticket
/// #346 timestamp codec). `None` when no `datetime` surface is in scope.
fn datetime_sid_opt(ctx: &StructCtx) -> Option<u32> {
    ctx.ids.get("DateTime").copied()
}

/// The `DateTime` field getters `zap_datetime_<field>(ts, is_local)`, in field
/// order (year, month, day, hour, minute, second) — mirrors the `datetime`
/// lowering facet.
const DT_GETTERS: [&str; 6] = [
    "zap_datetime_year",
    "zap_datetime_month",
    "zap_datetime_day",
    "zap_datetime_hour",
    "zap_datetime_minute",
    "zap_datetime_second",
];

/// Read a `DateTime` value's six civil components into a `[y,mo,d,h,mi,s]` arg
/// vector, ready for `zap_datetime_to_unix` / `zap_datetime_format_iso`.
fn gen_dt_read_components(b: &mut FnLower, dt_sid: u32, dt: Value) -> Vec<Value> {
    (0..6)
        .map(|idx| b.field_get(dt, dt_sid, idx, Ty::I64))
        .collect()
}

/// Build a UTC `DateTime` value from a Unix-timestamp value (`is_local == 0`),
/// one `zap_datetime_<field>` getter per component — mirrors `datetime`'s
/// `build_datetime`.
fn gen_dt_from_epoch(b: &mut FnLower, dt_sid: u32, ts: Value) -> Value {
    let local = b.iconst(Ty::I64, 0);
    let dest = b.new_value(Ty::Struct(dt_sid));
    b.emit(Inst::StructNew { dest, sid: dt_sid });
    for (idx, func) in DT_GETTERS.iter().enumerate() {
        let v = b.new_value(Ty::I64);
        b.emit(Inst::Call {
            dest: Some(v),
            func: (*func).to_string(),
            args: vec![ts, local],
        });
        b.emit(Inst::FieldSet {
            base: dest,
            sid: dt_sid,
            idx: idx as u32,
            val: v,
        });
    }
    dest
}

/// `@derive(Json)` + `@json(open)` enum codec (ticket #346, D-0086): generate
/// `to_json` / `from_json` / `from_json_value` for an open string enum. Known
/// (payload-free) variants map to their name as a JSON string; the `Unknown(str)`
/// variant carries the raw wire string for a value the enum does not name.
fn gen_open_enum_json(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Vec<Func>, LowerError> {
    Ok(vec![
        gen_enum_json_to(sid, sigs, ctx)?,
        gen_enum_json_from(sid, sigs, ctx)?,
        gen_enum_json_from_value(sid, sigs, ctx)?,
    ])
}

/// The lowered variants of the enum whose tagged-struct id is `sid`.
fn enum_variants_of(ctx: &StructCtx, sid: u32) -> Result<Vec<VariantLow>, LowerError> {
    let name = &ctx.names[sid as usize];
    ctx.enums
        .get(name)
        .map(|e| e.variants.clone())
        .ok_or_else(|| err(format!("enum '{name}' not registered for JSON derive")))
}

/// Allocate an open-enum tagged value for `v`, storing its tag and (for
/// `Unknown`) the single carried string payload.
fn build_enum_variant(b: &mut FnLower, sid: u32, v: &VariantLow, payload: Option<Value>) -> Value {
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::StructNew { dest, sid });
    let tag = b.iconst(Ty::I32, v.tag);
    b.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: tag,
    });
    if let Some(p) = payload {
        b.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: v.fields[0].1,
            val: p,
        });
    }
    dest
}

/// `{Enum}__to_json(self) -> str`: switch on the tag; a known variant emits its
/// quoted name, `Unknown(s)` emits the escaped carried string.
fn gen_enum_json_to(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = ctx.names[sid as usize].clone();
    let variants = enum_variants_of(ctx, sid)?;
    let mut b = FnLower::new(
        format!("{name}__to_json"),
        vec![Ty::Struct(sid)],
        Ty::Ptr,
        false,
        true,
        sigs,
        ctx,
    );
    let tag = b.new_value(Ty::I32);
    b.emit(Inst::FieldGet {
        dest: tag,
        base: 0,
        sid,
        idx: 0,
    });
    for v in &variants {
        let then_bb = b.new_block();
        let next_bb = b.new_block();
        let k = b.iconst(Ty::I32, v.tag);
        let (cond, _) = b.binop(AstBin::Eq, tag, Ty::I32, k, Ty::I32)?;
        b.set_term(Term::BrIf {
            cond,
            then_bb,
            else_bb: next_bb,
        });
        b.position(then_bb);
        let result = if v.name == "Unknown" {
            let payload = b.new_value(Ty::Ptr);
            b.emit(Inst::FieldGet {
                dest: payload,
                base: 0,
                sid,
                idx: v.fields[0].1,
            });
            let d = b.new_value(Ty::Ptr);
            b.emit(Inst::Call {
                dest: Some(d),
                func: "zap_json_escape".to_string(),
                args: vec![payload],
            });
            d
        } else {
            // A variant name is an identifier, so the literal needs no escaping.
            b.strconst(&format!("\"{}\"", v.name))
        };
        b.set_term(Term::Ret(Some(result)));
        b.position(next_bb);
    }
    // Unreachable (the tag always names a variant); emit a safe default.
    let nullv = b.strconst("null");
    b.set_term(Term::Ret(Some(nullv)));
    Ok(b.func)
}

/// `{Enum}__from_json(s) -> Self`: parse to the DOM then hand to the decode core.
fn gen_enum_json_from(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = ctx.names[sid as usize].clone();
    let jv_sid = json_value_sid(ctx)?;
    let mut b = FnLower::new(
        format!("{name}__from_json"),
        vec![Ty::Ptr],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    let jv = b.new_value(Ty::Struct(jv_sid));
    b.emit(Inst::Call {
        dest: Some(jv),
        func: "__json_loads".to_string(),
        args: vec![0],
    });
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::Call {
        dest: Some(dest),
        func: format!("{name}__from_json_value"),
        args: vec![jv],
    });
    b.set_term(Term::Ret(Some(dest)));
    Ok(b.func)
}

/// `{Enum}__from_json_value(v) -> Self`: read the string and match it against
/// each known variant name; an unrecognized string becomes `Unknown(raw)`
/// (forward-compatible with AWS adding enum values).
fn gen_enum_json_from_value(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = ctx.names[sid as usize].clone();
    let jv_sid = json_value_sid(ctx)?;
    let variants = enum_variants_of(ctx, sid)?;
    let mut b = FnLower::new(
        format!("{name}__from_json_value"),
        vec![Ty::Struct(jv_sid)],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    // s = __json_as_str(v) — the wire string (v is value 0).
    let s = b.new_value(Ty::Ptr);
    b.emit(Inst::Call {
        dest: Some(s),
        func: "__json_as_str".to_string(),
        args: vec![0],
    });
    for v in &variants {
        if v.name == "Unknown" {
            continue;
        }
        let then_bb = b.new_block();
        let next_bb = b.new_block();
        let lit = b.strconst(&v.name);
        let (cond, _) = b.binop(AstBin::Eq, s, Ty::Ptr, lit, Ty::Ptr)?;
        b.set_term(Term::BrIf {
            cond,
            then_bb,
            else_bb: next_bb,
        });
        b.position(then_bb);
        let built = build_enum_variant(&mut b, sid, v, None);
        b.set_term(Term::Ret(Some(built)));
        b.position(next_bb);
    }
    let unknown = variants
        .iter()
        .find(|v| v.name == "Unknown")
        .ok_or_else(|| err(format!("open enum '{name}' lost its Unknown variant")))?;
    let built = build_enum_variant(&mut b, sid, unknown, Some(s));
    b.set_term(Term::Ret(Some(built)));
    Ok(b.func)
}

/// Decode one field: convert its `JsonValue` (`jf`) to the field type `fty` via
/// the prelude typed accessors, recursing into a nested struct's
/// `from_json_value`.
fn gen_json_decode_field(
    b: &mut FnLower,
    jf: Value,
    fty: Ty,
    opts: &JsonFieldOpts,
    ctx: &StructCtx,
) -> Result<Value, LowerError> {
    // A top-level `Optional[T]` field is handled by `gen_json_decode_optional`; a
    // nested `Optional` payload (`Optional[Optional[T]]`) is not supported yet —
    // reject it clearly rather than treat it as a nested struct and emit a call to
    // the Optional instance's non-existent decoder.
    if optional_layout(ctx, fty).is_some() {
        return Err(err(
            "@derive(Json) does not support a nested Optional[...] payload yet \
             (an Optional[T] json field's T must be a scalar, str, or nested \
             @derive(Json) struct)",
        ));
    }
    let call1 = |b: &mut FnLower, func: &str, ret: Ty, arg: Value| -> Value {
        let d = b.new_value(ret);
        b.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        d
    };
    // AWS codec hooks (ticket #346, D-0086) — mirror of the encode side.
    // `bytes` ← a base64 JSON string, decoded via encoding/base64 (throwing on a
    // malformed string, like `base64.decode`).
    if fty == Ty::Bytes {
        let s = call1(b, "__json_as_str", Ty::Ptr, jf);
        let urlsafe = b.iconst(Ty::I64, 0);
        let d = b.new_value(Ty::Bytes);
        b.throwing_call(Some(d), "zap_base64_decode", vec![s, urlsafe]);
        return Ok(d);
    }
    // A `DateTime` field ← epoch-seconds number (default) or ISO-8601 string.
    if let Ty::Struct(s) = fty {
        if Some(s) == datetime_sid_opt(ctx) {
            let ts = match opts.timestamp.unwrap_or(TsFormat::Epoch) {
                TsFormat::Epoch => call1(b, "__json_as_int", Ty::I64, jf),
                TsFormat::Iso8601 => {
                    let iso = call1(b, "__json_as_str", Ty::Ptr, jf);
                    let t = b.new_value(Ty::I64);
                    b.emit(Inst::Call {
                        dest: Some(t),
                        func: "zap_datetime_parse_iso".to_string(),
                        args: vec![iso],
                    });
                    t
                }
            };
            return Ok(gen_dt_from_epoch(b, s, ts));
        }
    }
    let r = match derive_field_kind(fty) {
        Some(DeriveFieldKind::Bool) => call1(b, "__json_as_bool", Ty::I1, jf),
        Some(DeriveFieldKind::Float(Ty::F32)) => {
            let f = call1(b, "__json_as_float", Ty::F64, jf);
            b.coerce(f, Ty::F64, Ty::F32)
        }
        Some(DeriveFieldKind::Float(_)) => call1(b, "__json_as_float", Ty::F64, jf),
        Some(DeriveFieldKind::Int(Ty::I64)) => call1(b, "__json_as_int", Ty::I64, jf),
        Some(DeriveFieldKind::Int(w)) => {
            let i = call1(b, "__json_as_int", Ty::I64, jf);
            b.coerce(i, Ty::I64, w)
        }
        Some(DeriveFieldKind::Str) => call1(b, "__json_as_str", Ty::Ptr, jf),
        // A nested struct field — recurse through its own decode core.
        Some(DeriveFieldKind::Nested(fsid)) => {
            let fname = ctx.names[fsid as usize].clone();
            let d = b.new_value(Ty::Struct(fsid));
            b.emit(Inst::Call {
                dest: Some(d),
                func: format!("{fname}__from_json_value"),
                args: vec![jf],
            });
            d
        }
        None => {
            return Err(err(format!(
                "@derive(Json) from_json does not support field type {fty} yet \
                 (scalars, str, and nested @derive(Json) structs are; \
                 collections and enums are a follow-up)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Yaml)` encode direction (D-0063, I#149): a `Struct__to_yaml(self)
/// -> str` method producing a YAML block mapping, one `fname: value` line per
/// field (declaration order). Scalar values reuse the json inline formatters
/// (strings JSON-escaped+quoted, which is valid YAML); a nested struct field
/// recurses through its own derived `to_yaml`, re-indented by two spaces via
/// `zap_yaml_indent`, so nested records must also `@derive(Yaml)`.
fn gen_yaml_to_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__to_yaml");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::Ptr, false, true, sigs, ctx);
    let mut acc = b.strconst("");
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (fname, fty)) in fields.iter().enumerate() {
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        if let Some(DeriveFieldKind::Nested(fsid)) = derive_field_kind(*fty) {
            // `fname:\n` then the child's YAML, indented two spaces.
            let label = b.strconst(&format!("{fname}:\n"));
            acc = gen_concat(&mut b, acc, label);
            let child_name = ctx.names[fsid as usize].clone();
            let child = b.new_value(Ty::Ptr);
            b.emit(Inst::Call {
                dest: Some(child),
                func: format!("{child_name}__to_yaml"),
                args: vec![fv],
            });
            let pad = b.strconst("  ");
            let indented = b.new_value(Ty::Ptr);
            b.emit(Inst::Call {
                dest: Some(indented),
                func: "zap_yaml_indent".to_string(),
                args: vec![child, pad],
            });
            acc = gen_concat(&mut b, acc, indented);
        } else {
            // `fname: <scalar>\n`
            let label = b.strconst(&format!("{fname}: "));
            acc = gen_concat(&mut b, acc, label);
            let vstr = gen_yaml_encode_scalar(&mut b, fv, *fty)?;
            acc = gen_concat(&mut b, acc, vstr);
            let nl = b.strconst("\n");
            acc = gen_concat(&mut b, acc, nl);
        }
    }
    b.set_term(Term::Ret(Some(acc)));
    Ok(b.func)
}

/// Encode one scalar `@derive(Yaml)` field value to its inline YAML text
/// (reusing the json formatters; JSON scalars are valid YAML). Nested structs
/// are handled by the caller; other field types are rejected.
fn gen_yaml_encode_scalar(b: &mut FnLower, fv: Value, fty: Ty) -> Result<Value, LowerError> {
    let call1 = |b: &mut FnLower, func: &str, arg: Value| -> Value {
        let d = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        d
    };
    let r = match derive_field_kind(fty) {
        Some(DeriveFieldKind::Bool) => {
            let w = b.coerce(fv, Ty::I1, Ty::I64);
            call1(b, "zap_json_bool_str", w)
        }
        Some(DeriveFieldKind::Float(w)) => {
            let f = b.coerce(fv, w, Ty::F64);
            call1(b, "zap_f64_to_str", f)
        }
        Some(DeriveFieldKind::Int(w)) => {
            let i = b.coerce(fv, w, Ty::I64);
            call1(b, "zap_i64_to_str", i)
        }
        Some(DeriveFieldKind::Str) => call1(b, "zap_json_escape", fv),
        Some(DeriveFieldKind::Nested(_)) | None => {
            return Err(err(format!(
                "@derive(Yaml) does not support field type {fty} yet \
                 (scalars, str, and nested @derive(Yaml) structs are; \
                 collections and enums are a follow-up)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Yaml)` decode direction (D-0063, I#149): a `Struct__from_yaml(s:
/// str) -> Struct` associated function. YAML 1.2 is a JSON superset, so this
/// parses `s` to the shared `JsonValue` DOM (`__yaml_loads`, which transpiles
/// the YAML subset to JSON and reuses `__json_loads`), then hands off to the
/// same recursive `Struct__from_json_value` decode core as `@derive(Json)`.
fn gen_yaml_from_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__from_yaml");
    let jv_sid = json_value_sid(ctx)?;
    // One parameter: the source string (value 0); no receiver.
    let mut b = FnLower::new(
        name,
        vec![Ty::Ptr],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    let jv = b.new_value(Ty::Struct(jv_sid));
    b.emit(Inst::Call {
        dest: Some(jv),
        func: "__yaml_loads".to_string(),
        args: vec![0],
    });
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::Call {
        dest: Some(dest),
        func: format!("{sname}__from_json_value"),
        args: vec![jv],
    });
    b.set_term(Term::Ret(Some(dest)));
    Ok(b.func)
}

/// `@derive(Csv)` encode (D-0059 §4b, I#132): `Struct__to_csv(self) -> str`
/// producing one CSV record — each field formatted and CSV-quoted as needed,
/// joined with commas (declaration order = column order). Reuses the shared
/// `DeriveFieldKind` walk; nested structs / collections do not map to a flat
/// record and are rejected.
fn gen_csv_to_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__to_csv");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::Ptr, false, true, sigs, ctx);
    // parts = [] (list of column strings)
    let parts = b.new_value(Ty::List(Elem::Ptr));
    b.emit(Inst::Call {
        dest: Some(parts),
        func: "zap_list_new".to_string(),
        args: vec![],
    });
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        let col = gen_csv_encode_field(&mut b, fv, *fty)?;
        b.emit(Inst::Call {
            dest: None,
            func: "zap_list_push_ptr".to_string(),
            args: vec![parts, col],
        });
    }
    let d = b.new_value(Ty::Ptr);
    b.emit(Inst::Call {
        dest: Some(d),
        func: "zap_csv_join".to_string(),
        args: vec![parts],
    });
    b.set_term(Term::Ret(Some(d)));
    Ok(b.func)
}

/// Encode one `@derive(Csv)` field to its CSV column text.
fn gen_csv_encode_field(b: &mut FnLower, fv: Value, fty: Ty) -> Result<Value, LowerError> {
    let call1 = |b: &mut FnLower, func: &str, arg: Value| -> Value {
        let d = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        d
    };
    let r = match derive_field_kind(fty) {
        // Python csv convention: booleans render as True / False.
        Some(DeriveFieldKind::Bool) => {
            let w = b.coerce(fv, Ty::I1, Ty::I64);
            call1(b, "zap_bool_to_str", w)
        }
        Some(DeriveFieldKind::Float(w)) => {
            let f = b.coerce(fv, w, Ty::F64);
            call1(b, "zap_f64_to_str", f)
        }
        Some(DeriveFieldKind::Int(w)) => {
            let i = b.coerce(fv, w, Ty::I64);
            call1(b, "zap_i64_to_str", i)
        }
        Some(DeriveFieldKind::Str) => call1(b, "zap_csv_quote", fv),
        Some(DeriveFieldKind::Nested(_)) | None => {
            return Err(err(format!(
                "@derive(Csv) does not support field type {fty} \
                 (a CSV record is flat: scalars and str only; nested structs, \
                 collections, and enums do not map to columns)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Csv)` decode (D-0059 §4b, I#132): `Struct__from_csv(s: str) ->
/// Struct`, splitting one CSV record into columns and parsing each to its
/// field's scalar type. Lenient for v1 (unparseable column → 0 / 0.0 / false).
fn gen_csv_from_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = ctx.names[sid as usize].clone();
    let name = format!("{sname}__from_csv");
    // One parameter: the record line (value 0); no receiver.
    let mut b = FnLower::new(
        name,
        vec![Ty::Ptr],
        Ty::Struct(sid),
        false,
        false,
        sigs,
        ctx,
    );
    // cols = zap_csv_split(line)
    let cols = b.new_value(Ty::List(Elem::Ptr));
    b.emit(Inst::Call {
        dest: Some(cols),
        func: "zap_csv_split".to_string(),
        args: vec![0],
    });
    let dest = b.new_value(Ty::Struct(sid));
    b.emit(Inst::StructNew { dest, sid });
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let iidx = b.iconst(Ty::I64, idx as i64);
        let col = b.new_value(Ty::Ptr);
        b.emit(Inst::Call {
            dest: Some(col),
            func: "zap_list_get_ptr".to_string(),
            args: vec![cols, iidx],
        });
        let val = gen_csv_decode_field(&mut b, col, *fty)?;
        b.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val,
        });
    }
    b.set_term(Term::Ret(Some(dest)));
    Ok(b.func)
}

/// Decode one `@derive(Csv)` column string `col` to the field type `fty`.
fn gen_csv_decode_field(b: &mut FnLower, col: Value, fty: Ty) -> Result<Value, LowerError> {
    let r = match derive_field_kind(fty) {
        Some(DeriveFieldKind::Bool) => {
            let w = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(w),
                func: "zap_str_to_bool".to_string(),
                args: vec![col],
            });
            b.coerce(w, Ty::I64, Ty::I1)
        }
        Some(DeriveFieldKind::Float(w)) => {
            let f = b.new_value(Ty::F64);
            b.emit(Inst::Call {
                dest: Some(f),
                func: "zap_str_to_f64".to_string(),
                args: vec![col],
            });
            if w == Ty::F64 {
                f
            } else {
                b.coerce(f, Ty::F64, w)
            }
        }
        Some(DeriveFieldKind::Int(w)) => {
            let i = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(i),
                func: "zap_str_to_i64".to_string(),
                args: vec![col],
            });
            if w == Ty::I64 {
                i
            } else {
                b.coerce(i, Ty::I64, w)
            }
        }
        // The column is already the string value.
        Some(DeriveFieldKind::Str) => col,
        Some(DeriveFieldKind::Nested(_)) | None => {
            return Err(err(format!(
                "@derive(Csv) does not support field type {fty} \
                 (a CSV record is flat: scalars and str only; nested structs, \
                 collections, and enums do not map to columns)"
            )));
        }
    };
    Ok(r)
}

/// `@derive(Ord)` — a three-way comparison `Struct____cmp__(self, other) -> i64`
/// (the `__cmp__` dunder, D-0047) returning −1 / 0 / +1 lexicographically by
/// field. The four relational operators (`< <= > >=`) derive from it. Fields are
/// compared in declaration order; the first non-equal field decides the result.
fn gen_cmp_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = format!("{}__{}", ctx.names[sid as usize], "__cmp__");
    let mut b = FnLower::new(
        name,
        vec![Ty::Struct(sid), Ty::Struct(sid)],
        Ty::I64,
        false,
        true, // self_param: a derived method's receiver is by-reference
        sigs,
        ctx,
    );
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let idx = idx as u32;
        let a = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: a,
            base: 0,
            sid,
            idx,
        });
        let c = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: c,
            base: 1,
            sid,
            idx,
        });
        let cmp = gen_field_cmp(&mut b, a, c, *fty)?;
        // `if cmp != 0: return cmp` — otherwise fall through to the next field.
        let zero = b.iconst(Ty::I64, 0);
        let ne = b.new_value(Ty::I1);
        b.emit(Inst::ICmp {
            dest: ne,
            pred: CmpPred::Ne,
            lhs: cmp,
            rhs: zero,
        });
        let ret_bb = b.new_block();
        let next_bb = b.new_block();
        b.set_term(Term::BrIf {
            cond: ne,
            then_bb: ret_bb,
            else_bb: next_bb,
        });
        b.position(ret_bb);
        b.set_term(Term::Ret(Some(cmp)));
        b.position(next_bb);
    }
    // All fields equal.
    let zero = b.iconst(Ty::I64, 0);
    b.set_term(Term::Ret(Some(zero)));
    Ok(b.func)
}

/// Three-way compare one field, returning an `i64` of −1 / 0 / +1.
fn gen_field_cmp(b: &mut FnLower, a: Value, c: Value, fty: Ty) -> Result<Value, LowerError> {
    match fty {
        Ty::Ptr => {
            // Strings: lexicographic byte comparison via the runtime.
            let d = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(d),
                func: "zap_str_cmp".to_string(),
                args: vec![a, c],
            });
            Ok(d)
        }
        Ty::F32 | Ty::F64 => {
            let gt = b.new_value(Ty::I1);
            b.emit(Inst::FCmp {
                dest: gt,
                pred: CmpPred::Gt,
                lhs: a,
                rhs: c,
            });
            let lt = b.new_value(Ty::I1);
            b.emit(Inst::FCmp {
                dest: lt,
                pred: CmpPred::Lt,
                lhs: a,
                rhs: c,
            });
            Ok(gen_three_way(b, gt, lt))
        }
        t if t.int_width().is_some() => {
            // Compare at i64 so mixed-width fields share a type (D-0019).
            let ai = b.coerce(a, t, Ty::I64);
            let ci = b.coerce(c, t, Ty::I64);
            let gt = b.new_value(Ty::I1);
            b.emit(Inst::ICmp {
                dest: gt,
                pred: CmpPred::Gt,
                lhs: ai,
                rhs: ci,
            });
            let lt = b.new_value(Ty::I1);
            b.emit(Inst::ICmp {
                dest: lt,
                pred: CmpPred::Lt,
                lhs: ai,
                rhs: ci,
            });
            Ok(gen_three_way(b, gt, lt))
        }
        other => Err(err(format!(
            "@derive(Ord) does not support field type {other} yet \
             (only scalars and str)"
        ))),
    }
}

/// Fold a `greater-than` and `less-than` bit into an `i64` of +1 / −1 / 0
/// without integer extension pitfalls (an `i1` sign-extends to −1).
fn gen_three_way(b: &mut FnLower, gt: Value, lt: Value) -> Value {
    let neg1 = b.iconst(Ty::I64, -1);
    let zero = b.iconst(Ty::I64, 0);
    let one = b.iconst(Ty::I64, 1);
    let tmp = b.new_value(Ty::I64);
    b.emit(Inst::Select {
        dest: tmp,
        cond: lt,
        a: neg1,
        b: zero,
    });
    let d = b.new_value(Ty::I64);
    b.emit(Inst::Select {
        dest: d,
        cond: gt,
        a: one,
        b: tmp,
    });
    d
}

/// `@derive(Hash)` — a hash method `Struct____hash__(self) -> i64` (the
/// `__hash__` dunder, D-0047) combining every field's hash. The same fields feed
/// `__eq__` and `__hash__`, so the invariant `a == b ⟹ hash(a) == hash(b)` holds
/// (D-0047). Float fields are a documented follow-up (bit-hashing would break the
/// invariant against `__eq__`'s value compare), so they are rejected cleanly.
fn gen_hash_method(
    sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let name = format!("{}__{}", ctx.names[sid as usize], "__hash__");
    let mut b = FnLower::new(name, vec![Ty::Struct(sid)], Ty::I64, false, true, sigs, ctx);
    // Seed; each field hash is mixed in with `zap_hash_combine`.
    let mut acc = b.iconst(Ty::I64, 0);
    let fields = ctx.fields[sid as usize].clone();
    for (idx, (_fname, fty)) in fields.iter().enumerate() {
        let fv = b.new_value(*fty);
        b.emit(Inst::FieldGet {
            dest: fv,
            base: 0,
            sid,
            idx: idx as u32,
        });
        // Classify + hash the field through the shared structural-key plan
        // (D-0139/#808): a derived `__hash__` now supports float fields (with the
        // #803 canonical-key / reject-NaN semantics) and nested aggregate fields,
        // and rejects an unhashable field with the same clean message as a tuple
        // key — the same fold used by the tuple/Optional key paths.
        let key = b.elem_key_plan(*fty)?;
        let h = b.emit_elem_hash(fv, *fty, &key);
        let next = b.new_value(Ty::I64);
        b.emit(Inst::Call {
            dest: Some(next),
            func: "zap_hash_combine".to_string(),
            args: vec![acc, h],
        });
        acc = next;
    }
    b.set_term(Term::Ret(Some(acc)));
    Ok(b.func)
}

#[allow(clippy::too_many_arguments)]
fn lower_fn(
    f: &ast::Function,
    zir_name: String,
    self_sid: Option<u32>,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
    reflect_meta: &[(String, String)],
    param_meta: &[FnReflect],
    on_signal_meta: &[(i64, String)],
) -> Result<Func, LowerError> {
    let (params, ret) = sigs.get(&zir_name).cloned().unwrap();
    let is_main = zir_name == "main";
    // `main` and each REPL entry thunk (`__repl_entry*`, I#159) get the implicit
    // top-level landing pad (computed before `zir_name` is moved into FnLower).
    let wants_top_handler = is_main || zir_name.starts_with("__repl_entry");
    let mut b = FnLower::new(
        zir_name,
        params.clone(),
        ret,
        is_main,
        self_sid.is_some(),
        sigs,
        ctx,
    );

    // Parameters: values 0..n are the incoming args; copy each into a slot.
    for (i, p) in f.params.iter().enumerate() {
        let pty = params[i];
        let slot = b.alloca(pty);
        b.store(slot, i as Value);
        b.declare(&p.name, slot, pty);
    }

    if is_main {
        // Runtime data-race detector bootstrap (`--race`, #678, epic #665,
        // D-0114): arm the happens-before engine and register every module
        // global's name for report resolution. Emitted first in the prologue and
        // ONLY under `--race` (a non-`--race` build emits nothing here, so its
        // `main` prologue is unchanged). See `emit_race_bootstrap`.
        if b.ctx.race {
            b.emit_race_bootstrap();
        }
        // Reflection bootstrap (D-0036): populate the runtime metadata registry
        // once, at the top of `main`, so `reflect_has`/`reflect_names` queries
        // anywhere in the program see every annotated/decorated type. These are
        // plain (non-throwing) runtime calls emitted before the user body.
        b.emit_reflect_registrations(reflect_meta);
        // Signature reflection bootstrap (I#458, D-0089): record every reflectable
        // function/method's parameter metadata and precomputed signature string, so
        // `reflect_params`/`reflect_sig`/`reflect_param_*` queries resolve anywhere.
        b.emit_param_reflect_registrations(param_meta);
        // Signal subsystem bootstrap (ticket #77, D-0058): start the dedicated
        // signal thread + self-pipe and install the default diagnostic handlers
        // (SIGQUIT -> stacks/runtime dump, SIGUSR1 -> GC + mem stats), unless the
        // runtime is built -DZAP_NO_DEFAULT_SIGNALS. A no-op there; otherwise the
        // handlers are live for the whole program (user notify/@on_signal wins).
        b.emit(Inst::Call {
            dest: None,
            func: "zap_signal_init".to_string(),
            args: vec![],
        });
        // `@on_signal(Signal.X)` handler registrations (ticket #77): each spawns
        // a detached receiver coroutine over `signal.notify(X)` that invokes the
        // decorated function on every delivery. Emitted at the top of `main`.
        b.emit_on_signal_registrations(on_signal_meta);
    }
    // `main` — and each REPL entry thunk (`__repl_entry`, I#159) — installs an
    // implicit top-level landing pad so an exception that propagates out is
    // caught by `zap_uncaught` rather than ending in a raw abort (RUNTIME-ABI
    // §"Exception / unwinding ABI"). Every call therefore `invoke`s to this pad.
    // For a REPL entry `zap_uncaught` does not exit: when `zap_repl_call`
    // installed a catch it unwinds back to the loop, keeping the session alive.
    // The REPL entry gets the pad but NOT `main`'s bootstrap/`join_all`/C-`main`
    // signature (its ZIR `is_main` is false), so it is emitted as a plain
    // `void` function the JIT invokes once per entry.
    if wants_top_handler {
        let lp = b.new_block();
        b.landing_pads.push(lp);
        // Module-level persistent state (D-0112, #658 Phase 1): initialize every
        // module `const` exactly once, before the user body, by calling the
        // synthetic `@__zap_init_globals`. Emitted under the landing pad so a
        // raise inside an initializer unwinds to the top-level handler like any
        // other. Only `main` carries this (a REPL entry does not re-init).
        if is_main && b.ctx.has_init_work() {
            b.throwing_call(None, INIT_GLOBALS_FN, vec![]);
        }
        for st in &f.body.stmts {
            b.stmt(st)?;
        }
        b.landing_pads.pop();
        if !b.terminated() {
            b.set_term(Term::Ret(None));
        }
        if b.used_lps.contains(&lp) {
            b.position(lp);
            let exc = b.new_value(Ty::Ptr);
            b.emit(Inst::LandingPad { dest: exc });
            b.emit(Inst::Call {
                dest: None,
                func: "zap_uncaught".to_string(),
                args: vec![exc],
            });
            b.set_term(Term::Unreachable);
        }
    } else {
        for st in &f.body.stmts {
            b.stmt(st)?;
        }
        if !b.terminated() {
            if ret == Ty::Void {
                b.set_term(Term::Ret(None));
            } else {
                b.set_term(Term::Unreachable);
            }
        }
    }
    Ok(b.func)
}

/// Synthesize a tier-1 enum interface-dispatch **thunk** `{Enum}$box__{method}`
/// (#731, D-0016). A tier-1 (unboxed-integer) enum has no pointer identity, so to
/// put it behind an interface's `{data, itable}` fat pointer its scalar tag is
/// stored in a heap box `{Enum}$box = { $tag:i64 }` whose pointer becomes `data`.
/// Interface dispatch always passes `data` as a pointer `self` and calls the
/// itable slot, but the real method `{Enum}__{method}` takes `self` by value as
/// the backing integer. This thunk bridges the two: it receives the box pointer,
/// reloads the `i64` tag, and tail-calls the real method, returning its result.
/// It is the symbol placed in the `(box_sid, iface)` itable slot, so a dynamic
/// call reaches exactly the code a direct `value.method()` call does.
fn build_tier1_box_thunk(
    enum_name: &str,
    method_name: &str,
    box_sid: u32,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let thunk_name = format!("{enum_name}$box__{method_name}");
    let real_name = format!("{enum_name}__{method_name}");
    let (tparams, ret) = sigs
        .get(&thunk_name)
        .cloned()
        .ok_or_else(|| err(format!("internal: missing thunk sig for '{thunk_name}'")))?;
    let enum_id = ctx
        .enums
        .get(enum_name)
        .map(|e| e.enum_id)
        .ok_or_else(|| err(format!("internal: enum '{enum_name}' not registered")))?;
    let mut b = FnLower::new(thunk_name, tparams.clone(), ret, false, true, sigs, ctx);
    // Reload the backing tag from box field 0 (`self` is value 0, the box ptr).
    let tag = b.new_value(Ty::Enum(enum_id));
    b.emit(Inst::FieldGet {
        dest: tag,
        base: 0,
        sid: box_sid,
        idx: 0,
    });
    // Forward the tag as `self` plus every trailing parameter (values 1..N).
    let mut args = vec![tag];
    for i in 1..tparams.len() as Value {
        args.push(i);
    }
    let dest = if ret == Ty::Void {
        None
    } else {
        Some(b.new_value(ret))
    };
    b.emit(Inst::Call {
        dest,
        func: real_name,
        args,
    });
    b.set_term(Term::Ret(dest));
    Ok(b.func)
}

/// Lower the module-level declarations (D-0112) — `const T NAME = expr`
/// (immutable, #658 Phase 1) and `T NAME = expr` (mutable, #662 Phase 2) — into
/// the synthetic `@__zap_init_globals` function, run once at the top of `main`.
///
/// Each single-name module-level binding becomes a module global slot: its
/// initializer is lowered here — reusing the ordinary `let` lowering (coerce to
/// the declared type, or infer for `auto`; value-struct copy) — and stored into
/// the slot. The slot is registered in `ctx.globals` as it is created, so a
/// later binding's initializer, and every user function body lowered afterwards,
/// resolve a reference to it (an earlier binding; a forward reference fails as
/// use-before-init). Const and mutable bindings lower identically here — the
/// only difference is that a mutable one may later be reassigned, which lowers
/// to a `GlobalAddr` + `Store` through the same slot (see `name_lvalue`). Items
/// arrive in import-topological + source order (the module linker arranged
/// that), which is the eager init order. A pointer-typed slot is registered as a
/// permanent GC root (`zap_gc_add_root`) so the collector — conservative Boehm
/// (a no-op there, the data segment is scanned) or precise Immix — keeps its
/// referent live; for a mutable pointer global this covers whatever heap object
/// the slot currently holds, so a reassigned/grown container stays reachable.
fn lower_init_globals(
    module: &ast::Module,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let mut b = FnLower::new(
        INIT_GLOBALS_FN.to_string(),
        vec![],
        Ty::Void,
        false,
        false,
        sigs,
        ctx,
    );
    // Register every `@exception(Parent)` family edge (#1278) with the runtime
    // FIRST — before the module `const` initializers run — so the family is live
    // even if an initializer raises and catches a family member. Both ids are
    // plain i64 name-hashes (no GC roots); registration is idempotent in the
    // runtime, so a re-run (there is none today) would be harmless.
    for (member_id, parent_id) in &b.ctx.exception_parents {
        let m = b.iconst(Ty::I64, *member_id);
        let p = b.iconst(Ty::I64, *parent_id);
        b.emit(Inst::Call {
            dest: None,
            func: "zap_exception_register_parent".to_string(),
            args: vec![m, p],
        });
    }
    for item in &module.items {
        let ast::Item::Stmt(Stmt::Let {
            is_const,
            ty,
            name,
            value,
            ..
        }) = item
        else {
            continue;
        };
        let (v, declared) = match ty {
            Some(t) => {
                let declared = resolve_ty(t, &ctx.tyenv())?;
                (
                    b.expr_coerced(value, declared, zapc_diag::ErrorCode::ZAP4001)?,
                    declared,
                )
            }
            None => b.expr(value)?,
        };
        // A value struct is copied on binding (D-0051), like any `let`.
        let v = b.copy_if_value(v, declared, value);
        // Does `--race` instrument accesses of this global (#677, epic #665)? It
        // is the ZAP5125 sharing-lint set (#664): a MUTABLE, non-`@shareable`
        // global. A `const` can never race; a `@shareable` cell (user struct or a
        // `BUILTIN_SHAREABLE_TYPES` type) is asserted safe. Mirrors sema's
        // `declared_type_is_shareable` — decided on the declared type's nominal
        // base name, else the resolved type (a user struct → `Ty::Struct`).
        let shareable = match ty {
            Some(ast::Type::Named { name, .. }) => ctx.name_is_shareable(name),
            _ => match declared {
                Ty::Struct(sid) => ctx.name_is_shareable(&ctx.names[sid as usize]),
                _ => false,
            },
        };
        let racy = !is_const && !shareable;
        let idx = ctx.add_global(name, declared, racy);
        let slot = b.new_value(Ty::Ptr);
        b.emit(Inst::GlobalAddr { dest: slot, idx });
        // Root a pointer slot before storing into it, so the referent survives a
        // GC triggered by a later initializer's allocation.
        if declared.is_ptr() {
            b.emit(Inst::Call {
                dest: None,
                func: "zap_gc_add_root".to_string(),
                args: vec![slot],
            });
        }
        b.store(slot, v);
    }
    if !b.terminated() {
        b.set_term(Term::Ret(None));
    }
    Ok(b.func)
}

/// Discover a generator's live locals and element type `T` (I#67), by lowering
/// its body once in **discovery** mode and throwing the result away. Returns the
/// body's `let`-bound locals as `(name, type)` in first-seen order (the state
/// struct's local fields) and the element type (the join of the `yield` operand
/// types). The generator's parameters are handled by the caller (they are the
/// leading state-struct fields).
fn discover_generator(
    f: &ast::Function,
    key: &str,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<(Vec<(String, Ty)>, Ty), LowerError> {
    // `key` is the generator's callable name in `sigs`: a free generator's own
    // name, or a method generator's mangled `Struct__method` (I#429).
    let (params, _ret) = sigs.get(key).cloned().unwrap();
    let mut b = FnLower::new(
        key.to_string(),
        params.clone(),
        Ty::Void,
        false,
        false,
        sigs,
        ctx,
    );
    b.gen_discover = Some(Vec::new());
    for (i, p) in f.params.iter().enumerate() {
        let pty = params[i];
        let slot = b.alloca(pty);
        b.store(slot, i as Value);
        b.declare(&p.name, slot, pty);
    }
    for st in &f.body.stmts {
        b.stmt(st)?;
    }
    let locals = b.gen_discover.take().unwrap();
    let elem = join_yield_tys(&b.gen_elems)?;
    Ok((locals, elem))
}

/// The element type `T` of a generator: every `yield` must produce the same ZIR
/// type in v1 (I#67). Detection guarantees at least one `yield`.
fn join_yield_tys(tys: &[Ty]) -> Result<Ty, LowerError> {
    let mut it = tys.iter().copied();
    let first = it
        .next()
        .ok_or_else(|| err("internal: generator has no 'yield'"))?;
    for t in it {
        if t != first {
            return Err(err(
                "a generator must yield values of a single type in this version (I#67 v1)",
            ));
        }
    }
    Ok(first)
}

/// Emit a generator's `__iter__` (I#67, D-0048): an iterator is its own iterable,
/// so it returns `self`.
fn lower_generator_iter(
    gi: &GeneratorInfo,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Func {
    let sname = zapc_syntax::genanalysis::gen_state_name(&gi.name);
    let mangled = format!("{sname}____iter__");
    let mut b = FnLower::new(
        mangled,
        vec![Ty::Struct(gi.sid)],
        Ty::Struct(gi.sid),
        false,
        true,
        sigs,
        ctx,
    );
    b.set_term(Term::Ret(Some(0))); // `self` is value 0
    b.func
}

/// Emit a generator's comma-ok `__next__` (I#67, D-0048) as a state machine.
///
/// Shape (ZIR is a flat CFG with no `switch`, so dispatch is a `BrIf` ladder):
/// ```text
/// entry:   <hoisted allocas>                 ; one per live local / parameter
///          slot_i = FieldGet self, field_i   ; load persisted values
///          state  = FieldGet self, 0
///          if state == 0 -> body_start
///          if state == 1 -> resume_1
///          ...
///          else          -> done
/// body:    ... run to the first `yield` ...
/// resume_k:... continue after the k-th `yield` ...
/// done:    FieldSet self, 0, -1              ; latch exhausted
///          return (zeroed, false)
/// ```
/// Each `yield v` (lowered in [`FnLower::lower_yield_stmt`]) stores every live
/// slot back to its field, sets `$state`, and returns `(v, true)` — so the next
/// call's entry dispatch resumes just past it.
fn lower_generator_next(
    f: &ast::Function,
    gi: &GeneratorInfo,
    sigs: &HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &StructCtx,
) -> Result<Func, LowerError> {
    let sname = zapc_syntax::genanalysis::gen_state_name(&gi.name);
    let mangled = format!("{sname}____next__");
    let ret = Ty::Struct(gi.next_tuple_sid);
    let mut b = FnLower::new(
        mangled,
        vec![Ty::Struct(gi.sid)],
        ret,
        false,
        true,
        sigs,
        ctx,
    );
    let entry = 0; // block 0, created by `FnLower::new`
    let done = b.new_block();
    let body_start = b.new_block();

    let mut field_idx = HashMap::new();
    for (p, (name, _ty)) in gi.vars.iter().enumerate() {
        field_idx.insert(name.clone(), gen_var_field(p));
    }
    b.gen_entry = Some(GenEntry {
        self_val: 0,
        sid: gi.sid,
        entry_block: entry,
        done_block: done,
        next_tuple_sid: gi.next_tuple_sid,
        elem: gi.elem,
        field_idx,
        slots: HashMap::new(),
        order: Vec::new(),
        next_state: 1,
        resume: Vec::new(),
    });

    // Pre-create the per-call slots for the parameters (declared in the root
    // scope) so the body can reference them and they are field-loaded at entry.
    for p in 0..gi.n_params {
        let (name, ty) = gi.vars[p].clone();
        b.gen_entry_slot(&name, ty);
    }

    // Lower the body; `yield`s split it into resume blocks and `let`s create
    // their hoisted slots lazily (see `decl_slot`).
    b.position(body_start);
    for st in &f.body.stmts {
        b.stmt(st)?;
    }
    // Falling off the end exhausts the generator.
    b.ensure_br(done);

    // The exhaustion block: latch `$state = -1` and return `(zeroed, false)`.
    b.position(done);
    let neg1 = b.iconst(Ty::I64, -1);
    b.emit(Inst::FieldSet {
        base: 0,
        sid: gi.sid,
        idx: 0,
        val: neg1,
    });
    let tup = b.new_value(Ty::Struct(gi.next_tuple_sid));
    b.emit(Inst::StructNew {
        dest: tup,
        sid: gi.next_tuple_sid,
    });
    let no = b.bconst(false);
    b.emit(Inst::FieldSet {
        base: tup,
        sid: gi.next_tuple_sid,
        idx: 1,
        val: no,
    });
    b.set_term(Term::Ret(Some(tup)));

    // Fill the entry block: load every field into its slot, load `$state`, and
    // branch to the start / the right resume point / `done`.
    b.position(entry);
    let order = b.gen_entry.as_ref().unwrap().order.clone();
    for name in &order {
        let (slot, ty) = b.gen_entry.as_ref().unwrap().slots[name];
        let idx = b.gen_entry.as_ref().unwrap().field_idx[name];
        let v = b.new_value(ty);
        b.emit(Inst::FieldGet {
            dest: v,
            base: 0,
            sid: gi.sid,
            idx,
        });
        b.store(slot, v);
    }
    let state = b.new_value(Ty::I64);
    b.emit(Inst::FieldGet {
        dest: state,
        base: 0,
        sid: gi.sid,
        idx: 0,
    });
    let mut arms: Vec<(i64, BlockId)> = vec![(0, body_start)];
    arms.extend(b.gen_entry.as_ref().unwrap().resume.iter().copied());
    let mut cur_cmp = entry;
    let n = arms.len();
    for (i, (sval, target)) in arms.into_iter().enumerate() {
        b.position(cur_cmp);
        let kc = b.iconst(Ty::I64, sval);
        let cond = b.new_value(Ty::I1);
        b.emit(Inst::ICmp {
            dest: cond,
            pred: CmpPred::Eq,
            lhs: state,
            rhs: kc,
        });
        if i + 1 < n {
            let next_cmp = b.new_block();
            b.set_term(Term::BrIf {
                cond,
                then_bb: target,
                else_bb: next_cmp,
            });
            cur_cmp = next_cmp;
        } else {
            b.set_term(Term::BrIf {
                cond,
                then_bb: target,
                else_bb: done,
            });
        }
    }
    b.gen_entry = None;
    Ok(b.func)
}

/// The form of an iterator's `__next__` (D-0048), selected by its return
/// signature.
enum NextForm {
    /// Comma-ok `-> (T, bool)`: `tuple_sid` is the `(T, bool)` layout, `elem` is
    /// `T`. The preferred, exception-free form.
    CommaOk { tuple_sid: u32, elem: Ty },
    /// Python `-> T` that raises `StopIteration` at exhaustion; `elem` is `T`.
    StopIter { elem: Ty },
}

struct FnLower<'a> {
    func: Func,
    cur: BlockId,
    scopes: Vec<HashMap<String, (Value, Ty)>>,
    loops: Vec<LoopFrame>,
    term_set: HashSet<BlockId>, // blocks with a real terminator
    /// Stack of pending cleanups (innermost last), one per active `with`
    /// manager and per `try ... finally` (I#185, D-0069). The unwind path is
    /// handled by [`FnLower::landing_pads`]; this stack is walked only on the
    /// *structured* early-exit paths (`return`/`break`/`continue`) so
    /// `__exit__` / `finally` still run before the jump — closing the gap I#22
    /// deferred for `try/finally`. See [`FnLower::run_cleanups`].
    cleanups: Vec<Cleanup>,
    /// Stack of active landing-pad blocks (innermost `try` last). A call emitted
    /// while this is non-empty becomes an [`Inst::Invoke`] whose unwind edge goes
    /// to the top entry, so a raised exception transfers to the nearest handler.
    landing_pads: Vec<BlockId>,
    /// Landing-pad blocks actually targeted by an [`Inst::Invoke`]. A `try` whose
    /// body emitted no throwing call cannot raise, so its handler (and the
    /// `landingpad` that LLVM requires an `invoke` predecessor for) is omitted.
    used_lps: HashSet<BlockId>,
    /// Stack of in-flight exception objects, one per `except` handler body
    /// currently being lowered (innermost last). A bare `raise` (re-raise,
    /// #657) re-throws the top entry — the exception the dynamically-enclosing
    /// handler caught — preserving its type and identity. The stack is lexical
    /// and per function: a bare `raise` with an empty stack (outside any
    /// handler, or in a function merely called from one) is rejected at compile
    /// time with ZAP6002 (D-0116) rather than Python's runtime `RuntimeError`.
    active_exc: Vec<Value>,
    /// Stack of active `scope:` handles (innermost last), each the `ptr` value
    /// returned by `zap_scope_enter`. A `spawn` lowered while this is non-empty is
    /// owned by the top scope ([`Owner::Scope`]); a `spawn` at the root is
    /// [`Owner::Root`] (D-0044, structured concurrency).
    scope_stack: Vec<Value>,
    /// For each comprehension currently being lowered (innermost last), the
    /// length of `scopes` just before that comprehension pushed its per-clause
    /// scopes (#600). A walrus target inside a comprehension filter must bind in
    /// the scope containing the outermost enclosing comprehension (PEP 572), not
    /// in a per-clause scope that is popped before the comprehension's value is
    /// returned, so [`walrus_frame`](FnLower::walrus_frame) targets
    /// `comp_enclosing[0] - 1`. Empty outside any comprehension.
    comp_enclosing: Vec<usize>,
    /// Counter handing out distinct flag names for `with expect_raises(...)`
    /// desugaring (D-0054), so nested `with`s do not collide.
    with_counter: u32,
    /// Set while **discovering** a generator's live locals (I#67): each named
    /// binding is recorded here, first-seen first, as `(name, type)`. These
    /// become the state struct's fields. `None` outside generator discovery.
    gen_discover: Option<Vec<(String, Ty)>>,
    /// `yield` operand types seen during generator **discovery** (I#67); their
    /// join is the element type `T`. Empty otherwise.
    gen_elems: Vec<Ty>,
    /// Set while **emitting** a generator's `__next__` state machine (I#67).
    /// Carries the per-call working slots and the resume-point table.
    gen_entry: Option<GenEntry>,
    /// Per-function counter handing out unique synthetic names for a `for`
    /// loop's control state (induction index, cached iterable handle, loop-
    /// invariant bounds) when the loop is lowered inside a generator (I#453).
    /// Both generator passes — discovery and `__next__` emission — walk the
    /// body in the same order, so the names line up and the state-struct layout
    /// stays consistent. Only consulted inside a generator body.
    gen_loop_seq: u32,
    sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
    ctx: &'a StructCtx,
}

/// An active loop's jump targets plus the cleanup-stack depth when the loop
/// opened. `break`/`continue` run the cleanups pushed *inside* the loop (those
/// above `cleanup_base`) before branching (I#185).
struct LoopFrame {
    cont: BlockId,
    brk: BlockId,
    cleanup_base: usize,
}

/// A pending scope-exit action run on the structured early-exit paths
/// (`return`/`break`/`continue`) by [`FnLower::run_cleanups`] (I#185, D-0069).
/// The normal-completion and panic/unwind paths emit their cleanup inline, so
/// this enum is only consulted when a jump would otherwise skip it.
#[derive(Clone)]
enum Cleanup {
    /// A `with` manager: emit `exit_fn(mgr)` (the lowered `__exit__`), then the
    /// `@scoped` drop-glue — each owned `@scoped` field's `__exit__` in reverse
    /// declaration order (#876, D-0159). `mgr_sid` is the manager's struct id (for
    /// the field loads); `field_exits` is `(field_idx, field_sid, exit_symbol)` and
    /// is empty for every manager that owns no `@scoped` field.
    WithExit {
        mgr: Value,
        mgr_sid: u32,
        exit_fn: String,
        field_exits: Vec<(u32, u32, String)>,
    },
    /// A `try ... finally`: re-lower the `finally` block.
    Finally(ast::Block),
    /// Balance a landing-pad `__cxa_begin_catch` on a structured early exit.
    EndCatch,
    /// Leaving a structured concurrency region cancels and awaits its children.
    ScopeCancel(Value),
    /// Leaving a `with arena(...) as a:` block bulk-frees the arena's regions
    /// (#553, D-0158): emit `zap_arena_close(handle)`. Runs on every exit path —
    /// normal, exception unwind, and structured early-exit — like any `with`.
    ArenaClose(Value),
}

/// State threaded through the lowering of a generator's `__next__` (I#67). The
/// generator's live values live in the state struct's fields; each `__next__`
/// call loads them into per-call stack slots (allocas hoisted into the entry
/// block so they dominate every resume point), runs to the next `yield` (which
/// stores the slots back to the fields and returns `(value, true)`), and on
/// fall-off / bare `return` returns `(zeroed, false)`.
struct GenEntry {
    /// The `self` state-struct value (parameter 0 of `__next__`).
    self_val: Value,
    /// The state-struct id (for `FieldGet`/`FieldSet` on `self`).
    sid: u32,
    /// The entry block (block 0): hoisted allocas + field→slot loads + dispatch.
    entry_block: BlockId,
    /// The shared exhaustion block: sets `$state = -1` and returns `(_, false)`.
    done_block: BlockId,
    /// The `(T, bool)` tuple layout id returned by `__next__`.
    next_tuple_sid: u32,
    /// Element type `T` — the `yield` operand type (for coercing each operand).
    elem: Ty,
    /// Generator var name → its state-struct field index.
    field_idx: HashMap<String, u32>,
    /// Generator var name → `(per-call slot, type)`, in `order`.
    slots: HashMap<String, (Value, Ty)>,
    /// Var names in the order their slots were created (stable field-load/sync).
    order: Vec<String>,
    /// Next resume-state id to hand out (`0` is the start; `1..` are post-yield).
    next_state: i64,
    /// `(state id, resume block)` for every `yield`, used to build dispatch.
    resume: Vec<(i64, BlockId)>,
}

impl<'a> FnLower<'a> {
    fn new(
        name: String,
        params: Vec<Ty>,
        ret: Ty,
        is_main: bool,
        self_param: bool,
        sigs: &'a HashMap<String, (Vec<Ty>, Ty)>,
        ctx: &'a StructCtx,
    ) -> FnLower<'a> {
        let value_types = params.clone();
        let func = Func {
            name,
            params,
            ret,
            is_main,
            target_feature: None,
            self_param,
            value_types,
            blocks: vec![Block {
                id: 0,
                insts: Vec::new(),
                term: Term::Unreachable,
            }],
        };
        FnLower {
            func,
            cur: 0,
            scopes: vec![HashMap::new()],
            loops: Vec::new(),
            cleanups: Vec::new(),
            active_exc: Vec::new(),
            term_set: HashSet::new(),
            landing_pads: Vec::new(),
            used_lps: HashSet::new(),
            scope_stack: Vec::new(),
            comp_enclosing: Vec::new(),
            with_counter: 0,
            gen_discover: None,
            gen_elems: Vec::new(),
            gen_entry: None,
            gen_loop_seq: 0,
            sigs,
            ctx,
        }
    }

    /// Emit a call that may raise. Inside a `try` (a landing pad is active) this
    /// becomes an [`Inst::Invoke`] whose unwind edge targets the nearest handler;
    /// otherwise a plain [`Inst::Call`] (the exception, if any, unwinds through
    /// this frame to a caller's handler or to the top-level). The caller sets the
    /// terminator (e.g. `Unreachable` after `zap_panic`).
    /// Emit a dict/set runtime call that may raise on a **NaN float key/member**
    /// (#803, D-0138). A float dict KEY or set MEMBER operation (insert, lookup,
    /// `in`, `del`) canonicalizes and NaN-rejects in the runtime, so when the key
    /// kind is [`Elem::F64`] it may throw a catchable `ValueError` and must lower
    /// through [`throwing_call`] — an `invoke` to the active landing pad inside a
    /// `try`, a plain `call` otherwise. Every other key/member kind cannot raise
    /// here, so it stays an ordinary non-throwing `call`. (Key *read-back* —
    /// `zap_dict_keys_f64` — and float *values* never carry a NaN, so they are not
    /// routed here.)
    fn key_rt_call(&mut self, dest: Option<Value>, func: String, args: Vec<Value>, key: Elem) {
        if key == Elem::F64 {
            self.throwing_call(dest, &func, args);
        } else {
            self.emit(Inst::Call { dest, func, args });
        }
    }

    fn throwing_call(&mut self, dest: Option<Value>, func: &str, args: Vec<Value>) {
        if let Some(&lp) = self.landing_pads.last() {
            self.used_lps.insert(lp);
            self.emit(Inst::Invoke {
                dest,
                func: func.to_string(),
                args,
                unwind: lp,
            });
        } else {
            self.emit(Inst::Call {
                dest,
                func: func.to_string(),
                args,
            });
        }
    }

    /// Guard a member access against a `None` receiver (EXC-4, D-0007/D-0014):
    /// if `recv` (a reference value) is the null pointer, raise a catchable
    /// exception through `zap_panic` — an `invoke` inside a `try`, so `except`
    /// catches it, exactly like the index/divide faults (#383/EXC-3). On the
    /// non-null path execution falls through to the access. Callers apply this
    /// only to genuinely nullable reference receivers (not value structs).
    fn null_guard(&mut self, recv: Value, msg: &str) {
        let is_none = self.new_value(Ty::I1);
        self.emit(Inst::IsNull {
            dest: is_none,
            val: recv,
        });
        let throw_bb = self.new_block();
        let cont_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: is_none,
            then_bb: throw_bb,
            else_bb: cont_bb,
        });
        self.position(throw_bb);
        let m = self.new_value(Ty::Ptr);
        self.emit(Inst::StrConst {
            dest: m,
            bytes: msg.as_bytes().to_vec(),
        });
        let type_error = self.iconst(Ty::I64, 4);
        self.throwing_call(None, "zap_panic_typed", vec![type_error, m]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        self.position(cont_bb);
    }

    // --- builder primitives ---

    fn new_value(&mut self, ty: Ty) -> Value {
        let v = self.func.value_types.len() as Value;
        self.func.value_types.push(ty);
        v
    }

    fn new_block(&mut self) -> BlockId {
        let id = self.func.blocks.len() as BlockId;
        self.func.blocks.push(Block {
            id,
            insts: Vec::new(),
            term: Term::Unreachable,
        });
        id
    }

    fn position(&mut self, b: BlockId) {
        self.cur = b;
    }

    fn emit(&mut self, inst: Inst) {
        self.func.blocks[self.cur as usize].insts.push(inst);
    }

    fn set_term(&mut self, t: Term) {
        self.func.blocks[self.cur as usize].term = t;
        self.term_set.insert(self.cur);
    }

    fn terminated(&self) -> bool {
        self.term_set.contains(&self.cur)
    }

    fn ensure_br(&mut self, dest: BlockId) {
        if !self.terminated() {
            self.set_term(Term::Br(dest));
        }
    }

    /// Open a loop frame, recording the current cleanup-stack depth so
    /// `break`/`continue` know which cleanups are lexically inside the loop.
    fn push_loop(&mut self, cont: BlockId, brk: BlockId) {
        let cleanup_base = self.cleanups.len();
        self.loops.push(LoopFrame {
            cont,
            brk,
            cleanup_base,
        });
    }

    /// Emit the cleanup actions for the active `with`/`try-finally` entries from
    /// the top of the stack down to (not including) `down_to`, innermost first
    /// (I#185, D-0069). Run on the structured early-exit paths so `__exit__` /
    /// `finally` still execute before a `return`/`break`/`continue`.
    ///
    /// `landing_pads` is cleared for the duration, so a cleanup call here is a
    /// plain `Call` rather than an `Invoke` back into its own (or a sibling's)
    /// landing pad — which would double-run the cleanup. v1 consequence: if a
    /// cleanup *itself* raises on an early-exit path it propagates without
    /// running the remaining pending cleanups (lock/file `__exit__` never raise,
    /// so the motivating cases are unaffected; full chaining is a follow-up).
    /// While running entry `i` only the strictly-outer entries stay active, so a
    /// nested early exit inside a `finally` runs the outer cleanups correctly; if
    /// a cleanup diverts control (e.g. a `return` inside a `finally`) the walk
    /// stops.
    fn run_cleanups(&mut self, down_to: usize) -> Result<(), LowerError> {
        if self.cleanups.len() <= down_to {
            return Ok(());
        }
        let saved = self.cleanups.clone();
        let saved_lps = std::mem::take(&mut self.landing_pads);
        let mut i = saved.len();
        while i > down_to {
            i -= 1;
            self.cleanups.truncate(i);
            match &saved[i] {
                Cleanup::WithExit {
                    mgr,
                    mgr_sid,
                    exit_fn,
                    field_exits,
                } => {
                    let (mgr, mgr_sid, exit_fn, field_exits) =
                        (*mgr, *mgr_sid, exit_fn.clone(), field_exits.clone());
                    self.throwing_call(None, &exit_fn, vec![mgr]);
                    self.emit_scoped_field_teardown(mgr, mgr_sid, &field_exits);
                }
                Cleanup::Finally(block) => {
                    let block = block.clone();
                    self.lower_block(&block)?;
                }
                Cleanup::EndCatch => self.emit_end_catch(),
                Cleanup::ScopeCancel(handle) => {
                    self.emit(Inst::Call {
                        dest: None,
                        func: "zap_scope_exit_cancel".to_string(),
                        args: vec![*handle],
                    });
                }
                Cleanup::ArenaClose(handle) => {
                    self.emit(Inst::Call {
                        dest: None,
                        func: "zap_arena_close".to_string(),
                        args: vec![*handle],
                    });
                }
            }
            if self.terminated() {
                break;
            }
        }
        self.cleanups = saved;
        self.landing_pads = saved_lps;
        Ok(())
    }

    /// `@scoped` auto-teardown drop-glue (#876, D-0159): for a `with` manager that
    /// owns `@scoped` fields, run each field's `__exit__` in **reverse declaration
    /// order** (last-constructed first), after the owner's own `__exit__`. Each
    /// call loads the owned field value from the manager and invokes its
    /// `__exit__`; a throwing teardown routes to the enclosing manager/`try` like
    /// any other cleanup. A no-op when `field_exits` is empty.
    fn emit_scoped_field_teardown(
        &mut self,
        mgr: Value,
        mgr_sid: u32,
        field_exits: &[(u32, u32, String)],
    ) {
        for (idx, fsid, exit_fn) in field_exits.iter().rev() {
            if self.terminated() {
                break;
            }
            let fv = self.new_value(Ty::Struct(*fsid));
            self.emit(Inst::FieldGet {
                dest: fv,
                base: mgr,
                sid: mgr_sid,
                idx: *idx,
            });
            self.throwing_call(None, exit_fn, vec![fv]);
        }
    }

    // scopes
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    fn declare(&mut self, name: &str, ptr: Value, ty: Ty) {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), (ptr, ty));
    }
    fn lookup(&self, name: &str) -> Option<(Value, Ty)> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Resolve a name to a storable lvalue slot (pointer + type): a local's
    /// `alloca` if one is in scope, else a module-level global's slot (D-0112).
    /// For a global it emits a `GlobalAddr`, mirroring the read path in `expr`'s
    /// `Expr::Name` global fallback; the returned pointer is stored into exactly
    /// like a local slot, so a mutable module global (#662 Phase 2) is reassigned
    /// through its data-segment slot — which the GC already scans as a root. A
    /// `const` is also resolvable here, but sema has already rejected reassigning
    /// one (ZAP3017), so a store only ever reaches a mutable binding. Returns
    /// `None` for an unknown name (the caller reports it).
    fn name_lvalue(&mut self, name: &str) -> Option<(Value, Ty)> {
        if let Some(slot) = self.lookup(name) {
            return Some(slot);
        }
        if let Some((idx, ty)) = self.ctx.global(name) {
            let slot = self.new_value(Ty::Ptr);
            self.emit(Inst::GlobalAddr { dest: slot, idx });
            return Some((slot, ty));
        }
        None
    }

    /// Store `v` into the lvalue `slot` resolved for the assignment target
    /// `name`, emitting the module-global write barrier (#663, epic #661) when
    /// `name` reassigns a *mutable* module-level *pointer* global.
    ///
    /// A module-global pointer slot lives in the static data segment for the
    /// process lifetime and is a permanent GC root (registered with
    /// `zap_gc_add_root` by `@__zap_init_globals`). Overwriting it installs a new
    /// old→new edge out of an immortal root. Under the precise Immix collector
    /// (D-0009) a future generational/incremental mode needs that edge recorded
    /// so a minor cycle that does not re-scan every static root stays correct; a
    /// post-write barrier on the slot (`zap_gc_write_barrier(&slot)`) is the
    /// record point. Today's stop-the-world collector re-scans all roots each
    /// cycle, so the barrier is a linkable runtime no-op (and an exported no-op
    /// under conservative Boehm) — the hook, not its payload, is what #663 lands.
    ///
    /// The barrier is emitted for pointer-typed *global* stores only:
    /// - a local slot holds no permanent-root edge (it is scanned precisely via
    ///   the shadow stack), so it needs none;
    /// - a scalar global holds no pointer, so there is no edge to record;
    /// - an immutable reference `const` is stored exactly once, at init, and is
    ///   never reassigned (ZAP3017), so a reassignment store that reaches here is
    ///   a mutable binding by construction — and the one-time init store in
    ///   `lower_init_globals` deliberately does not route through here, keeping
    ///   startup consistent (no barrier before the collector/root table is live).
    ///
    /// Under `--race` (#677, epic #665) the same store is also a *write* access of
    /// a shared global, so a `__zap_race_write` hook is emitted via
    /// [`FnLower::emit_race_access`] — but on a BROADER predicate than the GC
    /// barrier: the detector must catch a racing SCALAR global too (the canonical
    /// `int64` counter), not just a pointer one, so the race hook fires for any
    /// mutable non-`@shareable` global, independent of `ty.is_ptr()`. `span` is
    /// the assignment target's source span, carried to the hook for the engine
    /// (slice 2) to report the racing site.
    fn store_name(&mut self, name: &str, slot: Value, ty: Ty, v: Value, span: Span) {
        self.store(slot, v);
        if ty.is_ptr() && self.lookup(name).is_none() && self.ctx.global(name).is_some() {
            self.emit(Inst::Call {
                dest: None,
                func: "zap_gc_write_barrier".to_string(),
                args: vec![slot],
            });
        }
        self.emit_race_access(name, slot, true, span);
    }

    /// Emit a runtime data-race access hook (`--race`, #677, epic #665, D-0114)
    /// for a read (`write == false`) or write (`write == true`) of the name
    /// `name` through its resolved slot `slot`, when — and only when — the access
    /// is in the ZAP5125 sharing-lint set (#664), so instrumentation scope
    /// exactly matches lint scope:
    /// - `--race` is on (otherwise nothing is emitted, keeping a non-`--race`
    ///   build byte-identical / golden-safe);
    /// - the access is NOT in the single-threaded module-init pre-pass
    ///   (`@__zap_init_globals`), which runs before any coroutine exists;
    /// - `name` is not shadowed by a local (`lookup` resolves to a local → skip,
    ///   mirroring sema's `resolves_to_module_global`);
    /// - `name` is a mutable, non-`@shareable` module global ([`racy_global_idx`]
    ///   returns its slot index) — a `const` or a `@shareable` cell is excluded.
    ///
    /// The emitted call is `__zap_race_{read,write}(slot, name_id, line, col)`
    /// (the same `Inst::Call` mechanism #663 uses for `zap_gc_write_barrier`),
    /// where `name_id` is the global's slot index (a stable per-global id) and
    /// `line`/`col` carry the access span's byte range for the engine (slice 2)
    /// to resolve to a source location. The runtime entry points are no-op stubs
    /// in this slice; the happens-before engine fills them in (#678).
    fn emit_race_access(&mut self, name: &str, slot: Value, write: bool, span: Span) {
        if !self.ctx.race || self.func.name == INIT_GLOBALS_FN {
            return;
        }
        if self.lookup(name).is_some() {
            return; // shadowed by a local — not the shared module global
        }
        let Some(idx) = self.ctx.racy_global_idx(name) else {
            return; // not a global, or a `const` / `@shareable` one
        };
        let name_id = self.iconst(Ty::I64, idx as i64);
        let line = self.iconst(Ty::I64, span.start as i64);
        let col = self.iconst(Ty::I64, span.end as i64);
        let func = if write {
            "__zap_race_write"
        } else {
            "__zap_race_read"
        };
        self.emit(Inst::Call {
            dest: None,
            func: func.to_string(),
            args: vec![slot, name_id, line, col],
        });
    }

    /// Emit a heap data-race access hook for a **struct field** (`--race`, #684,
    /// slice 4, epic #665, D-0114): a read (`write == false`) or write of field
    /// `idx` (named `fname`) of struct `sid` reached through the object pointer
    /// `base`. Emitted only under `--race`, never in the single-threaded module
    /// -init pre-pass, and only for a REFERENCE struct — a value struct is inline
    /// (stack / embedded) and cannot be shared by reference across coroutines, so
    /// it never races and is skipped (also avoiding a shadow entry keyed on a
    /// stack address). The conflict key the runtime forms is the pair
    /// (`base` object header, field index), which is relocation-safe: the precise,
    /// marked-in-place struct header is never moved by the Immix evacuator (#120).
    /// The call is `__zap_race_{read,write}_field(base, idx, "Type.field", line,
    /// col)`; every call carries the `__zap_race_` prefix, so the "race ZIR minus
    /// the race hooks == off ZIR" byte-identity invariant still holds.
    fn emit_race_field(
        &mut self,
        base: Value,
        sid: u32,
        idx: u32,
        fname: &str,
        write: bool,
        span: Span,
    ) {
        if !self.ctx.race || self.func.name == INIT_GLOBALS_FN {
            return;
        }
        if self.ctx.is_value_struct(sid) {
            return; // inline value struct — not a shared heap object
        }
        if self.ctx.name_is_shareable(&self.ctx.names[sid as usize]) {
            // A `@shareable` / built-in-shareable struct (`Atomic`, `Mutex`,
            // `Counter`, `Queue`, …) is an asserted-safe shared type, so — exactly
            // as for a `@shareable` module global (the ZAP5125 exemption) — its
            // field accesses are NOT instrumented; an `Atomic`/`concurrent`-cell
            // guarded version stays clean by construction (#664, D-0113).
            return;
        }
        let label = format!("{}.{}", self.ctx.names[sid as usize], fname);
        let name = self.strconst(&label);
        let sub = self.iconst(Ty::I64, idx as i64);
        let line = self.iconst(Ty::I64, span.start as i64);
        let col = self.iconst(Ty::I64, span.end as i64);
        let func = if write {
            "__zap_race_write_field"
        } else {
            "__zap_race_read_field"
        };
        self.emit(Inst::Call {
            dest: None,
            func: func.to_string(),
            args: vec![base, sub, name, line, col],
        });
    }

    /// Emit a heap data-race access hook for a **container element** (`--race`,
    /// #684, slice 4): a read/write of element `index` of the container reached
    /// through `base`, described to the report by `label` (e.g. `list element`).
    /// Emitted only under `--race` and never in the module-init pre-pass. The
    /// conflict key is the pair (`base` container header, `index`) — keyed on the
    /// stable header identity, so a relocation of the container's `zap_desc_atomic`
    /// backing buffer (the only thing the evacuator moves) is transparent to the
    /// shadow state. `index` is the element index value as already coerced to
    /// `i64` at the call site (its raw, pre-normalization form — a negative index
    /// and its wrapped positive are distinct keys, a documented v1 granularity
    /// limit). Call shape mirrors [`Self::emit_race_field`].
    fn emit_race_elem(&mut self, base: Value, index: Value, label: &str, write: bool, span: Span) {
        if !self.ctx.race || self.func.name == INIT_GLOBALS_FN {
            return;
        }
        let name = self.strconst(label);
        let line = self.iconst(Ty::I64, span.start as i64);
        let col = self.iconst(Ty::I64, span.end as i64);
        let func = if write {
            "__zap_race_write_field"
        } else {
            "__zap_race_read_field"
        };
        self.emit(Inst::Call {
            dest: None,
            func: func.to_string(),
            args: vec![base, index, name, line, col],
        });
    }

    /// The scope-stack index a fresh walrus binding (`name := expr`, #600) must
    /// be declared into so it outlives the construct it appears in. Outside any
    /// comprehension this is the current innermost scope — which, for an
    /// `if`/`while` condition, is already the enclosing block (conditions are
    /// lowered before the body scope is pushed), so the binding survives. Inside
    /// a comprehension it is the scope containing the outermost comprehension
    /// (PEP 572): every per-clause scope is popped before the comprehension's
    /// value is returned, so a walrus target must bind below them.
    fn walrus_frame(&self) -> usize {
        match self.comp_enclosing.first() {
            Some(&len) => len.saturating_sub(1),
            None => self.scopes.len().saturating_sub(1),
        }
    }

    /// Allocate and declare the stack slot for a named local binding (`let`,
    /// `auto`), returning the slot to store the initializer into.
    ///
    /// Outside a generator this is just `alloca` + `declare`. While **discovering**
    /// a generator's locals (I#67) it also records `(name, type)` so the name
    /// becomes a state-struct field. While **emitting** a generator's `__next__`
    /// it reuses the per-call slot created once in the entry block (so the binding
    /// persists across `yield`s via its field, and the alloca dominates every
    /// resume point); the first binding of each name creates that hoisted slot.
    fn decl_slot(&mut self, name: &str, ty: Ty) -> Value {
        if self.gen_discover.is_some() {
            if let Some(list) = self.gen_discover.as_mut() {
                if !list.iter().any(|(n, _)| n == name) {
                    list.push((name.to_string(), ty));
                }
            }
            let slot = self.alloca(ty);
            self.declare(name, slot, ty);
            return slot;
        }
        if self.gen_entry.is_some() {
            if let Some(&(slot, _)) = self.gen_entry.as_ref().unwrap().slots.get(name) {
                // Rebind the name to the existing hoisted slot (idempotent).
                self.declare(name, slot, ty);
                return slot;
            }
            return self.gen_entry_slot(name, ty);
        }
        let slot = self.alloca(ty);
        self.declare(name, slot, ty);
        slot
    }

    /// Create the per-call working slot for generator var `name` in the entry
    /// block (hoisted so it dominates every resume point), declare it, and record
    /// it for the entry field→slot load and the per-`yield` slot→field sync.
    fn gen_entry_slot(&mut self, name: &str, ty: Ty) -> Value {
        let entry = self.gen_entry.as_ref().unwrap().entry_block;
        let d = self.new_value(Ty::Ptr);
        self.func.blocks[entry as usize].insts.push(Inst::Alloca {
            dest: d,
            pointee: ty,
        });
        self.declare(name, d, ty);
        let ge = self.gen_entry.as_mut().unwrap();
        ge.slots.insert(name.to_string(), (d, ty));
        ge.order.push(name.to_string());
        d
    }

    /// Whether lowering is currently inside a generator body — in either the
    /// discovery pass (`gen_discover`) or the `__next__` emission pass
    /// (`gen_entry`). A `for` loop's control state must then live in hoisted
    /// state-struct slots so it survives a `yield` in the loop body (I#453).
    fn in_generator_body(&self) -> bool {
        self.gen_discover.is_some() || self.gen_entry.is_some()
    }

    /// A slot for a synthesized `for`-loop control value — the induction index
    /// or a spilled loop-invariant (the iterable handle, a `range` bound).
    /// Inside a generator this is a **hoisted** slot (reloaded at each resume
    /// entry and synced back at every `yield`, exactly like an `auto` local),
    /// under a synthetic name that is deterministic across the discovery and
    /// emission passes; elsewhere it is a plain `alloca` (unchanged behaviour,
    /// no extra state). This is why a `for` loop in a generator body did not
    /// survive suspend/resume before I#453: its index was a per-call `alloca`
    /// that read uninitialized on resume, and its iterable handle was an SSA
    /// value defined at loop setup that did not dominate the resume re-entry.
    fn for_control_slot(&mut self, ty: Ty) -> Value {
        if self.in_generator_body() {
            let n = self.gen_loop_seq;
            self.gen_loop_seq += 1;
            let name = format!("$for:{n}");
            self.decl_slot(&name, ty)
        } else {
            self.alloca(ty)
        }
    }

    /// Spill a loop-invariant SSA value (the iterable handle, a `range`
    /// bound/step, a string length) so it can be reloaded across a `yield`
    /// resume when the loop is lowered inside a generator: store it into a
    /// hoisted [`for_control_slot`] and return `Some(slot)`. Outside a generator
    /// no spill is needed and this returns `None` (the caller keeps using the
    /// SSA value directly). See [`reload_invariant`]. (I#453)
    fn spill_invariant(&mut self, v: Value, ty: Ty) -> Option<Value> {
        if self.in_generator_body() {
            let slot = self.for_control_slot(ty);
            self.store(slot, v);
            Some(slot)
        } else {
            None
        }
    }

    /// Reload a value spilled by [`spill_invariant`]: inside a generator load it
    /// from its hoisted slot (so the current block, which may be a resume
    /// re-entry, sees a dominating definition); otherwise return the original
    /// SSA value. (I#453)
    fn reload_invariant(&mut self, slot: Option<Value>, orig: Value, ty: Ty) -> Value {
        match slot {
            Some(s) => self.load(s, ty),
            None => orig,
        }
    }

    // instruction helpers
    fn alloca(&mut self, pointee: Ty) -> Value {
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Alloca { dest: d, pointee });
        d
    }
    fn store(&mut self, ptr: Value, val: Value) {
        self.emit(Inst::Store { ptr, val });
    }
    fn load(&mut self, ptr: Value, ty: Ty) -> Value {
        let d = self.new_value(ty);
        self.emit(Inst::Load { dest: d, ptr });
        d
    }

    // --- value structs (D-0051) ---

    /// If `v : ty` is a `value struct` obtained from an existing object
    /// (`src` is an lvalue read), copy it so value semantics hold: the result is
    /// an independent instance whose later mutation does not escape to `src`
    /// (and vice-versa). A freshly produced value (constructor/call/operator
    /// result) is already uniquely owned, so it is returned unchanged. Reference
    /// structs (D-0023) are always returned unchanged — they share identity.
    /// This is also the *boxing* copy used when a value struct is stored into a
    /// collection or another aggregate (D-0051): the stored copy is independent.
    fn copy_if_value(&mut self, v: Value, ty: Ty, src: &ast::Expr) -> Value {
        if let Ty::Struct(sid) = ty {
            if self.ctx.is_value_struct(sid) && is_lvalue_read(src) {
                return self.copy_value_struct(v, sid);
            }
        }
        v
    }

    /// Like [`copy_if_value`], but for a value crossing a **by-value calling
    /// boundary** — a function/method argument, a `return`, or a `spawn`
    /// argument. For an [`inline-eligible`](StructCtx::inline_eligible) aggregate
    /// (a flat value struct or tuple) no explicit copy is emitted: the
    /// register/sret ABI passes/returns it **by value**, which is itself an
    /// independent copy (D-0051), so a prior copy would be redundant and would
    /// reintroduce the heap allocation the inline layout removes. Non-inline
    /// value structs (those with a nested value-aggregate field, still a heap
    /// pointer) fall back to [`copy_if_value`]'s deep copy, preserving the
    /// observable value semantics unchanged.
    fn copy_for_pass(&mut self, v: Value, ty: Ty, src: &ast::Expr) -> Value {
        if let Ty::Struct(sid) = ty {
            if self.ctx.inline_eligible(sid) {
                return v;
            }
        }
        self.copy_if_value(v, ty, src)
    }

    /// Lower a call that carries `*`/`**` spread arguments (#597), or targets a
    /// callee with a variadic `TYPE... name` (#1218) or a `V **name`
    /// keyword-collector (#601) parameter.
    ///
    /// The call's arguments are assembled into the callee's declaration order:
    /// explicit positionals (and the elements of a statically-sized `*tuple`)
    /// bind to the fixed parameters in order; any overflow plus a runtime-length
    /// `*list`/`*set` spread are packed into the variadic `TYPE... name` list;
    /// keyword arguments bind to their named parameter, and any left over — plus
    /// the entries of a `**dict` spread — are collected into the `V **name`
    /// dict. Sema (`check_user_call_spread`) has already validated the shapes;
    /// the errors here are a defensive backstop.
    ///
    /// `recv` is the already-lowered `self` of a method call (#1218): when
    /// present it is forwarded as the leading argument and the callee's recorded
    /// `call_params` (which drop the implicit `self`, so index `pi` maps to the
    /// declared type `ptys[pi + 1]`) are indexed with that `+1` offset. For a
    /// free function `recv` is `None` and the offset is `0`.
    fn lower_call_assembled(
        &mut self,
        fname: &str,
        recv: Option<Value>,
        ptys: &[Ty],
        ret: Ty,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let ctx = self.ctx;
        // Declared-type offset for the implicit `self` of a method call: the
        // callee's parameter metadata excludes `self`, but `ptys` includes it.
        let off = usize::from(recv.is_some());
        let pinfo = ctx.call_params.get(fname).cloned().unwrap_or_default();
        let n = pinfo.len();
        let var_idx = pinfo.iter().position(|p| p.variadic);
        let kw_idx = pinfo.iter().position(|p| p.kwargs);
        // Fixed (positional-capacity) parameter indices, in order. The variadic
        // and the collector, when present, are always last.
        let fixed: Vec<usize> = (0..n)
            .filter(|&i| !pinfo[i].variadic && !pinfo[i].kwargs)
            .collect();

        // Classify the call's arguments. A `*tuple` is expanded into values now
        // (its arity is static); a `*list`/`*set` is kept as one evaluated value
        // to feed the variadic. `**dict` and keyword args feed the collector.
        let mut pos_srcs: Vec<PosSrc> = Vec::new();
        let mut seq_spread: Option<(Value, Ty)> = None;
        let mut kw_args: Vec<(&str, &Expr)> = Vec::new();
        let mut map_spread: Option<&ast::Arg> = None;
        for a in args {
            match a.spread {
                ast::ArgSpread::None => match &a.name {
                    Some(nm) => kw_args.push((nm.as_str(), &a.value)),
                    None => pos_srcs.push(PosSrc::Expr(&a.value)),
                },
                ast::ArgSpread::Iterable => {
                    let (sv, sty) = self.expr(&a.value)?;
                    let tuple_fields = match sty {
                        Ty::Struct(sid) => ctx.tuples.borrow().fields(sid).map(|f| (sid, f)),
                        _ => None,
                    };
                    if let Some((sid, fields)) = tuple_fields {
                        for (k, fty) in fields.iter().enumerate() {
                            let d = self.new_value(*fty);
                            self.emit(Inst::FieldGet {
                                dest: d,
                                base: sv,
                                sid,
                                idx: k as u32,
                            });
                            pos_srcs.push(PosSrc::Val(d, *fty));
                        }
                    } else {
                        seq_spread = Some((sv, sty));
                    }
                }
                ast::ArgSpread::Mapping => map_spread = Some(a),
            }
        }

        // Bind positionals to fixed parameters; keywords fill the rest.
        let n_fixed_pos = pos_srcs.len().min(fixed.len());
        let mut fixed_kw: Vec<Option<&Expr>> = vec![None; fixed.len()];
        let mut kw_for_collector: Vec<(&str, &Expr)> = Vec::new();
        for (nm, e) in &kw_args {
            if let Some(fi) = fixed.iter().position(|&pi| pinfo[pi].name == *nm) {
                if fi < n_fixed_pos || fixed_kw[fi].is_some() {
                    return Err(err(format!(
                        "'{fname}' got multiple values for parameter '{nm}'"
                    )));
                }
                fixed_kw[fi] = Some(e);
            } else {
                kw_for_collector.push((nm, e));
            }
        }

        // Assemble one value per parameter, in declaration order. A method
        // call's `self` leads the argument vector (#1218).
        let mut argvals: Vec<Value> = Vec::with_capacity(n + off);
        argvals.extend(recv);
        for pi in 0..n {
            if Some(pi) == var_idx {
                let overflow = &pos_srcs[n_fixed_pos..];
                let v = self.build_variadic_list(ptys[pi + off], overflow, seq_spread)?;
                argvals.push(v);
            } else if Some(pi) == kw_idx {
                let v = self.build_kwargs_dict(ptys[pi + off], &kw_for_collector, map_spread)?;
                argvals.push(v);
            } else {
                let fi = fixed.iter().position(|&x| x == pi).unwrap();
                let target = ptys[pi + off];
                let v = if fi < n_fixed_pos {
                    self.lower_pos_src(&pos_srcs[fi], target)?
                } else if let Some(e) = fixed_kw[fi] {
                    let v = self.expr_coerced(e, target, zapc_diag::ErrorCode::ZAP4021)?;
                    self.copy_for_pass(v, target, e)
                } else if let Some(def) = pinfo[pi].default.as_ref() {
                    let v = self.expr_coerced(def, target, zapc_diag::ErrorCode::ZAP4021)?;
                    self.copy_for_pass(v, target, def)
                } else {
                    return Err(err(format!(
                        "missing argument for parameter '{}' of '{fname}'",
                        pinfo[pi].name
                    )));
                };
                argvals.push(v);
            }
        }

        let dest = if ret == Ty::Void {
            None
        } else {
            Some(self.new_value(ret))
        };
        self.throwing_call(dest, fname, argvals);
        let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
        Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }))
    }

    /// Lower one positional source (#597) coerced to the parameter type.
    fn lower_pos_src(&mut self, src: &PosSrc, target: Ty) -> Result<Value, LowerError> {
        match src {
            PosSrc::Expr(e) => {
                let v = self.expr_coerced(e, target, zapc_diag::ErrorCode::ZAP4021)?;
                Ok(self.copy_for_pass(v, target, e))
            }
            PosSrc::Val(v, vt) => {
                let v = self.coerce(*v, *vt, target);
                Ok(self.box_if_value(v, target))
            }
        }
    }

    /// Build the variadic parameter's `list` (#597): push the overflow
    /// positional sources, then extend from a runtime-length `*list`/`*set`.
    fn build_variadic_list(
        &mut self,
        list_ty: Ty,
        overflow: &[PosSrc],
        seq: Option<(Value, Ty)>,
    ) -> Result<Value, LowerError> {
        let elem = match list_ty {
            Ty::List(e) => e,
            _ => Elem::I64,
        };
        let list = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let push = format!("zap_list_push_{}", elem.val_suffix());
        for src in overflow {
            let v = match src {
                PosSrc::Expr(e) => {
                    let (v, vt) = self.expr(e)?;
                    self.coerce(v, vt, elem.ty())
                }
                PosSrc::Val(v, vt) => self.coerce(*v, *vt, elem.ty()),
            };
            let v = self.box_if_value(v, elem.ty());
            self.emit(Inst::Call {
                dest: None,
                func: push.clone(),
                args: vec![list, v],
            });
        }
        if let Some((sv, sty)) = seq {
            let items = match sty {
                Ty::List(_) => sv,
                Ty::Set(se) => {
                    let items = self.new_value(Ty::List(se));
                    self.emit(Inst::Call {
                        dest: Some(items),
                        func: "zap_set_items".to_string(),
                        args: vec![sv],
                    });
                    items
                }
                other => {
                    return Err(err(format!(
                        "cannot `*`-spread a value of type {other:?} into a variadic parameter"
                    )))
                }
            };
            let se = match sty {
                Ty::List(e) | Ty::Set(e) => e,
                _ => elem,
            };
            self.iterate_list(items, se, |s, v, vt| {
                let v = s.coerce(v, vt, elem.ty());
                let v = s.box_if_value(v, elem.ty());
                s.emit(Inst::Call {
                    dest: None,
                    func: push.clone(),
                    args: vec![list, v],
                });
                Ok(())
            })?;
        }
        Ok(list)
    }

    /// Build the keyword-collector's `dict[str, V]` (#601): copy a `**dict`
    /// spread if present, then set each remaining keyword argument.
    fn build_kwargs_dict(
        &mut self,
        dict_ty: Ty,
        kws: &[(&str, &Expr)],
        map_spread: Option<&ast::Arg>,
    ) -> Result<Value, LowerError> {
        let vk = match dict_ty {
            Ty::Dict(_, v) => v,
            _ => Elem::Ptr,
        };
        let dict = self.new_value(Ty::Dict(Elem::Ptr, vk));
        if let Some(a) = map_spread {
            let (sv, _) = self.expr(&a.value)?;
            self.emit(Inst::Call {
                dest: Some(dict),
                func: "zap_dict_copy".to_string(),
                args: vec![sv],
            });
        } else {
            self.emit(Inst::Call {
                dest: Some(dict),
                func: "zap_dict_new".to_string(),
                args: vec![],
            });
        }
        let setf = format!("zap_dict_set_str_{}", vk.val_suffix());
        for (nm, e) in kws {
            let key = self.new_value(Ty::Ptr);
            self.emit(Inst::StrConst {
                dest: key,
                bytes: nm.as_bytes().to_vec(),
            });
            let (v, vt) = self.expr(e)?;
            let v = self.coerce(v, vt, vk.ty());
            let v = self.box_if_value(v, vk.ty());
            self.emit(Inst::Call {
                dest: None,
                func: setf.clone(),
                args: vec![dict, key, v],
            });
        }
        Ok(dict)
    }

    /// Box a value struct for storage in a collection or another aggregate
    /// (D-0051): a value struct stored in a list/dict/set is *boxed* — copied to
    /// the heap — so the stored element is independent of the original and later
    /// mutation of either does not affect the other. Unlike [`copy_if_value`],
    /// this copies unconditionally (the collection always owns its own instance),
    /// matching D-0051's "stored in a collection … is boxed (copied to the
    /// heap)". Non-value types pass through unchanged.
    fn box_if_value(&mut self, v: Value, ty: Ty) -> Value {
        if let Ty::Struct(sid) = ty {
            if self.ctx.is_value_struct(sid) {
                return self.copy_value_struct(v, sid);
            }
        }
        v
    }

    /// The interface fat-pointer struct id when `elem` is an interface-typed
    /// collection element, else `None`. An interface element flows as a
    /// `Ty::Struct(fat_sid)` word (`Elem::Struct(fat_sid)`), distinguished from
    /// an ordinary struct element by having a registered interface id.
    fn iface_elem(&self, elem: Elem) -> Option<u32> {
        match elem.ty() {
            Ty::Struct(sid) => self.ctx.iid_of_fat_sid(sid).map(|_| sid),
            _ => None,
        }
    }

    /// Box a value for storage in a collection element word (I#84/#829). Like
    /// [`box_if_value`], a value struct is copied to the heap (D-0051); in
    /// addition, an **interface value** — physically a two-word `{data, itable}`
    /// fat pointer built on a stack alloca — is copied into a heap cell so the
    /// element word is a stable, GC-traced pointer to the pair instead of a
    /// dangling stack address once the building frame returns. (A struct *field*
    /// of interface type is laid out inline-in-aggregate and so needs no box;
    /// only a uniform one-word collection cell does.) The heap cell is a
    /// pointer-free-leaf-safe copy: its `data` word is a heap pointer the tracer
    /// follows, and its `itable` word is a static global the precise tracer
    /// skips as a non-heap pointer. Read-back reconstitutes the same
    /// `Ty::Struct(fat_sid)` the dispatch (`IfaceCall`) and render paths expect.
    fn box_elem_for_store(&mut self, v: Value, ty: Ty) -> Value {
        if let Ty::Struct(sid) = ty {
            if self.ctx.iid_of_fat_sid(sid).is_some() {
                return self.copy_value_struct(v, sid);
            }
        }
        self.box_if_value(v, ty)
    }

    /// Does a value of type `vt` satisfy an interface-typed collection element of
    /// kind `want` by structural conformance (#829)? True when `want` is an
    /// interface element and `vt` is a concrete conformer source — a plain struct
    /// (reference or value, or a tier-3 enum's tagged struct) or a tier-1 enum —
    /// that is not itself an interface fat pointer (interface -> interface
    /// widening is a separate, unsupported conversion). Sema has already verified
    /// that `vt` actually conforms; this only recognizes the *shape* of a
    /// coercible source so [`coerce`] can box it via `Inst::IfaceMake`.
    fn conforms_to_iface_elem(&self, vt: Ty, want: Elem, actual: Elem) -> bool {
        self.iface_elem(want).is_some()
            && self.iface_elem(actual).is_none()
            && matches!(vt, Ty::Struct(_) | Ty::Enum(_))
    }

    /// A field-wise copy of value struct `sid` at `src` into a fresh instance.
    /// Nested value-struct fields are copied recursively (deep copy, so a nested
    /// value aggregate is independent too); reference-typed fields — including
    /// reference structs, strings, and collections — are shared (the pointer is
    /// copied), matching D-0023 reference semantics and "a value struct holding
    /// references is still GC-scanned".
    fn copy_value_struct(&mut self, src: Value, sid: u32) -> Value {
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        let fields = self.ctx.fields[sid as usize].clone();
        for (i, (_, fty)) in fields.iter().enumerate() {
            let idx = i as u32;
            let fv = self.new_value(*fty);
            self.emit(Inst::FieldGet {
                dest: fv,
                base: src,
                sid,
                idx,
            });
            let fv = match *fty {
                Ty::Struct(fsid) if self.ctx.is_value_struct(fsid) => {
                    self.copy_value_struct(fv, fsid)
                }
                _ => fv,
            };
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx,
                val: fv,
            });
        }
        dest
    }
    fn iconst(&mut self, ty: Ty, val: i64) -> Value {
        let d = self.new_value(ty);
        self.emit(Inst::IConst { dest: d, val });
        d
    }
    fn fconst(&mut self, ty: Ty, val: f64) -> Value {
        let d = self.new_value(ty);
        self.emit(Inst::FConst { dest: d, val });
        d
    }
    fn bconst(&mut self, val: bool) -> Value {
        let d = self.new_value(Ty::I1);
        self.emit(Inst::BConst { dest: d, val });
        d
    }

    // --- expressions ---

    fn expr(&mut self, e: &Expr) -> Result<(Value, Ty), LowerError> {
        match e {
            Expr::Int(s, _) => {
                if let Some(v) = parse_int_lit_opt(s) {
                    Ok((self.iconst(Ty::I64, v), Ty::I64))
                } else {
                    let literal = self.new_value(Ty::Ptr);
                    self.emit(Inst::StrConst {
                        dest: literal,
                        bytes: s.as_bytes().to_vec(),
                    });
                    let value = self.new_value(Ty::Bigint);
                    self.emit(Inst::Call {
                        dest: Some(value),
                        func: "zap_bigint_from_str".to_string(),
                        args: vec![literal],
                    });
                    Ok((value, Ty::Bigint))
                }
            }
            Expr::Float(s, _) => {
                let v: f64 = s.replace('_', "").parse().unwrap_or(0.0);
                Ok((self.fconst(Ty::F64, v), Ty::F64))
            }
            Expr::Imaginary(s, _) => {
                let text = s
                    .strip_suffix('j')
                    .or_else(|| s.strip_suffix('J'))
                    .unwrap_or(s)
                    .replace('_', "");
                let im = text.parse::<f64>().unwrap_or(0.0);
                let re_v = self.fconst(Ty::F64, 0.0);
                let im_v = self.fconst(Ty::F64, im);
                let dest = self.new_value(Ty::Complex);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_complex_new".to_string(),
                    args: vec![re_v, im_v],
                });
                Ok((dest, Ty::Complex))
            }
            Expr::Bool(b, _) => {
                let d = self.new_value(Ty::I1);
                self.emit(Inst::BConst { dest: d, val: *b });
                Ok((d, Ty::I1))
            }
            Expr::Str {
                value,
                raw,
                bytes,
                fstring,
                span,
                fstring_fields,
            } => {
                if *fstring {
                    return self.lower_fstring(value, *raw, span.start, fstring_fields);
                }
                let data = if *raw {
                    value.clone().into_bytes()
                } else {
                    unescape(value, *bytes)?
                };
                // `b"..."` and `"..."` share the `ZapStr` object; only the static
                // type differs so byte-wise ops route correctly (I#337). `unescape`
                // decodes `\xHH` as a raw byte for a bytes literal and as a
                // codepoint (UTF-8) for a str (#781).
                let ty = if *bytes { Ty::Bytes } else { Ty::Ptr };
                let d = self.new_value(ty);
                self.emit(Inst::StrConst {
                    dest: d,
                    bytes: data,
                });
                Ok((d, ty))
            }
            Expr::Name(n, span) => {
                if let Some((ptr, ty)) = self.lookup(n) {
                    return Ok((self.load(ptr, ty), ty));
                }
                // A module-level `const` (D-0112, #658 Phase 1): read its global
                // slot — the slot address followed by a load, exactly as a local
                // lowers to its `alloca` + load. The slot was registered by the
                // init pre-pass before any body was lowered.
                if let Some((idx, ty)) = self.ctx.global(n) {
                    let slot = self.new_value(Ty::Ptr);
                    self.emit(Inst::GlobalAddr { dest: slot, idx });
                    // Under `--race` (#677), a read of a mutable non-`@shareable`
                    // global is an instrumented shared access (the read site the
                    // ZAP5125 lint flags); a `const` global's read emits nothing.
                    self.emit_race_access(n, slot, false, *span);
                    return Ok((self.load(slot, ty), ty));
                }
                // A bare top-level function used as a **value** (I#163): `g(f)`,
                // `[inc, dec]`, `return inc`. Wrap it as a first-class function
                // value (a closure record whose code pointer calls the function).
                if self.sigs.contains_key(n) && !self.ctx.generators.contains_key(n) {
                    return self.fnval_of_name(n);
                }
                // A bare Python import alias (`import python "math" as m`) refers
                // to the module handle; importing is idempotent (CPython caches),
                // so a reference lowers to a `zap_py_import` (ticket #27, D-0012).
                if let Some(module) = self.ctx.py.get(n).cloned() {
                    return Ok((self.lower_py_import(&module), Ty::Py));
                }
                Err(err(format!("unknown variable '{n}'")))
            }
            Expr::Unary { op, expr, .. } => {
                let (v, t) = self.expr(expr)?;
                if matches!(t, Ty::Array(..)) {
                    // `~mask` inverts a boolean array (#510, D-0175).
                    if matches!(op, AstUn::BitNot) {
                        if let Ty::Array(Dtype::Bool, nd) = t {
                            let out = Ty::Array(Dtype::Bool, nd);
                            let dest = self.new_value(out);
                            self.emit(Inst::Call {
                                dest: Some(dest),
                                func: "zap_array_lognot".to_string(),
                                args: vec![v],
                            });
                            return Ok((dest, out));
                        }
                        return Err(err(
                            "'~' on an array is only supported for boolean masks (array[bool])",
                        ));
                    }
                    if matches!(op, AstUn::Neg) {
                        // A complex array negates element-wise (#938); other array
                        // dtypes can negate via `0 - a` (an element-wise binop).
                        if let Ty::Array(Dtype::Complex, nd) = t {
                            let out = Ty::Array(Dtype::Complex, nd);
                            let dest = self.new_value(out);
                            self.emit(Inst::Call {
                                dest: Some(dest),
                                func: "zap_array_cneg".to_string(),
                                args: vec![v],
                            });
                            return Ok((dest, out));
                        }
                        return Err(err("element-wise unary array operations are unsupported"));
                    }
                }
                // Unary operator overloading on user structs (D-0047): `-a` ->
                // `a.__neg__()`, `+a` -> `a.__pos__()`, `~a` -> `a.__invert__()`.
                // `not a` goes through `truthy` (which calls `__bool__`).
                if let Ty::Struct(sid) = t {
                    match op {
                        AstUn::Neg => return self.call_dunder(sid, "__neg__", vec![(v, t)]),
                        AstUn::Pos => return self.call_dunder(sid, "__pos__", vec![(v, t)]),
                        AstUn::BitNot => return self.call_dunder(sid, "__invert__", vec![(v, t)]),
                        AstUn::Not => {}
                    }
                }
                match op {
                    AstUn::Neg => {
                        if t == Ty::Bigint {
                            let d = self.new_value(Ty::Bigint);
                            self.emit(Inst::Call {
                                dest: Some(d),
                                func: "zap_bigint_neg".to_string(),
                                args: vec![v],
                            });
                            return Ok((d, t));
                        }
                        if t == Ty::Complex {
                            let d = self.new_value(Ty::Complex);
                            self.emit(Inst::Call {
                                dest: Some(d),
                                func: "zap_complex_neg".to_string(),
                                args: vec![v],
                            });
                            return Ok((d, t));
                        }
                        let d = self.new_value(t);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::Neg,
                            val: v,
                        });
                        Ok((d, t))
                    }
                    AstUn::Pos => Ok((v, t)),
                    AstUn::Not => {
                        let b = self.truthy(v, t);
                        let d = self.new_value(Ty::I1);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::Not,
                            val: b,
                        });
                        Ok((d, Ty::I1))
                    }
                    AstUn::BitNot => {
                        let d = self.new_value(t);
                        self.emit(Inst::Un {
                            dest: d,
                            op: UnOp::BitNot,
                            val: v,
                        });
                        Ok((d, t))
                    }
                }
            }
            // Boolean `and` / `or` short-circuit left-to-right (Python
            // semantics): the right operand is evaluated only when the left
            // does not already decide the result, so a guard idiom like
            // `i < len(xs) and xs[i] == v` never evaluates (and thus never
            // faults on) `xs[i]` when the bound check is false. The result is a
            // strict `bool` (D-0022): each operand is truthiness-tested, and
            // the surviving operand's truthy bit is the value. We merge the two
            // paths through a stack slot (the IR has no phi; cf. the Optional
            // build and the `in` fallback).
            Expr::Binary {
                op: op @ (AstBin::And | AstBin::Or),
                lhs,
                rhs,
                ..
            } => {
                let is_and = matches!(op, AstBin::And);
                let (l, lt) = self.expr(lhs)?;
                let lb = self.truthy(l, lt);
                // Default result is the left operand's truthy bit — the value
                // when we short-circuit (`false and _` -> false, `true or _`
                // -> true).
                let slot = self.alloca(Ty::I1);
                self.store(slot, lb);
                let rhs_bb = self.new_block();
                let join_bb = self.new_block();
                // `and`: descend into the right operand only when the left is
                // truthy. `or`: only when the left is falsy.
                if is_and {
                    self.set_term(Term::BrIf {
                        cond: lb,
                        then_bb: rhs_bb,
                        else_bb: join_bb,
                    });
                } else {
                    self.set_term(Term::BrIf {
                        cond: lb,
                        then_bb: join_bb,
                        else_bb: rhs_bb,
                    });
                }
                self.position(rhs_bb);
                let (r, rt) = self.expr(rhs)?;
                let rb = self.truthy(r, rt);
                self.store(slot, rb);
                self.ensure_br(join_bb);
                self.position(join_bb);
                Ok((self.load(slot, Ty::I1), Ty::I1))
            }
            // `x is None` / `x is not None` (#611, D18) and `x == None` /
            // `x != None` (#374/B3, SPEC §5: consistent with `is None`): the
            // common null/empty check. The bare `None` operand has no standalone
            // lowering, so this is intercepted here (where the other operand's
            // type is known) rather than in `binop`, and emits the same
            // empty-variant / null test as `.is_none()` and
            // `match case Optional.None`. Sema has already rejected a never-None
            // value operand for both forms, so the other side is nullable here.
            Expr::Binary { op, lhs, rhs, .. }
                if matches!(op, AstBin::Is | AstBin::IsNot | AstBin::Eq | AstBin::Ne)
                    && (matches!(lhs.as_ref(), Expr::None(_))
                        || matches!(rhs.as_ref(), Expr::None(_))) =>
            {
                self.lower_is_none(matches!(op, AstBin::IsNot | AstBin::Ne), lhs, rhs)
            }
            // A bare `None` membership test (`None in d`, `None in s`): evaluate
            // the container first to learn its key/element type, then lower the
            // (side-effect-free) `None` against it so it becomes the tagged
            // Optional `None` rather than hitting the unsupported-expression path
            // (#704). Mirrors the `d[None]` subscript threading above.
            Expr::Binary { op, lhs, rhs, .. }
                if matches!(op, AstBin::In | AstBin::NotIn)
                    && matches!(lhs.as_ref(), Expr::None(_)) =>
            {
                let (r, rt) = self.expr(rhs)?;
                let expected = match rt {
                    Ty::Dict(kk, _) => kk.ty(),
                    Ty::Set(ek) => ek.ty(),
                    _ => {
                        return Err(err(
                            "'in' with a `None` left operand requires a dict or set on \
                             the right",
                        ))
                    }
                };
                let l = self.expr_coerced(lhs, expected, zapc_diag::ErrorCode::ZAP4021)?;
                self.binop(*op, l, expected, r, rt)
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let (l, lt) = self.expr(lhs)?;
                let (r, rt) = self.expr(rhs)?;
                // Fast path (#374): skip the runtime guard when the right
                // operand is a safe literal constant — a nonzero divisor for
                // `/` (always float/true division), or a shift count in 0..63.
                // These lower to the plain inline op; everything else goes
                // through `binop`, which routes the guarded forms (zap_fdiv /
                // zap_ishl / zap_ishr) so a bad value raises a catchable fault.
                //
                // An array operand never takes this scalar fast path (#506): it
                // must reach `binop`, which dispatches to `array_binop` for the
                // element-wise SIMD kernel (NumPy true division, inf/nan on a
                // zero element, not a raise).
                let any_array = matches!(lt, Ty::Array(..)) || matches!(rt, Ty::Array(..));
                match op {
                    AstBin::Div if !any_array && Self::is_nonzero_numeric_literal(rhs) => {
                        let lf = self.as_f64(l, lt);
                        let rf = self.as_f64(r, rt);
                        let d = self.new_value(Ty::F64);
                        self.emit(Inst::Bin {
                            dest: d,
                            op: BinOp::Div,
                            lhs: lf,
                            rhs: rf,
                        });
                        Ok((d, Ty::F64))
                    }
                    AstBin::Shl | AstBin::Shr if !any_array && Self::is_safe_shift_literal(rhs) => {
                        // Preserve unsignedness so `>>` on an unsigned value
                        // lowers to a *logical* shift (codegen keys `lshr` vs
                        // `ashr` off the result type, #552). A mixed signed/
                        // unsigned shift was already rejected by sema (ZAP4015).
                        let int_ty = if lt.is_unsigned() || rt.is_unsigned() {
                            Ty::U64
                        } else {
                            Ty::I64
                        };
                        let li = self.coerce(l, lt, int_ty);
                        let ri = self.coerce(r, rt, int_ty);
                        let d = self.new_value(int_ty);
                        let irop = if matches!(op, AstBin::Shl) {
                            BinOp::Shl
                        } else {
                            BinOp::Shr
                        };
                        self.emit(Inst::Bin {
                            dest: d,
                            op: irop,
                            lhs: li,
                            rhs: ri,
                        });
                        Ok((d, int_ty))
                    }
                    _ => self.binop(*op, l, lt, r, rt),
                }
            }
            // Chained comparison `x0 op1 x1 op2 x2 ...` with Python semantics
            // (I#386): equivalent to `(x0 op1 x1) and (x1 op2 x2) and ...` where
            // each operand is evaluated exactly once, left to right, and the
            // chain short-circuits at the first link that is falsy. We carry the
            // previous operand's already-computed value forward so an interior
            // operand with side effects (e.g. a call) runs a single time, and we
            // branch to a shared join after each link just like `and` does. The
            // IR has no phi, so the result is merged through a stack slot.
            Expr::Compare { first, rest, .. } => {
                // A single array comparison `a > 3` / `a == b` is a *mask*
                // (#510, D-0175): return the `array[bool]` directly rather than
                // reducing it to a scalar truthy. A chained comparison with any
                // array operand (`lo < a < hi`) falls through to the scalar path,
                // where `truthy` on the first array link raises NumPy's
                // "ambiguous" ValueError — exactly as intended.
                if rest.len() == 1 {
                    let (lv, lt) = self.expr(first)?;
                    let (rv, rt) = self.expr(&rest[0].1)?;
                    if matches!(lt, Ty::Array(..)) || matches!(rt, Ty::Array(..)) {
                        return self.binop(rest[0].0, lv, lt, rv, rt);
                    }
                    let (cv, ct) = self.binop(rest[0].0, lv, lt, rv, rt)?;
                    let cb = self.truthy(cv, ct);
                    return Ok((cb, Ty::I1));
                }
                let slot = self.alloca(Ty::I1);
                let join_bb = self.new_block();
                let (mut prev_v, mut prev_t) = self.expr(first)?;
                let last = rest.len() - 1;
                for (i, (op, rhs)) in rest.iter().enumerate() {
                    let (rv, rt) = self.expr(rhs)?;
                    let (cv, ct) = self.binop(*op, prev_v, prev_t, rv, rt)?;
                    let cb = self.truthy(cv, ct);
                    self.store(slot, cb);
                    if i != last {
                        // Evaluate the next operand only if this link held.
                        let next_bb = self.new_block();
                        self.set_term(Term::BrIf {
                            cond: cb,
                            then_bb: next_bb,
                            else_bb: join_bb,
                        });
                        self.position(next_bb);
                    }
                    // The right operand becomes the next link's left operand,
                    // reusing the value computed above (evaluated once).
                    prev_v = rv;
                    prev_t = rt;
                }
                self.ensure_br(join_bb);
                self.position(join_bb);
                Ok((self.load(slot, Ty::I1), Ty::I1))
            }
            Expr::Call { callee, args, .. } => self.call(callee, args),
            Expr::RegexMatch {
                subject,
                pattern,
                flags,
                negated,
                ..
            } => self.lower_regex_match(subject, pattern, flags, *negated),
            Expr::RegexSubst {
                subject,
                pattern,
                replacement,
                flags,
                ..
            } => self.lower_regex_subst(subject, pattern, replacement, flags),
            Expr::List(elems, _) => self.lower_list_lit(elems),
            Expr::Dict(items, _) => self.lower_dict_lit(items),
            Expr::Set(elems, _) => self.lower_set_lit(elems),
            Expr::Tuple(elems, _) => self.lower_tuple_lit(elems),
            // A `*expr` is only meaningful as a direct element of a
            // list/set/tuple display, which consume it themselves (#598); sema
            // rejects it anywhere else, so reaching here is a lowering bug.
            Expr::Starred { .. } => Err(err(
                "`*expr` is only valid as an element of a list, set, or tuple display",
            )),
            Expr::ListComp { elem, clauses, .. } => self.lower_list_comp(elem, clauses),
            Expr::SetComp { elem, clauses, .. } => self.lower_set_comp(elem, clauses),
            Expr::GenExp { elem, clauses, .. } => self.lower_genexpr(elem, clauses),
            Expr::DictComp {
                key,
                value,
                clauses,
                ..
            } => self.lower_dict_comp(key, value, clauses),
            Expr::Index { base, index, span } => self.lower_index_get(base, index, *span),
            Expr::Slice {
                base, lo, hi, step, ..
            } => self.lower_slice(base, lo.as_deref(), hi.as_deref(), step.as_deref()),
            Expr::Attr { base, name, span } => {
                // Qualified enum member/variant access (`Color.RED`,
                // `Shape.Unit`): the base is a *type* name, resolved before it
                // would be (wrongly) evaluated as a value (D-0018/D-0046).
                if let Expr::Name(tyname, _) = base.as_ref() {
                    if self.ctx.enums.contains_key(tyname) {
                        return self.lower_enum_member(tyname, name);
                    }
                    // `e.cause` on a caught exception binding (#604, D-0103):
                    // read the explicit cause's message string off the stashed
                    // exception object (an empty str when there is no cause).
                    if name == "cause" {
                        if let Some((obj_slot, _)) = self.lookup(&exc_obj_key(tyname)) {
                            let obj = self.load(obj_slot, Ty::Ptr);
                            let d = self.new_value(Ty::Ptr);
                            self.emit(Inst::Call {
                                dest: Some(d),
                                func: "zap_exception_cause_message".to_string(),
                                args: vec![obj],
                            });
                            return Ok((d, Ty::Ptr));
                        }
                    }
                    // `eg.exceptions` on a caught exception binding (#605, D-0129):
                    // the sub-exception message strings of an `ExceptionGroup`, as
                    // a `list[str]` for manual inspection / iteration. A bare
                    // (non-group) exception yields a one-element list of its own
                    // message, so inspection is uniform. Resolved off the stashed
                    // exception object, exactly like `.cause`.
                    if name == "exceptions" {
                        if let Some((obj_slot, _)) = self.lookup(&exc_obj_key(tyname)) {
                            let obj = self.load(obj_slot, Ty::Ptr);
                            let d = self.new_value(Ty::List(Elem::Ptr));
                            self.emit(Inst::Call {
                                dest: Some(d),
                                func: "zap_exception_exceptions".to_string(),
                                args: vec![obj],
                            });
                            return Ok((d, Ty::List(Elem::Ptr)));
                        }
                    }
                    // Built-in module constant `math.pi`-style access (ticket
                    // #98), intercepted before the base is lowered as a value
                    // (module names are not bindings). Not intercepted when
                    // shadowed by a local of the same name.
                    if self.lookup(tyname).is_none() {
                        if let Some(r) = modules::lower_builtin_module_const(self, tyname, name) {
                            return Ok(r);
                        }
                    }
                }
                let (bv, bty) = self.expr(base)?;
                if let Ty::Array(d, ndim) = bty {
                    return self.lower_array_attr(bv, d, ndim, name);
                }
                // Attribute read on a Python handle, e.g. `np.pi` (ticket #27,
                // D-0012). A call `np.foo(...)` is intercepted in `call` before
                // reaching here, so this is a plain attribute access.
                if bty == Ty::Py {
                    return Ok((self.lower_py_getattr(bv, name), Ty::Py));
                }
                if let Ty::Enum(enum_id) = bty {
                    return self.lower_enum_name(bv, enum_id);
                }
                // The receiver type decides (I#141): a struct with a field
                // named `name` reads that field; the enum op applies only to a
                // non-struct (enum-backed) receiver.
                let Ty::Struct(sid) = bty else {
                    return Err(err("field access on a non-struct value"));
                };
                let (idx, fty) = self
                    .ctx
                    .field(sid, name)
                    .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
                // Member access on `None` throws (EXC-4, D-0007/D-0014): a
                // reference struct receiver may be the null pointer, so guard the
                // read with a catchable null check. Value structs are inline, not
                // nullable, so they need no guard (and `IsNull` would be wrong).
                if !self.ctx.is_value_struct(sid) {
                    self.null_guard(bv, "member access on None");
                }
                // --race (#684): a read of a shared heap struct field.
                self.emit_race_field(bv, sid, idx, name, false, *span);
                let d = self.new_value(fty);
                self.emit(Inst::FieldGet {
                    dest: d,
                    base: bv,
                    sid,
                    idx,
                });
                Ok((d, fty))
            }
            Expr::IfElse {
                cond, then, els, ..
            } => {
                // `a if cond else b` evaluates ONLY the chosen branch (#776): a
                // real branch on the condition, not an eager `Select` of both
                // operands (which would run the non-taken branch's side effects
                // and faults). The two paths merge through a stack slot, like the
                // `and`/`or` short-circuit above (the IR has no phi). The slot's
                // alloca is hoisted to the entry block (#91), so it dominates both
                // arms and the join even though it is created inside the then-arm.
                let (cv, cty) = self.expr(cond)?;
                let c = self.truthy(cv, cty);
                // The pre-branch block dominates both arms and the join, so the
                // merge slot's `alloca` must live here — not inside the then-arm,
                // where it would not dominate the else-arm's store (an SSA
                // violation). The result type is only known after lowering the
                // then-arm, so the slot is created once that type is in hand and
                // its alloca is appended to this dominating block (cf. the
                // or-pattern binding merge).
                let pre_bb = self.cur;
                let then_bb = self.new_block();
                let else_bb = self.new_block();
                let join_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: c,
                    then_bb,
                    else_bb,
                });
                self.position(then_bb);
                let (a, at) = self.expr(then)?;
                let slot = self.new_value(Ty::Ptr);
                self.func.blocks[pre_bb as usize].insts.push(Inst::Alloca {
                    dest: slot,
                    pointee: at,
                });
                self.store(slot, a);
                self.ensure_br(join_bb);
                self.position(else_bb);
                let (b, bt) = self.expr(els)?;
                let b = self.coerce(b, bt, at);
                self.store(slot, b);
                self.ensure_br(join_bb);
                self.position(join_bb);
                Ok((self.load(slot, at), at))
            }
            // `(name := expr)` — walrus assignment-expression (#600, epic #614
            // D8). Evaluate the operand once, bind `name` to it (by inference,
            // like `auto`) in the scope that must outlive this construct, and
            // evaluate to the operand's value. Each textual walrus is lowered
            // exactly once (an `if`/`while` condition block and a comprehension
            // loop body are each emitted once and re-executed at runtime), so a
            // runtime loop reuses the one slot created here.
            Expr::Named { name, value, .. } => {
                let (v, t) = self.expr(value)?;
                let frame = self.walrus_frame();
                if let Some((slot, slot_ty)) = self.scopes[frame].get(name).copied() {
                    // Already bound in the target frame (e.g. a repeated walrus
                    // with the same name): reuse its slot.
                    let v = self.coerce(v, t, slot_ty);
                    self.store(slot, v);
                    Ok((v, t))
                } else if frame + 1 == self.scopes.len() {
                    // Innermost scope (condition position): `decl_slot` keeps the
                    // generator state-struct handling intact.
                    let slot = self.decl_slot(name, t);
                    self.store(slot, v);
                    Ok((v, t))
                } else {
                    // A comprehension filter: bind in the scope enclosing the
                    // comprehension so the name survives the per-clause scopes.
                    // The alloca is hoisted to the entry block (ticket #91), so
                    // the slot dominates both the comprehension body and reads
                    // after it.
                    let slot = self.alloca(t);
                    self.scopes[frame].insert(name.to_string(), (slot, t));
                    self.store(slot, v);
                    Ok((v, t))
                }
            }
            // Channel receive `<-ch` (ticket #25, D-0031): blocks until a value
            // is available, then yields it. The element kind carried by the
            // channel type picks the typed runtime entry point and the result
            // type. (The comma-ok `v, ok = <-ch` form and `select` are a
            // documented follow-up.)
            Expr::ChanRecv { chan, .. } => {
                let (cv, cty) = self.expr(chan)?;
                let Ty::Chan(elem) = cty else {
                    return Err(err("the operand of '<-' (receive) must be a channel"));
                };
                // A nested-collection channel element arrives as its real type (#625).
                let ety = self.elem_ty(elem);
                let d = self.new_value(ety);
                self.throwing_call(
                    Some(d),
                    &format!("zap_chan_recv_{}", elem.val_suffix()),
                    vec![cv],
                );
                Ok((d, ety))
            }
            // Statement-position `yield` is handled in `stmt`; reaching the
            // expression lowerer means a `yield` in subexpression position, which
            // v1 does not support (I#67). In a non-generator function a `yield`
            // cannot occur (sema makes any `def` with `yield` a generator).
            Expr::Yield { .. } if self.gen_entry.is_some() || self.gen_discover.is_some() => Err(
                err("'yield' is only supported as a statement in this version \
                     (I#67 v1): e.g. `yield x`, not inside a larger expression"),
            ),
            Expr::Yield { .. } => Err(err("'yield' outside a generator function (I#67)")),
            // A lambda (I#163) needs a known function type to infer its parameter
            // types; it is lowered where that type is available (a call argument,
            // an annotated binding, or a `fn(...)`-returning `return`). A lambda
            // in an un-annotated context cannot be typed.
            Expr::Lambda { .. } => Err(err(
                "cannot infer this lambda's type here; give it a function-typed \
                 context, e.g. a `fn(T)->R` parameter, annotation, or return type",
            )),
            _ => Err(err(
                "this expression is not supported by codegen yet (Phase 1 slice)",
            )),
        }
    }

    /// Lower a statement-position `yield <value>` inside a generator (I#67).
    ///
    /// During **discovery** this only records the operand's type (a component of
    /// the element type `T`). During **emit** it is the generator's suspend
    /// point: store every live local back to its state-struct field, advance the
    /// `$state` discriminant, return `(value, true)`, and continue lowering in a
    /// fresh **resume block** that `__next__`'s entry dispatch will jump to.
    fn lower_yield_stmt(&mut self, value: Option<&Expr>) -> Result<(), LowerError> {
        if self.gen_discover.is_some() {
            let t = match value {
                Some(v) => self.expr(v)?.1,
                None => Ty::Void,
            };
            self.gen_elems.push(t);
            return Ok(());
        }
        if self.gen_entry.is_none() {
            return Err(err("'yield' outside a generator function (I#67)"));
        }
        let value = value.ok_or_else(|| err("'yield' requires a value (I#67 v1)"))?;

        let (self_val, sid, next_tuple_sid, elem) = {
            let ge = self.gen_entry.as_ref().unwrap();
            (ge.self_val, ge.sid, ge.next_tuple_sid, ge.elem)
        };
        let (v, vt) = self.expr(value)?;
        let v = self.coerce(v, vt, elem);

        // Persist every live local (and the parameters) to its field, so the
        // next `__next__` resumes with the same values (I#67).
        let order = self.gen_entry.as_ref().unwrap().order.clone();
        for name in &order {
            let (slot, ty) = self.gen_entry.as_ref().unwrap().slots[name];
            let idx = self.gen_entry.as_ref().unwrap().field_idx[name];
            let cur = self.load(slot, ty);
            self.emit(Inst::FieldSet {
                base: self_val,
                sid,
                idx,
                val: cur,
            });
        }

        // Advance the resume discriminant and return `(value, true)`.
        let k = {
            let ge = self.gen_entry.as_mut().unwrap();
            let k = ge.next_state;
            ge.next_state += 1;
            k
        };
        let kc = self.iconst(Ty::I64, k);
        self.emit(Inst::FieldSet {
            base: self_val,
            sid,
            idx: 0,
            val: kc,
        });
        let tup = self.new_value(Ty::Struct(next_tuple_sid));
        self.emit(Inst::StructNew {
            dest: tup,
            sid: next_tuple_sid,
        });
        self.emit(Inst::FieldSet {
            base: tup,
            sid: next_tuple_sid,
            idx: 0,
            val: v,
        });
        let yes = self.bconst(true);
        self.emit(Inst::FieldSet {
            base: tup,
            sid: next_tuple_sid,
            idx: 1,
            val: yes,
        });
        self.set_term(Term::Ret(Some(tup)));

        // Continue in the resume block; `__next__`'s dispatch branches here when
        // `$state == k`.
        let resume = self.new_block();
        self.gen_entry.as_mut().unwrap().resume.push((k, resume));
        self.position(resume);
        Ok(())
    }

    fn call(&mut self, callee: &Expr, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        // Qualified tier-3 variant construction `Shape.Circle(2.0)` (D-0046):
        // the callee is `Enum.Variant`, intercepted before the plain-name and
        // method-call dispatch.
        if let Expr::Attr { base, name, .. } = callee {
            if let Expr::Name(tyname, sp) = base.as_ref() {
                // User-defined associated function `Type.func(args)` (#1260): a
                // receiverless `def` on a user struct/enum lowers like a namespaced
                // free function `Type__func`, with NO receiver. Sema type-checked
                // the call and rejected instance/associated misuse. Dispatched
                // before variant construction / the derived decoders / the module
                // path by delegating to the free-function call path on the mangled
                // name, which assembles defaults, keyword and variadic arguments
                // exactly as for any free function. `pkg.Type.func(...)` arrives
                // here with `tyname` already modlink-rewritten to the imported
                // struct's mangled name, so its `Type__func` is the one registered.
                let mangled = format!("{tyname}__{name}");
                if self.ctx.associated_fns.contains(&mangled) && self.lookup(tyname).is_none() {
                    let synthetic = Expr::Name(mangled, *sp);
                    return self.call(&synthetic, args);
                }
                // `bytes.maketrans(frm, to)` → bytes (ticket #1099): an associated
                // constructor building a 256-byte table. Raises on unequal lengths,
                // so a `throwing_call` (invoke) that `try/except` can catch.
                // Intercepted before the module/value dispatch, mirroring the
                // `Struct.from_json` associated-function path. Sema type-checked it.
                if tyname == "bytes" && name == "maketrans" && self.lookup(tyname).is_none() {
                    let (frm, _) = self.expr(&args[0].value)?;
                    let (to, _) = self.expr(&args[1].value)?;
                    let d = self.new_value(Ty::Bytes);
                    self.throwing_call(Some(d), "zap_bytes_maketrans", vec![frm, to]);
                    return Ok((d, Ty::Bytes));
                }
                if self.ctx.enums.contains_key(tyname) {
                    // `Enum.from_json(s)` — an open string-enum's decode function
                    // (ticket #346, D-0086), intercepted before variant
                    // construction so `from_json` is not read as a variant name.
                    let fnname = format!("{tyname}__from_json");
                    if name == "from_json" && self.sigs.contains_key(&fnname) {
                        let (s, _) = self.expr(&args[0].value)?;
                        let sid = *self
                            .ctx
                            .ids
                            .get(tyname)
                            .ok_or_else(|| err("unknown enum in from_json"))?;
                        let d = self.new_value(Ty::Struct(sid));
                        self.emit(Inst::Call {
                            dest: Some(d),
                            func: fnname,
                            args: vec![s],
                        });
                        return Ok((d, Ty::Struct(sid)));
                    }
                    return self.lower_variant_construct(tyname, name, args);
                }
                // Built-in `signal`/`gc`/`runtime` module calls (ticket #77,
                // D-0058), intercepted before the base is lowered as a value
                // (these module names are not value bindings). Not intercepted
                // when shadowed by a local of the same name.
                if self.lookup(tyname).is_none() {
                    // Resolve an `arch.*` import alias (`x86` → `arch.x86_64`,
                    // D-0199) to its registry name, matching the sema dispatch.
                    let module_name = self
                        .ctx
                        .arch_imports
                        .get(tyname)
                        .copied()
                        .unwrap_or(tyname.as_str());
                    if let Some(r) =
                        modules::lower_builtin_module_call(self, module_name, name, args)?
                    {
                        return Ok(r);
                    }
                }
                // `Struct.from_json(s)` / `Struct.from_csv(s)` — the derived
                // decode associated functions (D-0059, I#100/I#132). Dispatched
                // when the generated `{Struct}__from_<fmt>` was registered (i.e.
                // the struct derived that codec); sema type-checked the call.
                if name == "from_json" || name == "from_csv" || name == "from_yaml" {
                    let fnname = format!("{tyname}__{name}");
                    if self.sigs.contains_key(&fnname) {
                        let (s, _) = self.expr(&args[0].value)?;
                        let sid = *self
                            .ctx
                            .ids
                            .get(tyname)
                            .ok_or_else(|| err("unknown struct in from_json"))?;
                        let d = self.new_value(Ty::Struct(sid));
                        self.emit(Inst::Call {
                            dest: Some(d),
                            func: fnname,
                            args: vec![s],
                        });
                        return Ok((d, Ty::Struct(sid)));
                    }
                }
            }
        }
        // Array construction `array[dtype]([...])` (ticket #24). The callee
        // parses as a subscript `array[dtype]`, so it is intercepted here before
        // the plain-name dispatch below.
        if let Expr::Index { base, index, .. } = callee {
            if is_name(base, "array") {
                return self.lower_array_new(index, args);
            }
            // Array constructors (#504, D-0167): `zeros[T](shape)` etc. The callee
            // parses as a subscript `name[dtype]`, like `array[dtype]`.
            if is_name(base, "zeros") {
                return self.lower_array_fill_ctor(index, args, ArrayFill::Zeros);
            }
            if is_name(base, "ones") {
                return self.lower_array_fill_ctor(index, args, ArrayFill::Ones);
            }
            if is_name(base, "full") {
                return self.lower_array_fill_ctor(index, args, ArrayFill::Full);
            }
            if is_name(base, "arange") {
                return self.lower_array_arange(index, args);
            }
            if is_name(base, "linspace") {
                return self.lower_array_linspace(index, args);
            }
            if is_name(base, "channel") {
                return self.lower_chan_new(index, args);
            }
            if is_name(base, "heap") {
                return self.lower_heap_new(index, args);
            }
            if is_name(base, "deque") {
                return self.lower_deque_new(index, args);
            }
            if is_name(base, "oset") {
                return self.lower_oset_new(index, args);
            }
            if is_name(base, "set") {
                return self.lower_set_new(index, args);
            }
            if is_name(base, "list") {
                return self.lower_list_new(index, args);
            }
            if is_name(base, "dict") {
                return self.lower_dict_new(index, args);
            }
            if is_name(base, "AtomicArray") {
                return self.lower_atomicarray_new(index, args);
            }
            if is_name(base, "ContextVar") {
                return self.lower_ctxvar_new(index, args);
            }
        }
        if let Expr::Name(fname, fspan) = callee {
            if fname == "sum" {
                if args.is_empty() || args.len() > 2 || args.iter().any(|a| a.name.is_some()) {
                    return Err(err(
                        "sum() takes one or two positional arguments (the iterable and \
                         an optional start)",
                    ));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                // `sum(xs, start)` (#1299): the optional start value seeds the
                // accumulator.
                let start = if args.len() == 2 {
                    Some(self.expr(&args[1].value)?)
                } else {
                    None
                };
                // An `array[T]` sums through its dedicated runtime reduction. A
                // start value is not supported on the array path.
                if let Ty::Array(d, _) = vt {
                    if start.is_some() {
                        return Err(err("sum(array, start) is not supported; sum(array)"));
                    }
                    let dest = self.new_value(d.ty());
                    self.emit(Inst::Call {
                        dest: Some(dest),
                        func: format!("zap_array_sum_{}", d.suffix()),
                        args: vec![v],
                    });
                    return Ok((dest, d.ty()));
                }
                // Any other iterable — a lazy generator / generator expression
                // (#596), or a `list` — is summed by draining it once through the
                // D-0048 protocol, accumulating into a running total (so a
                // genexpr builds no intermediate list). The accumulator starts at
                // `start` (or 0) and is the element type; `+` is specialized by
                // codegen per operand type, so this handles int and float elements.
                return self.lower_sum_iterable(v, vt, start);
            }
            // `dot(a, b)` — matrix multiply / dot product, the function spelling
            // of `a @ b` (#509). Same kernels, same NEP-50 result dtype.
            if fname == "dot" {
                if args.len() != 2 {
                    return Err(err("dot() takes exactly two arguments (dot(a, b))"));
                }
                let (lv, lt) = self.expr(&args[0].value)?;
                let (rv, rt) = self.expr(&args[1].value)?;
                if !matches!(lt, Ty::Array(..)) && !matches!(rt, Ty::Array(..)) {
                    return Err(err("dot() requires array arguments"));
                }
                return self.lower_array_matmul(lv, lt, rv, rt);
            }
            // `concatenate([a, b, ...], axis=0)` / `stack([a, b, ...], axis=0)`
            // (#511): join a list of arrays. concatenate keeps the rank; stack
            // adds a new axis (rank+1). A shape/dtype mismatch or bad axis raises
            // a catchable ValueError at runtime.
            if fname == "concatenate" || fname == "stack" {
                return self.lower_array_join(fname, args);
            }
            // `where(cond, x, y)` selects element-wise (#510, D-0175), with full
            // broadcasting; `where(cond)` returns a tuple of the true-element
            // coordinates (one index array per axis of `cond`).
            if fname == "where" {
                return self.lower_array_where(args);
            }
            // `sort(a, axis=, kind=)` / `argsort(a, ...)` (#932): a NEW array (a
            // sorted copy / an i64 index array), leaving `a` unchanged. The array
            // is positional; axis=/kind= are keyword-only. For a list, use the
            // `sorted()` builtin instead.
            if fname == "sort" || fname == "argsort" {
                let mut arr_arg: Option<&Expr> = None;
                let mut rest: Vec<ast::Arg> = Vec::new();
                for a in args {
                    if a.name.is_none() {
                        if arr_arg.is_some() {
                            return Err(err(format!(
                                "{fname}() takes a single positional argument (the array)"
                            )));
                        }
                        arr_arg = Some(&a.value);
                    } else {
                        rest.push(a.clone());
                    }
                }
                let Some(arr_expr) = arr_arg else {
                    return Err(err(format!(
                        "{fname}() takes the array to sort as its argument"
                    )));
                };
                let (recv, rt) = self.expr(arr_expr)?;
                let Ty::Array(d, ndim) = rt else {
                    return Err(err(format!(
                        "{fname}() requires a numeric array (use sorted() for a list)"
                    )));
                };
                return self.lower_array_sortish(recv, d, ndim, fname == "argsort", false, &rest);
            }
            // `unique(a, return_index=, return_inverse=, return_counts=)` (#934).
            if fname == "unique" {
                return self.lower_unique(args);
            }
            // `argmin`/`argmax`/`searchsorted`/`partition`/`argpartition`(a, ...)
            // (#934): the array is the first positional; the rest (v/kth + axis=/
            // side=) go to the shared lowering. Builtin partition returns a copy.
            if matches!(
                fname.as_str(),
                "argmin" | "argmax" | "searchsorted" | "partition" | "argpartition"
            ) {
                let mut arr_arg: Option<&Expr> = None;
                let mut rest: Vec<ast::Arg> = Vec::new();
                for a in args {
                    if a.name.is_none() && arr_arg.is_none() {
                        arr_arg = Some(&a.value);
                    } else {
                        rest.push(a.clone());
                    }
                }
                let Some(arr_expr) = arr_arg else {
                    return Err(err(format!(
                        "{fname}() takes the array as its first argument"
                    )));
                };
                let (recv, rt) = self.expr(arr_expr)?;
                let Ty::Array(d, ndim) = rt else {
                    return Err(err(format!("{fname}() requires a numeric array")));
                };
                return self.lower_array_search(recv, d, ndim, fname, false, &rest);
            }
            // `sorted(xs, key=..., reverse=...)` — a new sorted list, leaving the
            // input untouched (ticket #336). The list is positional; `key` /
            // `reverse` are keyword-only, same as `xs.sort(...)`.
            if fname == "sorted" {
                let mut list_arg: Option<&Expr> = None;
                let mut rest: Vec<ast::Arg> = Vec::new();
                for a in args {
                    if a.name.is_none() {
                        if list_arg.is_some() {
                            return Err(err(
                                "sorted() takes a single positional argument (the list)",
                            ));
                        }
                        list_arg = Some(&a.value);
                    } else {
                        rest.push(a.clone());
                    }
                }
                let Some(list_expr) = list_arg else {
                    return Err(err("sorted() takes the list to sort as its argument"));
                };
                let (lst, lt) = self.expr(list_expr)?;
                let Ty::List(elem) = lt else {
                    return Err(err("sorted() expects a list value"));
                };
                // Copy first so the caller's list is left unchanged, then build
                // the sort arguments against the copy and sort it in place.
                let copy = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(copy),
                    func: "zap_list_copy".to_string(),
                    args: vec![lst],
                });
                let (keys, kind, reverse) = self.prepare_sort(copy, elem, "sorted", &rest)?;
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_list_sort_by".to_string(),
                    args: vec![copy, keys, kind, reverse],
                });
                return Ok((copy, Ty::List(elem)));
            }
            if fname == "print" {
                let mut vals = Vec::new();
                for a in args {
                    let (v, vt) = self.expr(&a.value)?;
                    // A Python handle prints via its `str()` (ticket #27,
                    // D-0012): marshal it to a Zap string first so codegen's
                    // `Print` renders it like any other string.
                    if vt == Ty::Py {
                        let s = self.new_value(Ty::Ptr);
                        self.emit(Inst::Call {
                            dest: Some(s),
                            func: "zap_py_str".to_string(),
                            args: vec![v],
                        });
                        vals.push(s);
                    } else if matches!(vt, Ty::Struct(sid) if self.is_any_view_sid(sid)) {
                        // print(view) (#1082): materialize the borrowed bytes to an
                        // owned value so it writes the TEXT, not the view's internal
                        // `(base, off, len)` tuple. A `strview` renders as a string,
                        // a `bytesview` as bytes (its materialized result keeps the
                        // backing's kind). A true zero-alloc write-to-output borrow
                        // is a deferred optimization; correctness first.
                        let Ty::Struct(sid) = vt else { unreachable!() };
                        let out_ty = if self.is_bytesview_sid(sid) {
                            Ty::Bytes
                        } else {
                            Ty::Ptr
                        };
                        let (base, off, len) = self.view_fields(v, sid);
                        let s = self.new_value(out_ty);
                        self.emit(Inst::Call {
                            dest: Some(s),
                            func: "zap_strview_to_str".to_string(),
                            args: vec![base, off, len],
                        });
                        // Both `str` and `bytes` print on the raw path (no special
                        // Display form), so push the materialized value directly.
                        vals.push(s);
                    } else if matches!(
                        vt,
                        Ty::Struct(_) | Ty::List(_) | Ty::Dict(_, _) | Ty::Set(_)
                    ) || vt == Ty::Complex
                        || vt == Ty::Bigint
                        || vt.is_float()
                        || vt == Ty::I1
                        || vt == Ty::Char
                        || vt.is_unsigned()
                        || matches!(vt, Ty::Enum(_))
                    {
                        // Everything with a non-trivial Display form routes through
                        // `render_str` — the one value renderer `str()` uses — so
                        // `print(x) == print(str(x))`:
                        //   * a struct / tuple / `Optional` (`Ty::Struct`) renders
                        //     via `__str__`/`__repr__`/`@derive(Repr)` or the
                        //     default `TypeName(field=value, …)` (#274, D-0124);
                        //   * a `list`/`dict`/`set` renders its Python form
                        //     `[…]`/`{…: …}`/`{…}` (#274, D-0124);
                        //   * a tier-1 enum renders `EnumName.VARIANT`;
                        //   * a float prints `%g`-style (not C `%f`'s `27.500000`)
                        //     and a bool `True`/`False` (not the `-1` bitpattern) —
                        //     the I#493 fix;
                        //   * a `char` renders its one-codepoint string (e.g.
                        //     `print(s[0])` -> `H`, not the decimal codepoint),
                        //     Python-style, now that `char` is a distinct IR type
                        //     (#709).
                        // `int`/`str`/`bytes` already print correctly natively
                        // (and a bare top-level string stays UNQUOTED), so they
                        // stay on the raw path below.
                        vals.push(self.render_str(v, vt)?);
                    } else {
                        vals.push(v);
                    }
                }
                self.emit(Inst::Print { args: vals });
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            if fname == "input" {
                // input([prompt]) -> str (#969, D-0212). Write the optional
                // prompt to stdout, then read one line from stdin. With no
                // prompt, pass an empty string constant — the runtime writes
                // nothing for a zero-length prompt. The call can raise
                // (RuntimeError at EOF), so it is a throwing call that `try` can
                // catch.
                let prompt = if let Some(a) = args.first() {
                    let (v, _) = self.expr(&a.value)?;
                    v
                } else {
                    let e = self.new_value(Ty::Ptr);
                    self.emit(Inst::StrConst {
                        dest: e,
                        bytes: Vec::new(),
                    });
                    e
                };
                let dest = self.new_value(Ty::Ptr);
                self.throwing_call(Some(dest), "zap_input", vec![prompt]);
                return Ok((dest, Ty::Ptr));
            }
            // REPL session-binding slot access (I#159, design/0025-repl.md). The
            // driver synthesizes these calls to persist `let` values across
            // entries: a load reads a slot (integer bindings via `_i64`, GC
            // pointers via `_ptr`), a store writes one back. The slot index is
            // the driver-assigned integer literal.
            if let Some((rt, ret)) = match fname.as_str() {
                "__repl_load_i64" => Some(("zap_repl_slot_get_i64", Ty::I64)),
                "__repl_load_ptr" => Some(("zap_repl_slot_get_ptr", Ty::Ptr)),
                "__repl_load_f64" => Some(("zap_repl_slot_get_f64", Ty::F64)),
                _ => None,
            } {
                if args.len() != 1 {
                    return Err(err(format!("{fname}() takes one slot index")));
                }
                let (idx, idxty) = self.expr(&args[0].value)?;
                let idx = self.coerce(idx, idxty, Ty::I64);
                let dest = self.new_value(ret);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: rt.to_string(),
                    args: vec![idx],
                });
                return Ok((dest, ret));
            }
            // `None` as the target type means a pointer store (pass through);
            // `Some(t)` coerces the value to `t` for the slot cell.
            if let Some((rt, target)) = match fname.as_str() {
                "__repl_store_i64" => Some(("zap_repl_slot_set_i64", Some(Ty::I64))),
                "__repl_store_ptr" => Some(("zap_repl_slot_set_ptr", None)),
                "__repl_store_f64" => Some(("zap_repl_slot_set_f64", Some(Ty::F64))),
                _ => None,
            } {
                if args.len() != 2 {
                    return Err(err(format!("{fname}() takes a slot index and a value")));
                }
                let (idx, idxty) = self.expr(&args[0].value)?;
                let idx = self.coerce(idx, idxty, Ty::I64);
                let (val, valty) = self.expr(&args[1].value)?;
                // Integer/bool bindings widen to the i64 slot cell and floats to
                // the f64 slot ABI; pointer bindings pass through unchanged.
                let val = match target {
                    Some(t) => self.coerce(val, valty, t),
                    None => val,
                };
                self.emit(Inst::Call {
                    dest: None,
                    func: rt.to_string(),
                    args: vec![idx, val],
                });
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            if fname == "range" {
                return Err(err("range() can only be used in a for-loop (Phase 1)"));
            }
            // `close(ch)` (ticket #25, D-0031): mark a channel closed. After this,
            // a send raises and a receive drains then reports not-ok.
            if fname == "close" {
                if args.len() != 1 {
                    return Err(err("close() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if !matches!(vt, Ty::Chan(_)) {
                    return Err(err("close() expects a channel"));
                }
                self.throwing_call(None, "zap_chan_close", vec![v]);
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            // `checkpoint()` (D-0044): a cooperative cancellation point for long
            // CPU-bound loops. It yields the coroutine and, if its owning `scope:`
            // has been cancelled, raises `Cancelled` — so it may unwind.
            if fname == "checkpoint" {
                if !args.is_empty() {
                    return Err(err("checkpoint() takes no arguments"));
                }
                self.throwing_call(None, "zap_checkpoint", vec![]);
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            // `__netpoll_selftest()` (ticket #80): internal netpoller
            // integration-test hook; returns the number of sub-checks that
            // passed. No arguments, no public I/O surface yet (sockets #79).
            if fname == "__netpoll_selftest" {
                if !args.is_empty() {
                    return Err(err("__netpoll_selftest() takes no arguments"));
                }
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_netpoll_selftest".to_string(),
                    args: vec![],
                });
                return Ok((d, Ty::I64));
            }
            // `__signal_selftest()` (ticket #77): internal signal-subsystem
            // integration-test hook; returns the number of sub-checks that passed.
            if fname == "__signal_selftest" {
                if !args.is_empty() {
                    return Err(err("__signal_selftest() takes no arguments"));
                }
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_signal_selftest".to_string(),
                    args: vec![],
                });
                return Ok((d, Ty::I64));
            }
            // `__gc_finalizer_selftest()` (ticket #92): internal finalizer
            // integration-test hook; returns the number of finalizers that ran.
            if fname == "__gc_finalizer_selftest" {
                if !args.is_empty() {
                    return Err(err("__gc_finalizer_selftest() takes no arguments"));
                }
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_gc_finalizer_selftest".to_string(),
                    args: vec![],
                });
                return Ok((d, Ty::I64));
            }
            // `__ht_resize_count()` (#1083): cumulative dict/set table regrow
            // count. Internal test hook behind the capacity / reserve goldens.
            if fname == "__ht_resize_count" {
                if !args.is_empty() {
                    return Err(err("__ht_resize_count() takes no arguments"));
                }
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_ht_resize_count".to_string(),
                    args: vec![],
                });
                return Ok((d, Ty::I64));
            }
            // `__raise_signal(n)` (ticket #77): internal test hook; send signal n
            // to this process so a golden can trigger delivery deterministically.
            if fname == "__raise_signal" {
                if args.len() != 1 {
                    return Err(err("__raise_signal(n) takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let n = self.coerce(v, vt, Ty::I64);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_signal_raise".to_string(),
                    args: vec![n],
                });
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            // `__signal_drain()` (ticket #77): block until the signal thread has
            // dispatched everything raised so far (deterministic goldens).
            if fname == "__signal_drain" {
                if !args.is_empty() {
                    return Err(err("__signal_drain() takes no arguments"));
                }
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_signal_drain".to_string(),
                    args: vec![],
                });
                return Ok((self.iconst(Ty::I32, 0), Ty::I32));
            }
            if fname == "len" {
                if args.len() != 1 {
                    return Err(err("len() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                // `len(v)` on a borrowed `strview` (#1082): the codepoint count of
                // the borrowed range, with no owned allocation. Intercepted before
                // the struct `__len__` path (a strview has no user dunders).
                if let Ty::Struct(sid) = vt {
                    if self.is_strview_sid(sid) {
                        // `len(strview)` = codepoint count of the borrowed range.
                        let (base, off, len) = self.strview_fields(v);
                        let d = self.new_value(Ty::I64);
                        self.emit(Inst::Call {
                            dest: Some(d),
                            func: "zap_strview_cp_count".to_string(),
                            args: vec![base, off, len],
                        });
                        return Ok((d, Ty::I64));
                    }
                    if self.is_bytesview_sid(sid) {
                        // `len(bytesview)` = byte count = the length field (I#337).
                        let (_base, _off, len) = self.view_fields(v, sid);
                        return Ok((len, Ty::I64));
                    }
                }
                // Size overloading (D-0047): `len(a)` -> `a.__len__()`.
                if let Ty::Struct(sid) = vt {
                    return self.call_dunder(sid, "__len__", vec![(v, vt)]);
                }
                let func = match vt {
                    Ty::List(_) => "zap_list_len",
                    Ty::Dict(..) => "zap_dict_len",
                    Ty::Set(_) => "zap_set_len",
                    Ty::Heap(_) => "zap_heap_len",
                    Ty::Deque(_) => "zap_deque_len",
                    Ty::OSet(_) => "zap_oset_len",
                    Ty::Array(..) => "zap_array_len",
                    // `len(str)` is the Unicode **codepoint** count (D-0025,
                    // #284/#374/B6), consistent with codepoint indexing/slicing/
                    // iteration — so `len("héllo") == 5`, not the byte length.
                    // `str` is the sema-level `Ty::Ptr` here; `bytes` is the
                    // distinct `Ty::Bytes` arm below and stays a byte count.
                    Ty::Ptr => "zap_str_len_cp",
                    // `bytes`: byte count (I#337). `zap_str_len` returns the
                    // ZapStr length, which is the byte count for a `bytes`.
                    Ty::Bytes => "zap_str_len",
                    _ => {
                        return Err(err(
                            "len() expects a list, dict, set, heap, deque, oset, array, str, \
                             or bytes",
                        ))
                    }
                };
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![v],
                });
                return Ok((d, Ty::I64));
            }
            // hash_combine(acc, h) -> int (#1064): fold one field hash into an
            // accumulator for a hand-written `__hash__`, via the same
            // `zap_hash_combine` the derived/tuple structural hashes use — so a
            // hand-written struct hash agrees with `@derive(Hash)` in-run.
            if fname == "hash_combine" {
                if args.len() != 2 {
                    return Err(err("hash_combine() takes exactly two arguments"));
                }
                let (a, at) = self.expr(&args[0].value)?;
                let a = self.coerce(a, at, Ty::I64);
                let (b, bt) = self.expr(&args[1].value)?;
                let b = self.coerce(b, bt, Ty::I64);
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_hash_combine".to_string(),
                    args: vec![a, b],
                });
                return Ok((d, Ty::I64));
            }
            // Exact scalar `min`/`max` (#1279 two-arg, #1284 variadic): compute
            // the lesser / greater of two or more numeric scalars by folding a
            // compare + `select` left to right. Integer operands are widened to
            // a common 64-bit width and compared with a signed (or unsigned,
            // #552) `icmp` — never through `f64` — so adjacent `int64` values
            // above 2^53 stay distinct. A tie keeps the current accumulator, so
            // ties resolve to the FIRST argument (Python `min`/`max` semantics):
            // `min` keeps the accumulator on `acc <= x`, `max` on `acc >= x`.
            // Float operands compare at `f64` (mirroring the `<`/`<=` operators).
            // Sema has already restricted this to numeric scalars of a single
            // signedness.
            if fname == "min" || fname == "max" {
                let is_max = fname == "max";
                let positional: Vec<&ast::Arg> = args.iter().filter(|a| a.name.is_none()).collect();
                // Iterable form `min(xs)` / `min(xs, key=f)` / `min(xs, default=d)`
                // (#1299): exactly one positional (a list), plus optional key=/
                // default= keywords.
                if positional.len() == 1 {
                    let key = args.iter().find(|a| a.name.as_deref() == Some("key"));
                    let default = args.iter().find(|a| a.name.as_deref() == Some("default"));
                    return self.lower_minmax_iterable(
                        is_max,
                        &positional[0].value,
                        key.map(|a| &a.value),
                        default.map(|a| &a.value),
                    );
                }
                // Variadic scalar form: two or more positional numeric arguments.
                if positional.len() < 2 || args.iter().any(|a| a.name.is_some()) {
                    return Err(err(
                        "min()/max() takes one iterable, or two or more positional \
                         numeric arguments",
                    ));
                }
                let operands: Vec<(Value, Ty)> = positional
                    .iter()
                    .map(|a| self.expr(&a.value))
                    .collect::<Result<_, _>>()?;
                // The common type is uniform across the call (sema forbids mixing
                // signed and unsigned): float if any operand is a float, else a
                // 64-bit int whose signedness any unsigned operand selects.
                let is_float = operands.iter().any(|(_, t)| matches!(t, Ty::F32 | Ty::F64));
                if is_float {
                    let pred = if is_max { CmpPred::Ge } else { CmpPred::Le };
                    let mut acc = self.as_f64(operands[0].0, operands[0].1);
                    for (v, t) in &operands[1..] {
                        let x = self.as_f64(*v, *t);
                        let cond = self.new_value(Ty::I1);
                        self.emit(Inst::FCmp {
                            dest: cond,
                            pred,
                            lhs: acc,
                            rhs: x,
                        });
                        let d = self.new_value(Ty::F64);
                        self.emit(Inst::Select {
                            dest: d,
                            cond,
                            a: acc,
                            b: x,
                        });
                        acc = d;
                    }
                    return Ok((acc, Ty::F64));
                }
                let unsigned = operands.iter().any(|(_, t)| t.is_unsigned());
                let int_ty = if unsigned { Ty::U64 } else { Ty::I64 };
                let pred = if is_max { CmpPred::Ge } else { CmpPred::Le };
                let mut acc = self.coerce(operands[0].0, operands[0].1, int_ty);
                for (v, t) in &operands[1..] {
                    let x = self.coerce(*v, *t, int_ty);
                    let cond = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: cond,
                        pred,
                        lhs: acc,
                        rhs: x,
                    });
                    let d = self.new_value(int_ty);
                    self.emit(Inst::Select {
                        dest: d,
                        cond,
                        a: acc,
                        b: x,
                    });
                    acc = d;
                }
                return Ok((acc, int_ty));
            }
            // Scalar integer-preserving `abs` (#1283). A float yields its f64
            // magnitude via `zap_math_fabs` (correct for -0.0 and NaN). An
            // unsigned value is already its own magnitude (identity, uint64). A
            // signed integer stays an integer: `|x| = x < 0 ? -x : x`, computed
            // with an integer negate and `select` — never through float. The one
            // value with no int64 result, `INT64_MIN` (its magnitude 2^63 does
            // not fit int64), raises a catchable ArithmeticError rather than
            // silently wrapping back to INT64_MIN.
            if fname == "abs" {
                if args.len() != 1 || args.iter().any(|a| a.name.is_some()) {
                    return Err(err("abs() takes exactly one positional numeric argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if matches!(vt, Ty::F32 | Ty::F64) {
                    let f = self.as_f64(v, vt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_math_fabs".to_string(),
                        args: vec![f],
                    });
                    return Ok((d, Ty::F64));
                }
                if vt.is_unsigned() {
                    let u = self.coerce(v, vt, Ty::U64);
                    return Ok((u, Ty::U64));
                }
                let li = self.coerce(v, vt, Ty::I64);
                // Guard INT64_MIN: `-INT64_MIN` overflows (two's complement wraps
                // back to INT64_MIN), so raise ArithmeticError instead.
                let intmin = self.iconst(Ty::I64, i64::MIN);
                let is_min = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: is_min,
                    pred: CmpPred::Eq,
                    lhs: li,
                    rhs: intmin,
                });
                let throw_bb = self.new_block();
                let cont_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: is_min,
                    then_bb: throw_bb,
                    else_bb: cont_bb,
                });
                self.position(throw_bb);
                let msg = self.new_value(Ty::Ptr);
                self.emit(Inst::StrConst {
                    dest: msg,
                    bytes: b"abs() of the smallest int64 (-2**63) has no int64 result".to_vec(),
                });
                let arith = self.iconst(Ty::I64, 8); // ZAP_EXC_ARITHMETIC_ERROR
                self.throwing_call(None, "zap_panic_typed", vec![arith, msg]);
                if !self.terminated() {
                    self.set_term(Term::Unreachable);
                }
                self.position(cont_bb);
                let zero = self.iconst(Ty::I64, 0);
                let is_neg = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: is_neg,
                    pred: CmpPred::Lt,
                    lhs: li,
                    rhs: zero,
                });
                let neg = self.new_value(Ty::I64);
                self.emit(Inst::Un {
                    dest: neg,
                    op: UnOp::Neg,
                    val: li,
                });
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Select {
                    dest: d,
                    cond: is_neg,
                    a: neg,
                    b: li,
                });
                return Ok((d, Ty::I64));
            }
            // Scalar `divmod(a, b)` -> `(quotient, remainder)` (#1283). Uses the
            // same floored `//` and `%` helpers the operators do, so
            // `divmod(a, b) == (a // b, a % b)` for every sign and the integer
            // path never goes through float. A zero divisor raises
            // ZeroDivisionError from the helper (an unwinding `invoke`).
            if fname == "divmod" {
                if args.len() != 2 || args.iter().any(|a| a.name.is_some()) {
                    return Err(err(
                        "divmod() takes exactly two positional numeric arguments",
                    ));
                }
                let (l, lt) = self.expr(&args[0].value)?;
                let (r, rt) = self.expr(&args[1].value)?;
                let is_float = matches!(lt, Ty::F32 | Ty::F64) || matches!(rt, Ty::F32 | Ty::F64);
                let (q, rem, elem_ty) = if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    // Float floor-div by zero yields inf (a plain call, like `//`);
                    // float modulo by zero raises (a throwing call, like `%`).
                    let q = self.new_value(Ty::F64);
                    self.emit(Inst::Call {
                        dest: Some(q),
                        func: "zap_ffloordiv".to_string(),
                        args: vec![lf, rf],
                    });
                    let rem = self.new_value(Ty::F64);
                    self.throwing_call(Some(rem), "zap_ffmod", vec![lf, rf]);
                    (q, rem, Ty::F64)
                } else {
                    let unsigned = lt.is_unsigned() || rt.is_unsigned();
                    let int_ty = if unsigned { Ty::U64 } else { Ty::I64 };
                    let li = self.coerce(l, lt, int_ty);
                    let ri = self.coerce(r, rt, int_ty);
                    let (divf, modf) = if unsigned {
                        ("zap_udiv", "zap_umod")
                    } else {
                        ("zap_ifloordiv", "zap_imod")
                    };
                    let q = self.new_value(int_ty);
                    self.throwing_call(Some(q), divf, vec![li, ri]);
                    let rem = self.new_value(int_ty);
                    self.throwing_call(Some(rem), modf, vec![li, ri]);
                    (q, rem, int_ty)
                };
                // Build the `(quotient, remainder)` 2-tuple (same shape as a
                // tuple literal / `return a, b`).
                let sid = self.ctx.tuples.borrow_mut().intern(vec![elem_ty, elem_ty]);
                let dest = self.new_value(Ty::Struct(sid));
                self.emit(Inst::StructNew { dest, sid });
                self.emit(Inst::FieldSet {
                    base: dest,
                    sid,
                    idx: 0,
                    val: q,
                });
                self.emit(Inst::FieldSet {
                    base: dest,
                    sid,
                    idx: 1,
                    val: rem,
                });
                return Ok((dest, Ty::Struct(sid)));
            }
            // Truth reductions `any(xs)` / `all(xs)` over a `list[bool]` (#1285):
            // drain the list once, folding the `i1` accumulator with `or` (any)
            // or `and` (all). The accumulator's initial value is the empty-list
            // result — `any([])` is false, `all([])` is true. This drains fully
            // (no short-circuit), which gives the same result for a finite list;
            // sema restricts the argument to a `list[bool]`.
            if fname == "any" || fname == "all" {
                if args.len() != 1 || args.iter().any(|a| a.name.is_some()) {
                    return Err(err(
                        "any()/all() takes exactly one positional list argument",
                    ));
                }
                let is_all = fname == "all";
                let (v, vt) = self.expr(&args[0].value)?;
                let Ty::List(elem) = vt else {
                    return Err(err("any()/all() expects a list"));
                };
                let acc = self.alloca(Ty::I1);
                let init = self.iconst(Ty::I1, if is_all { 1 } else { 0 });
                self.store(acc, init);
                let op = if is_all { BinOp::And } else { BinOp::Or };
                self.iterate_list(v, elem, move |s, ev, et| {
                    let t = s.truthy(ev, et);
                    let cur = s.load(acc, Ty::I1);
                    let next = s.new_value(Ty::I1);
                    s.emit(Inst::Bin {
                        dest: next,
                        op,
                        lhs: cur,
                        rhs: t,
                    });
                    s.store(acc, next);
                    Ok(())
                })?;
                let result = self.load(acc, Ty::I1);
                return Ok((result, Ty::I1));
            }
            // `reversed(xs)` (#1285): a NEW list with `xs`'s elements reversed,
            // leaving `xs` unchanged — a shallow copy (`zap_list_copy`) reversed
            // in place (`zap_list_reverse`). `for x in reversed(xs)` then iterates
            // the new list through the ordinary list path.
            if fname == "reversed" {
                if args.len() != 1 || args.iter().any(|a| a.name.is_some()) {
                    return Err(err("reversed() takes exactly one positional list argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let Ty::List(elem) = vt else {
                    return Err(err("reversed() expects a list"));
                };
                let d = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_list_copy".to_string(),
                    args: vec![v],
                });
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_list_reverse".to_string(),
                    args: vec![d],
                });
                return Ok((d, Ty::List(elem)));
            }
            // `enumerate`/`zip` (#1302) are only supported directly in a for-loop
            // header (`for i, x in enumerate(xs)`), where `lower_for` intercepts
            // them; a standalone value is a follow-up.
            if fname == "enumerate" || fname == "zip" {
                return Err(err(format!(
                    "{fname}() is only supported directly in a for-loop \
                     (e.g. `for i, x in enumerate(xs)`); a standalone {fname}() value \
                     is a follow-up"
                )));
            }
            // String/char builtins (ticket #284, D-0025).
            if fname == "ord" {
                // ord(c): the codepoint number of a `char`, as an `int`.
                if args.len() != 1 {
                    return Err(err("ord() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                return Ok((self.coerce(v, vt, Ty::I64), Ty::I64));
            }
            if fname == "chr" || fname == "char" {
                // chr(n)/char(n): the `char` whose codepoint is `n` (validated).
                if args.len() != 1 {
                    return Err(err(format!("{fname}() takes exactly one argument")));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let v = self.coerce(v, vt, Ty::I64);
                let d = self.new_value(Ty::Char);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_chr".to_string(),
                    args: vec![v],
                });
                return Ok((d, Ty::Char));
            }
            if fname == "bigint" {
                if args.len() != 1 {
                    return Err(err("bigint() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                return Ok((self.coerce(v, vt, Ty::Bigint), Ty::Bigint));
            }
            if fname == "complex" {
                if args.is_empty() {
                    let zero = self.fconst(Ty::F64, 0.0);
                    let d = self.new_value(Ty::Complex);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_complex_from_f64".to_string(),
                        args: vec![zero],
                    });
                    return Ok((d, Ty::Complex));
                }
                if args.len() == 1 {
                    let (v, vt) = self.expr(&args[0].value)?;
                    let d = self.coerce(v, vt, Ty::Complex);
                    return Ok((d, Ty::Complex));
                }
                if args.len() == 2 {
                    let (re, ret) = self.expr(&args[0].value)?;
                    let (im, imt) = self.expr(&args[1].value)?;
                    let re = self.as_f64(re, ret);
                    let im = self.as_f64(im, imt);
                    let d = self.new_value(Ty::Complex);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_complex_new".to_string(),
                        args: vec![re, im],
                    });
                    return Ok((d, Ty::Complex));
                }
                return Err(err("complex() takes at most two arguments"));
            }
            if fname == "str" {
                // str(x): the textual form of a scalar/string. `char` (an `i32`)
                // yields its one-codepoint string; a plain int is `i64`, so
                // `str(42)` is decimal. (int32 and char share the `i32`
                // representation, so `str(int32_v)` also renders a character —
                // format an int32 as decimal via `str(int64(x))`; a full fix
                // needs `char` threaded through the IR — follow-up.)
                if args.len() != 1 {
                    return Err(err("str() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                return Ok((self.render_str(v, vt)?, Ty::Ptr));
            }
            if fname == "bytes" {
                // bytes(): empty; bytes(str): the UTF-8 bytes (shares the ZapStr
                // buffer — both are immutable); bytes(list[int64]): one byte per
                // element, each validated to 0..=255 (I#337).
                if args.is_empty() {
                    let d = self.new_value(Ty::Bytes);
                    self.emit(Inst::StrConst {
                        dest: d,
                        bytes: Vec::new(),
                    });
                    return Ok((d, Ty::Bytes));
                }
                if args.len() != 1 {
                    return Err(err("bytes() takes at most one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                return match vt {
                    // str / bytes share the ZapStr repr — relabel, no copy.
                    Ty::Ptr | Ty::Bytes => Ok((v, Ty::Bytes)),
                    Ty::List(_) => {
                        // Validates each element is 0..=255, raising otherwise.
                        let d = self.new_value(Ty::Bytes);
                        self.throwing_call(Some(d), "zap_bytes_from_list", vec![v]);
                        Ok((d, Ty::Bytes))
                    }
                    _ => Err(err("bytes() expects a str or list[int64] argument")),
                };
            }
            if fname == "set" {
                // set(iterable): a fresh set of the iterable's deduplicated
                // elements (D17, #609). The element kind is fixed by the first
                // element added; `set()` with no argument is rejected (sema
                // directs the user to `set[T]()`).
                if args.len() != 1 || args[0].name.is_some() {
                    return Err(err(
                        "set(iterable) takes exactly one positional argument; use set[T]() for an empty set",
                    ));
                }
                return self.lower_set_from_iterable(&args[0].value, None);
            }
            if fname == "int" {
                // int(x): parse a `str` to an `int`, or convert a number to int.
                if args.len() != 1 {
                    return Err(err("int() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if vt == Ty::Ptr {
                    // A malformed string raises a catchable `ValueError` (#456),
                    // so this must be an unwinding `invoke` inside a `try` — a
                    // plain `call` would let the exception escape past the
                    // handler and terminate, like the index/divide faults.
                    let d = self.new_value(Ty::I64);
                    self.throwing_call(Some(d), "zap_str_parse_int", vec![v]);
                    return Ok((d, Ty::I64));
                }
                return Ok((self.convert(v, vt, Ty::I64), Ty::I64));
            }
            if fname == "float" {
                // float(x): parse a `str` to a float, or convert a number/bool.
                if args.len() != 1 {
                    return Err(err("float() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if vt == Ty::Ptr {
                    // A malformed string raises a catchable `ValueError`
                    // (zap_str_parse_float -> zap_panic, #741), so this must be
                    // an unwinding `invoke` inside a `try` — a plain `call`
                    // would let the exception escape past the handler and
                    // terminate, exactly like int() above (#456).
                    let d = self.new_value(Ty::F64);
                    self.throwing_call(Some(d), "zap_str_parse_float", vec![v]);
                    return Ok((d, Ty::F64));
                }
                return Ok((self.as_f64(v, vt), Ty::F64));
            }
            if fname == "bool" {
                // bool(x): Python-style truthiness to an `i1` (#285).
                if args.len() != 1 {
                    return Err(err("bool() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                return Ok((self.value_to_bool(v, vt)?, Ty::I1));
            }
            if fname == "format" {
                // format(value, "spec"): render per the format mini-language
                // (D-0026). The spec is a compile-time string literal in v1
                // (sema enforces), so it lowers to the same formatter f-string
                // fields use.
                if args.len() != 2 {
                    return Err(err("format() takes exactly two arguments"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let Expr::Str {
                    value,
                    fstring: false,
                    ..
                } = &args[1].value
                else {
                    return Err(err("format() spec must be a string literal"));
                };
                let spec = fstring::parse_format_spec(value)
                    .map_err(|m| err(format!("invalid format spec: {m}")))?;
                return Ok((self.format_value(v, vt, &spec)?, Ty::Ptr));
            }
            // Explicit numeric conversion: `int32(x)`, `float64(x)`, ... (D-0020).
            // These are the way to narrow or cross the int/float boundary, which
            // implicit coercion refuses.
            if let Some(to) = numeric_conv_ty(fname) {
                if args.len() != 1 {
                    return Err(err(format!("{fname}() takes exactly one argument")));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                // Marshal a Python handle back to a Zap scalar (ticket #27,
                // D-0012): `int64(x)` / `float64(x)` are the conversion surface
                // the design uses (`print(int64(b))`). Route through the runtime
                // marshaler (i64 or f64), then coerce to the requested width.
                if vt == Ty::Py {
                    let (helper, mid) = if to.is_float() {
                        ("zap_py_to_f64", Ty::F64)
                    } else {
                        ("zap_py_to_i64", Ty::I64)
                    };
                    let raw = self.new_value(mid);
                    self.throwing_call(Some(raw), helper, vec![v]);
                    return Ok((self.convert(raw, mid, to), to));
                }
                if !(vt.is_float() || vt.int_width().is_some() || vt == Ty::Bigint) {
                    return Err(err(format!("{fname}() expects a numeric value")));
                }
                return Ok((self.convert(v, vt, to), to));
            }
            // Runtime reflection (ticket #16, D-0036): `reflect_has(type, name)`
            // and `reflect_names(type)` query the metadata registry populated at
            // the top of `main`. Both take string arguments.
            if fname == "reflect_has" || fname == "reflect_names" {
                return self.lower_reflect_query(fname, args);
            }
            // Signature reflection (I#458, D-0089): `reflect_params`/`reflect_sig`
            // and the per-parameter queries read the signature registry populated
            // alongside the struct/type registry at the top of `main`.
            if matches!(
                fname.as_str(),
                "reflect_params"
                    | "reflect_sig"
                    | "reflect_param_type"
                    | "reflect_param_annotations"
                    | "reflect_param_has_default"
            ) {
                return self.lower_reflect_param_query(fname, args);
            }
            // Tier-1 enum conversion `Color(x)` (D-0018/D-0020): the enum is its
            // backing `i64`, so this is just a coercion of the int argument.
            if let Some(info) = self.ctx.enums.get(fname) {
                if !info.is_sum {
                    if args.len() != 1 {
                        return Err(err(format!("{fname}() takes exactly one argument")));
                    }
                    let (v, vt) = self.expr(&args[0].value)?;
                    let ty = Ty::Enum(info.enum_id);
                    return Ok((self.coerce(v, vt, ty), ty));
                }
            }
            // Struct construction: `StructName(fields...)`.
            // A generator call `gen(args)` (I#67) constructs the compiler-
            // generated state struct — capturing the arguments in its parameter
            // fields and starting at `$state == 0` — rather than calling a
            // function; the original body was compiled into the state struct's
            // `__next__`.
            if let Some(gi) = self.ctx.generators.get(fname).cloned() {
                return self.construct_generator(&gi, args);
            }
            if let Some(&sid) = self.ctx.ids.get(fname) {
                return self.construct(sid, args);
            }
            if let Some((ptys, ret)) = self.sigs.get(fname).cloned() {
                // `self.ctx` is a shared `&'a` reference; copy it out so the
                // parameter metadata it holds can be borrowed across the
                // `&mut self` lowering calls below.
                let ctx = self.ctx;
                // A call carrying `*`/`**` spreads (#597), or into a callee with
                // a `V **name` keyword-collector (#601), is assembled by a
                // dedicated path that forwards into the callee's variadic /
                // collector parameters and builds the collector dict.
                let has_spread = args.iter().any(|a| a.spread != ast::ArgSpread::None);
                let callee_kwargs = ctx
                    .call_params
                    .get(fname)
                    .is_some_and(|pi| pi.iter().any(|p| p.kwargs));
                if has_spread || callee_kwargs {
                    return self.lower_call_assembled(fname, None, &ptys, ret, args);
                }
                // Default arguments and out-of-order keyword arguments (D-0038,
                // I#298): assemble the call's arguments into declaration order,
                // filling omitted defaulted parameters, before lowering. A
                // variadic function (or one with no recorded metadata) keeps the
                // positional lowering path.
                let plan = match ctx.call_params.get(fname) {
                    Some(pinfo) if !pinfo.iter().any(|p| p.variadic) => {
                        Some((pinfo, plan_call_args(fname, pinfo, args)?))
                    }
                    _ => None,
                };
                let mut argvals = Vec::new();
                if let Some((pinfo, plan)) = plan {
                    for (i, slot) in plan.iter().enumerate() {
                        // Either an expression written at the call site or the
                        // omitted parameter's default (lowered fresh here, so it
                        // is recomputed per call — D-0038).
                        let e: &Expr = match slot {
                            ArgSlot::Given(j) => &args[*j].value,
                            ArgSlot::Default(p) => pinfo[*p].default.as_ref().unwrap(),
                        };
                        // A function-typed parameter accepts a `lambda` literal or
                        // a bare function name as a first-class value (I#163),
                        // lowered against the declared parameter type.
                        let v = match ptys.get(i) {
                            Some(&target) => {
                                let v =
                                    self.expr_coerced(e, target, zapc_diag::ErrorCode::ZAP4021)?;
                                // A value struct passed by value is copied
                                // (D-0051): the callee's mutations do not escape.
                                // An inline-eligible aggregate gets that copy from
                                // the register/sret ABI; a non-inline value struct
                                // is deep-copied. See `copy_for_pass`.
                                self.copy_for_pass(v, target, e)
                            }
                            None => {
                                let (v, vt) = self.expr(e)?;
                                self.copy_for_pass(v, vt, e)
                            }
                        };
                        argvals.push(v);
                    }
                } else if let Some(vi) = ctx
                    .call_params
                    .get(fname)
                    .and_then(|pinfo| pinfo.iter().position(|p| p.variadic))
                {
                    // A variadic function (D-0038): the fixed parameters before
                    // the variadic are lowered positionally, then the remaining
                    // arguments are packed into a fresh `list` of the variadic's
                    // element type — "received as a `list`" (SPEC §7). A bare
                    // `f()` with no trailing arguments passes an empty list.
                    for (i, a) in args.iter().take(vi).enumerate() {
                        let v = match ptys.get(i) {
                            Some(&target) => {
                                let v = self.expr_coerced(
                                    &a.value,
                                    target,
                                    zapc_diag::ErrorCode::ZAP4021,
                                )?;
                                self.copy_for_pass(v, target, &a.value)
                            }
                            None => {
                                let (v, vt) = self.expr(&a.value)?;
                                self.copy_for_pass(v, vt, &a.value)
                            }
                        };
                        argvals.push(v);
                    }
                    let elem = match ptys.get(vi) {
                        Some(&Ty::List(e)) => e,
                        _ => Elem::I64,
                    };
                    let list = self.new_value(Ty::List(elem));
                    self.emit(Inst::Call {
                        dest: Some(list),
                        func: "zap_list_new".to_string(),
                        args: vec![],
                    });
                    let push = format!("zap_list_push_{}", elem.val_suffix());
                    for a in args.iter().skip(vi) {
                        let (v, vt) = self.expr(&a.value)?;
                        let v = self.coerce(v, vt, elem.ty());
                        let v = self.box_if_value(v, elem.ty());
                        self.emit(Inst::Call {
                            dest: None,
                            func: push.clone(),
                            args: vec![list, v],
                        });
                    }
                    argvals.push(list);
                } else {
                    for (i, a) in args.iter().enumerate() {
                        let v = match ptys.get(i) {
                            Some(&target) => {
                                let v = self.expr_coerced(
                                    &a.value,
                                    target,
                                    zapc_diag::ErrorCode::ZAP4021,
                                )?;
                                self.copy_for_pass(v, target, &a.value)
                            }
                            None => {
                                let (v, vt) = self.expr(&a.value)?;
                                self.copy_for_pass(v, vt, &a.value)
                            }
                        };
                        argvals.push(v);
                    }
                }
                let dest = if ret == Ty::Void {
                    None
                } else {
                    Some(self.new_value(ret))
                };
                self.throwing_call(dest, fname, argvals);
                let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
                return Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }));
            }
            // A first-class function value held in a local (I#163): `f(args)`
            // where `f` is a variable of function type is an indirect call
            // through its closure record.
            if let Some((slot, Ty::Func(idx))) = self.lookup(fname) {
                let cv = self.load(slot, Ty::Func(idx));
                return self.lower_closure_call(cv, idx, args);
            }
            // A callable user struct held in a variable (D-0047): `f(args)` where
            // `f` is a struct value desugars to `f.__call__(args)`. `lookup`
            // yields the variable's stack slot, so the receiver must be loaded.
            // Routed through the resolved-method path (not `call_dunder`) so the
            // sugar fills omitted default arguments and accepts keyword / out-of-
            // order arguments exactly as the explicit `f.__call__(args)` form does
            // (#1228): `__call__` is an ordinary method, and the dateutil-style
            // `MO()` default-argument call is the motivating case.
            if let Some((slot, Ty::Struct(sid))) = self.lookup(fname) {
                let recv = self.load(slot, Ty::Struct(sid));
                let mangled = format!("{}____call__", self.ctx.names[sid as usize]);
                return self.call_resolved_method(recv, "__call__", mangled, args);
            }
            // A callable module-level `const`/global whose type implements
            // `__call__` (D-0047, #1228): `MO(args)` where `MO` is a module global
            // of struct type desugars to `MO.__call__(args)`, exactly like the
            // local-value form above (same resolved-method path, so defaults and
            // keyword arguments work identically). Checked after the local lookups
            // so a local of the same name shadows the global (matching the
            // `Expr::Name` value-read path), and keyed on the global registry — an
            // imported constant, which modlink rewrites to its mangled global name,
            // resolves through this very path (`mod.MO(args)` → `<prefix>MO(args)`).
            if let Some((idx, Ty::Struct(sid))) = self.ctx.global(fname) {
                let slot = self.new_value(Ty::Ptr);
                self.emit(Inst::GlobalAddr { dest: slot, idx });
                self.emit_race_access(fname, slot, false, *fspan);
                let recv = self.load(slot, Ty::Struct(sid));
                let mangled = format!("{}____call__", self.ctx.names[sid as usize]);
                return self.call_resolved_method(recv, "__call__", mangled, args);
            }
            // A Python callable held in a variable (`auto f = m.sqrt; f(2.0)`)
            // or an imported module name used as a callable (ticket #27, D-0012).
            if matches!(self.lookup(fname), Some((_, Ty::Py))) {
                let (callable, _) = self.expr(callee)?;
                return self.lower_py_call(callable, args);
            }
            if let Some(module) = self.ctx.py.get(fname).cloned() {
                let callable = self.lower_py_import(&module);
                return self.lower_py_call(callable, args);
            }
            Err(err(format!("call to unknown function '{fname}'")))
        } else if let Expr::Attr { base, name, .. } = callee {
            // Method call: `recv.method(args)`.
            let (recv, bty) = self.expr(base)?;
            if let Ty::List(elem) = bty {
                return self.lower_list_method(recv, elem, name, args);
            }
            if let Ty::Set(elem) = bty {
                return self.lower_set_method(recv, elem, name, args);
            }
            if let Ty::Dict(kk, vk) = bty {
                return self.lower_dict_method(recv, kk, vk, name, args);
            }
            if let Ty::Heap(elem) = bty {
                return self.lower_heap_method(recv, elem, name, args);
            }
            if let Ty::Deque(elem) = bty {
                return self.lower_deque_method(recv, elem, name, args);
            }
            if let Ty::OSet(elem) = bty {
                return self.lower_oset_method(recv, elem, name, args);
            }
            if let Ty::AtomicArray(elem) = bty {
                return self.lower_atomicarray_method(recv, elem, name, args);
            }
            if let Ty::ContextVar(elem) = bty {
                return self.lower_ctxvar_method(recv, elem, name, args);
            }
            if let Ty::Array(d, ndim) = bty {
                return self.lower_array_method(recv, d, ndim, name, args);
            }
            if bty == Ty::Match {
                return self.lower_match_method(recv, name, args);
            }
            if bty == Ty::Py {
                // `obj.method(args)` on a Python handle (ticket #27, D-0012):
                // read the bound attribute, then call it with marshaled args.
                let callable = self.lower_py_getattr(recv, name);
                return self.lower_py_call(callable, args);
            }
            // A capability dunder (`__cmp__`/`__eq__`/`__hash__`, D-0139) called by
            // name on a value of an INTRINSIC type lowers to the same primitive op
            // the operator surface (`<`, `==`, dict-key hashing) already emits, so
            // the method and operator surfaces agree for every type a capability
            // admits (I#820). This is reached when a generic bound parameter
            // `[K: Comparable]` / `[K: Hashable]` is monomorphized to a scalar /
            // `str` / `bytes` / `float` / tier-1 enum and the body names the dunder
            // — such a value carries no method symbol to dispatch to, so without
            // this it fell through to the string-method path (which does not define
            // these) or the non-struct error below. Checked before the `str`
            // dispatch so a `str` (`Ty::Ptr`) receiver routes here too; these three
            // names are not string methods, so no `str` call is intercepted.
            if matches!(name.as_str(), "__cmp__" | "__eq__" | "__hash__")
                && (bty.int_width().is_some()
                    || matches!(bty, Ty::F32 | Ty::F64 | Ty::Ptr | Ty::Bytes))
            {
                return self.lower_intrinsic_capability_dunder(recv, bty, name, args);
            }
            // `str` methods (ticket #284): `str` lowers to `Ptr`, so a method
            // call on a `Ptr` receiver is a string method (sema has already
            // rejected any other `Ptr`-typed method receiver).
            if bty == Ty::Ptr {
                return self.lower_str_method(recv, name, args);
            }
            // `bytes.view()` → a borrowed `bytesview` (#1082 follow-up). `view` is
            // the only method a `bytes` receiver carries; bounds are byte indices.
            if bty == Ty::Bytes && name == "view" {
                if args.len() > 2 {
                    return Err(err("bytes.view() takes 0, 1, or 2 arguments"));
                }
                let (lo, has_lo) = match args.first() {
                    Some(a) => (self.expr(&a.value)?.0, self.iconst(Ty::I64, 1)),
                    None => (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0)),
                };
                let (hi, has_hi) = match args.get(1) {
                    Some(a) => (self.expr(&a.value)?.0, self.iconst(Ty::I64, 1)),
                    None => (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0)),
                };
                return Ok((
                    self.build_bytesview(recv, lo, has_lo, hi, has_hi),
                    self.bytesview_ty(),
                ));
            }
            // `bytes.translate(table)` / `bytes.translate(table, delete)` → bytes
            // (ticket #1099): one native pass over the whole buffer. Raises when
            // `table` is not 256 bytes, so it is a `throwing_call` (invoke) a
            // `try/except` can catch. The runtime takes a 0/1 `has_delete` flag;
            // when 0, the delete-set pointer is unused (we pass `table` as a
            // harmless non-null placeholder).
            if bty == Ty::Bytes && name == "translate" {
                if args.is_empty() || args.len() > 2 {
                    return Err(err(
                        "bytes.translate() takes 1 or 2 arguments (table[, delete])",
                    ));
                }
                let (table, _) = self.expr(&args[0].value)?;
                let (delete, has_delete) = match args.get(1) {
                    Some(a) => (self.expr(&a.value)?.0, self.iconst(Ty::I64, 1)),
                    None => (table, self.iconst(Ty::I64, 0)),
                };
                let d = self.new_value(Ty::Bytes);
                self.throwing_call(
                    Some(d),
                    "zap_bytes_translate",
                    vec![recv, table, delete, has_delete],
                );
                return Ok((d, Ty::Bytes));
            }
            // Tier-1 (unboxed-integer) enum method call (#731): the receiver is a
            // backing integer, not a struct, so the generic struct path below
            // would reject it. Dispatch directly to the real `{Enum}__{method}`
            // (self by value) — the heap box and itable thunk exist only for the
            // interface-value path (`coerce`).
            if let Ty::Enum(id) = bty {
                let ename = self
                    .ctx
                    .enums
                    .values()
                    .find(|e| !e.is_sum && e.enum_id == id)
                    .map(|e| e.enum_name.clone())
                    .ok_or_else(|| err("method call on an unregistered enum value"))?;
                let mangled = format!("{ename}__{name}");
                return self.call_resolved_method(recv, name, mangled, args);
            }
            let Ty::Struct(sid) = bty else {
                return Err(err("method call on a non-struct value"));
            };
            // Borrowed `strview` (#1082): its physical layout is the tuple
            // `(str, int64, int64)`, so route its read-only methods to the
            // view-ABI lowering before the generic struct path. Checked by the
            // interned tuple id; a plain `(str, int64, int64)` tuple has no
            // methods (sema rejects them), so only a real `strview` lands here.
            if self.is_strview_sid(sid) {
                return self.lower_strview_method(recv, name, args);
            }
            if self.is_bytesview_sid(sid) {
                return self.lower_bytesview_method(recv, name, args);
            }
            // Arena (#553, D-0158): `a.alloc(bytes, n)` / `a.new(T, ...)` on an
            // arena handle lower to the runtime region allocator, not a struct
            // method call. Sema has already enforced leaf-only and the escape rule.
            if is_arena_struct(self.ctx, sid) {
                return self.lower_arena_method(recv, name, args);
            }
            // `Atomic[T]` (I#176): a method on a monomorphized `Atomic` box
            // (`Atomic$int64`, `Atomic$str`, …) lowers to an inline LLVM atomic on
            // its cell, not to a struct-method call. Recognized by the mangled
            // name prefix, exactly like the niche-`Optional` layout.
            if is_atomic_struct(self.ctx, sid) {
                return self.lower_atomic_method(recv, sid, name, args);
            }
            // Dynamic dispatch through an interface value (D-0016): the receiver
            // is an interface fat pointer, not a concrete struct.
            if let Some(iid) = self.ctx.iid_of_fat_sid(sid) {
                return self.lower_iface_call(recv, iid, name, args);
            }
            let mangled = format!("{}__{}", self.ctx.names[sid as usize], name);
            self.call_resolved_method(recv, name, mangled, args)
        } else {
            // A function value produced by an arbitrary expression (I#163),
            // e.g. `(get_fn())(x)` or a list element `fns[i](x)`: evaluate the
            // callee and, if it is a first-class function value, call indirectly.
            let (cv, ct) = self.expr(callee)?;
            if let Ty::Func(idx) = ct {
                return self.lower_closure_call(cv, idx, args);
            }
            Err(err(
                "only direct function calls are supported by codegen yet",
            ))
        }
    }

    /// Lower a direct method call to a resolved (statically known) target
    /// `mangled`, with `recv` already lowered as `self`. Shared by the struct
    /// method path and the tier-1 enum method path (#731): a generator method
    /// (I#429) constructs its state struct instead, and otherwise the explicit
    /// arguments are assembled into declaration order — filling omitted defaults
    /// and placing out-of-order keyword arguments (I#481) — then the call is
    /// emitted (an `invoke` inside a `try`, else a plain `call`). `name` is used
    /// only for the "no such method" diagnostic.
    fn call_resolved_method(
        &mut self,
        recv: Value,
        name: &str,
        mangled: String,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // A generator method call `obj.gen(args)` (I#429) constructs the
        // compiler-generated state struct rather than calling a function; its body
        // was compiled into the state struct's `__next__` (like a free generator,
        // I#67). Tier-1 enum method generators are rejected in sema, so this only
        // fires for structs / tier-3 enums.
        if let Some(gi) = self.ctx.generators.get(&mangled).cloned() {
            return self.construct_generator_method(&gi, recv, args);
        }
        let (ptys, ret) = self
            .sigs
            .get(&mangled)
            .cloned()
            .ok_or_else(|| err(format!("no method '{name}' on that value")))?;
        let ctx = self.ctx;
        // A method call carrying `*`/`**` spreads (#597), or whose callee has a
        // variadic `TYPE... args` (#1218, D-0038) or a `V **name` keyword
        // collector (#601), is assembled by the shared path: it collects the
        // trailing arguments into the single `list` the callee expects (an empty
        // list for a bare call, so the arg count always matches) and forwards
        // `self` first. Without this the call site passed the receiver alone or
        // the receiver plus each vararg separately — an arg-count mismatch that
        // only the native/LLVM backend rejects ("Incorrect number of arguments
        // passed to called function!").
        let has_spread = args.iter().any(|a| a.spread != ast::ArgSpread::None);
        let callee_collects = ctx
            .call_params
            .get(&mangled)
            .is_some_and(|pi| pi.iter().any(|p| p.variadic || p.kwargs));
        if has_spread || callee_collects {
            return self.lower_call_assembled(&mangled, Some(recv), &ptys, ret, args);
        }
        // `self` is passed first (by reference for a struct receiver, by value for
        // a tier-1 enum's backing integer); the remaining arguments are copied
        // when they are value structs (D-0051). An inline-eligible argument is
        // copied by the register/sret ABI, so no explicit copy is emitted for it
        // (see `copy_for_pass`).
        let mut argvals = vec![recv]; // self
                                      // Default arguments and out-of-order keyword arguments on methods
                                      // (I#481, parity with free functions I#298): assemble the explicit
                                      // arguments into declaration order, filling omitted defaulted
                                      // parameters, before lowering. The recorded `call_params` drop the
                                      // implicit `self`, so a plan slot `i` corresponds to parameter type
                                      // `ptys[i + 1]`. A method with no recorded metadata (e.g. a derived
                                      // method) keeps the positional lowering path; the variadic case is
                                      // handled above by the shared assembled path (#1218).
        let plan = match ctx.call_params.get(&mangled) {
            Some(pinfo) if !pinfo.iter().any(|p| p.variadic) => {
                Some((pinfo, plan_call_args(&mangled, pinfo, args)?))
            }
            _ => None,
        };
        if let Some((pinfo, plan)) = plan {
            for (i, slot) in plan.iter().enumerate() {
                // Either an expression written at the call site or the
                // omitted parameter's default (lowered fresh here, so it is
                // recomputed per call — D-0038).
                let e: &Expr = match slot {
                    ArgSlot::Given(j) => &args[*j].value,
                    ArgSlot::Default(p) => pinfo[*p].default.as_ref().unwrap(),
                };
                let v = match ptys.get(i + 1) {
                    Some(&target) => {
                        let v = self.expr_coerced(e, target, zapc_diag::ErrorCode::ZAP4021)?;
                        self.copy_for_pass(v, target, e)
                    }
                    None => {
                        let (v, vt) = self.expr(e)?;
                        self.copy_for_pass(v, vt, e)
                    }
                };
                argvals.push(v);
            }
        } else {
            for (i, a) in args.iter().enumerate() {
                let (v, vt) = self.expr(&a.value)?;
                let target = *ptys.get(i + 1).unwrap_or(&vt);
                let v = self.coerce(v, vt, target);
                let v = self.copy_for_pass(v, target, &a.value);
                argvals.push(v);
            }
        }
        let dest = if ret == Ty::Void {
            None
        } else {
            Some(self.new_value(ret))
        };
        self.throwing_call(dest, &mangled, argvals);
        let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
        Ok((rv, if ret == Ty::Void { Ty::I32 } else { ret }))
    }

    /// Lower an **indirect call** through a first-class function value (I#163):
    /// coerce each argument to the value's signature and emit an
    /// [`Inst::CallClosure`] (an `invoke` on an exception edge). `callee` is the
    /// closure-record pointer; `sig_idx` indexes the interned signature.
    fn lower_closure_call(
        &mut self,
        callee: Value,
        sig_idx: u32,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let sig = self.ctx.fn_sigs.borrow().get(sig_idx).clone();
        let mut argvals = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let v = match sig.params.get(i) {
                Some(&target) => {
                    let v = self.expr_coerced(&a.value, target, zapc_diag::ErrorCode::ZAP4021)?;
                    self.copy_for_pass(v, target, &a.value)
                }
                None => {
                    let (v, vt) = self.expr(&a.value)?;
                    self.copy_for_pass(v, vt, &a.value)
                }
            };
            argvals.push(v);
        }
        let dest = if sig.ret == Ty::Void {
            None
        } else {
            Some(self.new_value(sig.ret))
        };
        let unwind = self.landing_pads.last().copied();
        if let Some(lp) = unwind {
            self.used_lps.insert(lp);
        }
        self.emit(Inst::CallClosure {
            dest,
            callee,
            args: argvals,
            params: sig.params.clone(),
            ret: sig.ret,
            unwind,
        });
        let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
        Ok((
            rv,
            if sig.ret == Ty::Void {
                Ty::I32
            } else {
                sig.ret
            },
        ))
    }

    /// Does a value of ZIR type `it` denote a bare **payload** for an
    /// `Optional[T]` whose payload type is `payload` — i.e. a `T` that must be
    /// wrapped as `Some(T)` (#1262)? True for an exact payload match (a
    /// reference struct, a `str`/`bytes` pointer, a nested `Optional`, or the
    /// same scalar), the numeric widenings/narrowings `coerce` performs (an
    /// `int` literal into `Optional[int32]`, into `Optional[float]`, or into
    /// `Optional[bigint]`/`Optional[complex]`), and a concrete struct / tier-1
    /// enum flowing into an `Optional[SomeInterface]` (where `coerce` builds the
    /// fat pointer). It is deliberately conservative: an unrelated source — or
    /// one that is already a *different* `Optional` — returns false and is left
    /// to the caller's `coerce` / sema (a genuine mismatch is rejected there,
    /// never silently wrapped). The already-`Optional[T]` self case (an explicit
    /// `Optional.Some(...)`, an Optional-typed binding) is handled separately by
    /// the `it == target` guard at the wrap site, so it is never reached here.
    fn opt_payload_source(&self, it: &Ty, payload: Ty) -> bool {
        if *it == payload {
            return true;
        }
        let it_num = it.int_width().is_some() || it.is_float();
        let payload_num = payload.int_width().is_some()
            || payload.is_float()
            || payload == Ty::Bigint
            || payload == Ty::Complex;
        if it_num && payload_num {
            return true;
        }
        // A concrete struct / tier-1 enum into an interface-typed payload: the
        // inner `coerce` builds the `{data, itable}` fat pointer (D-0016), so the
        // boxed interface value is a valid `Some` payload.
        if let Ty::Struct(psid) = payload {
            if self.ctx.iid_of_fat_sid(psid).is_some() && matches!(it, Ty::Struct(_) | Ty::Enum(_))
            {
                return true;
            }
        }
        false
    }

    /// Collection literals take their element kinds from the destination when
    /// known (#637). In particular, an empty `{}` must not default to
    /// dict[int64, int64] when appended to list[dict[str, bool]]. Recurse through
    /// nested literals, but leave named values alone: a real type mismatch must
    /// still be rejected, never erased by relabelling its pointer.
    fn expr_collection_context(&mut self, e: &Expr, target: Ty) -> Result<(Value, Ty), LowerError> {
        // A bare `None` collection element / contextual value materializes the
        // canonical value for `target` (#1075, #1080), the same way `expr_coerced`
        // does for arguments and returns. Routing it through `lower_none_expr`
        // here covers list/dict literal elements, list comprehensions, and
        // index/key assignment, so `list[Optional[T]] = [None]` no longer bails.
        if matches!(e, Expr::None(_)) {
            return Ok((self.lower_none_expr(target)?, target));
        }
        // A bare payload value in an `Optional[T]` position auto-wraps to
        // `Some(payload)` (#1262) — the symmetric partner of the bare-`None`
        // lowering above (`lower_none_expr`). Without it a scalar keyword such as
        // `Delta(day=1)` stored the raw `int` straight into the Optional field
        // (`iconst` + `fieldset`, no tag or allocation), so a later `.unwrap()`
        // dereferenced the integer as an Optional object and segfaulted (a
        // memory-safety violation in a checked program). This is the single
        // chokepoint every type-contextual position flows through: constructor
        // fields, field assignment, call arguments and returns reach it via
        // `expr_coerced`; collection elements (`list[Optional[T]]`, dict values,
        // `.append`, index/key assignment) reach it directly. Sema currently
        // accepts a bare payload only in the constructor-field, field/index/key
        // assignment, and `.append` positions (the ones that crashed); it still
        // rejects `return` / parameter / typed-binding / list-literal payloads
        // with a clean type error, so the wrap there is defensive and inert until
        // that implicit-widening design decision (#1265) is taken. See
        // `decisions/lang/1262-optional-scalar-coercion.md`.
        //
        // The value is lowered against the *payload* type (so a nested
        // collection payload still inherits its element kinds), then tagged via
        // the shared `gen_opt_some` — the same already-GC-rooted
        // `StructNew`/`FieldSet` construction an explicit `Optional.Some(...)`
        // emits, so no new rooted `Inst` and no raising runtime call are
        // introduced. An already-`Optional[T]` source (an Optional-typed binding,
        // `Optional.Some(...)`, a ternary of Optionals) lowers to the Optional
        // type itself and is returned unwrapped, so it is never double-wrapped;
        // an incompatible source passes through unchanged for the caller's
        // `coerce` / sema to reject (never silently wrapped).
        if let Some(opt) = optional_layout(self.ctx, target) {
            let (iv, it) = self.expr_collection_context(e, opt.payload_ty)?;
            if it == target {
                return Ok((iv, target));
            }
            if self.opt_payload_source(&it, opt.payload_ty) {
                let inner = self.coerce(iv, it, opt.payload_ty);
                let some = gen_opt_some(self, &opt, inner);
                return Ok((some, target));
            }
            return Ok((iv, it));
        }
        match (e, target) {
            (Expr::List(items, _), Ty::List(elem)) => {
                self.lower_list_lit_expected(items, Some(elem))
            }
            (Expr::ListComp { elem, clauses, .. }, Ty::List(ek)) => {
                self.lower_list_comp_expected(elem, clauses, Some(ek))
            }
            (Expr::Dict(items, _), Ty::Dict(key, value)) => {
                self.lower_dict_lit_expected(items, Some((key, value)))
            }
            // A tuple literal against a declared tuple type (#1195): thread each
            // declared field type to its element, so a bare `None` element
            // lowers as that field's `Optional[T]` null (e.g.
            // `(Optional[bytes], Optional[bytes])` returned as `(None, None)`).
            // A non-tuple `Ty::Struct` destination (a user struct) has no field
            // types here, so it falls through to the natural lowering.
            (Expr::Tuple(elems, _), Ty::Struct(sid)) => {
                // `fields` returns an owned clone, so no `tuples` borrow is held
                // across the `&mut self` lowering call below.
                let ftys = self.ctx.tuples.borrow().fields(sid);
                match ftys {
                    Some(ftys) => self.lower_tuple_lit_expected(elems, Some(&ftys)),
                    None => self.expr(e),
                }
            }
            (
                Expr::IfElse {
                    cond, then, els, ..
                },
                Ty::List(_) | Ty::Dict(..),
            ) => {
                // A conditional whose value is a list/dict: evaluate ONLY the
                // chosen branch (#776), merging through a slot of the known
                // `target` type (not an eager `Select` of both branches).
                let (cv, cty) = self.expr(cond)?;
                let c = self.truthy(cv, cty);
                let slot = self.alloca(target);
                let then_bb = self.new_block();
                let else_bb = self.new_block();
                let join_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: c,
                    then_bb,
                    else_bb,
                });
                self.position(then_bb);
                let (a, at) = self.expr_collection_context(then, target)?;
                if at != target {
                    return Err(err(format!(
                        "conditional collection branch must match expected `{}` (found `{}`)",
                        self.render_ty(&target),
                        self.render_ty(&at)
                    )));
                }
                self.store(slot, a);
                self.ensure_br(join_bb);
                self.position(else_bb);
                let (b, bt) = self.expr_collection_context(els, target)?;
                if bt != target {
                    return Err(err(format!(
                        "conditional collection branch must match expected `{}` (found `{}`)",
                        self.render_ty(&target),
                        self.render_ty(&bt)
                    )));
                }
                self.store(slot, b);
                self.ensure_br(join_bb);
                self.position(join_bb);
                Ok((self.load(slot, target), target))
            }
            _ => self.expr(e),
        }
    }

    /// Lower a bare `None` literal in a position whose expected ZIR type is
    /// `target` — the single type-contextual `None` mechanism shared by call
    /// arguments / returns (`expr_coerced`), struct-constructor fields
    /// (`fill_struct_fields`), and collection elements
    /// (`expr_collection_context`, list/dict literals + comprehensions).
    ///
    /// Mono rewrites `None` into `Optional[T].None` wherever it threads an
    /// expected type (plain bindings, returns — Fork I/O), but *not* for call
    /// arguments, defaulted parameters, struct-constructor fields, or collection
    /// elements, so a bare `Expr::None` reaches lowering with only the
    /// destination type to go on. The destination's `None` is NOT always a raw
    /// null pointer: a scalar-payload `Optional` tags its discriminant, so a null
    /// would make `is_some()`/`is_none()` dereference nil at native runtime
    /// (#564). `gen_opt_none` yields the tagged `None` for a scalar payload and
    /// the null pointer for a niche/pointer payload, so niche `Optional`s are
    /// byte-identical to the old behaviour.
    ///
    /// Only call this when the expression is `Expr::None`.
    fn lower_none_expr(&mut self, target: Ty) -> Result<Value, LowerError> {
        if target.is_ptr() {
            if let Some(opt) = optional_layout(self.ctx, target) {
                return Ok(gen_opt_none(self, &opt));
            }
            // A plain reference type (a struct instance, `str`, or a collection):
            // its null value IS the null pointer. Member access on it then throws
            // (D-0007, EXC-4), enforced by the null-guard at the access site.
            // Value structs are not nullable (D-0014), so `None` is rejected.
            if let Ty::Struct(sid) = target {
                if self.ctx.is_value_struct(sid) {
                    return Err(err(
                        "None cannot be assigned to a value struct (value types are \
                         not nullable, D-0014)",
                    ));
                }
            }
            let d = self.new_value(target);
            self.emit(Inst::NullConst { dest: d });
            return Ok(d);
        }
        // A non-reference destination (a scalar `int`/`float`/`bool`/`char`/enum)
        // cannot hold `None`: reject a bare `None` supplied for a non-optional
        // field, element, or argument with a clear type error rather than
        // deferring to the generic "unsupported expression" bail (#1075, #1080).
        Err(err(format!(
            "None is not a valid `{}` value (only optional and reference types \
             are nullable)",
            self.render_ty(&target)
        )))
    }

    /// Lower an expression in a position whose expected ZIR type is `target`
    /// (I#163): a `lambda` is lowered against the expected function type (the
    /// only way its parameter types are known). Collection literals also inherit
    /// their destination element kinds; other expressions lower normally and
    /// are coerced.
    fn expr_coerced(
        &mut self,
        e: &Expr,
        target: Ty,
        mismatch_code: zapc_diag::ErrorCode,
    ) -> Result<Value, LowerError> {
        if let (Expr::Lambda { params, body, .. }, Ty::Func(idx)) = (e, target) {
            // A `lambda` takes its parameter types from the expected signature,
            // so it matches `target` by construction (`lower_lambda` checks the
            // parameter count) — no compatibility check needed here.
            let (v, _) = self.lower_lambda(params, body, idx)?;
            return Ok(v);
        }
        // A bare `None` in a type-contextual position lowers to the canonical
        // value for `target` (#564, #1075, #1080). See `lower_none_expr`.
        if matches!(e, Expr::None(_)) {
            return self.lower_none_expr(target);
        }
        let (v, vt) = self.expr_collection_context(e, target)?;
        // A function value coerced to a `fn(...)` slot (argument / assignment /
        // return) must have the *exact* structural signature of that slot
        // (I#465). First-class function values are monomorphic: the
        // `<fn>$fnval` thunk is emitted to the source function's own signature
        // while the indirect call site uses the expected `fn`-type, so any
        // disagreement on arity, parameter types, or return type miscompiles
        // under codegen. Reject it here with a precise type error, the same way
        // a direct call with a wrong argument is rejected.
        if let Ty::Func(t_idx) = target {
            self.check_fn_coercion(&vt, t_idx, mismatch_code)?;
        }
        Ok(self.coerce(v, vt, target))
    }

    /// Reject coercing a **function value with a known signature** to a
    /// `fn(...)` type whose signature differs (I#465, D-0067). First-class
    /// function values are monomorphic; there is no fn-subtyping, so an exact
    /// match on arity, each parameter type, and the return type is required (a
    /// coercing adapter thunk — e.g. dropping defaulted trailing parameters — is
    /// a separate design decision tracked with #466).
    ///
    /// The check fires only when the source is itself a `Ty::Func`, i.e. a value
    /// whose exact signature is *known* to disagree (a bare function name, a
    /// lambda-typed binding, a struct field read). A value that has already lost
    /// its signature — a collection element or other uniform reference word read
    /// back as a plain `ptr` — carries no contradicting signature, so
    /// re-attaching the expected call signature is the legitimate D-0067
    /// round-trip (see `fnval_store`), not a mismatch, and is left to `coerce`.
    fn check_fn_coercion(
        &self,
        from: &Ty,
        to_idx: u32,
        code: zapc_diag::ErrorCode,
    ) -> Result<(), LowerError> {
        // Interned signatures are deduplicated, so equal signatures share the
        // same index — a matching `Ty::Func` is exactly `Ty::Func(to_idx)`, and
        // any other `Ty::Func(from_idx)` is a genuine structural mismatch.
        let Ty::Func(from_idx) = from else {
            // A source that is NOT a function value. Two cases are legitimate
            // and must pass (#1104 must not over-reject):
            //  * `Ty::Ptr` — a function value whose signature was erased (read
            //    back from a collection element, a uniform reference word).
            //    Re-attaching the expected call signature is the D-0067
            //    round-trip, not a mismatch (see `container_roundtrip_accepted`).
            //  * `Ty::Py` — a dynamically-typed value; stay permissive.
            if matches!(from, Ty::Ptr | Ty::Py) {
                return Ok(());
            }
            // Anything else (a scalar, a struct, a concrete collection, ...) is
            // definitely not a function, so coercing it to a `fn(...)` slot would
            // feed the wrong word kind into a closure-record pointer (e.g. an
            // `iconst` into a `ptr`) — a silent miscompile (#1104). Reject it
            // with the same code a normal argument/parameter mismatch uses, the
            // sibling of the #465 signature-mismatch check.
            let expected = self.ctx.fn_sigs.borrow().get(to_idx).clone();
            let want = self.render_fn_sig(&expected);
            return Err(LowerError {
                code,
                message: format!(
                    "value of type `{}` is not a function, but `{want}` is expected",
                    self.render_ty(from)
                ),
                derive: None,
            });
        };
        if *from_idx == to_idx {
            return Ok(());
        }
        let expected = self.ctx.fn_sigs.borrow().get(to_idx).clone();
        let want = self.render_fn_sig(&expected);
        let got_sig = self.ctx.fn_sigs.borrow().get(*from_idx).clone();
        let got = self.render_fn_sig(&got_sig);
        let detail = if got_sig.params.len() != expected.params.len() {
            format!(
                "it takes {} argument(s), but `{want}` takes {}",
                got_sig.params.len(),
                expected.params.len()
            )
        } else if let Some((i, (g, e))) = got_sig
            .params
            .iter()
            .zip(expected.params.iter())
            .enumerate()
            .find(|(_, (g, e))| g != e)
        {
            format!(
                "parameter {} is `{}`, but `{}` is expected",
                i + 1,
                self.render_ty(g),
                self.render_ty(e)
            )
        } else {
            format!(
                "its return type `{}` is not `{}`",
                self.render_ty(&got_sig.ret),
                self.render_ty(&expected.ret)
            )
        };
        let message = format!(
            "function value of type `{got}` is not compatible with the \
             expected type `{want}`: {detail}"
        );
        Err(LowerError {
            code,
            message,
            derive: None,
        })
    }

    /// Render a ZIR type as a readable source-ish type for a diagnostic: a
    /// struct id resolves to its declared name, a function type to its
    /// `fn(...) -> ...` form, and everything else to its ZIR spelling.
    fn render_ty(&self, t: &Ty) -> String {
        match t {
            Ty::Struct(sid) => self
                .ctx
                .names
                .get(*sid as usize)
                .cloned()
                .unwrap_or_else(|| format!("struct#{sid}")),
            Ty::Func(idx) => {
                let sig = self.ctx.fn_sigs.borrow().get(*idx).clone();
                self.render_fn_sig(&sig)
            }
            other => other.to_string(),
        }
    }

    /// Render an interned function signature as `fn(P, Q) -> R` for diagnostics.
    fn render_fn_sig(&self, sig: &FnSig) -> String {
        let params = sig
            .params
            .iter()
            .map(|p| self.render_ty(p))
            .collect::<Vec<_>>()
            .join(", ");
        format!("fn({params}) -> {}", self.render_ty(&sig.ret))
    }

    /// Lower a method call on an interface-typed receiver to **dynamic dispatch**
    /// (D-0016): resolve the method's slot in the interface's dispatch table and
    /// emit an [`Inst::IfaceCall`] / [`Inst::IfaceInvoke`] that loads the slot
    /// from the fat pointer's itable and calls it with `data` as `self`. The
    /// callee's signature is the interface's declared one, so arguments are
    /// coerced to the interface method's parameter types.
    fn lower_iface_call(
        &mut self,
        iface: Value,
        iid: u32,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let iface_info = &self.ctx.interfaces[iid as usize];
        let method = iface_info
            .methods
            .iter()
            .position(|m| m.name == name)
            .ok_or_else(|| {
                err(format!(
                    "interface '{}' has no method '{name}'",
                    iface_info.name
                ))
            })? as u32;
        // Clone the slot signature (ends the borrow of `self.ctx`).
        let msig = self.ctx.interfaces[iid as usize].methods[method as usize].clone();
        let mut argvals = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let (v, vt) = self.expr(&a.value)?;
            let target = *msig.params.get(i).unwrap_or(&vt);
            let v = self.coerce(v, vt, target);
            let v = self.copy_for_pass(v, target, &a.value);
            argvals.push(v);
        }
        let dest = if msig.ret == Ty::Void {
            None
        } else {
            Some(self.new_value(msig.ret))
        };
        if let Some(&lp) = self.landing_pads.last() {
            self.used_lps.insert(lp);
            self.emit(Inst::IfaceInvoke {
                dest,
                iface,
                iid,
                method,
                args: argvals,
                unwind: lp,
            });
        } else {
            self.emit(Inst::IfaceCall {
                dest,
                iface,
                iid,
                method,
                args: argvals,
            });
        }
        let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
        Ok((
            rv,
            if msig.ret == Ty::Void {
                Ty::I32
            } else {
                msig.ret
            },
        ))
    }

    /// Lower an `arena` method call (#553, D-0158): `a.alloc(bytes, n)` to a
    /// non-moving byte buffer (`zap_arena_alloc_bytes`), and `a.new(T, ...)` to a
    /// leaf struct bump-allocated in the arena (`Inst::ArenaNew` →
    /// `zap_arena_alloc_atomic`) with its fields initialized like `construct`. Both
    /// guard the result against a NULL (cap / heap-OOM) return by raising through
    /// `zap_arena_oom`. Sema (`check_arena_method`) has already enforced the
    /// argument shape and leaf-only rule, so this trusts them.
    fn lower_arena_method(
        &mut self,
        recv: Value,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "alloc" => {
                // `a.alloc(bytes, n)`: arg0 is the element type (`bytes`, per sema);
                // arg1 is the byte count.
                let n = if args.len() >= 2 {
                    self.expr(&args[1].value)?.0
                } else {
                    self.iconst(Ty::I64, 0)
                };
                let dest = self.new_value(Ty::Bytes);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_arena_alloc_bytes".to_string(),
                    args: vec![recv, n],
                });
                self.arena_oom_guard(dest);
                Ok((dest, Ty::Bytes))
            }
            "new" => {
                // `a.new(T, field_args...)`: arg0 is the leaf struct type.
                let Some(first) = args.first() else {
                    return Err(err("a.new(T, ...) requires a type argument"));
                };
                let Expr::Name(tname, _) = &first.value else {
                    return Err(err("a.new(T, ...)'s first argument must be a struct type"));
                };
                let Some(&tsid) = self.ctx.ids.get(tname) else {
                    return Err(err(format!("a.new: unknown struct type '{tname}'")));
                };
                let dest = self.new_value(Ty::Struct(tsid));
                self.emit(Inst::ArenaNew {
                    dest,
                    sid: tsid,
                    arena: recv,
                });
                self.arena_oom_guard(dest);
                self.fill_struct_fields(dest, tsid, &args[1..])?;
                Ok((dest, Ty::Struct(tsid)))
            }
            other => Err(err(format!(
                "`arena` has no method '{other}' (use `alloc`/`new`)"
            ))),
        }
    }

    /// Raise `zap_arena_oom` (a catchable error) when an arena allocation returned
    /// NULL (cap exceeded / heap OOM). The raise is a `throwing_call`, so inside the
    /// arena's `with` it unwinds to the block's landing pad and the arena is torn
    /// down (`Cleanup::ArenaClose`) on the way out.
    fn arena_oom_guard(&mut self, obj: Value) {
        let isnull = self.new_value(Ty::I1);
        self.emit(Inst::IsNull {
            dest: isnull,
            val: obj,
        });
        let oom_bb = self.new_block();
        let ok_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: isnull,
            then_bb: oom_bb,
            else_bb: ok_bb,
        });
        self.position(oom_bb);
        self.throwing_call(None, "zap_arena_oom", vec![]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        self.position(ok_bb);
    }

    fn construct(&mut self, sid: u32, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        self.fill_struct_fields(dest, sid, args)?;
        Ok((dest, Ty::Struct(sid)))
    }

    /// Initialize the fields of an already-allocated struct `dest` of type `sid`
    /// from `args` (supplied values + declared defaults) and run its `@validate` /
    /// `@range` checks. Shared by the heap `construct` and the arena `a.new(...)`
    /// path (#553), which differ only in how `dest` is allocated.
    fn fill_struct_fields(
        &mut self,
        dest: Value,
        sid: u32,
        args: &[ast::Arg],
    ) -> Result<(), LowerError> {
        // Track which field slots the caller supplies, so every omitted field
        // that carries a declared default (D-0038) is still initialized (#461).
        let mut set = vec![false; self.ctx.fields[sid as usize].len()];
        for (i, a) in args.iter().enumerate() {
            let (idx, fty) = match &a.name {
                Some(fname) => self
                    .ctx
                    .field(sid, fname)
                    .ok_or_else(|| err(format!("no field '{fname}' on struct")))?,
                None => {
                    let f = self.ctx.fields[sid as usize]
                        .get(i)
                        .ok_or_else(|| err("too many fields in constructor"))?;
                    (i as u32, f.1)
                }
            };
            // Lower against the field's declared type so a bare `None` for an
            // `Optional[T]` field materializes the canonical empty option (#1075),
            // a `fn(...)` field accepts a lambda (I#163), and a collection field
            // inherits its element kinds — matching field assignment
            // (`obj.field = value`), which already uses `expr_coerced`.
            let v = self.expr_coerced(&a.value, fty, zapc_diag::ErrorCode::ZAP4001)?;
            // A value-struct field is copied in (D-0051): the new struct holds an
            // independent instance of the aggregate.
            let v = self.copy_if_value(v, fty, &a.value);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx,
                val: v,
            });
            if let Some(slot) = set.get_mut(idx as usize) {
                *slot = true;
            }
        }
        // Fill omitted fields from their declared defaults (D-0038, #461), in
        // declaration order. Without this a defaulted field the caller omits
        // (`Box()` vs `Box(xs=[])`) is left null — a silent null read for
        // pointer-typed defaults and a SIGSEGV when that null is used as, e.g.,
        // a list. The default is lowered fresh per construction in the caller's
        // scope, matching the explicit-argument path (same coerce + value-struct
        // copy), so a mutable default like `[]` is a distinct object each time.
        for (idx, is_set) in set.iter().enumerate() {
            if *is_set {
                continue;
            }
            let default = self
                .ctx
                .field_defaults
                .get(sid as usize)
                .and_then(|d| d.get(idx))
                .and_then(|d| d.clone());
            let Some(default) = default else {
                continue;
            };
            let fty = self.ctx.fields[sid as usize][idx].1;
            // Defaults lower against the field type too, so a `None` default for
            // an `Optional[T]` field (#461 adjacent gap noted under #564) becomes
            // the canonical empty option rather than a stray null.
            let v = self.expr_coerced(&default, fty, zapc_diag::ErrorCode::ZAP4001)?;
            let v = self.copy_if_value(v, fty, &default);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: idx as u32,
                val: v,
            });
        }
        // Run the generated field validators (`@validate`/`@range`, D-0101) once
        // every field is initialized. The call may raise a `ValueError`
        // (D-0007); routing it through `throwing_call` makes it an `invoke`
        // inside a `try`, so construction errors are catchable at the site.
        if self.ctx.field_checks.contains_key(&sid) {
            let validator = format!("{}____validate__", self.ctx.names[sid as usize]);
            self.throwing_call(None, &validator, vec![dest]);
        }
        Ok(())
    }

    /// Construct a generator's state struct for a call `gen(args)` (I#67).
    ///
    /// Allocates the (zeroed → `$state == 0`) state struct and stores each
    /// argument into its parameter field, so the first `__next__` starts the body
    /// from the top with the captured arguments. Arguments may be positional or
    /// keyword (matched to parameter names), mirroring an ordinary call.
    fn construct_generator(
        &mut self,
        gi: &GeneratorInfo,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if args.len() != gi.n_params {
            return Err(err(format!(
                "generator '{}' expects {} argument(s), got {}",
                gi.name,
                gi.n_params,
                args.len()
            )));
        }
        let sid = gi.sid;
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (i, a) in args.iter().enumerate() {
            let p = match &a.name {
                Some(kw) => gi.vars[..gi.n_params]
                    .iter()
                    .position(|(n, _)| n == kw)
                    .ok_or_else(|| {
                        err(format!("generator '{}' has no parameter '{kw}'", gi.name))
                    })?,
                None => i,
            };
            let pty = gi.vars[p].1;
            let (v, vt) = self.expr(&a.value)?;
            let v = self.coerce(v, vt, pty);
            // Capturing a value-struct argument copies it (D-0051), like any
            // by-value parameter pass.
            let v = self.copy_for_pass(v, pty, &a.value);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: gen_var_field(p),
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// Construct a **generator method**'s state struct (I#429) for a call
    /// `obj.gen(args)`. Identical to [`Self::construct_generator`] except the
    /// receiver `recv` (already lowered) is captured into the leading `self`
    /// field; the explicit `args` fill the remaining parameter fields. `self` is
    /// thus just another captured value in the generator frame.
    fn construct_generator_method(
        &mut self,
        gi: &GeneratorInfo,
        recv: Value,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // Param 0 is `self`; the remaining `n_params - 1` are the user arguments.
        let want = gi.n_params - 1;
        if args.len() != want {
            return Err(err(format!(
                "generator method '{}' expects {} argument(s), got {}",
                gi.name,
                want,
                args.len()
            )));
        }
        let sid = gi.sid;
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        // Capture `self` (the receiver) into field 0 of the parameter block. The
        // receiver is captured as-is — passed by reference like any method `self`
        // (D-0051), so iteration reads the live receiver.
        self.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: gen_var_field(0),
            val: recv,
        });
        // Capture the explicit arguments into params `1..n_params` (positional or
        // by keyword, matching the free-generator constructor).
        for (i, a) in args.iter().enumerate() {
            let p = match &a.name {
                Some(kw) => gi.vars[1..gi.n_params]
                    .iter()
                    .position(|(n, _)| n == kw)
                    .map(|j| j + 1)
                    .ok_or_else(|| {
                        err(format!(
                            "generator method '{}' has no parameter '{kw}'",
                            gi.name
                        ))
                    })?,
                None => i + 1,
            };
            let pty = gi.vars[p].1;
            let (v, vt) = self.expr(&a.value)?;
            let v = self.coerce(v, vt, pty);
            let v = self.copy_for_pass(v, pty, &a.value);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: gen_var_field(p),
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    // --- collections (ticket #21) ---
    //
    // Lists and dicts lower to runtime calls (like strings), so the ZIR -> LLVM
    // step stays mechanical. The element/key/value `Elem` kind chosen here
    // selects the typed entry point (`..._i64` / `..._ptr` / `..._str`).

    // --- numeric arrays (ticket #24) ---
    //
    // Construction, element access, `len`, and `sum` lower to runtime calls
    // (like collections). Only the element-wise ops (`binop`) use a dedicated
    // ZIR node so codegen can emit SIMD; everything else stays a plain call.

    /// `array[dtype]([...])` — allocate an array from a list literal and fill it
    /// element-by-element in row-major order. A flat literal builds a 1-D array;
    /// a **nested rectangular** literal builds an N-D array whose static rank is
    /// the nesting depth and whose shape is the per-level length (`[[1, 2],
    /// [3, 4]]` -> shape `(2, 2)`, #500). Ragged (non-rectangular) literals are
    /// rejected. Non-literal arguments (`array[f64](xs)`) and `zeros`/`arange`
    /// constructors remain deferred (#504).
    fn lower_array_new(
        &mut self,
        dtype_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let Expr::Name(dname, _) = dtype_expr else {
            return Err(err("array[...] expects a dtype name, e.g. array[f64]"));
        };
        let dtype = dtype_from_name(dname)?;
        if args.len() != 1 || args[0].name.is_some() {
            return Err(err("array[T](...) takes one positional list literal"));
        }
        let Expr::List(elems, _) = &args[0].value else {
            return Err(err(
                "array[T](...) takes a list literal, e.g. array[f64]([1.0, 2.0, 3.0]) \
                 or a nested one array[f64]([[1.0, 2.0], [3.0, 4.0]]) \
                 (non-literal construction is not supported yet)",
            ));
        };

        // The shape is read by descending into the first child while it is a
        // list; `flatten_array_literal` then validates every other row against
        // that shape and collects the scalar leaves in row-major order.
        let mut shape: Vec<i64> = vec![elems.len() as i64];
        {
            let mut cur = elems;
            while let Some(Expr::List(inner, _)) = cur.first() {
                shape.push(inner.len() as i64);
                cur = inner;
            }
        }
        if shape.len() > u8::MAX as usize {
            return Err(err("array rank (nesting depth) is too large"));
        }
        let ndim = shape.len() as u8;
        let mut flat: Vec<&Expr> = Vec::new();
        for c in elems {
            flatten_array_literal(c, &shape[1..], &mut flat)?;
        }
        let ty = Ty::Array(dtype, ndim);

        let code = self.iconst(Ty::I64, dtype.code());
        let arr = self.new_value(ty);
        if ndim == 1 {
            // 1-D: the historical single-dim construction (unchanged IR shape).
            let n = self.iconst(Ty::I64, shape[0]);
            self.emit(Inst::Call {
                dest: Some(arr),
                func: "zap_array_new".to_string(),
                args: vec![code, n],
            });
        } else {
            // N-D: allocate the rank-`ndim` header + total-element buffer, then
            // write each axis length before filling the flat elements.
            let total: i64 = shape.iter().product();
            let ndim_v = self.iconst(Ty::I64, ndim as i64);
            let total_v = self.iconst(Ty::I64, total);
            self.emit(Inst::Call {
                dest: Some(arr),
                func: "zap_array_new_nd".to_string(),
                args: vec![code, ndim_v, total_v],
            });
            for (axis, dim) in shape.iter().enumerate() {
                let ax = self.iconst(Ty::I64, axis as i64);
                let dv = self.iconst(Ty::I64, *dim);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_array_set_dim".to_string(),
                    args: vec![arr, ax, dv],
                });
            }
        }
        let setter = format!("zap_array_set_{}", dtype.suffix());
        for (i, e) in flat.iter().enumerate() {
            let (v, vt) = self.expr(e)?;
            // Numeric scalars fill any array; a `complex` element additionally
            // fills a `complex` array (#938). Real elements of a complex array
            // promote to `complex` via `coerce` (imag 0), matching NumPy's
            // `np.array([1, 2], dtype=complex)`.
            let elem_ok = vt.int_width().is_some()
                || vt.is_float()
                || (matches!(dtype, Dtype::Complex) && matches!(vt, Ty::Complex));
            if !elem_ok {
                return Err(err(
                    "array construction requires numeric scalars in a (possibly nested) list literal",
                ));
            }
            let v = self.coerce(v, vt, dtype.ty());
            let idx = self.iconst(Ty::I64, i as i64);
            self.emit(Inst::Call {
                dest: None,
                func: setter.clone(),
                args: vec![arr, idx, v],
            });
        }
        Ok((arr, ty))
    }

    /// The dtype named by a constructor subscript like `zeros[f64]`.
    fn ctor_dtype(&self, index: &Expr) -> Result<Dtype, LowerError> {
        let Expr::Name(dname, _) = index else {
            return Err(err(
                "an array constructor expects a dtype name, e.g. zeros[f64]((3, 4))",
            ));
        };
        dtype_from_name(dname)
    }

    /// Evaluate a constructor shape argument into one `i64` dim value per axis
    /// (#504). A tuple literal `(d0, d1, ...)` is N-D (rank = its arity); a single
    /// expression is 1-D. The rank is therefore static (as D-0164 requires); the
    /// dim *values* may be runtime. An empty shape `()` is rejected.
    fn array_shape_dims(&mut self, shape: &Expr) -> Result<Vec<Value>, LowerError> {
        let parts: Vec<&Expr> = match shape {
            Expr::Tuple(elems, _) => elems.iter().collect(),
            other => vec![other],
        };
        if parts.is_empty() {
            return Err(err("an array shape needs at least one dimension"));
        }
        if parts.len() > u8::MAX as usize {
            return Err(err("array rank (shape length) is too large"));
        }
        let mut dims = Vec::with_capacity(parts.len());
        for p in parts {
            let (v, t) = self.expr(p)?;
            if t.int_width().is_none() {
                return Err(err("array shape dimensions must be integers"));
            }
            dims.push(self.coerce(v, t, Ty::I64));
        }
        Ok(dims)
    }

    /// Allocate a rank-`dims.len()` array of `dtype` with the given (runtime) dim
    /// values: compute the element count as their product, allocate via
    /// `zap_array_new_nd`, then write each axis with `zap_array_set_dim`.
    fn array_alloc_shaped(&mut self, dtype: Dtype, dims: &[Value]) -> Value {
        let ndim = dims.len() as u8;
        let code = self.iconst(Ty::I64, dtype.code());
        let mut total = self.iconst(Ty::I64, 1);
        for &d in dims {
            let next = self.new_value(Ty::I64);
            self.emit(Inst::Bin {
                dest: next,
                op: BinOp::Mul,
                lhs: total,
                rhs: d,
            });
            total = next;
        }
        let ndim_v = self.iconst(Ty::I64, ndim as i64);
        let arr = self.new_value(Ty::Array(dtype, ndim));
        self.emit(Inst::Call {
            dest: Some(arr),
            func: "zap_array_new_nd".to_string(),
            args: vec![code, ndim_v, total],
        });
        for (axis, &d) in dims.iter().enumerate() {
            let ax = self.iconst(Ty::I64, axis as i64);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_array_set_dim".to_string(),
                args: vec![arr, ax, d],
            });
        }
        arr
    }

    /// `zeros[T](shape)` / `ones[T](shape)` / `full[T](shape, value)` (#504): a
    /// shaped array filled with 0 / 1 / a given scalar. The fill value crosses as
    /// an `i64` word (integer/bool dtypes) or an `f64` word (float dtypes).
    fn lower_array_fill_ctor(
        &mut self,
        index: &Expr,
        args: &[ast::Arg],
        fill: ArrayFill,
    ) -> Result<(Value, Ty), LowerError> {
        let dtype = self.ctor_dtype(index)?;
        let want = if matches!(fill, ArrayFill::Full) {
            2
        } else {
            1
        };
        if args.len() != want || args.iter().any(|a| a.name.is_some()) {
            return Err(err(if matches!(fill, ArrayFill::Full) {
                "full[T](shape, value) takes a shape and a fill value"
            } else {
                "zeros[T](shape) / ones[T](shape) take one shape argument"
            }));
        }
        let dims = self.array_shape_dims(&args[0].value)?;
        let ndim = dims.len() as u8;
        let arr = self.array_alloc_shaped(dtype, &dims);
        if dtype.is_float() {
            let fv = match fill {
                ArrayFill::Zeros => self.fconst(Ty::F64, 0.0),
                ArrayFill::Ones => self.fconst(Ty::F64, 1.0),
                ArrayFill::Full => {
                    let (v, vt) = self.expr(&args[1].value)?;
                    if vt.int_width().is_none() && !vt.is_float() {
                        return Err(err("full value must be a numeric scalar"));
                    }
                    self.coerce(v, vt, Ty::F64)
                }
            };
            self.emit(Inst::Call {
                dest: None,
                func: "zap_array_fill_f".to_string(),
                args: vec![arr, fv],
            });
        } else {
            let iv = match fill {
                ArrayFill::Zeros => self.iconst(Ty::I64, 0),
                ArrayFill::Ones => self.iconst(Ty::I64, 1),
                ArrayFill::Full => {
                    let (v, vt) = self.expr(&args[1].value)?;
                    if vt.int_width().is_none() && !vt.is_float() {
                        return Err(err("full value must be a numeric scalar"));
                    }
                    self.coerce(v, vt, Ty::I64)
                }
            };
            self.emit(Inst::Call {
                dest: None,
                func: "zap_array_fill_i".to_string(),
                args: vec![arr, iv],
            });
        }
        Ok((arr, Ty::Array(dtype, ndim)))
    }

    /// Evaluate a constructor scalar argument, coercing to `to` (`Ty::I64`/`F64`).
    fn ctor_scalar(&mut self, arg: &ast::Arg, to: Ty) -> Result<Value, LowerError> {
        let (v, t) = self.expr(&arg.value)?;
        if t.int_width().is_none() && !t.is_float() {
            return Err(err("array constructor arguments must be numeric scalars"));
        }
        Ok(self.coerce(v, t, to))
    }

    /// `arange[T](start, stop[, step])` (#504): a rank-1 array `start, start+step,
    /// ...` up to (excluding) `stop`; the length is a runtime value. Float dtypes
    /// use the `_f` runtime entry point, integer dtypes `_i`.
    fn lower_array_arange(
        &mut self,
        index: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let dtype = self.ctor_dtype(index)?;
        if !(2..=3).contains(&args.len()) || args.iter().any(|a| a.name.is_some()) {
            return Err(err(
                "arange[T](start, stop[, step]) takes two or three positional arguments",
            ));
        }
        let ty = Ty::Array(dtype, 1);
        let arr = self.new_value(ty);
        let code = self.iconst(Ty::I64, dtype.code());
        let word = if dtype.is_float() { Ty::F64 } else { Ty::I64 };
        let start = self.ctor_scalar(&args[0], word)?;
        let stop = self.ctor_scalar(&args[1], word)?;
        let step = if args.len() == 3 {
            self.ctor_scalar(&args[2], word)?
        } else if dtype.is_float() {
            self.fconst(Ty::F64, 1.0)
        } else {
            self.iconst(Ty::I64, 1)
        };
        let func = if dtype.is_float() {
            "zap_array_arange_f"
        } else {
            "zap_array_arange_i"
        };
        // arange raises a catchable ValueError on a zero step, so it must be a
        // throwing call (invoke) for an enclosing `try` to catch it.
        self.throwing_call(Some(arr), func, vec![code, start, stop, step]);
        Ok((arr, ty))
    }

    /// `linspace[T](start, stop, num)` (#504): a rank-1 array of `num` evenly
    /// spaced samples from `start` to `stop` inclusive of both ends.
    fn lower_array_linspace(
        &mut self,
        index: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let dtype = self.ctor_dtype(index)?;
        if args.len() != 3 || args.iter().any(|a| a.name.is_some()) {
            return Err(err(
                "linspace[T](start, stop, num) takes three positional arguments",
            ));
        }
        let ty = Ty::Array(dtype, 1);
        let arr = self.new_value(ty);
        let code = self.iconst(Ty::I64, dtype.code());
        let start = self.ctor_scalar(&args[0], Ty::F64)?;
        let stop = self.ctor_scalar(&args[1], Ty::F64)?;
        let num = self.ctor_scalar(&args[2], Ty::I64)?;
        self.emit(Inst::Call {
            dest: Some(arr),
            func: "zap_array_linspace".to_string(),
            args: vec![code, start, stop, num],
        });
        Ok((arr, ty))
    }

    /// Evaluate the axis indices of an array subscript, for an assignment target.
    /// `m[i, j]` arrives as an `Expr::Tuple`; `a[i]` as a single expression. Each
    /// index must be an integer; a sliced assignment target is not supported (a
    /// sliced read goes through `lower_array_tuple`). Returns one `i64` index
    /// value per axis supplied.
    fn array_axis_indices(&mut self, index: &Expr) -> Result<Vec<Value>, LowerError> {
        let parts: Vec<&Expr> = match index {
            Expr::Tuple(elems, _) => elems.iter().collect(),
            other => vec![other],
        };
        let mut out = Vec::with_capacity(parts.len());
        for p in parts {
            if matches!(p, Expr::Slice { .. } | Expr::SliceSpec { .. }) {
                return Err(err(
                    "assigning to a sliced array index (a strided-view store) is not \
                     supported yet (#502); index all axes with integers",
                ));
            }
            let (iv, it) = self.expr(p)?;
            if it.int_width().is_none() {
                return Err(err("array indices must be integers"));
            }
            out.push(self.coerce_index(iv, it));
        }
        if out.is_empty() {
            return Err(err("an array index requires at least one integer index"));
        }
        Ok(out)
    }

    /// Fold a sequence of leading axis indices into successive **row views**
    /// (`zap_array_row`), each reducing the rank by one (#500). Returns the final
    /// (lower-rank) array value; an empty `idxs` returns `arr` unchanged.
    fn array_fold_rows(&mut self, arr: Value, d: Dtype, ndim: u8, idxs: &[Value]) -> Value {
        let mut cur = arr;
        let mut rank = ndim;
        for &i in idxs {
            rank -= 1;
            let v = self.new_value(Ty::Array(d, rank));
            self.emit(Inst::Call {
                dest: Some(v),
                func: "zap_array_row".to_string(),
                args: vec![cur, i],
            });
            cur = v;
        }
        cur
    }

    /// Validate one axis index of a full integer index against that axis's
    /// length (`zap_array_ckidx`, #894) and return the validated value, so the
    /// flat-offset Horner can thread it in. Preserves the per-axis out-of-range
    /// abort the former `zap_array_row` fold performed, without allocating a
    /// view: e.g. `m[0, 5]` on a `(3, 3)` array still raises rather than
    /// silently reading flat slot 5.
    fn array_ckidx(&mut self, arr: Value, axis: usize, i: Value) -> Value {
        let ax = self.iconst(Ty::I64, axis as i64);
        let v = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(v),
            func: "zap_array_ckidx".to_string(),
            args: vec![arr, ax, i],
        });
        v
    }

    /// Compute the whole-shape row-major flat element index of a FULL integer
    /// index `m[i0, i1, ..., i_{n-1}]` inline (#894), bounds-checking each axis
    /// (`array_ckidx`) and folding the dims with Horner's rule
    /// (`flat = (..(i0 * d1 + i1) * d2 + i2) ..`, the trailing-axis dims read via
    /// `zap_array_dim`). No `zap_array_row` view is allocated; the single
    /// `zap_array_get/set_<dtype>` that consumes the result addresses storage
    /// through `zap_array_phys_nd`, so this composes with strided views.
    /// `idxs.len()` must equal `ndim` (>= 2 — rank 1 is already a direct get).
    fn array_full_flat_index(&mut self, arr: Value, ndim: u8, idxs: &[Value]) -> Value {
        let n = ndim as usize;
        debug_assert_eq!(idxs.len(), n);
        let mut flat = self.array_ckidx(arr, 0, idxs[0]);
        for (k, &idx) in idxs.iter().enumerate().skip(1) {
            let axis = self.iconst(Ty::I64, k as i64);
            let dk = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(dk),
                func: "zap_array_dim".to_string(),
                args: vec![arr, axis],
            });
            let ik = self.array_ckidx(arr, k, idx);
            let scaled = self.new_value(Ty::I64);
            self.emit(Inst::Bin {
                dest: scaled,
                op: BinOp::Mul,
                lhs: flat,
                rhs: dk,
            });
            let next = self.new_value(Ty::I64);
            self.emit(Inst::Bin {
                dest: next,
                op: BinOp::Add,
                lhs: scaled,
                rhs: ik,
            });
            flat = next;
        }
        flat
    }

    /// Is `e` a pure, side-effect-free scalar index safe to lower
    /// speculatively while probing a chain (#894 part 2)? Conservative: a bare
    /// name or an integer literal — the hot `m[i][j]` loop-index / constant
    /// forms. Because such an expression has no side effect and cannot raise,
    /// abandoning a non-matching chain leaves only dead, re-executable IR, so
    /// the fall-through per-step lowering stays correct. Anything else
    /// (arithmetic, a call, a nested index that could raise) is not fused.
    fn is_pure_scalar_index(e: &Expr) -> bool {
        matches!(e, Expr::Name(..) | Expr::Int(..))
    }

    /// #894 part 2: fuse a chained FULL integer index `root[i0][i1]…[i_{n-1}]`
    /// into a single flat scalar get — identical to the comma form
    /// `root[i0, …, i_{n-1}]` (Increment 19). Each `root[i0]…[i_k]` prefix would
    /// otherwise materialize a rank-reduced **row view** (one `zap_array_row`
    /// allocation per link) only to index it again; when every link is a pure
    /// scalar index those intermediates are dead, single-use temporaries that
    /// provably cannot escape or alias, so the whole chain lowers to the inline
    /// flat offset with no allocation.
    ///
    /// Returns `None` (fall through to the per-step lowering, unchanged
    /// behavior) unless: every link is a pure scalar index, the root is a bare
    /// name that lowers to an `array[T]`, and the chain indexes ALL `ndim` axes
    /// (a partial chain still yields a bindable view per step; a slice / tuple /
    /// fancy-array index is not a plain integer chain). The pure-index guard also
    /// keeps the `None` paths sound: any IR emitted before bailing is
    /// side-effect-free and the caller re-lowers the chain per step.
    fn try_fuse_full_index_chain(
        &mut self,
        base: &Expr,
        index: &Expr,
    ) -> Result<Option<(Value, Ty)>, LowerError> {
        if !Self::is_pure_scalar_index(index) {
            return Ok(None);
        }
        // Collect the chain, outer index first, walking inward through nested
        // `Expr::Index` bases until the first non-index root.
        let mut rev_idx: Vec<&Expr> = vec![index];
        let mut root = base;
        while let Expr::Index {
            base: inner,
            index: idx,
            ..
        } = root
        {
            if !Self::is_pure_scalar_index(idx) {
                return Ok(None);
            }
            rev_idx.push(idx);
            root = inner;
        }
        // A lone `m[i]` is not a chain; and the root must be a bare name so that
        // re-lowering it on the fall-through path is a harmless repeated load.
        if rev_idx.len() < 2 || !matches!(root, Expr::Name(..)) {
            return Ok(None);
        }
        let (rv, rty) = self.expr(root)?;
        let (d, ndim) = match rty {
            Ty::Array(d, n) => (d, n),
            _ => return Ok(None),
        };
        // Only a FULL chain (one index per axis) fuses to a scalar; a partial
        // chain still produces a bindable rank-reduced view per step.
        if rev_idx.len() != ndim as usize {
            return Ok(None);
        }
        // Source order is root-most first, so reverse the outer-first list and
        // evaluate each index exactly once in that order (matching the per-step
        // order: the base `m[i]` — hence `i` — before the outer index `j`).
        let mut idxs: Vec<Value> = Vec::with_capacity(rev_idx.len());
        for ie in rev_idx.iter().rev() {
            let (iv, it) = self.expr(ie)?;
            if it.int_width().is_none() {
                // An array-typed index is advanced indexing (#510), not a plain
                // integer chain — do not fuse.
                return Ok(None);
            }
            idxs.push(self.coerce_index(iv, it));
        }
        let flat = self.array_full_flat_index(rv, ndim, &idxs);
        let dest = self.new_value(d.ty());
        self.emit(Inst::Call {
            dest: Some(dest),
            func: format!("zap_array_get_{}", d.suffix()),
            args: vec![rv, flat],
        });
        Ok(Some((dest, d.ty())))
    }

    /// Lower an N-D array subscript **get** `m[i, j, ...]`. A full index (one per
    /// axis) names a scalar element; a partial index (fewer than `ndim`) yields a
    /// rank-reduced **view** (#500). Too many indices is an error.
    fn lower_array_index_get(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        index: &Expr,
    ) -> Result<(Value, Ty), LowerError> {
        // A multi-axis index that contains any slice (`m[:, j]`, `m[lo:hi, k]`,
        // the 1-D `a[i]` never reaches here) folds per axis into a strided view
        // (#502); the all-integer paths below stay the plain contiguous
        // row-fold + scalar get.
        // Advanced indexing (#510, D-0175): a single array operand is a boolean
        // mask (`a[mask]`) or an integer index array (`a[idx]`, fancy gather).
        // Detect by lowering the lone subscript once; a scalar integer index or
        // a view is handled inline here (reusing the already-evaluated value so
        // the index runs exactly once).
        if !matches!(
            index,
            Expr::Tuple(..) | Expr::Slice { .. } | Expr::SliceSpec { .. }
        ) {
            let (iv, it) = self.expr(index)?;
            if let Ty::Array(idt, ind) = it {
                return self.lower_array_single_advanced(arr, d, ndim, iv, idt, ind);
            }
            if it.int_width().is_none() {
                return Err(err("array indices must be integers"));
            }
            let i = self.coerce_index(iv, it);
            if ndim == 1 {
                let dest = self.new_value(d.ty());
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: format!("zap_array_get_{}", d.suffix()),
                    args: vec![arr, i],
                });
                return Ok((dest, d.ty()));
            }
            let view = self.array_fold_rows(arr, d, ndim, &[i]);
            return Ok((view, Ty::Array(d, ndim - 1)));
        }

        let parts: Vec<&Expr> = match index {
            Expr::Tuple(elems, _) => elems.iter().collect(),
            other => vec![other],
        };
        self.lower_array_tuple(arr, d, ndim, &parts)
    }

    /// Lower a tuple subscript `a[p0, p1, ...]` (and the single-slice case),
    /// classifying every part with exactly one evaluation. If any part is an
    /// array it is advanced indexing (#510) routed to `lower_array_advanced`;
    /// otherwise it is the pre-#510 behavior — a strided view when any slice is
    /// present (#502), else a scalar get (full integer index) or a rank-reduced
    /// view (partial integer index, #500).
    fn lower_array_tuple(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        parts: &[&Expr],
    ) -> Result<(Value, Ty), LowerError> {
        let n = ndim as usize;
        if parts.len() > n {
            return Err(err(format!(
                "too many indices for a {n}-dimensional array ({} given)",
                parts.len()
            )));
        }
        let mut cls: Vec<IdxPart> = Vec::with_capacity(parts.len());
        let mut has_adv = false;
        let mut has_slice = false;
        for p in parts {
            match p {
                Expr::SliceSpec { lo, hi, step, .. } => {
                    let lo = self.array_slice_bound(lo.as_deref())?;
                    let hi = self.array_slice_bound(hi.as_deref())?;
                    let st = self.array_slice_bound(step.as_deref())?;
                    has_slice = true;
                    cls.push(IdxPart::Slice { lo, hi, step: st });
                }
                Expr::Slice { lo, hi, .. } => {
                    let lo = self.array_slice_bound(lo.as_deref())?;
                    let hi = self.array_slice_bound(hi.as_deref())?;
                    let st = (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0));
                    has_slice = true;
                    cls.push(IdxPart::Slice { lo, hi, step: st });
                }
                other => {
                    let (v, t) = self.expr(other)?;
                    if let Ty::Array(idt, ind) = t {
                        has_adv = true;
                        cls.push(IdxPart::Adv(v, idt, ind));
                    } else if t.int_width().is_some() {
                        cls.push(IdxPart::Int(v, t));
                    } else {
                        return Err(err("array indices must be integers or index arrays"));
                    }
                }
            }
        }
        if has_adv {
            return self.lower_array_advanced(arr, d, ndim, cls);
        }
        // No advanced parts: reproduce the pre-#510 slice / integer behavior.
        if has_slice {
            let mut cur = arr;
            let mut rank = ndim;
            let mut cur_axis: i64 = 0;
            for part in cls {
                match part {
                    IdxPart::Slice { lo, hi, step } => {
                        let axis = self.iconst(Ty::I64, cur_axis);
                        let v = self.new_value(Ty::Array(d, rank));
                        self.emit(Inst::Call {
                            dest: Some(v),
                            func: "zap_array_slice_axis".to_string(),
                            args: vec![cur, axis, lo.0, hi.0, step.0, lo.1, hi.1, step.1],
                        });
                        cur = v;
                        cur_axis += 1;
                    }
                    IdxPart::Int(v, t) => {
                        let i = self.coerce_index(v, t);
                        let axis = self.iconst(Ty::I64, cur_axis);
                        rank -= 1;
                        let out = Ty::Array(d, rank);
                        let nv = self.new_value(out);
                        self.emit(Inst::Call {
                            dest: Some(nv),
                            func: "zap_array_index_axis".to_string(),
                            args: vec![cur, axis, i],
                        });
                        cur = nv;
                    }
                    IdxPart::Adv(..) => unreachable!("has_adv is false here"),
                }
            }
            return Ok((cur, Ty::Array(d, rank)));
        }
        // All-integer index: fold to a rank-1 view then a scalar get (full
        // index), or a rank-reduced view (partial index).
        let idxs: Vec<Value> = cls
            .into_iter()
            .map(|p| match p {
                IdxPart::Int(v, t) => self.coerce_index(v, t),
                _ => unreachable!(),
            })
            .collect();
        let k = idxs.len();
        if k == n {
            // Full integer index -> a single scalar get. Rank 1 is already a
            // direct get on the array; rank >= 2 computes the flat offset inline
            // (#894) instead of folding through an allocated rank-1 row view.
            let flat = if n == 1 {
                idxs[0]
            } else {
                self.array_full_flat_index(arr, ndim, &idxs)
            };
            let dest = self.new_value(d.ty());
            self.emit(Inst::Call {
                dest: Some(dest),
                func: format!("zap_array_get_{}", d.suffix()),
                args: vec![arr, flat],
            });
            Ok((dest, d.ty()))
        } else {
            let view = self.array_fold_rows(arr, d, ndim, &idxs);
            Ok((view, Ty::Array(d, ndim - k as u8)))
        }
    }

    /// Lower general advanced indexing `a[p0, p1, ...]` (#510, D-0175) with at
    /// least one array part. Builds a per-axis spec buffer (7 i64 per axis:
    /// tag + slice/int/group params) and the ordered list of integer index
    /// arrays, then calls `zap_array_fancy`. A boolean array of rank k is
    /// expanded into k integer index arrays via `nonzero_axis`, consuming k
    /// axes. The result rank = (broadcast rank of the advanced arrays) + (number
    /// of kept FULL/SLICE axes). The kernel raises IndexError/ValueError, so it
    /// is a `throwing_call`.
    fn lower_array_advanced(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        cls: Vec<IdxPart>,
    ) -> Result<(Value, Ty), LowerError> {
        let n = ndim as usize;
        let zero = self.iconst(Ty::I64, 0);
        let mut spec: Vec<[Value; 7]> = (0..n).map(|_| [zero; 7]).collect();
        let adv_list = self.new_value(Ty::List(Elem::Ptr));
        self.emit(Inst::Call {
            dest: Some(adv_list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let mut axis = 0usize;
        let mut g: i64 = 0;
        let mut b: u8 = 0;
        let mut slices: u8 = 0;
        for part in cls {
            if axis >= n {
                return Err(err("too many indices for the array"));
            }
            match part {
                IdxPart::Slice { lo, hi, step } => {
                    let tag = self.iconst(Ty::I64, 1);
                    spec[axis] = [tag, lo.0, hi.0, step.0, lo.1, hi.1, step.1];
                    slices += 1;
                    axis += 1;
                }
                IdxPart::Int(v, t) => {
                    let iv = self.coerce(v, t, Ty::I64);
                    let tag = self.iconst(Ty::I64, 2);
                    spec[axis] = [tag, iv, zero, zero, zero, zero, zero];
                    axis += 1;
                }
                IdxPart::Adv(v, idt, ind) => {
                    if matches!(idt, Dtype::Bool) {
                        let k = ind as usize;
                        if axis + k > n {
                            return Err(err("boolean index spans more axes than the array has"));
                        }
                        for t in 0..k {
                            let axc = self.iconst(Ty::I64, t as i64);
                            let nz = self.new_value(Ty::Array(Dtype::I64, 1));
                            self.emit(Inst::Call {
                                dest: Some(nz),
                                func: "zap_array_nonzero_axis".to_string(),
                                args: vec![v, axc],
                            });
                            self.emit(Inst::Call {
                                dest: None,
                                func: "zap_list_push_ptr".to_string(),
                                args: vec![adv_list, nz],
                            });
                            let tag = self.iconst(Ty::I64, 3);
                            let gc = self.iconst(Ty::I64, g);
                            spec[axis] = [tag, gc, zero, zero, zero, zero, zero];
                            axis += 1;
                            g += 1;
                        }
                        if b < 1 {
                            b = 1;
                        }
                    } else {
                        self.emit(Inst::Call {
                            dest: None,
                            func: "zap_list_push_ptr".to_string(),
                            args: vec![adv_list, v],
                        });
                        let tag = self.iconst(Ty::I64, 3);
                        let gc = self.iconst(Ty::I64, g);
                        spec[axis] = [tag, gc, zero, zero, zero, zero, zero];
                        if ind > b {
                            b = ind;
                        }
                        axis += 1;
                        g += 1;
                    }
                }
            }
        }
        // Kept axes (preserved in the result) = trailing FULL axes + the SLICE
        // axes placed above. Integer and advanced axes are consumed.
        let kept: u8 = (n - axis) as u8 + slices;
        let rank = b + kept;
        let total_slots = (n * 7) as i64;
        let nd_slots = self.iconst(Ty::I64, total_slots);
        let buf = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(buf),
            func: "zap_i64buf".to_string(),
            args: vec![nd_slots],
        });
        for (ax, rec) in spec.iter().enumerate() {
            for (s, &val) in rec.iter().enumerate() {
                let slot = self.iconst(Ty::I64, (ax * 7 + s) as i64);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_i64set".to_string(),
                    args: vec![buf, slot, val],
                });
            }
        }
        let ndim_v = self.iconst(Ty::I64, ndim as i64);
        let out = Ty::Array(d, rank);
        let dest = self.new_value(out);
        self.throwing_call(
            Some(dest),
            "zap_array_fancy",
            vec![arr, buf, ndim_v, adv_list],
        );
        Ok((dest, out))
    }

    /// Lower a single advanced subscript `a[ix]` where `ix` is an array (#510,
    /// D-0175): a boolean mask (`array[bool]`) selects via `zap_array_mask_select`
    /// (result rank `1 + ndim - mask.ndim`, a 1-D copy when the mask is
    /// full-shape); an integer index array gathers via `zap_array_take` (result
    /// rank `idx.ndim + ndim - 1`). Both produce a fresh copy and may raise a
    /// catchable `IndexError`, so the kernels are `throwing_call`s.
    fn lower_array_single_advanced(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        ix: Value,
        ix_dtype: Dtype,
        ix_ndim: u8,
    ) -> Result<(Value, Ty), LowerError> {
        if matches!(ix_dtype, Dtype::Bool) {
            if ix_ndim > ndim {
                return Err(err("boolean index has more dimensions than the array"));
            }
            let rank = 1 + ndim - ix_ndim;
            let out = Ty::Array(d, rank);
            let dest = self.new_value(out);
            self.throwing_call(Some(dest), "zap_array_mask_select", vec![arr, ix]);
            Ok((dest, out))
        } else {
            // Integer index array -> fancy gather along axis 0.
            let rank = ix_ndim + ndim - 1;
            let out = Ty::Array(d, rank);
            let dest = self.new_value(out);
            self.throwing_call(Some(dest), "zap_array_take", vec![arr, ix]);
            Ok((dest, out))
        }
    }

    /// Lower `where(...)` (#510, D-0175). Three args: element-wise select from
    /// `x` where `cond` is true else `y`, with full NumPy broadcasting (result
    /// rank = max of the operand ranks; a scalar `x`/`y` is wrapped as a 1-element
    /// array so one runtime path serves scalars and arrays). The result dtype is
    /// the NEP-50 promotion of `x` and `y` (weak scalars). One arg: a tuple of
    /// `cond.ndim` index arrays (`cond.nonzero()`). The select kernel raises a
    /// catchable ValueError on incompatible shapes, so it is a `throwing_call`.
    fn lower_array_where(&mut self, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        if args.iter().any(|a| a.name.is_some()) {
            return Err(err("where() takes positional arguments only"));
        }
        if args.len() == 1 {
            let (cv, ct) = self.expr(&args[0].value)?;
            let Ty::Array(_, cnd) = ct else {
                return Err(err("where(cond) requires an array condition"));
            };
            let mut vals = Vec::with_capacity(cnd as usize);
            let mut tys = Vec::with_capacity(cnd as usize);
            for k in 0..cnd as i64 {
                let axis = self.iconst(Ty::I64, k);
                let out = Ty::Array(Dtype::I64, 1);
                let d = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_array_nonzero_axis".to_string(),
                    args: vec![cv, axis],
                });
                vals.push(d);
                tys.push(out);
            }
            let sid = self.ctx.tuples.borrow_mut().intern(tys);
            let dest = self.new_value(Ty::Struct(sid));
            self.emit(Inst::StructNew { dest, sid });
            for (idx, v) in vals.into_iter().enumerate() {
                self.emit(Inst::FieldSet {
                    base: dest,
                    sid,
                    idx: idx as u32,
                    val: v,
                });
            }
            return Ok((dest, Ty::Struct(sid)));
        }
        if args.len() != 3 {
            return Err(err("where() takes 1 argument (nonzero) or 3 (cond, x, y)"));
        }
        let (cv, ct) = self.expr(&args[0].value)?;
        let Ty::Array(_, cnd) = ct else {
            return Err(err("where(cond, x, y): cond must be an array"));
        };
        let (xv, xt) = self.expr(&args[1].value)?;
        let (yv, yt) = self.expr(&args[2].value)?;
        // Operand -> (wrapped array value, its dtype-or-None-if-scalar, rank).
        let wrap =
            |s: &mut Self, v: Value, t: Ty| -> Result<(Value, Option<Dtype>, u8), LowerError> {
                match t {
                    Ty::Array(dd, nd) => Ok((v, Some(dd), nd)),
                    _ if t.is_float() => {
                        let f = s.coerce(v, t, Ty::F64);
                        let a = s.new_value(Ty::Array(Dtype::F64, 1));
                        s.emit(Inst::Call {
                            dest: Some(a),
                            func: "zap_array_from_scalar_f".to_string(),
                            args: vec![f],
                        });
                        Ok((a, None, 1))
                    }
                    _ if t.int_width().is_some() => {
                        let i = s.coerce(v, t, Ty::I64);
                        let a = s.new_value(Ty::Array(Dtype::I64, 1));
                        s.emit(Inst::Call {
                            dest: Some(a),
                            func: "zap_array_from_scalar_i".to_string(),
                            args: vec![i],
                        });
                        Ok((a, None, 1))
                    }
                    _ => Err(err(
                        "where(cond, x, y): x and y must be arrays or numeric scalars",
                    )),
                }
            };
        let x_is_float = xt.is_float();
        let y_is_float = yt.is_float();
        let (xa, xd, xnd) = wrap(self, xv, xt)?;
        let (ya, yd, ynd) = wrap(self, yv, yt)?;
        // NEP-50 result dtype: array⊗array promotes both dtypes; a scalar is weak.
        let out = match (xd, yd) {
            (Some(a), Some(b)) => Dtype::promote(a, b),
            (Some(a), None) => Dtype::promote_scalar(a, y_is_float),
            (None, Some(b)) => Dtype::promote_scalar(b, x_is_float),
            (None, None) => {
                if xt.is_float() || yt.is_float() {
                    Dtype::F64
                } else {
                    Dtype::I64
                }
            }
        };
        let rank = cnd.max(xnd).max(ynd);
        let out_ty = Ty::Array(out, rank);
        let code = self.iconst(Ty::I64, out.code());
        let dest = self.new_value(out_ty);
        self.throwing_call(Some(dest), "zap_array_where", vec![cv, xa, ya, code]);
        Ok((dest, out_ty))
    }

    /// Lower an advanced-index assignment `a[ix] = value` (#510, D-0175) where
    /// `ix` is an array. A boolean mask fills (scalar) or scatters (array of
    /// values) the selected elements; an integer index array scatters by index.
    /// The write goes into `a`'s real storage (so the runtime maps each slot
    /// through `zap_array_phys_nd`), and may raise a catchable IndexError/
    /// ValueError, so the kernels are `throwing_call`s. `value` is lowered once.
    #[allow(clippy::too_many_arguments)]
    fn lower_array_advanced_assign(
        &mut self,
        target: Value,
        _d: Dtype,
        ndim: u8,
        ix: Value,
        ix_dtype: Dtype,
        ix_ndim: u8,
        value: &Expr,
    ) -> Result<(), LowerError> {
        let (v, vt) = self.expr(value)?;
        let is_mask = matches!(ix_dtype, Dtype::Bool);
        if is_mask && ix_ndim > ndim {
            return Err(err("boolean index has more dimensions than the array"));
        }
        let (scalar_fn, array_fn) = if is_mask {
            (
                "zap_array_mask_assign_scalar",
                "zap_array_mask_assign_array",
            )
        } else {
            ("zap_array_scatter_scalar", "zap_array_scatter_array")
        };
        match vt {
            Ty::Array(..) => {
                self.throwing_call(None, array_fn, vec![target, ix, v]);
                Ok(())
            }
            _ if vt.is_float() || vt.int_width().is_some() => {
                let (si, sf, isf) = self.array_scalar_words(v, vt);
                self.throwing_call(None, scalar_fn, vec![target, ix, si, sf, isf]);
                Ok(())
            }
            _ => Err(err(
                "advanced-index assignment requires a numeric scalar or an array of values",
            )),
        }
    }

    /// Resolve an N-D array subscript to the `(array, flat element index)` naming
    /// a single scalar slot, for `m[i, j] = v` / augmented assignment — read or
    /// written by the caller through `zap_array_get/set_<dtype>` (which address
    /// storage via `zap_array_phys_nd`). The index is evaluated exactly once.
    /// Only a full index names a slot; assigning a partial index (a whole-row
    /// store) is a documented follow-up (#501/#511). Rank 1 is the element
    /// index directly; rank >= 2 computes the flat offset inline (#894) instead
    /// of folding through an allocated rank-1 row view.
    fn array_scalar_slot(
        &mut self,
        arr: Value,
        ndim: u8,
        index: &Expr,
    ) -> Result<(Value, Value), LowerError> {
        let idxs = self.array_axis_indices(index)?;
        let k = idxs.len();
        let n = ndim as usize;
        if k > n {
            return Err(err(format!(
                "too many indices for a {n}-dimensional array ({k} given)"
            )));
        }
        if k < n {
            return Err(err(format!(
                "assigning to a partial index of a {n}-dimensional array (a whole-row store) \
                 is not supported yet; index all {n} axes"
            )));
        }
        let flat = if n == 1 {
            idxs[0]
        } else {
            self.array_full_flat_index(arr, ndim, &idxs)
        };
        Ok((arr, flat))
    }

    /// Lower an array attribute read (#500): `a.ndim` (the static rank, a
    /// compile-time constant), `a.size` (total element count across all axes),
    /// and `a.shape` (a fixed-arity `tuple` of the per-axis lengths, one `int64`
    /// per axis read from the runtime `dims`). `len(a)` still gives the first
    /// axis; other attribute names are rejected with a pointer to these.
    fn lower_array_attr(
        &mut self,
        arr: Value,
        d: Dtype,
        ndim: u8,
        name: &str,
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "ndim" => Ok((self.iconst(Ty::I64, ndim as i64), Ty::I64)),
            // `a.T` (#511): the full-axis-reverse transpose, a view (same as
            // `a.transpose()`).
            "T" => {
                let out = Ty::Array(d, ndim);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_transpose".to_string(),
                    args: vec![arr],
                });
                Ok((dest, out))
            }
            "size" => {
                let dest = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_count".to_string(),
                    args: vec![arr],
                });
                Ok((dest, Ty::I64))
            }
            "shape" => {
                // A statically rank-`ndim` array has a fixed-arity shape tuple;
                // build it as a `tuple[int64; ndim]` from per-axis `zap_array_dim`
                // reads. (Rank 1 still yields a one-element tuple, matching NumPy.)
                let tys = vec![Ty::I64; ndim as usize];
                let sid = self.ctx.tuples.borrow_mut().intern(tys);
                let dest = self.new_value(Ty::Struct(sid));
                self.emit(Inst::StructNew { dest, sid });
                for axis in 0..ndim {
                    let ax = self.iconst(Ty::I64, axis as i64);
                    let dim = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(dim),
                        func: "zap_array_dim".to_string(),
                        args: vec![arr, ax],
                    });
                    self.emit(Inst::FieldSet {
                        base: dest,
                        sid,
                        idx: axis as u32,
                        val: dim,
                    });
                }
                Ok((dest, Ty::Struct(sid)))
            }
            other => Err(err(format!(
                "array attribute '{other}' is unsupported; arrays expose '.shape', \
                 '.ndim', '.size', '.T' (transpose), and len(a) for the first axis"
            ))),
        }
    }

    /// Lower channel construction `channel[T]()` (unbuffered) or
    /// `channel[T](cap)` (buffered) (ticket #25, D-0031). The runtime stores
    /// elements as uniform 8-byte words, so construction only needs the
    /// capacity; the element kind `T` is carried in the ZIR [`Ty::Chan`] so
    /// later send/recv pick the typed entry point. An absent capacity means an
    /// unbuffered (rendezvous) channel.
    fn lower_chan_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // A channel stores its elements as uniform 8-byte words (D-0031). The
        // element type is resolved through the shared value-position classifier
        // (`expr_to_type` + [`value_elem_of`]), exactly like a dict value, a list
        // element, or an `AtomicArray` element, so every element kind keeps its
        // *precise* [`Elem`] — a struct is `Elem::Struct(sid)`, a nested
        // collection is `Elem::Nested`, `bytes`/`complex`/`enum`/`float` each
        // their own — and therefore recovers its real type on receive (#1100).
        //
        // Classifying a bare *name* instead (the earlier `chan_elem_from_name`)
        // collapsed *every* reference element to an opaque `Elem::Ptr`, so a
        // `channel[Struct]` value read back as a bare pointer (`str`): field
        // access on the received value then failed with "field access on a
        // non-struct value" (ZAP6001). This mirrors the #625/#1074 read-back
        // fixes (a collection element must stay recoverable, never flattened to
        // `Ptr`). A **tuple** element is a value aggregate whose boxing/unboxing
        // across the channel ABI this slice does not implement (unlike a set/dict
        // key, which hashes a tuple through its key functions), so it is rejected
        // cleanly here with a specific message (#703; tuple channels are a
        // follow-up) before the generic classifier would intern it as a struct.
        let elem = match elem_expr {
            Expr::Tuple(..) => {
                return Err(err(
                    "channels of tuple elements are not supported yet (a tuple is a \
                     value aggregate; channel elements are uniform words)",
                ))
            }
            _ => value_elem_of(&expr_to_type(elem_expr)?, &self.ctx.tyenv())?,
        };
        let cap = match args.len() {
            0 => self.iconst(Ty::I64, 0),
            1 => {
                let (v, vt) = self.expr(&args[0].value)?;
                self.coerce(v, vt, Ty::I64)
            }
            _ => {
                return Err(err(
                    "channel[T](...) takes at most one argument (the buffer capacity)",
                ))
            }
        };
        let ch = self.new_value(Ty::Chan(elem));
        self.emit(Inst::Call {
            dest: Some(ch),
            func: "zap_chan_new".to_string(),
            args: vec![cap],
        });
        Ok((ch, Ty::Chan(elem)))
    }

    /// Lower `AtomicArray[T](cap)` construction (#844): allocate a fixed-capacity
    /// runtime atomic array, passing the capacity and whether the element is a
    /// reference (so the collector traces the slots). The element kind `T` is
    /// carried in [`Ty::AtomicArray`] so later load/store/cas pick the typed
    /// entry point.
    fn lower_atomicarray_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let elem = value_elem_of(&expr_to_type(elem_expr)?, &self.ctx.tyenv())?;
        let cap = match args.len() {
            1 => {
                let (v, vt) = self.expr(&args[0].value)?;
                self.coerce(v, vt, Ty::I64)
            }
            _ => {
                return Err(err(
                    "AtomicArray[T](cap) takes exactly one argument (the capacity)",
                ))
            }
        };
        // A word element is `int64` only (bool/char/float/… are not supported in
        // v1 — rejected by sema); every other admitted element is a reference.
        let elem_is_ptr = !matches!(elem, Elem::I64);
        let flag = self.iconst(Ty::I64, i64::from(elem_is_ptr));
        let a = self.new_value(Ty::AtomicArray(elem));
        self.emit(Inst::Call {
            dest: Some(a),
            func: "zap_atomicarray_new".to_string(),
            args: vec![cap, flag],
        });
        Ok((a, Ty::AtomicArray(elem)))
    }

    /// Lower an `AtomicArray[T]` method (#844) to its seq_cst runtime entry
    /// point. Reference elements use the `_ptr` entry points (value is a pointer
    /// word, no cast needed); `int64` elements use the `_i64` ones.
    fn lower_atomicarray_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let is_ptr = !matches!(elem, Elem::I64);
        let suffix = if is_ptr { "ptr" } else { "i64" };
        let ety = elem.ty();
        // The index argument, coerced to i64.
        let index = |s: &mut Self, a: &ast::Arg| -> Result<Value, LowerError> {
            let (v, vt) = s.expr(&a.value)?;
            Ok(s.coerce(v, vt, Ty::I64))
        };
        match name {
            "len" => {
                if !args.is_empty() {
                    return Err(err("AtomicArray.len() takes no arguments"));
                }
                let dest = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_atomicarray_len".to_string(),
                    args: vec![recv],
                });
                Ok((dest, Ty::I64))
            }
            "load" => {
                if args.len() != 1 {
                    return Err(err("AtomicArray.load(i) takes exactly one argument"));
                }
                let i = index(self, &args[0])?;
                let dest = self.new_value(ety);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: format!("zap_atomicarray_load_{suffix}"),
                    args: vec![recv, i],
                });
                Ok((dest, ety))
            }
            "store" => {
                if args.len() != 2 {
                    return Err(err("AtomicArray.store(i, v) takes exactly two arguments"));
                }
                let i = index(self, &args[0])?;
                let (val, vt) = self.expr(&args[1].value)?;
                let val = self.coerce(val, vt, ety);
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_atomicarray_store_{suffix}"),
                    args: vec![recv, i, val],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            "compare_and_swap" => {
                if args.len() != 3 {
                    return Err(err(
                        "AtomicArray.compare_and_swap(i, old, new) takes exactly three arguments",
                    ));
                }
                let i = index(self, &args[0])?;
                let (old, ot) = self.expr(&args[1].value)?;
                let old = self.coerce(old, ot, ety);
                let (new, nt) = self.expr(&args[2].value)?;
                let new = self.coerce(new, nt, ety);
                let word = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(word),
                    func: format!("zap_atomicarray_cas_{suffix}"),
                    args: vec![recv, i, old, new],
                });
                // The runtime returns 0/1; surface it as a `bool` (`i1`).
                let zero = self.iconst(Ty::I64, 0);
                let dest = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest,
                    pred: CmpPred::Ne,
                    lhs: word,
                    rhs: zero,
                });
                Ok((dest, Ty::I1))
            }
            other => Err(err(format!(
                "AtomicArray has no method '{other}' (supported: load, store, \
                 compare_and_swap, len)"
            ))),
        }
    }

    /// Lower `ContextVar[T]("name")` / `ContextVar[T]("name", default=d)`
    /// construction (#1237) to a `zap_ctxvar_new_{ptr,i64}` runtime allocation.
    /// The returned object's identity IS the per-coroutine lookup key; the
    /// optional default is stored in it (returned by `get()` when unbound). The
    /// `_ptr` vs `_i64` split follows the element kind, like `AtomicArray`.
    fn lower_ctxvar_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let elem = value_elem_of(&expr_to_type(elem_expr)?, &self.ctx.tyenv())?;
        let is_ptr = !matches!(elem, Elem::I64);
        let suffix = if is_ptr { "ptr" } else { "i64" };
        let ety = elem.ty();
        // The name is the first positional argument (a `str`, a pointer word).
        let Some(name_arg) = args.first() else {
            return Err(err("ContextVar[T](name, default=...) requires a name"));
        };
        if name_arg.name.is_some() {
            return Err(err(
                "ContextVar[T]'s first argument (the name) is positional",
            ));
        }
        let (nv, nt) = self.expr(&name_arg.value)?;
        let name_val = self.coerce(nv, nt, Ty::Ptr);
        // Optional default: a second positional argument, or a `default=` keyword.
        let default_arg = args
            .iter()
            .enumerate()
            .find_map(|(i, a)| match a.name.as_deref() {
                Some("default") => Some(a),
                None if i == 1 => Some(a),
                _ => None,
            });
        let (dval, has) = match default_arg {
            Some(a) => {
                let (v, vt) = self.expr(&a.value)?;
                (self.coerce(v, vt, ety), 1i64)
            }
            None => {
                // A placeholder the runtime never reads (has_default == 0): a null
                // reference for the `_ptr` entry point, a zero word for `_i64`.
                let z = if is_ptr {
                    let d = self.new_value(ety);
                    self.emit(Inst::NullConst { dest: d });
                    d
                } else {
                    self.iconst(Ty::I64, 0)
                };
                (z, 0)
            }
        };
        let has_v = self.iconst(Ty::I64, has);
        let cv = self.new_value(Ty::ContextVar(elem));
        self.emit(Inst::Call {
            dest: Some(cv),
            func: format!("zap_ctxvar_new_{suffix}"),
            args: vec![name_val, dval, has_v],
        });
        Ok((cv, Ty::ContextVar(elem)))
    }

    /// Lower a `ContextVar[T]` method (#1237) to its runtime entry point. The
    /// receiver `recv` is the ContextVar object (the lookup key). `get` may raise
    /// `LookupError` (unbound with no default), so it routes through
    /// [`throwing_call`] — an `invoke` to the active landing pad inside a `try`,
    /// a plain `call` otherwise — matching the index/divide faults (#383). `set`
    /// returns a `ContextToken`; `reset` consumes one. The `_ptr`/`_i64` split
    /// picks how the value word is passed/returned (a reference is a pointer, an
    /// `int64` a word), like `AtomicArray`.
    fn lower_ctxvar_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let is_ptr = !matches!(elem, Elem::I64);
        let suffix = if is_ptr { "ptr" } else { "i64" };
        let ety = elem.ty();
        match name {
            "get" => {
                if !args.is_empty() {
                    return Err(err("ContextVar.get() takes no arguments"));
                }
                let dest = self.new_value(ety);
                self.throwing_call(Some(dest), &format!("zap_ctxvar_get_{suffix}"), vec![recv]);
                Ok((dest, ety))
            }
            "set" => {
                if args.len() != 1 {
                    return Err(err("ContextVar.set(v) takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let v = self.coerce(v, vt, ety);
                let tok = self.new_value(Ty::CtxToken);
                self.emit(Inst::Call {
                    dest: Some(tok),
                    func: format!("zap_ctxvar_set_{suffix}"),
                    args: vec![recv, v],
                });
                Ok((tok, Ty::CtxToken))
            }
            "reset" => {
                if args.len() != 1 {
                    return Err(err("ContextVar.reset(token) takes exactly one argument"));
                }
                let (t, tt) = self.expr(&args[0].value)?;
                let t = self.coerce(t, tt, Ty::CtxToken);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_ctxvar_reset".to_string(),
                    args: vec![t],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            other => Err(err(format!(
                "ContextVar has no method '{other}' (supported: get, set, reset)"
            ))),
        }
    }

    fn lower_list_lit(&mut self, elems: &[Expr]) -> Result<(Value, Ty), LowerError> {
        self.lower_list_lit_expected(elems, None)
    }

    fn lower_list_lit_expected(
        &mut self,
        elems: &[Expr],
        expected: Option<Elem>,
    ) -> Result<(Value, Ty), LowerError> {
        // Build a fresh list and append each element in source order. A `*expr`
        // splat (#598) iterates its operand and appends each spliced element in
        // place. Use the expected element kind when known, otherwise infer from
        // the first appended element (an unconstrained empty literal defaults to
        // `i64`). Every element must share that representation.
        let list = self.new_value(Ty::List(Elem::I64));
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let elem: std::cell::Cell<Option<Elem>> = std::cell::Cell::new(expected);
        let push = |s: &mut FnLower<'a>, v: Value, t: Ty| -> Result<(), LowerError> {
            let ek = match elem.get() {
                Some(e) => {
                    let actual = list_elem_kind(t, &s.ctx.nested)?;
                    // A concrete value whose representation differs from the
                    // declared element is accepted only when the element is an
                    // interface and the value structurally conforms (#829):
                    // `coerce` boxes it into the `{data, itable}` fat pointer
                    // below. Any other mismatch is still a representation error.
                    if actual != e && !s.conforms_to_iface_elem(t, e, actual) {
                        return Err(err(format!(
                            "list element expects `{}`, found `{}`",
                            s.render_ty(&s.elem_ty(e)),
                            s.render_ty(&t)
                        )));
                    }
                    e
                }
                None => {
                    let e = list_elem_kind(t, &s.ctx.nested)?;
                    elem.set(Some(e));
                    e
                }
            };
            let v = s.coerce(v, t, ek.ty());
            let v = s.box_elem_for_store(v, ek.ty());
            s.emit(Inst::Call {
                dest: None,
                func: format!("zap_list_push_{}", ek.val_suffix()),
                args: vec![list, v],
            });
            Ok(())
        };
        for el in elems {
            match el {
                Expr::Starred { value, .. } => {
                    if let (Some(kind), Expr::List(_, _)) = (elem.get(), value.as_ref()) {
                        let (source, _) = self.expr_collection_context(value, Ty::List(kind))?;
                        self.iterate_list(source, kind, |s, v, t| push(s, v, t))?;
                    } else {
                        self.iterate(value, |s, v, t| push(s, v, t))?;
                    }
                }
                _ => {
                    let (v, t) = match elem.get() {
                        Some(kind) => self.expr_collection_context(el, self.elem_ty(kind))?,
                        None => self.expr(el)?,
                    };
                    push(self, v, t)?;
                }
            }
        }
        let ek = elem.get().unwrap_or(Elem::I64);
        self.func.value_types[list as usize] = Ty::List(ek);
        Ok((list, Ty::List(ek)))
    }

    fn lower_dict_lit(&mut self, items: &[DictItem]) -> Result<(Value, Ty), LowerError> {
        self.lower_dict_lit_expected(items, None)
    }

    fn lower_dict_lit_expected(
        &mut self,
        items: &[DictItem],
        expected: Option<(Elem, Elem)>,
    ) -> Result<(Value, Ty), LowerError> {
        // Build a fresh dict and set each entry in source order. A `**expr` merge
        // (#598) copies another dict's entries in place, so a later entry — a
        // pair or a merge — overrides an earlier key (Python's last-wins
        // semantics). Use expected key/value kinds when available, otherwise
        // infer them from the first entry.
        let dict = self.new_value(Ty::Dict(Elem::I64, Elem::I64));
        self.emit(Inst::Call {
            dest: Some(dict),
            func: "zap_dict_new".to_string(),
            args: vec![],
        });
        let kinds: std::cell::Cell<Option<(Elem, Elem)>> = std::cell::Cell::new(expected);
        for it in items {
            match it {
                DictItem::Pair(k, v) => {
                    let (kv, kt) = self.expr(k)?;
                    let (vv, vt) = match kinds.get() {
                        Some((_, kind)) => self.expr_collection_context(v, self.elem_ty(kind))?,
                        None => self.expr(v)?,
                    };
                    let (kk, vk) = match kinds.get() {
                        Some((kk, vk)) => {
                            // A dict VALUE may be a concrete conformer of an
                            // interface-typed value position (#829): it is boxed
                            // into the fat pointer below. Keys cannot be interface
                            // typed (an interface value is unhashable), so the key
                            // still requires an exact representation match.
                            let va = value_elem_kind(vt, &self.ctx.nested)?;
                            if elem_kind(kt, &self.ctx.nested)? != kk
                                || (va != vk && !self.conforms_to_iface_elem(vt, vk, va))
                            {
                                return Err(err(
                                    "dict keys/values must all have the same representation",
                                ));
                            }
                            (kk, vk)
                        }
                        None => {
                            let kk = elem_kind(kt, &self.ctx.nested)?;
                            // The value is a value position (#800): a `float`/`f64`
                            // dict value is admitted; the key above keeps
                            // `elem_kind`, so a float *key* is still rejected.
                            let vk = value_elem_kind(vt, &self.ctx.nested)?;
                            kinds.set(Some((kk, vk)));
                            (kk, vk)
                        }
                    };
                    let kv = self.coerce(kv, kt, kk.ty());
                    let vv = self.coerce(vv, vt, vk.ty());
                    let kv = self.box_if_value(kv, kk.ty());
                    let vv = self.box_elem_for_store(vv, vk.ty());
                    let kfns = self.key_fn_args(kk)?;
                    let mut cargs = vec![dict, kv, vv];
                    cargs.extend_from_slice(&kfns);
                    self.key_rt_call(
                        None,
                        format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                        cargs,
                        kk,
                    );
                }
                DictItem::Spread(e) => {
                    let (src, st) = match kinds.get() {
                        Some((key, value)) => {
                            self.expr_collection_context(e, Ty::Dict(key, value))?
                        }
                        None => self.expr(e)?,
                    };
                    let Ty::Dict(skk, svk) = st else {
                        return Err(err("`**` in a dict display requires a dict"));
                    };
                    match kinds.get() {
                        Some((kk, vk)) => {
                            if skk != kk || svk != vk {
                                return Err(err(
                                    "dict keys/values must all have the same representation",
                                ));
                            }
                        }
                        None => kinds.set(Some((skk, svk))),
                    }
                    self.merge_dict_into(dict, src, skk, svk)?;
                }
            }
        }
        let (kk, vk) = kinds.get().unwrap_or((Elem::I64, Elem::I64));
        self.func.value_types[dict as usize] = Ty::Dict(kk, vk);
        Ok((dict, Ty::Dict(kk, vk)))
    }

    /// Copy every entry of `src` (a `dict[kk, vk]`) into `dst` (#598): snapshot
    /// `src`'s keys, then for each key set `dst[key] = src[key]`, so a key
    /// already in `dst` is overwritten (last-wins merge semantics).
    fn merge_dict_into(
        &mut self,
        dst: Value,
        src: Value,
        kk: Elem,
        vk: Elem,
    ) -> Result<(), LowerError> {
        let keys = self.new_value(Ty::List(kk));
        self.emit(Inst::Call {
            dest: Some(keys),
            func: format!("zap_dict_keys_{}", kk.val_suffix()),
            args: vec![src],
        });
        let kfns = self.key_fn_args(kk)?;
        let get_fn = format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix());
        let set_fn = format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix());
        self.iterate_list(keys, kk, |s, key, _| {
            let val = s.new_value(vk.ty());
            let mut gargs = vec![src, key];
            gargs.extend_from_slice(&kfns);
            s.emit(Inst::Call {
                dest: Some(val),
                func: get_fn.clone(),
                args: gargs,
            });
            let mut sargs = vec![dst, key, val];
            sargs.extend_from_slice(&kfns);
            s.emit(Inst::Call {
                dest: None,
                func: set_fn.clone(),
                args: sargs,
            });
            Ok(())
        })
    }

    fn lower_set_lit(&mut self, elems: &[Expr]) -> Result<(Value, Ty), LowerError> {
        // Build a fresh set and add each element in source order; a `*expr` splat
        // (#598) iterates its operand and adds each spliced element. The element
        // kind is fixed by the first added element (an empty `{}` is a dict, so a
        // set literal always has >= 1 element).
        let set = self.new_value(Ty::Set(Elem::I64));
        self.emit(Inst::Call {
            dest: Some(set),
            func: "zap_set_new".to_string(),
            args: vec![],
        });
        let elem: std::cell::Cell<Option<Elem>> = std::cell::Cell::new(None);
        let add = |s: &mut FnLower<'a>, v: Value, t: Ty| -> Result<(), LowerError> {
            let ek = match elem.get() {
                Some(e) => {
                    if elem_kind(t, &s.ctx.nested)? != e {
                        return Err(err("set elements must all have the same representation"));
                    }
                    e
                }
                None => {
                    let e = elem_kind(t, &s.ctx.nested)?;
                    elem.set(Some(e));
                    e
                }
            };
            let v = s.coerce(v, t, ek.ty());
            let v = s.box_if_value(v, ek.ty());
            let kfns = s.key_fn_args(ek)?;
            let mut cargs = vec![set, v];
            cargs.extend_from_slice(&kfns);
            s.key_rt_call(None, format!("zap_set_add_{}", ek.key_suffix()), cargs, ek);
            Ok(())
        };
        for el in elems {
            match el {
                Expr::Starred { value, .. } => {
                    self.iterate(value, |s, v, t| add(s, v, t))?;
                }
                _ => {
                    let (v, t) = self.expr(el)?;
                    add(self, v, t)?;
                }
            }
        }
        let ek = elem.get().unwrap_or(Elem::I64);
        self.func.value_types[set as usize] = Ty::Set(ek);
        Ok((set, Ty::Set(ek)))
    }

    /// Lower `set[T]()` (empty) / `set[T](iterable)` (from an iterable), D17 #609.
    /// The element kind `T` comes from the subscript type name; an empty call
    /// builds a fresh `set[T]`, and a single positional iterable argument builds
    /// `set[T]` of that iterable's deduplicated elements.
    fn lower_set_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // The element type may be any valid type expression, not just a bare
        // name (#703): `set[(int64, int64)]()` and `set[list[int64]]()` are now
        // accepted, consistent with the set literal `{(1, 2)}` and the
        // `dict[(..), ..]` key forms (D-0122). A tuple element routes through the
        // same per-type `__hash__`/`__eq__` key path as the literal; an
        // unhashable element (a nested collection, a float) is still rejected
        // cleanly below / in `key_fn_args`, never with an ICE.
        let ety = resolve_ty(&expr_to_type(elem_expr)?, &self.ctx.tyenv())?;
        let elem = elem_kind(ety, &self.ctx.nested)?;
        // A nested collection is unhashable (#625): reject `set[list[..]]()` loudly
        // rather than letting it hash a pointer as a string on a later `.add`.
        if let Elem::Nested(id) = elem {
            let ty = self.ctx.nested.borrow().get(id);
            return Err(err(format!(
                "a {ty} is unhashable and cannot be a set element (collections have no stable hash)"
            )));
        }
        // A single positional argument is an iterable (`set[T](xs)`); a single
        // `capacity=` keyword (#1083) pre-sizes an empty set. No argument builds
        // an empty set.
        if args.len() == 1 && args[0].name.is_none() {
            return self.lower_set_from_iterable(&args[0].value, Some(elem));
        }
        let cap_arg = self.dict_set_capacity_arg(args, "set")?;
        let set = self.new_value(Ty::Set(elem));
        self.emit(Inst::Call {
            dest: Some(set),
            func: "zap_set_new".to_string(),
            args: vec![],
        });
        if let Some(n) = cap_arg {
            self.emit(Inst::Call {
                dest: None,
                func: "zap_set_reserve".to_string(),
                args: vec![set, n],
            });
        }
        Ok((set, Ty::Set(elem)))
    }

    /// Empty `list[T]()` construction (#1004): the generic-context synonym for
    /// the `[]` literal — same `zap_list_new` the literal lowers to — so code
    /// whose element type is a type parameter can build an empty list without a
    /// literal. Takes no arguments (a populated list uses `[...]`).
    fn lower_list_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let ety = resolve_ty(&expr_to_type(elem_expr)?, &self.ctx.tyenv())?;
        let elem = list_elem_kind(ety, &self.ctx.nested)?;
        if !args.is_empty() {
            return Err(err(
                "list[T]() takes no arguments; build a populated list with a `[...]` literal",
            ));
        }
        let list = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        Ok((list, Ty::List(elem)))
    }

    /// Empty `dict[K, V]()` construction (#1004): the generic-context synonym for
    /// the `{}` literal — same `zap_dict_new` the literal lowers to. The subscript
    /// carries the two type arguments (`Expr::Tuple`). Takes no arguments.
    fn lower_dict_new(
        &mut self,
        index: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let (k_expr, v_expr) = match index {
            Expr::Tuple(elems, _) if elems.len() == 2 => (&elems[0], &elems[1]),
            _ => {
                return Err(err(
                    "dict[K, V]() needs two type arguments (e.g. dict[str, int64]())",
                ))
            }
        };
        let kt = resolve_ty(&expr_to_type(k_expr)?, &self.ctx.tyenv())?;
        let vt = resolve_ty(&expr_to_type(v_expr)?, &self.ctx.tyenv())?;
        let kk = elem_kind(kt, &self.ctx.nested)?;
        let vk = value_elem_kind(vt, &self.ctx.nested)?;
        // Optional `capacity=` initial-capacity hint (#1083): pre-sizes the SoA
        // table so a known-large build does not regrow. Sema has validated the
        // keyword name, int type, and non-negative literal; lowering emits the
        // fresh dict then a `reserve` for the hint.
        let cap_arg = self.dict_set_capacity_arg(args, "dict")?;
        let dict = self.new_value(Ty::Dict(kk, vk));
        self.emit(Inst::Call {
            dest: Some(dict),
            func: "zap_dict_new".to_string(),
            args: vec![],
        });
        if let Some(n) = cap_arg {
            self.emit(Inst::Call {
                dest: None,
                func: "zap_dict_reserve".to_string(),
                args: vec![dict, n],
            });
        }
        Ok((dict, Ty::Dict(kk, vk)))
    }

    /// Extract and lower an optional `capacity=` constructor keyword into an
    /// `i64` capacity value (#1083), shared by `dict[K,V](...)` and
    /// `set[T](capacity=...)`. Returns `None` for no argument. The single
    /// argument must be the `capacity` keyword; a positional or misnamed
    /// argument, or a negative literal, is rejected (sema reports the same with
    /// a precise code, but lowering guards independently — set construction is
    /// loosely typed in sema). `container` names the surface for messages.
    fn dict_set_capacity_arg(
        &mut self,
        args: &[ast::Arg],
        container: &str,
    ) -> Result<Option<Value>, LowerError> {
        match args.len() {
            0 => Ok(None),
            1 => {
                let a = &args[0];
                if a.name.as_deref() != Some("capacity") {
                    return Err(err(format!(
                        "{container}[...](...) accepts only the keyword `capacity` \
                         (an initial-capacity hint)"
                    )));
                }
                if is_negative_int_literal(&a.value) {
                    return Err(err(format!("{container} capacity cannot be negative")));
                }
                let (v, vt) = self.expr(&a.value)?;
                Ok(Some(self.coerce(v, vt, Ty::I64)))
            }
            _ => Err(err(format!(
                "{container}[...](...) takes at most one `capacity=` keyword"
            ))),
        }
    }

    /// Build a set from an iterable (D17 #609): `set(iterable)` and the
    /// `set[T](iterable)` tail. Iterates the operand through the general protocol
    /// and adds each element (dedup is the runtime's), mirroring `lower_set_lit`'s
    /// add path. `declared` fixes the element kind for `set[T](..)`; `None`
    /// (plain `set(..)`) lets the first element fix it, defaulting to `i64` for an
    /// empty iterable.
    fn lower_set_from_iterable(
        &mut self,
        iterable: &Expr,
        declared: Option<Elem>,
    ) -> Result<(Value, Ty), LowerError> {
        let set = self.new_value(Ty::Set(declared.unwrap_or(Elem::I64)));
        self.emit(Inst::Call {
            dest: Some(set),
            func: "zap_set_new".to_string(),
            args: vec![],
        });
        let elem: std::cell::Cell<Option<Elem>> = std::cell::Cell::new(declared);
        self.iterate(iterable, |s, v, t| {
            let ek = match elem.get() {
                Some(e) => {
                    if elem_kind(t, &s.ctx.nested)? != e {
                        return Err(err("set elements must all have the same representation"));
                    }
                    e
                }
                None => {
                    let e = elem_kind(t, &s.ctx.nested)?;
                    elem.set(Some(e));
                    e
                }
            };
            let v = s.coerce(v, t, ek.ty());
            let v = s.box_if_value(v, ek.ty());
            let kfns = s.key_fn_args(ek)?;
            let mut cargs = vec![set, v];
            cargs.extend_from_slice(&kfns);
            s.key_rt_call(None, format!("zap_set_add_{}", ek.key_suffix()), cargs, ek);
            Ok(())
        })?;
        let ek = elem.get().unwrap_or(Elem::I64);
        self.func.value_types[set as usize] = Ty::Set(ek);
        Ok((set, Ty::Set(ek)))
    }

    fn lower_set_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "add" => {
                if args.len() != 1 {
                    return Err(err("add() takes exactly one argument"));
                }
                // A bare `None` element needs the set's element type as context so
                // it lowers to the tagged Optional `None` (`set[Optional[T]]`,
                // #704); every other element keeps the explicit element-type check.
                let v = if matches!(&args[0].value, Expr::None(_)) {
                    self.expr_coerced(&args[0].value, elem.ty(), zapc_diag::ErrorCode::ZAP4021)?
                } else {
                    let (v, vt) = self.expr(&args[0].value)?;
                    if elem_kind(vt, &self.ctx.nested)? != elem {
                        return Err(err("add() value has the wrong element type"));
                    }
                    self.coerce(v, vt, elem.ty())
                };
                let v = self.box_if_value(v, elem.ty());
                let kfns = self.key_fn_args(elem)?;
                let mut cargs = vec![recv, v];
                cargs.extend_from_slice(&kfns);
                self.key_rt_call(
                    None,
                    format!("zap_set_add_{}", elem.key_suffix()),
                    cargs,
                    elem,
                );
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            // `reserve(n)` (#1083): pre-size the SoA table for ~n more adds. One
            // positional integer arg; sema (loose for sets) is backed by this
            // arity/type check, and a negative literal is rejected here too.
            "reserve" => {
                if args.len() != 1 || args.iter().any(|a| a.name.is_some()) {
                    return Err(err("set.reserve(n) takes exactly one positional argument"));
                }
                if is_negative_int_literal(&args[0].value) {
                    return Err(err("set.reserve(n) capacity cannot be negative"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                let n = self.coerce(v, vt, Ty::I64);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_set_reserve".to_string(),
                    args: vec![recv, n],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            other => Err(err(format!(
                "set has no method '{other}' (supported: add, reserve)"
            ))),
        }
    }

    /// Lower a built-in `dict[K, V]` method call (ticket #299). All four take no
    /// arguments: `keys()`/`values()` snapshot a `list`; `items()` builds a
    /// `list[(K, V)]`; `copy()` makes a shallow dict copy. Sema has checked arity.
    fn lower_dict_method(
        &mut self,
        recv: Value,
        kk: Elem,
        vk: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // `dict.get(key) -> Optional[V]` (ticket #281). Monomorphization has
        // encoded the concrete `Optional$V` instance name into the method symbol
        // (`get$Optional$str`), since the lossy `Elem` kind here cannot name it.
        if let Some(opt_name) = name.strip_prefix("get$") {
            return self.lower_dict_get(recv, kk, vk, opt_name, args);
        }
        // `dict.get(key, default) -> V` (ticket #570): monomorphization marked
        // the two-arg form `get_default`. The result is a plain `V`, so (unlike
        // the one-arg `Optional[V]` form) no `Optional$V` instance name is needed.
        if name == "get_default" {
            return self.lower_dict_get_default(recv, kk, vk, args);
        }
        // `reserve(n)` (#1083): pre-size the SoA table for ~n more inserts. One
        // positional integer arg; sema has already rejected a negative literal.
        if name == "reserve" {
            if args.len() != 1 {
                return Err(err("dict.reserve(n) takes exactly one argument"));
            }
            let (v, vt) = self.expr(&args[0].value)?;
            let n = self.coerce(v, vt, Ty::I64);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_dict_reserve".to_string(),
                args: vec![recv, n],
            });
            return Ok((self.iconst(Ty::I32, 0), Ty::I32));
        }
        if !args.is_empty() {
            return Err(err(format!("dict.{name}() takes no arguments")));
        }
        match name {
            "keys" => {
                let list = self.new_value(Ty::List(kk));
                self.emit(Inst::Call {
                    dest: Some(list),
                    func: format!("zap_dict_keys_{}", kk.val_suffix()),
                    args: vec![recv],
                });
                Ok((list, Ty::List(kk)))
            }
            "values" => {
                let list = self.new_value(Ty::List(vk));
                self.emit(Inst::Call {
                    dest: Some(list),
                    func: format!("zap_dict_values_{}", vk.val_suffix()),
                    args: vec![recv],
                });
                Ok((list, Ty::List(vk)))
            }
            "copy" => {
                let d = self.new_value(Ty::Dict(kk, vk));
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_dict_copy".to_string(),
                    args: vec![recv],
                });
                Ok((d, Ty::Dict(kk, vk)))
            }
            "items" => self.lower_dict_items(recv, kk, vk),
            "get" => Err(err(
                "dict.get()'s result type could not be resolved; annotate the \
                 result as `Optional[V]` or give the dict a concrete type \
                 (ticket #281)",
            )),
            other => Err(err(format!(
                "dict has no method '{other}' (supported: keys, values, items, copy, get, reserve)"
            ))),
        }
    }

    /// Lower `d.get(key) -> Optional[V]` (ticket #281): a safe, non-aborting read
    /// — `Some(value)` when the key is present, `None` when absent. The precise
    /// `Optional$V` instance name is passed in `opt_name` (encoded into the method
    /// symbol by monomorphization, which alone knew the concrete `V`). Presence is
    /// decided by the `#299` membership probe (`zap_dict_contains_*`), never by the
    /// fetched word, so a legitimately-stored `0`/null value is reported as
    /// `Some`, not confused with a miss. The branch builds `Some`/`None` with the
    /// shared niche/tagged `Optional` helpers and joins through a slot.
    fn lower_dict_get(
        &mut self,
        recv: Value,
        kk: Elem,
        vk: Elem,
        opt_name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if args.len() != 1 {
            return Err(err(format!(
                "dict.get() takes exactly one argument (the key), got {}",
                args.len()
            )));
        }
        let sid = self.ctx.ids.get(opt_name).copied().ok_or_else(|| {
            err(format!(
                "dict.get()'s result type '{opt_name}' was not instantiated (internal; ticket #281)"
            ))
        })?;
        let opt = optional_layout(self.ctx, Ty::Struct(sid)).ok_or_else(|| {
            err(format!(
                "dict.get()'s result type '{opt_name}' is not an Optional instance (internal)"
            ))
        })?;

        // The key, plus the shared presence/fetch call setup. A borrowed `strview`
        // key on a str-keyed dict (#1082) probes the borrowed bytes directly via
        // the `_strview` entry points — no owned key allocation, no root — passing
        // the view's `(base, off, len)` triple in place of a key word. Every other
        // key is coerced to the dict's key kind and carries the struct-key hash/eq
        // function pointers (empty for scalar/string keys).
        let (kv, kt) = self.expr(&args[0].value)?;
        let view_sid = match kt {
            Ty::Struct(s) if self.is_any_view_sid(s) => Some(s),
            _ => None,
        };
        let view_key = view_sid.is_some() && matches!(kk, Elem::Ptr | Elem::Bytes);
        let (contains_fn, get_fn, probe_args) = if let (true, Some(vs)) = (view_key, view_sid) {
            let (base, off, len) = self.view_fields(kv, vs);
            (
                "zap_dict_contains_strview".to_string(),
                format!("zap_dict_get_strview_{}", vk.val_suffix()),
                vec![recv, base, off, len],
            )
        } else {
            let key = self.coerce(kv, kt, kk.ty());
            let mut a = vec![recv, key];
            a.extend_from_slice(&self.key_fn_args(kk)?);
            (
                format!("zap_dict_contains_{}", kk.key_suffix()),
                format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix()),
                a,
            )
        };

        // present = (contains(d, key) != 0). A float key canonicalizes + NaN-rejects
        // in the runtime (#803), so this probe may raise; the value fetch below is
        // reached only for a present (hence non-NaN) key, so it cannot.
        let word = self.new_value(Ty::I64);
        self.key_rt_call(Some(word), contains_fn, probe_args.clone(), kk);
        let zero = self.iconst(Ty::I64, 0);
        let present = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: present,
            pred: CmpPred::Ne,
            lhs: word,
            rhs: zero,
        });

        let slot = self.alloca(Ty::Struct(sid));
        let some_bb = self.new_block();
        let none_bb = self.new_block();
        let join_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: present,
            then_bb: some_bb,
            else_bb: none_bb,
        });

        // present -> Some(d[key])
        self.position(some_bb);
        let raw = self.new_value(vk.ty());
        self.emit(Inst::Call {
            dest: Some(raw),
            func: get_fn,
            args: probe_args,
        });
        let inner = self.coerce(raw, vk.ty(), opt.payload_ty);
        let someval = gen_opt_some(self, &opt, inner);
        self.store(slot, someval);
        self.set_term(Term::Br(join_bb));

        // absent -> None
        self.position(none_bb);
        let noneval = gen_opt_none(self, &opt);
        self.store(slot, noneval);
        self.set_term(Term::Br(join_bb));

        self.position(join_bb);
        let out = self.load(slot, Ty::Struct(sid));
        Ok((out, Ty::Struct(sid)))
    }

    /// Lower `d.get(key, default) -> V` (ticket #570): the value when the key is
    /// present, else `default`. Like the one-arg form it reuses the `#299`
    /// membership probe (`zap_dict_contains_*`) to decide presence — never the
    /// fetched word — so a legitimately-stored `0`/null value selects the stored
    /// value, not the default. There is no `Optional` wrapper: both arms produce
    /// a plain `V`, joined through a slot. The lookup stays single-sourced
    /// (`zap_dict_get_*`); no new C runtime entry point is added.
    fn lower_dict_get_default(
        &mut self,
        recv: Value,
        kk: Elem,
        vk: Elem,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if args.len() != 2 {
            return Err(err(format!(
                "dict.get() takes one or two arguments (the key, and an optional \
                 default), got {}",
                args.len()
            )));
        }

        // The key + shared probe setup (see `lower_dict_get`): a borrowed `strview`
        // key on a str-keyed dict (#1082) probes the borrowed bytes via the
        // `_strview` entry points with no owned key alloc; any other key is coerced
        // and carries the struct-key hash/eq function pointers.
        let (kv, kt) = self.expr(&args[0].value)?;
        let view_sid = match kt {
            Ty::Struct(s) if self.is_any_view_sid(s) => Some(s),
            _ => None,
        };
        let view_key = view_sid.is_some() && matches!(kk, Elem::Ptr | Elem::Bytes);
        let (contains_fn, get_fn, probe_args) = if let (true, Some(vs)) = (view_key, view_sid) {
            let (base, off, len) = self.view_fields(kv, vs);
            (
                "zap_dict_contains_strview".to_string(),
                format!("zap_dict_get_strview_{}", vk.val_suffix()),
                vec![recv, base, off, len],
            )
        } else {
            let key = self.coerce(kv, kt, kk.ty());
            let mut a = vec![recv, key];
            a.extend_from_slice(&self.key_fn_args(kk)?);
            (
                format!("zap_dict_contains_{}", kk.key_suffix()),
                format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix()),
                a,
            )
        };

        // The default, coerced to the dict's value kind — the same `V` the fetch
        // yields, so both arms agree on the slot's type.
        let (dv, dt) = self.expr(&args[1].value)?;
        let default = self.coerce(dv, dt, vk.ty());

        // present = (contains(d, key) != 0). A float key canonicalizes + NaN-rejects
        // in the runtime (#803), so this probe may raise; the value fetch below is
        // reached only for a present (hence non-NaN) key, so it cannot.
        let word = self.new_value(Ty::I64);
        self.key_rt_call(Some(word), contains_fn, probe_args.clone(), kk);
        let zero = self.iconst(Ty::I64, 0);
        let present = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: present,
            pred: CmpPred::Ne,
            lhs: word,
            rhs: zero,
        });

        let slot = self.alloca(vk.ty());
        let some_bb = self.new_block();
        let none_bb = self.new_block();
        let join_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: present,
            then_bb: some_bb,
            else_bb: none_bb,
        });

        // present -> d[key]
        self.position(some_bb);
        let raw = self.new_value(vk.ty());
        self.emit(Inst::Call {
            dest: Some(raw),
            func: get_fn,
            args: probe_args,
        });
        self.store(slot, raw);
        self.set_term(Term::Br(join_bb));

        // absent -> default
        self.position(none_bb);
        self.store(slot, default);
        self.set_term(Term::Br(join_bb));

        self.position(join_bb);
        let out = self.load(slot, vk.ty());
        Ok((out, vk.ty()))
    }

    /// Lower `d.items()` to a `list[(K, V)]` (ticket #299): snapshot the keys,
    /// then walk them building a `(key, value)` tuple per entry and pushing it.
    /// The element list is a list of tuple pointers, so `for k, v in d.items()`
    /// rides the ordinary list iterator + tuple-destructuring loop target.
    fn lower_dict_items(
        &mut self,
        recv: Value,
        kk: Elem,
        vk: Elem,
    ) -> Result<(Value, Ty), LowerError> {
        // The tuple's field types are the key/value *full* types (#625) so a
        // nested-collection value (`dict[str, list[int64]].items()`) binds `v` as
        // its real `list`/`dict`/... rather than a `str` pointer.
        let kty = self.elem_ty(kk);
        let vty = self.elem_ty(vk);
        let tuple_sid = self.ctx.tuples.borrow_mut().intern(vec![kty, vty]);
        let out_elem = Elem::Struct(tuple_sid);
        // keys snapshot, result list, and the struct-key function pointers.
        let keys = self.new_value(Ty::List(kk));
        self.emit(Inst::Call {
            dest: Some(keys),
            func: format!("zap_dict_keys_{}", kk.val_suffix()),
            args: vec![recv],
        });
        let out = self.new_value(Ty::List(out_elem));
        self.emit(Inst::Call {
            dest: Some(out),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let kfns = self.key_fn_args(kk)?;
        let get_fn = format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix());

        // for i in 0..len(keys): out.push((keys[i], d[keys[i]]))
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let len = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_list_len".to_string(),
            args: vec![keys],
        });
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: len,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let key = self.new_value(kty);
        self.emit(Inst::Call {
            dest: Some(key),
            func: format!("zap_list_get_{}", kk.val_suffix()),
            args: vec![keys, i],
        });
        let val = self.new_value(vty);
        let mut gargs = vec![recv, key];
        gargs.extend_from_slice(&kfns);
        self.emit(Inst::Call {
            dest: Some(val),
            func: get_fn,
            args: gargs,
        });
        let tup = self.new_value(Ty::Struct(tuple_sid));
        self.emit(Inst::StructNew {
            dest: tup,
            sid: tuple_sid,
        });
        self.emit(Inst::FieldSet {
            base: tup,
            sid: tuple_sid,
            idx: 0,
            val: key,
        });
        self.emit(Inst::FieldSet {
            base: tup,
            sid: tuple_sid,
            idx: 1,
            val,
        });
        self.emit(Inst::Call {
            dest: None,
            func: "zap_list_push_ptr".to_string(),
            args: vec![out, tup],
        });
        let next = self.new_value(Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: i,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));
        self.position(end_bb);
        Ok((out, Ty::List(out_elem)))
    }

    /// Lower `http.req_headers(req) -> list[(str, str)]` (#960, D-0197): enumerate
    /// the full received request header block as `(name, value)` pairs.
    ///
    /// Mirrors [`lower_dict_items`]: three infallible runtime accessors on the
    /// parsed request handle — `zap_http_req_header_count` and the positional
    /// `zap_http_req_header_name` / `zap_http_req_header_val` — drive a counted
    /// loop that assembles each pair with the compiler's own tuple path (intern +
    /// `StructNew` + `FieldSet`). Received order is preserved and duplicate header
    /// names stay as separate pairs; the runtime returns names verbatim (original
    /// case), leaving case-insensitive discovery to the by-name `req_header`.
    ///
    /// Building the tuples in lowering (not the runtime) keeps the GC/field layout
    /// agreement codegen's `FieldGet` relies on: a `str` flows as a [`Ty::Ptr`] in
    /// ZIR, so the interned fields are `(Ptr, Ptr)` — byte-identical to a
    /// `dict[str, str].items()` element, which is also iterated as `for name,
    /// value in ...`.
    /// `http.req_headers(req)` — enumerate a request's received header block.
    fn lower_http_req_headers(&mut self, req: Value) -> (Value, Ty) {
        self.lower_http_header_pairs(
            req,
            "zap_http_req_header_count",
            "zap_http_req_header_name",
            "zap_http_req_header_val",
        )
    }

    /// `http.headers(resp)` — enumerate a client response's received header block
    /// (#989); the response twin of `req_headers`, so multiple `Set-Cookie` lines
    /// are each visible.
    fn lower_http_resp_headers(&mut self, resp: Value) -> (Value, Ty) {
        self.lower_http_header_pairs(
            resp,
            "zap_http_resp_header_count",
            "zap_http_resp_header_name",
            "zap_http_resp_header_val",
        )
    }

    /// Shared body for the header-block enumerators: build a `list[(str, str)]`
    /// by looping `0..count_fn(handle)` and pushing `(name_fn, val_fn)` pairs.
    fn lower_http_header_pairs(
        &mut self,
        req: Value,
        count_fn: &str,
        name_fn: &str,
        val_fn: &str,
    ) -> (Value, Ty) {
        let tuple_sid = self.ctx.tuples.borrow_mut().intern(vec![Ty::Ptr, Ty::Ptr]);
        let out_elem = Elem::Struct(tuple_sid);
        let out = self.new_value(Ty::List(out_elem));
        self.emit(Inst::Call {
            dest: Some(out),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        // n = <count_fn>(handle): loop-invariant, computed once.
        let count = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(count),
            func: count_fn.to_string(),
            args: vec![req],
        });

        // for i in 0..n: out.push((name(req, i), val(req, i)))
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: count,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let name = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(name),
            func: name_fn.to_string(),
            args: vec![req, i],
        });
        let val = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(val),
            func: val_fn.to_string(),
            args: vec![req, i],
        });
        let tup = self.new_value(Ty::Struct(tuple_sid));
        self.emit(Inst::StructNew {
            dest: tup,
            sid: tuple_sid,
        });
        self.emit(Inst::FieldSet {
            base: tup,
            sid: tuple_sid,
            idx: 0,
            val: name,
        });
        self.emit(Inst::FieldSet {
            base: tup,
            sid: tuple_sid,
            idx: 1,
            val,
        });
        self.emit(Inst::Call {
            dest: None,
            func: "zap_list_push_ptr".to_string(),
            args: vec![out, tup],
        });
        let next = self.new_value(Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: i,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));
        self.position(end_bb);
        (out, Ty::List(out_elem))
    }

    /// `http.parse_set_cookie(value)` — parse one `Set-Cookie` header value into
    /// the prelude `Cookie` value struct (#989), one
    /// `zap_http_cookie_<field>(value)` getter per field (the `url.split` /
    /// `UrlParts` pattern). Field order matches `PRELUDE_SRC`: name(0) value(1)
    /// domain(2) path(3) expires(4) max_age(5) secure(6) http_only(7)
    /// same_site(8). `secure` / `http_only` are C `int64` 0/1 normalized to the
    /// struct's `bool` fields.
    fn lower_http_parse_set_cookie(&mut self, v: Value) -> (Value, Ty) {
        let sid = *self
            .ctx
            .ids
            .get("Cookie")
            .expect("Cookie struct unavailable (built-in prelude missing)");
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        let str_fields: [(&str, u32); 6] = [
            ("zap_http_cookie_name", 0),
            ("zap_http_cookie_value", 1),
            ("zap_http_cookie_domain", 2),
            ("zap_http_cookie_path", 3),
            ("zap_http_cookie_expires", 4),
            ("zap_http_cookie_same_site", 8),
        ];
        for (func, idx) in str_fields {
            let f = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(f),
                func: func.to_string(),
                args: vec![v],
            });
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx,
                val: f,
            });
        }
        let ma = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(ma),
            func: "zap_http_cookie_max_age".to_string(),
            args: vec![v],
        });
        self.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: 5,
            val: ma,
        });
        for (func, idx) in [
            ("zap_http_cookie_secure", 6u32),
            ("zap_http_cookie_http_only", 7u32),
        ] {
            let raw = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(raw),
                func: func.to_string(),
                args: vec![v],
            });
            let zero = self.iconst(Ty::I64, 0);
            let b = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: b,
                pred: CmpPred::Ne,
                lhs: raw,
                rhs: zero,
            });
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx,
                val: b,
            });
        }
        (dest, Ty::Struct(sid))
    }

    /// Lower `fs.scandir(path)` to a `list[DirEntry]` (#1032, D-0224). The runtime
    /// `zap_fs_scandir` materialises the directory into a handle; this enumerates
    /// it with a count + indexed getters (the `req_headers` pattern), building one
    /// prelude `DirEntry { name, is_dir, is_file }` per entry. The `is_*` getters
    /// return a C `int64` 0/1, normalised to `i1` for the `bool` fields.
    fn lower_fs_scandir(&mut self, path: Value) -> (Value, Ty) {
        // Materialise the directory into a handle first (raises on a bad path),
        // then enumerate it — the arg is the path, not the handle.
        let handle = self.new_value(Ty::Ptr);
        self.throwing_call(Some(handle), "zap_fs_scandir", vec![path]);
        let de_sid = *self
            .ctx
            .ids
            .get("DirEntry")
            .expect("DirEntry struct unavailable (built-in prelude missing)");
        let out_elem = Elem::Struct(de_sid);
        let out = self.new_value(Ty::List(out_elem));
        self.emit(Inst::Call {
            dest: Some(out),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let count = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(count),
            func: "zap_fs_scandir_count".to_string(),
            args: vec![handle],
        });
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: count,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let name = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(name),
            func: "zap_fs_scandir_name".to_string(),
            args: vec![handle, i],
        });
        let is_dir = self.scandir_bool("zap_fs_scandir_is_dir", handle, i);
        let is_file = self.scandir_bool("zap_fs_scandir_is_file", handle, i);
        let de = self.new_value(Ty::Struct(de_sid));
        self.emit(Inst::StructNew {
            dest: de,
            sid: de_sid,
        });
        self.emit(Inst::FieldSet {
            base: de,
            sid: de_sid,
            idx: 0,
            val: name,
        });
        self.emit(Inst::FieldSet {
            base: de,
            sid: de_sid,
            idx: 1,
            val: is_dir,
        });
        self.emit(Inst::FieldSet {
            base: de,
            sid: de_sid,
            idx: 2,
            val: is_file,
        });
        self.emit(Inst::Call {
            dest: None,
            func: "zap_list_push_ptr".to_string(),
            args: vec![out, de],
        });
        let next = self.new_value(Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: i,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));
        self.position(end_bb);
        (out, Ty::List(out_elem))
    }

    /// Call an infallible `int64` 0/1 scandir getter and normalise it to `i1`.
    fn scandir_bool(&mut self, func: &str, handle: Value, i: Value) -> Value {
        let word = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(word),
            func: func.to_string(),
            args: vec![handle, i],
        });
        let zero = self.iconst(Ty::I64, 0);
        let b = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: b,
            pred: CmpPred::Ne,
            lhs: word,
            rhs: zero,
        });
        b
    }

    /// Lower `fs.walk(root)` to a `list[(str, list[str], list[str])]` (#1032,
    /// D-0224). `zap_fs_walk` materialises the whole tree into a handle; this
    /// enumerates it with a count + indexed getters, building one
    /// `(dirpath, dirs, files)` tuple per visited directory. `dirs`/`files` are
    /// `list[str]` the runtime already built, handed back as pointers.
    fn lower_fs_walk(&mut self, root: Value) -> (Value, Ty) {
        // Materialise the whole tree into a handle first (raises on an unreadable
        // root), then enumerate it — the arg is the root path, not the handle.
        let handle = self.new_value(Ty::Ptr);
        self.throwing_call(Some(handle), "zap_fs_walk", vec![root]);
        // Intern the tuple with the REAL field types — `(str, list[str],
        // list[str])` — not three bare `Ptr`s. `bind_tuple_loop_target` types the
        // destructured `dirs`/`files` from these field types, so a bare `Ptr`
        // would make the inner `for d in dirs` iterate a string instead of the
        // list (#1032).
        let str_list = Ty::List(Elem::Ptr);
        let tuple_sid = self
            .ctx
            .tuples
            .borrow_mut()
            .intern(vec![Ty::Ptr, str_list, str_list]);
        let out_elem = Elem::Struct(tuple_sid);
        let out = self.new_value(Ty::List(out_elem));
        self.emit(Inst::Call {
            dest: Some(out),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let count = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(count),
            func: "zap_fs_walk_count".to_string(),
            args: vec![handle],
        });
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: count,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let dirpath = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(dirpath),
            func: "zap_fs_walk_dirpath".to_string(),
            args: vec![handle, i],
        });
        let dirs = self.new_value(Ty::List(Elem::Ptr));
        self.emit(Inst::Call {
            dest: Some(dirs),
            func: "zap_fs_walk_dirs".to_string(),
            args: vec![handle, i],
        });
        let files = self.new_value(Ty::List(Elem::Ptr));
        self.emit(Inst::Call {
            dest: Some(files),
            func: "zap_fs_walk_files".to_string(),
            args: vec![handle, i],
        });
        let tup = self.new_value(Ty::Struct(tuple_sid));
        self.emit(Inst::StructNew {
            dest: tup,
            sid: tuple_sid,
        });
        for (fidx, val) in [(0u32, dirpath), (1, dirs), (2, files)] {
            self.emit(Inst::FieldSet {
                base: tup,
                sid: tuple_sid,
                idx: fidx,
                val,
            });
        }
        self.emit(Inst::Call {
            dest: None,
            func: "zap_list_push_ptr".to_string(),
            args: vec![out, tup],
        });
        let next = self.new_value(Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: i,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));
        self.position(end_bb);
        (out, Ty::List(out_elem))
    }

    /// Lower `heap[T]()` / `heap[T](cmp="max")` (D-0057). The element kind is the
    /// subscript; the only argument is the keyword `cmp` selecting direction
    /// (`"min"` is the default min-heap, `"max"` a max-heap). Lowers to a
    /// `zap_heap_new(kind, dir)` runtime call where `kind` picks the element word
    /// representation (0 = i64, 1 = str) and `dir` the ordering (0 = min, 1 = max).
    fn lower_heap_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // Accept any valid type expression and classify it exactly like the
        // `heap[T]` type annotation (#703): `heap_elem_of` admits only the
        // *ordered* element kinds (integers/bools/chars and `str`), so a tuple —
        // which has no defined ordering — is rejected cleanly here, the same way
        // the annotation rejects it, rather than with the misleading "expects an
        // element type name". (Ordered tuple/struct heap elements are a separate
        // follow-up.)
        let elem = heap_elem_of(&expr_to_type(elem_expr)?, &self.ctx.tyenv())?;
        let mut dir = 0i64; // 0 = min-heap (default), 1 = max-heap
                            // A single leading positional argument is the initial items list to
                            // heapify in O(n) (#978, D-0236): `heap[T](xs)` / `heap[T](xs, cmp=...)`.
        let mut items: Option<&Expr> = None;
        for (i, a) in args.iter().enumerate() {
            match a.name.as_deref() {
                Some("cmp") => {
                    let Expr::Str { value, .. } = &a.value else {
                        return Err(err(
                            "heap cmp must be a string literal (\"min\" or \"max\")",
                        ));
                    };
                    dir = match value.as_str() {
                        "min" => 0,
                        "max" => 1,
                        other => {
                            return Err(err(format!(
                                "heap cmp must be \"min\" or \"max\", found \"{other}\""
                            )))
                        }
                    };
                }
                Some(other) => {
                    return Err(err(format!(
                        "heap[T](...) has no keyword argument '{other}' (only 'cmp')"
                    )))
                }
                None if i == 0 && items.is_none() => items = Some(&a.value),
                None => {
                    return Err(err(
                        "heap[T](...) takes at most one positional argument (the items \
                         list to heapify) plus the keyword 'cmp' (e.g. cmp=\"max\")",
                    ))
                }
            }
        }
        // 0 = i64 word (integers/bools/chars, and a tier-1 enum carried as its
        // backing `i64` discriminant), 1 = str (pointer bytes), 2 = f64 (IEEE value
        // via a total-order comparator, #800). A tier-1 enum element MUST select
        // the `i64` comparator, not the pointer/`str` one: it is an `i64` word
        // (`Elem::Enum.val_suffix() == "i64"`, so `push`/`pop`/`peek` already route
        // to `zap_heap_push_i64` &c.) and ordering it by backing value is D-0018 /
        // D-0057. Before, `Elem::Enum` fell into the `_` arm and picked `str`, so
        // `zap_heap_new` built a pointer-comparator heap and the first `push`
        // reached `zap_str_cmp` on the enum word and aborted (I#818).
        let kind = self.iconst(
            Ty::I64,
            match elem {
                Elem::I64 | Elem::Enum(_) => 0,
                Elem::F64 => 2,
                _ => 1,
            },
        );
        let d = self.iconst(Ty::I64, dir);
        // Heapify form `heap[T](xs)`: build the heap from the list's element
        // words in O(n) via `zap_heap_from_list` (Floyd), instead of an empty
        // `zap_heap_new` + N sift-up pushes. The element word representation must
        // match (sema has already checked the list is `list[T]`; the `val_suffix`
        // assert guards the word-copy against any future kind drift).
        if let Some(items_expr) = items {
            let (lst, lty) = self.expr(items_expr)?;
            let Ty::List(list_elem) = lty else {
                return Err(err(
                    "heap[T](items) expects a list of the element type as its \
                     positional argument",
                ));
            };
            if list_elem.val_suffix() != elem.val_suffix() {
                return Err(err(
                    "heap[T](items) list element type does not match the heap element type",
                ));
            }
            let h = self.new_value(Ty::Heap(elem));
            self.emit(Inst::Call {
                dest: Some(h),
                func: "zap_heap_from_list".to_string(),
                args: vec![lst, kind, d],
            });
            return Ok((h, Ty::Heap(elem)));
        }
        let h = self.new_value(Ty::Heap(elem));
        self.emit(Inst::Call {
            dest: Some(h),
            func: "zap_heap_new".to_string(),
            args: vec![kind, d],
        });
        Ok((h, Ty::Heap(elem)))
    }

    /// Lower a heap method call (D-0057): `push(v)`, `pop()`, `peek()`. Each maps
    /// to the element-kind-suffixed runtime entry point. `pop`/`peek` return the
    /// element and abort at runtime on an empty heap (no exception path yet, like
    /// an out-of-range list index).
    fn lower_heap_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "push" => {
                if args.len() != 1 {
                    return Err(err("push() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                // `heap[float64]` admits an `f64` element (#800), so classify the
                // pushed value with `value_elem_kind` too.
                if value_elem_kind(vt, &self.ctx.nested)? != elem {
                    return Err(err("push() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_heap_push_{}", elem.val_suffix()),
                    args: vec![recv, v],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            "pop" => {
                if !args.is_empty() {
                    return Err(err("pop() takes no arguments"));
                }
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_heap_pop_{}", elem.val_suffix()),
                    args: vec![recv],
                });
                Ok((d, elem.ty()))
            }
            "peek" => {
                if !args.is_empty() {
                    return Err(err("peek() takes no arguments"));
                }
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_heap_peek_{}", elem.val_suffix()),
                    args: vec![recv],
                });
                Ok((d, elem.ty()))
            }
            // `replace(x)` (heapreplace) and `pushpop(x)` (heappushpop): both pop
            // one item and push one, differing in ORDER (#978, D-0236). replace
            // pops the current root first then stores x (so it may return an item
            // smaller than x, and requires a non-empty heap); pushpop pushes x
            // first then pops (so it returns the min of `{x} ∪ heap` and is
            // defined on the empty heap). Both return the popped element `T`.
            "replace" | "pushpop" => {
                if args.len() != 1 {
                    return Err(err(format!("{name}() takes exactly one argument")));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if value_elem_kind(vt, &self.ctx.nested)? != elem {
                    return Err(err(format!("{name}() value has the wrong element type")));
                }
                let v = self.coerce(v, vt, elem.ty());
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_heap_{name}_{}", elem.val_suffix()),
                    args: vec![recv, v],
                });
                Ok((d, elem.ty()))
            }
            other => Err(err(format!(
                "heap has no method '{other}' \
                 (supported: push, pop, peek, replace, pushpop)"
            ))),
        }
    }

    /// Lower `heap.nlargest(n, xs)` / `heap.nsmallest(n, xs)` with an optional
    /// `key=` selector (#978, D-0236). `dir` is 1 (max-heap → largest-first) for
    /// `nlargest` and 0 (min-heap → smallest-first) for `nsmallest`. `args` are
    /// already in declared order `[n, iterable, key?]` (the seam binder). Returns
    /// a `list[T]` of the selected elements in priority order.
    ///
    /// Without `key=` this is a single `zap_heap_topk(xs, n, dir, kind)` call
    /// (heapify + pop-n). With `key=`, the selector is a first-class `fn(T) -> K`
    /// value, so we evaluate it per element HERE — a lowered loop that calls the
    /// closure and builds a parallel `list[K]` of keys — and hand both lists to
    /// `zap_heap_topk_keyed`, which orders by key (stable on ties) and returns
    /// the matching ELEMENTS. The runtime therefore never calls back into Zap.
    fn lower_heap_topk(
        &mut self,
        dir: i64,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if args.len() < 2 || args.len() > 3 {
            return Err(err(format!(
                "heap.{name}(n, iterable, key=...) takes 2 arguments plus an \
                 optional key"
            )));
        }
        let (nv, nt) = self.expr(&args[0].value)?;
        let nv = self.coerce(nv, nt, Ty::I64);
        let (lst, lty) = self.expr(&args[1].value)?;
        let Ty::List(elem) = lty else {
            return Err(err(format!(
                "heap.{name}() expects a list as its iterable argument, found {lty:?}"
            )));
        };
        let d = self.iconst(Ty::I64, dir);
        let elem_tag = self.iconst(Ty::I64, deque_elem_tag(elem));

        // No key: order the elements directly, so the element type itself must be
        // one the heap can order (an `i64` word, `f64`, or `str`). A struct /
        // nested-collection / complex element has no runtime ordering — it needs a
        // `key=` that maps it to an orderable key (same rule as `heap[T]`).
        if args.len() < 3 {
            if !heap_orderable(elem) {
                return Err(err(format!(
                    "heap.{name}() needs a list of an orderable element \
                     (integer, bool, char, enum, float64, or str), or a key= \
                     selecting an orderable key"
                )));
            }
            let out = self.new_value(Ty::List(elem));
            self.emit(Inst::Call {
                dest: Some(out),
                func: "zap_heap_topk".to_string(),
                args: vec![lst, nv, d, elem_tag],
            });
            return Ok((out, Ty::List(elem)));
        }

        // key=: a `fn(T) -> K` value. Resolve its signature to learn K.
        let (keyfn, kft) = self.expr(&args[2].value)?;
        let Ty::Func(sig_idx) = kft else {
            return Err(err(format!(
                "heap.{name}() key= must be a function value fn(T) -> K, found {kft:?}"
            )));
        };
        let sig = self.ctx.fn_sigs.borrow().get(sig_idx).clone();
        let param0 = sig.params.first().cloned().ok_or_else(|| {
            err(format!(
                "heap.{name}() key= function must take one argument"
            ))
        })?;
        let kret = sig.ret;
        let key_elem = value_elem_kind(kret, &self.ctx.nested)?;
        if !heap_orderable(key_elem) {
            return Err(err(format!(
                "heap.{name}() key= must return an orderable key \
                 (integer, bool, char, enum, float64, or str), found {kret:?}"
            )));
        }
        let key_tag = self.iconst(Ty::I64, deque_elem_tag(key_elem));

        // keys = [] ; for i in 0..len(xs): keys.push(key(xs[i]))
        let keys = self.new_value(Ty::List(key_elem));
        self.emit(Inst::Call {
            dest: Some(keys),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let len = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_list_len".to_string(),
            args: vec![lst],
        });
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: len,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let x = self.new_value(elem.ty());
        self.emit(Inst::Call {
            dest: Some(x),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst, i],
        });
        let xc = self.coerce(x, elem.ty(), param0);
        let k = self.new_value(kret);
        let unwind = self.landing_pads.last().copied();
        if let Some(lp) = unwind {
            self.used_lps.insert(lp);
        }
        self.emit(Inst::CallClosure {
            dest: Some(k),
            callee: keyfn,
            args: vec![xc],
            params: sig.params.clone(),
            ret: kret,
            unwind,
        });
        self.emit(Inst::Call {
            dest: None,
            func: format!("zap_list_push_{}", key_elem.val_suffix()),
            args: vec![keys, k],
        });
        let next = self.new_value(Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: i,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));
        self.position(end_bb);

        let out = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(out),
            func: "zap_heap_topk_keyed".to_string(),
            args: vec![lst, keys, nv, d, key_tag, elem_tag],
        });
        Ok((out, Ty::List(elem)))
    }

    /// Lower `deque[T]()` / `deque[T](maxlen=n)` (#983) to `zap_deque_new(kind)` /
    /// `zap_deque_bounded(maxlen, kind)` where `kind` tags the element word
    /// representation (0 = i64, 1 = ptr/str, 2 = f64) for GC tracing and the typed
    /// entry points. A deque is a value sequence (like `list`), so an `f64`
    /// element is admitted; struct/tuple elements are a documented follow-up (as
    /// for `heap`).
    fn lower_deque_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let ety = resolve_ty(&expr_to_type(elem_expr)?, &self.ctx.tyenv())?;
        let elem = value_elem_kind(ety, &self.ctx.nested)?;
        Self::reject_nonprimitive_elem(elem, "deque")?;
        let mut maxlen: Option<Value> = None;
        for a in args {
            match a.name.as_deref() {
                Some("maxlen") => {
                    let (m, mt) = self.expr(&a.value)?;
                    maxlen = Some(self.coerce(m, mt, Ty::I64));
                }
                Some(other) => {
                    return Err(err(format!(
                        "deque[T](...) has no keyword argument '{other}' (only 'maxlen')"
                    )))
                }
                None => {
                    return Err(err(
                        "deque[T](...) takes only the keyword argument 'maxlen' (e.g. maxlen=8)",
                    ))
                }
            }
        }
        let kind = self.iconst(Ty::I64, deque_elem_tag(elem));
        let d = self.new_value(Ty::Deque(elem));
        match maxlen {
            Some(m) => self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_deque_bounded".to_string(),
                args: vec![m, kind],
            }),
            None => self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_deque_new".to_string(),
                args: vec![kind],
            }),
        }
        Ok((d, Ty::Deque(elem)))
    }

    /// Lower a `deque[T]` method (#983): `append`/`appendleft` (-> unit),
    /// `pop`/`popleft`/`get` (-> T), `clear` (-> unit). Each maps to the
    /// element-kind-suffixed runtime entry point, like `heap`.
    fn lower_deque_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let suffix = elem.val_suffix();
        match name {
            "append" | "appendleft" => {
                if args.len() != 1 {
                    return Err(err(format!("{name}() takes exactly one argument")));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if value_elem_kind(vt, &self.ctx.nested)? != elem {
                    return Err(err(format!("{name}() value has the wrong element type")));
                }
                let v = self.coerce(v, vt, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_deque_{name}_{suffix}"),
                    args: vec![recv, v],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            "pop" | "popleft" => {
                if !args.is_empty() {
                    return Err(err(format!("{name}() takes no arguments")));
                }
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_deque_{name}_{suffix}"),
                    args: vec![recv],
                });
                Ok((d, elem.ty()))
            }
            "get" => {
                if args.len() != 1 {
                    return Err(err("get() takes exactly one argument"));
                }
                let (i, it) = self.expr(&args[0].value)?;
                let i = self.coerce(i, it, Ty::I64);
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_deque_get_{suffix}"),
                    args: vec![recv, i],
                });
                Ok((d, elem.ty()))
            }
            "clear" => {
                if !args.is_empty() {
                    return Err(err("clear() takes no arguments"));
                }
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_deque_clear".to_string(),
                    args: vec![recv],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            other => Err(err(format!(
                "deque has no method '{other}' (supported: append, appendleft, pop, \
                 popleft, get, clear)"
            ))),
        }
    }

    /// Lower `oset[T]()` / `oset[T](iterable)` (#983) to `zap_oset_new(kind)` plus,
    /// for the iterable form, an `add` per element. v1 supports `i64` and `str`
    /// elements (`kind` 0/1); struct/other-pointer elements are a documented
    /// follow-up (as for `heap`/`set`-fn keys).
    fn lower_oset_new(
        &mut self,
        elem_expr: &Expr,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let ety = resolve_ty(&expr_to_type(elem_expr)?, &self.ctx.tyenv())?;
        let elem = elem_kind(ety, &self.ctx.nested)?;
        Self::reject_nonprimitive_elem(elem, "oset")?;
        if matches!(elem, Elem::F64) {
            return Err(err(
                "oset does not support a float element yet (use int64 or str); \
                 this is a documented follow-up",
            ));
        }
        let s = self.new_value(Ty::OSet(elem));
        let kind = self.iconst(Ty::I64, oset_elem_tag(elem));
        self.emit(Inst::Call {
            dest: Some(s),
            func: "zap_oset_new".to_string(),
            args: vec![kind],
        });
        match args.len() {
            0 => {}
            1 if args[0].name.is_none() => {
                let suffix = elem.val_suffix();
                self.iterate(&args[0].value, |st, v, t| {
                    if value_elem_kind(t, &st.ctx.nested)? != elem {
                        return Err(err("oset elements must all have the same representation"));
                    }
                    let v = st.coerce(v, t, elem.ty());
                    st.emit(Inst::Call {
                        dest: None,
                        func: format!("zap_oset_add_{suffix}"),
                        args: vec![s, v],
                    });
                    Ok(())
                })?;
            }
            _ => {
                return Err(err(
                    "oset[T](...) takes at most one positional iterable argument",
                ))
            }
        }
        Ok((s, Ty::OSet(elem)))
    }

    /// Lower an `oset[T]` method (#983): `add` (-> unit), `contains` (-> bool),
    /// `get` (-> T). Membership `x in s` lowers separately.
    fn lower_oset_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let suffix = elem.val_suffix();
        match name {
            "add" => {
                if args.len() != 1 {
                    return Err(err("add() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if value_elem_kind(vt, &self.ctx.nested)? != elem {
                    return Err(err("add() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_oset_add_{suffix}"),
                    args: vec![recv, v],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            "contains" => {
                if args.len() != 1 {
                    return Err(err("contains() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                if value_elem_kind(vt, &self.ctx.nested)? != elem {
                    return Err(err("contains() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                let r = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(r),
                    func: format!("zap_oset_contains_{suffix}"),
                    args: vec![recv, v],
                });
                // The runtime returns 1/0 as i64; normalize to a bool.
                let zero = self.iconst(Ty::I64, 0);
                let b = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: b,
                    pred: CmpPred::Ne,
                    lhs: r,
                    rhs: zero,
                });
                Ok((b, Ty::I1))
            }
            "get" => {
                if args.len() != 1 {
                    return Err(err("get() takes exactly one argument"));
                }
                let (i, it) = self.expr(&args[0].value)?;
                let i = self.coerce(i, it, Ty::I64);
                let d = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_oset_get_{suffix}"),
                    args: vec![recv, i],
                });
                Ok((d, elem.ty()))
            }
            other => Err(err(format!(
                "oset has no method '{other}' (supported: add, contains, get)"
            ))),
        }
    }

    /// Reject a non-primitive (struct/tuple/nested/complex) element for the v1
    /// `deque`/`oset` collections (#983): like `heap`, only the primitive element
    /// kinds (integers/bool/char, `str`, and — for a value sequence — `f64`) are
    /// supported for now; aggregate elements are a documented follow-up.
    fn reject_nonprimitive_elem(elem: Elem, what: &str) -> Result<(), LowerError> {
        match elem {
            Elem::Struct(_) | Elem::Nested(_) | Elem::Complex => Err(err(format!(
                "a {what} of a struct/tuple/complex element is not supported yet \
                 (only integers, str, and — for deque — float64); this is a follow-up"
            ))),
            _ => Ok(()),
        }
    }

    /// Lower a tuple literal (D-0049): a by-value anonymous-struct aggregate. The
    /// element-type sequence interns to a struct id, then `StructNew` + one
    /// `FieldSet` per element build the value — exactly the struct path, so
    /// codegen needs no tuple-specific handling.
    fn lower_tuple_lit(&mut self, elems: &[Expr]) -> Result<(Value, Ty), LowerError> {
        self.lower_tuple_lit_expected(elems, None)
    }

    /// Lower a tuple literal in a position whose expected type is the tuple
    /// `expected` field types (#1195): each element is lowered against its
    /// declared field type via `expr_coerced`, so a bare `None` element reaches
    /// the shared type-contextual `None` mechanism (`lower_none_expr`) and gets
    /// its `Optional[T]` null — exactly as a bare `None` struct-constructor field
    /// (#1075) or `list[Optional[T]]` element (#1080) does. With no expected
    /// type (a tuple display in an un-annotated context, e.g. `auto t = (a, b)`)
    /// each element lowers with its own natural type, unchanged.
    ///
    /// Per-element expected types are threaded only when the expected arity
    /// matches the element count and no `*t` splice is present — a splice
    /// expands to a statically-unknown-here number of fields, so a 1:1 source
    /// element↦declared field mapping would not hold. In that case (and for any
    /// arity mismatch, which sema reports separately) every element falls back
    /// to its natural type, preserving the prior behaviour.
    fn lower_tuple_lit_expected(
        &mut self,
        elems: &[Expr],
        expected: Option<&[Ty]>,
    ) -> Result<(Value, Ty), LowerError> {
        let mut vals = Vec::with_capacity(elems.len());
        let mut tys = Vec::with_capacity(elems.len());
        // Thread per-element expected types only for a 1:1 splice-free literal.
        let per_elem = match expected {
            Some(ftys)
                if ftys.len() == elems.len()
                    && !elems.iter().any(|e| matches!(e, Expr::Starred { .. })) =>
            {
                Some(ftys)
            }
            _ => None,
        };
        for (idx, e) in elems.iter().enumerate() {
            if let Some(ftys) = per_elem {
                // The declared field type drives the element — a bare `None`
                // lowers as that field's `Optional[T]` null, and a present
                // element is coerced to the declared field type (a no-op when it
                // already matches, e.g. an `Optional.Some(...)`).
                let ft = ftys[idx];
                let v = self.expr_coerced(e, ft, zapc_diag::ErrorCode::ZAP4003)?;
                vals.push(v);
                tys.push(ft);
                continue;
            }
            match e {
                // `*t` splices a statically-sized tuple's fields in place (#598);
                // only a tuple can be spliced into a tuple, since the result's
                // arity must stay statically known.
                Expr::Starred { value, .. } => {
                    let (tv, tt) = self.expr(value)?;
                    let Ty::Struct(sid) = tt else {
                        return Err(err("`*` in a tuple display requires a tuple"));
                    };
                    let fields = self
                        .ctx
                        .tuples
                        .borrow()
                        .fields(sid)
                        .ok_or_else(|| err("`*` in a tuple display requires a tuple"))?;
                    for (idx, fty) in fields.into_iter().enumerate() {
                        let fv = self.new_value(fty);
                        self.emit(Inst::FieldGet {
                            dest: fv,
                            base: tv,
                            sid,
                            idx: idx as u32,
                        });
                        vals.push(fv);
                        tys.push(fty);
                    }
                }
                _ => {
                    let (v, t) = self.expr(e)?;
                    vals.push(v);
                    tys.push(t);
                }
            }
        }
        let sid = self.ctx.tuples.borrow_mut().intern(tys);
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (idx, v) in vals.into_iter().enumerate() {
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: idx as u32,
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// Element-wise tuple comparison (D-0049). `op` is `==` or `!=`; the result
    /// is the conjunction of the per-element `==` results (then negated for
    /// `!=`). Elements are compared by recursing through [`binop`], so nested
    /// tuples and scalar elements work; a reference-typed element (e.g. `str`)
    /// currently surfaces `binop`'s "unsupported reference type" error.
    fn tuple_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        ls: u32,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        let Ty::Struct(rs) = rt else {
            return Err(err("a tuple can only be compared with another tuple"));
        };
        if ls != rs {
            return Err(err("cannot compare tuples of different types"));
        }
        let field_tys = self
            .ctx
            .tuples
            .borrow()
            .fields(ls)
            .ok_or_else(|| err("not a tuple"))?;
        // Start from `true` and AND each element's equality.
        let mut acc = self.new_value(Ty::I1);
        self.emit(Inst::BConst {
            dest: acc,
            val: true,
        });
        for (i, fty) in field_tys.iter().enumerate() {
            let lf = self.field_get(l, ls, i as u32, *fty);
            let rf = self.field_get(r, rs, i as u32, *fty);
            let (eq, _) = self.binop(AstBin::Eq, lf, *fty, rf, *fty)?;
            let next = self.new_value(Ty::I1);
            self.emit(Inst::Bin {
                dest: next,
                op: BinOp::And,
                lhs: acc,
                rhs: eq,
            });
            acc = next;
        }
        if matches!(op, AstBin::Ne) {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: acc,
            });
            return Ok((d, Ty::I1));
        }
        Ok((acc, Ty::I1))
    }

    /// Structural list equality (#1239): `a == b` / `a != b` on two lists is an
    /// element-wise, order- and length-sensitive comparison — Python's value
    /// semantics for sequences — not a pointer-identity test (`is` / `is not`
    /// keep identity, and are intercepted at the top of `binop`). Without this
    /// the operands fall through to the generic numeric/reference path, which
    /// `ptrtoint`-compares the two list headers and so reports two separately
    /// allocated but equal lists as unequal.
    ///
    /// Lengths are compared first; if they differ the lists are unequal and no
    /// elements are read. Otherwise each pair of elements is compared through
    /// `binop(Eq, ..)`, so elements reuse the exact same equality as a scalar
    /// `==`: numeric joins, `str`/`bytes` by contents, a struct's `__eq__`, and
    /// a nested `list`/`dict`/... element recurses into this same path (its full
    /// ZIR type is recovered via [`FnLower::elem_ty`], #625). Lowered entirely
    /// in-IR as a short-circuiting loop — no runtime entry point and no new ZIR
    /// `Inst` variant, so no new codegen surface. `!=` negates the result.
    fn list_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        lelem: Elem,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        let Ty::List(relem) = rt else {
            return Err(err("a list compares for equality only with another list"));
        };
        // `result` holds the running verdict: it starts as "lengths agree" and is
        // cleared to False on the first element mismatch. `idx` is the element
        // cursor. Both are allocated in the current (pre-branch) block, like the
        // list-membership scan, so they do not rely on alloca hoisting out of a
        // conditionally entered block.
        let result = self.alloca(Ty::I1);
        let idx = self.alloca(Ty::I64);
        let llen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(llen),
            func: "zap_list_len".to_string(),
            args: vec![l],
        });
        let rlen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(rlen),
            func: "zap_list_len".to_string(),
            args: vec![r],
        });
        let len_eq = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: len_eq,
            pred: CmpPred::Eq,
            lhs: llen,
            rhs: rlen,
        });
        self.store(result, len_eq);

        let loop_bb = self.new_block();
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let neq_bb = self.new_block();
        let end_bb = self.new_block();

        // Only walk elements when the lengths match; otherwise `result` is already
        // False and we jump straight to the end.
        self.set_term(Term::BrIf {
            cond: len_eq,
            then_bb: loop_bb,
            else_bb: end_bb,
        });

        self.position(loop_bb);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        self.set_term(Term::Br(cond_bb));

        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let in_range = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: in_range,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: llen,
        });
        self.set_term(Term::BrIf {
            cond: in_range,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        // Element read-back dispatches on each side's own element kind and full
        // ZIR type (a nested collection reads back as its real `list`/`dict`/...
        // type, not a bare `str` pointer). Indices are always in range, so a
        // plain `Call` suffices (no catchable out-of-range raise).
        let lety = self.elem_ty(lelem);
        let le = self.new_value(lety);
        self.emit(Inst::Call {
            dest: Some(le),
            func: format!("zap_list_get_{}", lelem.val_suffix()),
            args: vec![l, i],
        });
        let rety = self.elem_ty(relem);
        let re = self.new_value(rety);
        self.emit(Inst::Call {
            dest: Some(re),
            func: format!("zap_list_get_{}", relem.val_suffix()),
            args: vec![r, i],
        });
        let (elem_eq, _) = self.binop(AstBin::Eq, le, lety, re, rety)?;
        self.set_term(Term::BrIf {
            cond: elem_eq,
            then_bb: incr_bb,
            else_bb: neq_bb,
        });

        // First mismatching element: the lists are unequal, stop scanning.
        self.position(neq_bb);
        let f = self.new_value(Ty::I1);
        self.emit(Inst::BConst {
            dest: f,
            val: false,
        });
        self.store(result, f);
        self.set_term(Term::Br(end_bb));

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        let eq = self.load(result, Ty::I1);
        if matches!(op, AstBin::Ne) {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: eq,
            });
            return Ok((d, Ty::I1));
        }
        Ok((eq, Ty::I1))
    }

    /// Lexicographic list ordering (#1334 sibling): `a < b` / `<=` / `>` / `>=`
    /// compare two lists element by element — the first differing pair decides
    /// (via element `<`), and if one list is a prefix of the other the shorter is
    /// smaller — matching Python's sequence ordering. Without this the operands
    /// fall through to the generic numeric path, which `ptrtoint`-compares the two
    /// list headers (a meaningless allocation-address compare). `is` / `is not`
    /// keep identity (handled at the top of `binop`).
    ///
    /// Computed as a 3-way sign in a slot (`-1`/`0`/`1`), like `zap_str_cmp` /
    /// `zap_bigint_cmp`, then mapped to the operator by comparing the sign against
    /// 0. Element ordering routes through `binop(Lt/Gt, ..)`, so each element type
    /// reuses scalar `<`/`>` (numeric joins, `str`/`bytes` by contents, a nested
    /// element's own ordering) and a non-orderable element type raises a clear
    /// error there. The element's full ZIR type is recovered via
    /// [`FnLower::elem_ty`] (#625) so a nested element reads back as its real type.
    fn list_order_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        lelem: Elem,
        r: Value,
        relem: Elem,
    ) -> Result<(Value, Ty), LowerError> {
        let cmp = self.alloca(Ty::I64);
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(cmp, zero);
        self.store(idx, zero);
        let llen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(llen),
            func: "zap_list_len".to_string(),
            args: vec![l],
        });
        let rlen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(rlen),
            func: "zap_list_len".to_string(),
            args: vec![r],
        });

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let set_neg_bb = self.new_block();
        let check_gt_bb = self.new_block();
        let set_pos_bb = self.new_block();
        let incr_bb = self.new_block();
        let after_bb = self.new_block();
        let lencmp_bb = self.new_block();
        let final_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));

        // Loop while both indices are in range AND no decision has been made yet.
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let in_l = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: in_l,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: llen,
        });
        let in_r = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: in_r,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: rlen,
        });
        let in_range = self.new_value(Ty::I1);
        self.emit(Inst::Bin {
            dest: in_range,
            op: BinOp::And,
            lhs: in_l,
            rhs: in_r,
        });
        let cur_cmp = self.load(cmp, Ty::I64);
        let still_zero = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: still_zero,
            pred: CmpPred::Eq,
            lhs: cur_cmp,
            rhs: zero,
        });
        let go = self.new_value(Ty::I1);
        self.emit(Inst::Bin {
            dest: go,
            op: BinOp::And,
            lhs: in_range,
            rhs: still_zero,
        });
        self.set_term(Term::BrIf {
            cond: go,
            then_bb: body_bb,
            else_bb: after_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let lety = self.elem_ty(lelem);
        let le = self.new_value(lety);
        self.emit(Inst::Call {
            dest: Some(le),
            func: format!("zap_list_get_{}", lelem.val_suffix()),
            args: vec![l, i],
        });
        let rety = self.elem_ty(relem);
        let re = self.new_value(rety);
        self.emit(Inst::Call {
            dest: Some(re),
            func: format!("zap_list_get_{}", relem.val_suffix()),
            args: vec![r, i],
        });
        let (lt_bit, _) = self.binop(AstBin::Lt, le, lety, re, rety)?;
        self.set_term(Term::BrIf {
            cond: lt_bit,
            then_bb: set_neg_bb,
            else_bb: check_gt_bb,
        });

        self.position(set_neg_bb);
        let neg = self.iconst(Ty::I64, -1);
        self.store(cmp, neg);
        self.set_term(Term::Br(incr_bb));

        self.position(check_gt_bb);
        let i2 = self.load(idx, Ty::I64);
        let le2 = self.new_value(lety);
        self.emit(Inst::Call {
            dest: Some(le2),
            func: format!("zap_list_get_{}", lelem.val_suffix()),
            args: vec![l, i2],
        });
        let re2 = self.new_value(rety);
        self.emit(Inst::Call {
            dest: Some(re2),
            func: format!("zap_list_get_{}", relem.val_suffix()),
            args: vec![r, i2],
        });
        let (gt_bit, _) = self.binop(AstBin::Gt, le2, lety, re2, rety)?;
        self.set_term(Term::BrIf {
            cond: gt_bit,
            then_bb: set_pos_bb,
            else_bb: incr_bb,
        });

        self.position(set_pos_bb);
        let pos = self.iconst(Ty::I64, 1);
        self.store(cmp, pos);
        self.set_term(Term::Br(incr_bb));

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        // All compared elements were equal: the shorter list is the smaller.
        self.position(after_bb);
        let cur_cmp2 = self.load(cmp, Ty::I64);
        let is_zero = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: is_zero,
            pred: CmpPred::Eq,
            lhs: cur_cmp2,
            rhs: zero,
        });
        self.set_term(Term::BrIf {
            cond: is_zero,
            then_bb: lencmp_bb,
            else_bb: final_bb,
        });

        self.position(lencmp_bb);
        let len_lt = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: len_lt,
            pred: CmpPred::Lt,
            lhs: llen,
            rhs: rlen,
        });
        let len_gt = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: len_gt,
            pred: CmpPred::Gt,
            lhs: llen,
            rhs: rlen,
        });
        // cmp = (llen>rlen) - (llen<rlen): -1 / 0 / 1, built from the two bits.
        let neg_l = self.new_value(Ty::I64);
        self.emit(Inst::Cast {
            dest: neg_l,
            kind: CastKind::ZExt,
            val: len_lt,
        });
        let pos_g = self.new_value(Ty::I64);
        self.emit(Inst::Cast {
            dest: pos_g,
            kind: CastKind::ZExt,
            val: len_gt,
        });
        let len_cmp = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: len_cmp,
            op: BinOp::Sub,
            lhs: pos_g,
            rhs: neg_l,
        });
        self.store(cmp, len_cmp);
        self.set_term(Term::Br(final_bb));

        self.position(final_bb);
        let c = self.load(cmp, Ty::I64);
        let pred = match op {
            AstBin::Lt => CmpPred::Lt,
            AstBin::Le => CmpPred::Le,
            AstBin::Gt => CmpPred::Gt,
            AstBin::Ge => CmpPred::Ge,
            _ => unreachable!("list_order_cmp only handles < <= > >="),
        };
        let d = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: d,
            pred,
            lhs: c,
            rhs: zero,
        });
        Ok((d, Ty::I1))
    }

    /// Structural dict equality (#1328): `a == b` / `a != b` on two dicts is an
    /// **order-independent** comparison by entries — Python's value semantics for
    /// mappings — not a pointer-identity test (`is` / `is not` keep identity).
    /// Without this the operands fall through to the generic numeric/reference
    /// path, which `ptrtoint`-compares the two dict headers and so reports two
    /// separately allocated but equal dicts (e.g. the same keys inserted in a
    /// different order) as unequal.
    ///
    /// Equal lengths are checked first; if they differ the dicts are unequal and
    /// no entries are read. Otherwise the left dict's keys are materialized
    /// (`zap_dict_keys_*`) and each is probed in the right dict
    /// (`zap_dict_contains_*`): a missing key is a mismatch, and a present key's
    /// two values are compared through `binop(Eq, ..)`, so values reuse the exact
    /// same equality as a scalar `==` — numeric joins, `str`/`bytes` by contents,
    /// a struct value's `__eq__`, and a nested `list`/`dict`/... value recurses
    /// (its full ZIR type recovered via [`FnLower::elem_ty`], #625). Equal length
    /// plus "every left key maps to an equal right value" is sufficient for
    /// equality (pigeonhole). Lowered entirely in-IR as a short-circuiting loop —
    /// no runtime entry point and no new ZIR `Inst` variant. `!=` negates.
    ///
    /// The key kinds must match (the hashing/identity of a key is per key kind);
    /// the two value kinds may differ in representation (e.g. `int64` vs
    /// `float64`), since `binop(Eq, ..)` applies the numeric join, matching
    /// Python's `{"x": 1} == {"x": 1.0}`.
    #[allow(clippy::too_many_arguments)]
    fn dict_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        lkk: Elem,
        lvk: Elem,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        let Ty::Dict(rkk, rvk) = rt else {
            return Err(err("a dict compares for equality only with another dict"));
        };
        if lkk != rkk {
            return Err(err(
                "cannot compare dicts with different key types for equality",
            ));
        }
        let result = self.alloca(Ty::I1);
        let idx = self.alloca(Ty::I64);
        let llen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(llen),
            func: "zap_dict_len".to_string(),
            args: vec![l],
        });
        let rlen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(rlen),
            func: "zap_dict_len".to_string(),
            args: vec![r],
        });
        let len_eq = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: len_eq,
            pred: CmpPred::Eq,
            lhs: llen,
            rhs: rlen,
        });
        self.store(result, len_eq);

        let loop_bb = self.new_block();
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let cmp_bb = self.new_block();
        let incr_bb = self.new_block();
        let neq_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::BrIf {
            cond: len_eq,
            then_bb: loop_bb,
            else_bb: end_bb,
        });

        // Materialize the left dict's keys as a `list[K]` once, then walk it. The
        // keys list has exactly `llen` entries, so that length also bounds the loop.
        self.position(loop_bb);
        let keys = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(keys),
            func: format!("zap_dict_keys_{}", lkk.val_suffix()),
            args: vec![l],
        });
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        self.set_term(Term::Br(cond_bb));

        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let in_range = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: in_range,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: llen,
        });
        self.set_term(Term::BrIf {
            cond: in_range,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let kty = self.elem_ty(lkk);
        let key = self.new_value(kty);
        self.emit(Inst::Call {
            dest: Some(key),
            func: format!("zap_list_get_{}", lkk.val_suffix()),
            args: vec![keys, i],
        });
        // Probe the right dict for this key. A struct key carries its
        // `__hash__`/`__eq__` function pointers (`key_fn_args`); scalar/string keys
        // pass none. The key comes from an existing dict, so a float key is never
        // NaN — `key_rt_call` keeps the (here-inert) NaN guard for uniformity.
        let kfns = self.key_fn_args(lkk)?;
        let present_word = self.new_value(Ty::I64);
        let mut cargs = vec![r, key];
        cargs.extend_from_slice(&kfns);
        self.key_rt_call(
            Some(present_word),
            format!("zap_dict_contains_{}", lkk.key_suffix()),
            cargs,
            lkk,
        );
        let present = self.new_value(Ty::I1);
        let zc = self.iconst(Ty::I64, 0);
        self.emit(Inst::ICmp {
            dest: present,
            pred: CmpPred::Ne,
            lhs: present_word,
            rhs: zc,
        });
        self.set_term(Term::BrIf {
            cond: present,
            then_bb: cmp_bb,
            else_bb: neq_bb,
        });

        // Key present on both sides: compare the two values through `binop(Eq, ..)`
        // so a nested/struct value reuses scalar equality (and recurses).
        self.position(cmp_bb);
        let lvty = self.elem_ty(lvk);
        let lval = self.new_value(lvty);
        let mut lget = vec![l, key];
        lget.extend_from_slice(&kfns);
        self.emit(Inst::Call {
            dest: Some(lval),
            func: format!("zap_dict_get_{}_{}", lkk.key_suffix(), lvk.val_suffix()),
            args: lget,
        });
        let rvty = self.elem_ty(rvk);
        let rval = self.new_value(rvty);
        let mut rget = vec![r, key];
        rget.extend_from_slice(&kfns);
        self.emit(Inst::Call {
            dest: Some(rval),
            func: format!("zap_dict_get_{}_{}", rkk.key_suffix(), rvk.val_suffix()),
            args: rget,
        });
        let (veq, _) = self.binop(AstBin::Eq, lval, lvty, rval, rvty)?;
        self.set_term(Term::BrIf {
            cond: veq,
            then_bb: incr_bb,
            else_bb: neq_bb,
        });

        self.position(neq_bb);
        let f = self.new_value(Ty::I1);
        self.emit(Inst::BConst {
            dest: f,
            val: false,
        });
        self.store(result, f);
        self.set_term(Term::Br(end_bb));

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        let eq = self.load(result, Ty::I1);
        if matches!(op, AstBin::Ne) {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: eq,
            });
            return Ok((d, Ty::I1));
        }
        Ok((eq, Ty::I1))
    }

    /// Structural set equality (#1328): `a == b` / `a != b` on two sets is a
    /// **membership-based, order-independent** comparison — Python's set value
    /// semantics — not a pointer-identity test. Equal lengths plus "every member
    /// of the left set is a member of the right set" is sufficient for equality
    /// (pigeonhole). The left set's members are materialized (`zap_set_items`) and
    /// each is probed in the right set (`zap_set_contains_*`); a missing member is
    /// a mismatch. A struct member carries its `__hash__`/`__eq__` pointers.
    /// Lowered in-IR as a short-circuiting loop; `!=` negates.
    fn set_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        lelem: Elem,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        let Ty::Set(relem) = rt else {
            return Err(err("a set compares for equality only with another set"));
        };
        if lelem != relem {
            return Err(err(
                "cannot compare sets with different element types for equality",
            ));
        }
        let result = self.alloca(Ty::I1);
        let idx = self.alloca(Ty::I64);
        let llen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(llen),
            func: "zap_set_len".to_string(),
            args: vec![l],
        });
        let rlen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(rlen),
            func: "zap_set_len".to_string(),
            args: vec![r],
        });
        let len_eq = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: len_eq,
            pred: CmpPred::Eq,
            lhs: llen,
            rhs: rlen,
        });
        self.store(result, len_eq);

        let loop_bb = self.new_block();
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let neq_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::BrIf {
            cond: len_eq,
            then_bb: loop_bb,
            else_bb: end_bb,
        });

        self.position(loop_bb);
        let items = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_set_items".to_string(),
            args: vec![l],
        });
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        self.set_term(Term::Br(cond_bb));

        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let in_range = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: in_range,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: llen,
        });
        self.set_term(Term::BrIf {
            cond: in_range,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let ety = self.elem_ty(lelem);
        let e = self.new_value(ety);
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", lelem.val_suffix()),
            args: vec![items, i],
        });
        let kfns = self.key_fn_args(lelem)?;
        let present_word = self.new_value(Ty::I64);
        let mut cargs = vec![r, e];
        cargs.extend_from_slice(&kfns);
        self.key_rt_call(
            Some(present_word),
            format!("zap_set_contains_{}", lelem.key_suffix()),
            cargs,
            lelem,
        );
        let present = self.new_value(Ty::I1);
        let zc = self.iconst(Ty::I64, 0);
        self.emit(Inst::ICmp {
            dest: present,
            pred: CmpPred::Ne,
            lhs: present_word,
            rhs: zc,
        });
        self.set_term(Term::BrIf {
            cond: present,
            then_bb: incr_bb,
            else_bb: neq_bb,
        });

        self.position(neq_bb);
        let f = self.new_value(Ty::I1);
        self.emit(Inst::BConst {
            dest: f,
            val: false,
        });
        self.store(result, f);
        self.set_term(Term::Br(end_bb));

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        let eq = self.load(result, Ty::I1);
        if matches!(op, AstBin::Ne) {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: eq,
            });
            return Ok((d, Ty::I1));
        }
        Ok((eq, Ty::I1))
    }

    /// Structural `oset` / `deque` equality (#1328): both are **ordered**
    /// sequences, so `==` / `!=` is an order-sensitive, length-sensitive,
    /// element-wise comparison — the same shape as structural list equality
    /// (#1239) — not a pointer-identity test. `oset` is order-sensitive by
    /// decision (its insertion order is observable and is the reason it exists,
    /// distinct from `set`); `deque` mirrors Python's `collections.deque`, whose
    /// `==` is order-sensitive. Each pair of elements is compared through
    /// `binop(Eq, ..)` (so nested/struct elements recurse). `len`/`get` are the
    /// per-container typed entry points named by `len_fn`/`get_fn`. `!=` negates.
    #[allow(clippy::too_many_arguments)]
    fn ordered_seq_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        lelem: Elem,
        r: Value,
        relem: Elem,
        len_fn: &str,
        get_fn: &str,
    ) -> Result<(Value, Ty), LowerError> {
        let result = self.alloca(Ty::I1);
        let idx = self.alloca(Ty::I64);
        let llen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(llen),
            func: len_fn.to_string(),
            args: vec![l],
        });
        let rlen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(rlen),
            func: len_fn.to_string(),
            args: vec![r],
        });
        let len_eq = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: len_eq,
            pred: CmpPred::Eq,
            lhs: llen,
            rhs: rlen,
        });
        self.store(result, len_eq);

        let loop_bb = self.new_block();
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let neq_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::BrIf {
            cond: len_eq,
            then_bb: loop_bb,
            else_bb: end_bb,
        });

        self.position(loop_bb);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        self.set_term(Term::Br(cond_bb));

        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let in_range = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: in_range,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: llen,
        });
        self.set_term(Term::BrIf {
            cond: in_range,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let lety = self.elem_ty(lelem);
        let le = self.new_value(lety);
        self.emit(Inst::Call {
            dest: Some(le),
            func: format!("{get_fn}_{}", lelem.val_suffix()),
            args: vec![l, i],
        });
        let rety = self.elem_ty(relem);
        let re = self.new_value(rety);
        self.emit(Inst::Call {
            dest: Some(re),
            func: format!("{get_fn}_{}", relem.val_suffix()),
            args: vec![r, i],
        });
        let (elem_eq, _) = self.binop(AstBin::Eq, le, lety, re, rety)?;
        self.set_term(Term::BrIf {
            cond: elem_eq,
            then_bb: incr_bb,
            else_bb: neq_bb,
        });

        self.position(neq_bb);
        let f = self.new_value(Ty::I1);
        self.emit(Inst::BConst {
            dest: f,
            val: false,
        });
        self.store(result, f);
        self.set_term(Term::Br(end_bb));

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        let eq = self.load(result, Ty::I1);
        if matches!(op, AstBin::Ne) {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: eq,
            });
            return Ok((d, Ty::I1));
        }
        Ok((eq, Ty::I1))
    }

    /// Whether every member of set `x` is also a member of set `y` (`x ⊆ y`),
    /// as an `i1`. Walks `x`'s members (`zap_set_items`) and probes each in `y`
    /// (`zap_set_contains_*`), short-circuiting to False on the first member of
    /// `x` that `y` lacks. The empty set is a subset of everything, so the result
    /// starts True. Used by [`FnLower::set_order_cmp`] (#1334). Leaves the builder
    /// positioned at the join block, with the result loaded from a slot.
    fn set_subset(&mut self, x: Value, xelem: Elem, y: Value) -> Result<Value, LowerError> {
        let result = self.alloca(Ty::I1);
        let t = self.new_value(Ty::I1);
        self.emit(Inst::BConst { dest: t, val: true });
        self.store(result, t);
        let idx = self.alloca(Ty::I64);
        let xlen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(xlen),
            func: "zap_set_len".to_string(),
            args: vec![x],
        });
        let items = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_set_items".to_string(),
            args: vec![x],
        });
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let miss_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));

        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let in_range = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: in_range,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: xlen,
        });
        self.set_term(Term::BrIf {
            cond: in_range,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let ety = self.elem_ty(xelem);
        let e = self.new_value(ety);
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", xelem.val_suffix()),
            args: vec![items, i],
        });
        let kfns = self.key_fn_args(xelem)?;
        let present_word = self.new_value(Ty::I64);
        let mut cargs = vec![y, e];
        cargs.extend_from_slice(&kfns);
        self.key_rt_call(
            Some(present_word),
            format!("zap_set_contains_{}", xelem.key_suffix()),
            cargs,
            xelem,
        );
        let present = self.new_value(Ty::I1);
        let zc = self.iconst(Ty::I64, 0);
        self.emit(Inst::ICmp {
            dest: present,
            pred: CmpPred::Ne,
            lhs: present_word,
            rhs: zc,
        });
        self.set_term(Term::BrIf {
            cond: present,
            then_bb: incr_bb,
            else_bb: miss_bb,
        });

        self.position(miss_bb);
        let f = self.new_value(Ty::I1);
        self.emit(Inst::BConst {
            dest: f,
            val: false,
        });
        self.store(result, f);
        self.set_term(Term::Br(end_bb));

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        Ok(self.load(result, Ty::I1))
    }

    /// Ordering comparison on two sets (#1334): Python's subset/superset partial
    /// order, not a pointer compare.
    ///
    /// - `a <= b` — `a` is a subset of `b`.
    /// - `a <  b` — `a` is a *proper* subset (`a ⊆ b` and `len(a) < len(b)`).
    /// - `a >= b` — `b` is a subset of `a`.
    /// - `a >  b` — `a` is a *proper* superset (`b ⊆ a` and `len(a) > len(b)`).
    ///
    /// This is a partial order: two sets can be incomparable (both `<` and `>`
    /// False). Membership reuses [`FnLower::set_subset`]; strictness ANDs in a
    /// length inequality. Unlike equality, the operation is **not** symmetric, so
    /// the operands are never swapped (the caller passes the left operand as `l`).
    fn set_order_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        lelem: Elem,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        let Ty::Set(relem) = rt else {
            return Err(err("a set's `< <= > >=` compares only with another set"));
        };
        if lelem != relem {
            return Err(err(
                "cannot order-compare sets with different element types",
            ));
        }
        let llen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(llen),
            func: "zap_set_len".to_string(),
            args: vec![l],
        });
        let rlen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(rlen),
            func: "zap_set_len".to_string(),
            args: vec![r],
        });
        // Subset in the direction the operator needs; `<`/`<=` ask `l ⊆ r`,
        // `>`/`>=` ask `r ⊆ l`.
        let subset = match op {
            AstBin::Le | AstBin::Lt => self.set_subset(l, lelem, r)?,
            AstBin::Ge | AstBin::Gt => self.set_subset(r, relem, l)?,
            _ => unreachable!("set_order_cmp only handles < <= > >="),
        };
        // Non-strict operators are the subset test alone; strict operators AND in
        // the proper length inequality (a proper subset/superset differs in size).
        let result = match op {
            AstBin::Le | AstBin::Ge => subset,
            AstBin::Lt | AstBin::Gt => {
                let len_lt = self.new_value(Ty::I1);
                let (a, b) = if matches!(op, AstBin::Lt) {
                    (llen, rlen)
                } else {
                    (rlen, llen)
                };
                self.emit(Inst::ICmp {
                    dest: len_lt,
                    pred: CmpPred::Lt,
                    lhs: a,
                    rhs: b,
                });
                let d = self.new_value(Ty::I1);
                self.emit(Inst::Bin {
                    dest: d,
                    op: BinOp::And,
                    lhs: subset,
                    rhs: len_lt,
                });
                d
            }
            _ => unreachable!(),
        };
        Ok((result, Ty::I1))
    }

    /// Ordering comparison on two deques (#1334): Python `collections.deque` is
    /// orderable **lexicographically** — compare elements left to right; the first
    /// differing pair decides (via element `<`), and if one deque is a prefix of
    /// the other the shorter one is smaller. Computed as a 3-way sign in a slot
    /// (`-1`/`0`/`1`), like `zap_str_cmp` / `zap_bigint_cmp`, then mapped to the
    /// operator by comparing the sign against 0. Element ordering routes through
    /// `binop(Lt/Gt, ..)`, so non-orderable elements raise a clear error there.
    fn deque_order_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        lelem: Elem,
        r: Value,
        relem: Elem,
    ) -> Result<(Value, Ty), LowerError> {
        let cmp = self.alloca(Ty::I64);
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(cmp, zero);
        self.store(idx, zero);
        let llen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(llen),
            func: "zap_deque_len".to_string(),
            args: vec![l],
        });
        let rlen = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(rlen),
            func: "zap_deque_len".to_string(),
            args: vec![r],
        });

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let set_neg_bb = self.new_block();
        let check_gt_bb = self.new_block();
        let set_pos_bb = self.new_block();
        let incr_bb = self.new_block();
        let after_bb = self.new_block();
        let lencmp_bb = self.new_block();
        let final_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));

        // Loop while both indices are in range AND no decision has been made yet.
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let in_l = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: in_l,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: llen,
        });
        let in_r = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: in_r,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: rlen,
        });
        let in_range = self.new_value(Ty::I1);
        self.emit(Inst::Bin {
            dest: in_range,
            op: BinOp::And,
            lhs: in_l,
            rhs: in_r,
        });
        let cur_cmp = self.load(cmp, Ty::I64);
        let still_zero = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: still_zero,
            pred: CmpPred::Eq,
            lhs: cur_cmp,
            rhs: zero,
        });
        let go = self.new_value(Ty::I1);
        self.emit(Inst::Bin {
            dest: go,
            op: BinOp::And,
            lhs: in_range,
            rhs: still_zero,
        });
        self.set_term(Term::BrIf {
            cond: go,
            then_bb: body_bb,
            else_bb: after_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let lety = self.elem_ty(lelem);
        let le = self.new_value(lety);
        self.emit(Inst::Call {
            dest: Some(le),
            func: format!("zap_deque_get_{}", lelem.val_suffix()),
            args: vec![l, i],
        });
        let rety = self.elem_ty(relem);
        let re = self.new_value(rety);
        self.emit(Inst::Call {
            dest: Some(re),
            func: format!("zap_deque_get_{}", relem.val_suffix()),
            args: vec![r, i],
        });
        let (lt_bit, _) = self.binop(AstBin::Lt, le, lety, re, rety)?;
        self.set_term(Term::BrIf {
            cond: lt_bit,
            then_bb: set_neg_bb,
            else_bb: check_gt_bb,
        });

        self.position(set_neg_bb);
        let neg = self.iconst(Ty::I64, -1);
        self.store(cmp, neg);
        self.set_term(Term::Br(incr_bb));

        self.position(check_gt_bb);
        // Read the elements again for the `>` test: SSA values from `body_bb` do
        // not dominate this block (it is reached from `body_bb`, so they would —
        // but re-emitting keeps each block self-contained and matches the get ABI).
        let i2 = self.load(idx, Ty::I64);
        let le2 = self.new_value(lety);
        self.emit(Inst::Call {
            dest: Some(le2),
            func: format!("zap_deque_get_{}", lelem.val_suffix()),
            args: vec![l, i2],
        });
        let re2 = self.new_value(rety);
        self.emit(Inst::Call {
            dest: Some(re2),
            func: format!("zap_deque_get_{}", relem.val_suffix()),
            args: vec![r, i2],
        });
        let (gt_bit, _) = self.binop(AstBin::Gt, le2, lety, re2, rety)?;
        self.set_term(Term::BrIf {
            cond: gt_bit,
            then_bb: set_pos_bb,
            else_bb: incr_bb,
        });

        self.position(set_pos_bb);
        let pos = self.iconst(Ty::I64, 1);
        self.store(cmp, pos);
        self.set_term(Term::Br(incr_bb));

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        // All compared elements were equal: the shorter deque is the smaller.
        self.position(after_bb);
        let cur_cmp2 = self.load(cmp, Ty::I64);
        let is_zero = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: is_zero,
            pred: CmpPred::Eq,
            lhs: cur_cmp2,
            rhs: zero,
        });
        self.set_term(Term::BrIf {
            cond: is_zero,
            then_bb: lencmp_bb,
            else_bb: final_bb,
        });

        self.position(lencmp_bb);
        let len_lt = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: len_lt,
            pred: CmpPred::Lt,
            lhs: llen,
            rhs: rlen,
        });
        let len_gt = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: len_gt,
            pred: CmpPred::Gt,
            lhs: llen,
            rhs: rlen,
        });
        // cmp = (llen<rlen) ? -1 : (llen>rlen ? 1 : 0). Build it branchlessly from
        // the two bits: (-1 * len_lt) + (1 * len_gt), widened to i64.
        let neg_l = self.new_value(Ty::I64);
        self.emit(Inst::Cast {
            dest: neg_l,
            kind: CastKind::ZExt,
            val: len_lt,
        });
        let pos_g = self.new_value(Ty::I64);
        self.emit(Inst::Cast {
            dest: pos_g,
            kind: CastKind::ZExt,
            val: len_gt,
        });
        let len_cmp = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: len_cmp,
            op: BinOp::Sub,
            lhs: pos_g,
            rhs: neg_l,
        });
        self.store(cmp, len_cmp);
        self.set_term(Term::Br(final_bb));

        self.position(final_bb);
        let c = self.load(cmp, Ty::I64);
        let pred = match op {
            AstBin::Lt => CmpPred::Lt,
            AstBin::Le => CmpPred::Le,
            AstBin::Gt => CmpPred::Gt,
            AstBin::Ge => CmpPred::Ge,
            _ => unreachable!("deque_order_cmp only handles < <= > >="),
        };
        let d = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: d,
            pred,
            lhs: c,
            rhs: zero,
        });
        Ok((d, Ty::I1))
    }

    /// The full ZIR type a collection element of kind `e` flows as. Identical to
    /// [`Elem::ty`] except for [`Elem::Nested`] (#625), whose precise
    /// `list`/`dict`/`set`/... type is recovered from the lowering interner so a
    /// nested-collection read-back (`rows[i]`, `d[k]`, iteration) dispatches as the
    /// real collection rather than defaulting its bare pointer to `str`.
    fn elem_ty(&self, e: Elem) -> Ty {
        match e {
            Elem::Nested(id) => self.ctx.nested.borrow().get(id),
            other => other.ty(),
        }
    }

    /// The narrow scalar type a value *read back* from a collection element of
    /// kind `e` must carry. A `bool` (`Elem::Bool`) and a `char` (`Elem::Char`)
    /// are stored and physically read as an `i64` word — [`Elem::ty`] is `i64`
    /// so the runtime `zap_*_get_i64` ABI is honoured — yet the *logical* value
    /// a program observes is a `bool`/`char`, not an integer. Typing the read
    /// back as the physical `i64` word lost that identity: a `bool` read from a
    /// `list[bool]` (by iteration or indexing) came back as `i64`, so
    /// [`list_elem_kind`] mapped it to `Elem::I64`, and re-appending/re-storing
    /// it into a `list[bool]` was wrongly rejected with ZAP6001 "wrong element
    /// type" even though it round-trips byte-for-byte (#1241). String iteration
    /// already yields a `char` for exactly this reason (`for c in s`), so lists
    /// (and the set/dict/heap/deque/oset snapshots that funnel through list
    /// iteration) must match. Every other element kind reads back at its
    /// physical [`elem_ty`], so this is a no-op for them.
    fn elem_read_ty(&self, e: Elem) -> Ty {
        match e {
            Elem::Bool => Ty::I1,
            Elem::Char => Ty::Char,
            other => self.elem_ty(other),
        }
    }

    /// Narrow a freshly-read collection element word `v` (physically read at
    /// [`elem_ty`]) to its logical read-back type ([`elem_read_ty`]). For the
    /// int-word Display kinds (`bool`/`char`) this truncates the `i64` storage
    /// word to `i1`/`char`; for every other kind it is the identity (#1241).
    /// Returns the narrowed value and its logical type.
    fn narrow_read_elem(&mut self, v: Value, e: Elem) -> (Value, Ty) {
        let phys = self.elem_ty(e);
        let read = self.elem_read_ty(e);
        if phys == read {
            return (v, phys);
        }
        (self.coerce(v, phys, read), read)
    }

    fn lower_index_get(
        &mut self,
        base: &Expr,
        index: &Expr,
        span: Span,
    ) -> Result<(Value, Ty), LowerError> {
        // #894 part 2: fuse a chained full integer index `m[i][j]…` into the
        // same flat scalar access as the comma form `m[i, j]`, dropping the
        // dead intermediate row-view allocations. Falls through for anything
        // that is not a pure all-axes integer chain into an array.
        if let Some(res) = self.try_fuse_full_index_chain(base, index)? {
            return Ok(res);
        }
        let (bv, bty) = self.expr(base)?;
        // Tuple element access `t[k]` (D-0049): `k` must be a compile-time
        // constant (a tuple is heterogeneous), and the result is that field.
        if let Ty::Struct(sid) = bty {
            if let Some(field_tys) = self.ctx.tuples.borrow().fields(sid) {
                let k = const_int_value(index)
                    .ok_or_else(|| err("a tuple index must be a compile-time-constant integer"))?;
                if k < 0 || k as usize >= field_tys.len() {
                    return Err(err(format!(
                        "tuple index {k} out of range for a {}-element tuple",
                        field_tys.len()
                    )));
                }
                let fty = field_tys[k as usize];
                let d = self.new_value(fty);
                self.emit(Inst::FieldGet {
                    dest: d,
                    base: bv,
                    sid,
                    idx: k as u32,
                });
                return Ok((d, fty));
            }
        }
        match bty {
            Ty::List(elem) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce_index(iv, it);
                // --race (#684): a read of a shared heap list element, keyed on
                // (list header, index).
                self.emit_race_elem(bv, iv, "list element", false, span);
                // The element's full type (#625): a nested-collection element reads
                // back as its real `list`/`dict`/... type, not a bare `str` pointer.
                let ety = self.elem_ty(elem);
                let d = self.new_value(ety);
                // Out-of-range raises a catchable Zap exception (#383), so this
                // must be an `invoke` inside a `try` for the handler to catch it.
                self.throwing_call(
                    Some(d),
                    &format!("zap_list_get_{}", elem.val_suffix()),
                    vec![bv, iv],
                );
                // A `bool`/`char` element reads its physical `i64` word; narrow
                // it to `bool`/`char` so the value round-trips through the
                // element-kind check and re-appends/re-stores into a same-kind
                // collection (#1241).
                let (d, ety) = self.narrow_read_elem(d, elem);
                Ok((d, ety))
            }
            // `deque[T]` indexing (#983): `d[i]` yields the element at left-index
            // `i` (0 == leftmost), the method-free form of `d.get(i)`. Out-of-
            // range aborts (like a list before #383's catchable path), so a plain
            // call suffices.
            Ty::Deque(elem) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce_index(iv, it);
                let ety = self.elem_ty(elem);
                let d = self.new_value(ety);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_deque_get_{}", elem.val_suffix()),
                    args: vec![bv, iv],
                });
                Ok((d, ety))
            }
            Ty::Dict(kk, vk) => {
                // Key type as expected context, so a bare `None` index
                // (`d[None]` on a `dict[Optional[T], _]`) lowers to the tagged
                // Optional `None` instead of the unsupported-expression path (#704).
                // A read passes the key transiently, so (like the pre-#704 path) it
                // is not heap-boxed — only an insert persists the key.
                let kv = self.expr_coerced(index, kk.ty(), zapc_diag::ErrorCode::ZAP4001)?;
                let kfns = self.key_fn_args(kk)?;
                // The value's full type (#625): a nested-collection value reads back
                // as its real `list`/`dict`/... type, not a bare `str` pointer.
                let vty = self.elem_ty(vk);
                let d = self.new_value(vty);
                let mut cargs = vec![bv, kv];
                cargs.extend_from_slice(&kfns);
                // A float key canonicalizes + NaN-rejects in the runtime (#803), so
                // `d[k]` may raise a catchable ValueError on a NaN key.
                self.key_rt_call(
                    Some(d),
                    format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    cargs,
                    kk,
                );
                Ok((d, vty))
            }
            Ty::Array(d, ndim) => {
                // N-D indexing (#500): a full index -> scalar, a partial index ->
                // a rank-reduced view. `m[i, j]` arrives as an `Expr::Tuple`.
                self.lower_array_index_get(bv, d, ndim, index)
            }
            Ty::Struct(sid) => {
                // Indexing overloading (D-0047): `a[i]` -> `a.__getitem__(i)`.
                let (iv, it) = self.expr(index)?;
                self.call_dunder(sid, "__getitem__", vec![(bv, bty), (iv, it)])
            }
            // `bytes` indexing (I#337): `b[i]` yields the byte at byte index `i`
            // as an `int64` in 0..=255. Negative indices count from the end;
            // out-of-range faults. Distinct from `str`'s codepoint indexing.
            Ty::Bytes => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce_index(iv, it);
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bytes_index".to_string(),
                    args: vec![bv, iv],
                });
                Ok((d, Ty::I64))
            }
            // `str` lowers to `Ptr`; `s[i]` yields the codepoint at codepoint
            // index `i` as a `char` (a distinct IR type over i32, #709), Python
            // semantics (D-0025).
            Ty::Ptr => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce_index(iv, it);
                let d = self.new_value(Ty::Char);
                // Out-of-range raises a catchable Zap exception (#383); use an
                // `invoke` inside a `try` so the handler catches it.
                self.throwing_call(Some(d), "zap_str_index", vec![bv, iv]);
                Ok((d, Ty::Char))
            }
            _ => Err(err(
                "indexing is only supported on str, list, dict, and array values",
            )),
        }
    }

    /// One slice bound → `(value, present-flag)`. An omitted bound passes 0 with
    /// the flag clear, and the runtime defaults it by the step's sign (D-0025).
    fn slice_bound(&mut self, e: Option<&Expr>) -> Result<(Value, Value), LowerError> {
        match e {
            Some(x) => {
                let (v, t) = self.expr(x)?;
                let v = self.coerce(v, t, Ty::I64);
                let present = self.iconst(Ty::I64, 1);
                Ok((v, present))
            }
            None => {
                let zero = self.iconst(Ty::I64, 0);
                Ok((zero, zero))
            }
        }
    }

    fn array_slice_bound(&mut self, expr: Option<&Expr>) -> Result<(Value, Value), LowerError> {
        if let Some(expr) = expr {
            let (value, ty) = self.expr(expr)?;
            if ty.int_width().is_none() {
                return Err(err("array slice bounds must be integers"));
            }
            let value = self.coerce(value, ty, Ty::I64);
            let present = self.iconst(Ty::I64, 1);
            Ok((value, present))
        } else {
            let zero = self.iconst(Ty::I64, 0);
            Ok((zero, zero))
        }
    }

    /// `s[lo:hi:step]` → a new value. `str` slices by codepoint (D-0025) and
    /// `bytes` by byte (I#337), both producing a fresh value of the same type. A
    /// `list[T]` slice is a **copy** and an `array[T]` slice is a **view**
    /// (D-0030): the runtime distinguishes the two.
    fn lower_slice(
        &mut self,
        base: &Expr,
        lo: Option<&Expr>,
        hi: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Result<(Value, Ty), LowerError> {
        let (bv, bty) = self.expr(base)?;
        match bty {
            // `list[T]` slice -> a NEW list (a copy, D-0030). Element-type-
            // agnostic: one runtime entry point copies the words and inherits
            // the element kind, so the result is `list[T]` with the same `T`.
            Ty::List(elem) => {
                let (lo_v, has_lo) = self.slice_bound(lo)?;
                let (hi_v, has_hi) = self.slice_bound(hi)?;
                let (st_v, has_st) = self.slice_bound(step)?;
                let out = Ty::List(elem);
                let d = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_list_slice".to_string(),
                    args: vec![bv, lo_v, hi_v, st_v, has_lo, has_hi, has_st],
                });
                Ok((d, out))
            }
            // `array[T]` slice -> a VIEW sharing the buffer (D-0030, #502). A
            // 1-D contiguous (absent or literal-`1` step) slice stays the fast
            // `zap_array_slice` (NULL strides); a stepped/reversed 1-D slice or
            // an N-D axis-0 window is a strided view via `zap_array_slice_axis`
            // (which preserves rank), so `m[lo:hi]` keeps rank-n.
            Ty::Array(dt, ndim) => {
                let out = Ty::Array(dt, ndim);
                let step_is_unit = step.is_none() || step.map(const_int_value) == Some(Some(1));
                if ndim == 1 && step_is_unit {
                    let (lo_v, has_lo) = self.array_slice_bound(lo)?;
                    let (hi_v, has_hi) = self.array_slice_bound(hi)?;
                    let d = self.new_value(out);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_array_slice".to_string(),
                        args: vec![bv, lo_v, hi_v, has_lo, has_hi],
                    });
                    return Ok((d, out));
                }
                // Strided / N-D axis-0 slice: keeps the rank (#502).
                let axis = self.iconst(Ty::I64, 0);
                let (lo_v, has_lo) = self.array_slice_bound(lo)?;
                let (hi_v, has_hi) = self.array_slice_bound(hi)?;
                let (st_v, has_st) = self.array_slice_bound(step)?;
                let d = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_array_slice_axis".to_string(),
                    args: vec![bv, axis, lo_v, hi_v, st_v, has_lo, has_hi, has_st],
                });
                Ok((d, out))
            }
            Ty::Ptr => self.emit_seq_slice(bv, "zap_str_slice", Ty::Ptr, lo, hi, step),
            Ty::Bytes => self.emit_seq_slice(bv, "zap_bytes_slice", Ty::Bytes, lo, hi, step),
            _ => Err(err(
                "slicing is only supported on str, bytes, list, and array values",
            )),
        }
    }

    /// Shared tail for the `str`/`bytes` slice path: emit a call to `func` with
    /// the three Python slice bounds `(lo, hi, step)` and their present-flags.
    fn emit_seq_slice(
        &mut self,
        bv: Value,
        func: &str,
        out_ty: Ty,
        lo: Option<&Expr>,
        hi: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Result<(Value, Ty), LowerError> {
        let (lo_v, has_lo) = self.slice_bound(lo)?;
        let (hi_v, has_hi) = self.slice_bound(hi)?;
        let (st_v, has_st) = self.slice_bound(step)?;
        let d = self.new_value(out_ty);
        self.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![bv, lo_v, hi_v, st_v, has_lo, has_hi, has_st],
        });
        Ok((d, out_ty))
    }

    fn check_index_value(&self, actual: Ty, expected: Elem, list: bool) -> Result<(), LowerError> {
        // Indexed float stores already widen integers via `coerce` — a list
        // element (I#569) or a dict value (#800), both value positions whose
        // expected kind is `Elem::F64`.
        if expected == Elem::F64 && actual.int_width().is_some() {
            return Ok(());
        }
        // A value position (a list element, or — `expected == F64` — a dict
        // value) admits an `f64` element through `value_elem_kind`; a dict/set
        // *key* keeps `elem_kind`, which still rejects floats (#800 Phase 2).
        let kind = if list || expected == Elem::F64 {
            value_elem_kind(actual, &self.ctx.nested)?
        } else {
            elem_kind(actual, &self.ctx.nested)?
        };
        // `i64`/`bool`/`char` elements are one physical cell, distinct only for
        // rendering (#727 regression); a value of any of them stores into a
        // destination of any of them. All other kinds keep exact identity.
        if kind != expected && !(is_int_word_elem(kind) && is_int_word_elem(expected)) {
            return Err(err(format!(
                "indexed assignment expects `{}`, found `{}`",
                self.render_ty(&self.elem_ty(expected)),
                self.render_ty(&actual)
            )));
        }
        Ok(())
    }

    /// Find an existing assignment destination's type without evaluating its
    /// base or index. Chained assignment must still evaluate its RHS only once,
    /// before any target's side effects, while contextualizing empty literals.
    fn assignment_target_ty(&self, target: &Expr) -> Option<Ty> {
        match target {
            Expr::Name(name, _) => self.lookup(name).map(|(_, ty)| ty),
            Expr::Attr { base, name, .. } => {
                let Ty::Struct(sid) = self.assignment_target_ty(base)? else {
                    return None;
                };
                self.ctx.field(sid, name).map(|(_, ty)| ty)
            }
            Expr::Index { base, .. } => match self.assignment_target_ty(base)? {
                Ty::List(elem) | Ty::Dict(_, elem) => Some(self.elem_ty(elem)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Lower a user-supplied subscript index to the 64-bit index word, with
    /// signedness-correct normalization of the from-the-end convention (#552).
    ///
    /// A signed index sign-extends, so a negative value counts from the end
    /// (`xs[-1]`). An unsigned index zero-extends, so it is non-negative by
    /// construction and must never be read as from-the-end: `uint8`/`uint16`/
    /// `uint32` always fit in `[0, 2^32)` and are already safe, but a
    /// `uint64`/`usize` above `INT64_MAX` would reinterpret to a negative
    /// `int64` and be mistaken for a from-the-end offset. Those overflow values
    /// are routed through `zap_uindex`, which clamps them past any possible
    /// length so the collection's own bounds check raises `IndexError` instead
    /// of silently returning an element from the end.
    ///
    /// Used by every element subscript (list/array/bytes/str get, set,
    /// augmented assignment, and `del`). Slice bounds keep plain `i64`
    /// coercion: a negative slice bound is a legitimate, clamped from-the-end
    /// bound, not an error.
    fn coerce_index(&mut self, iv: Value, it: Ty) -> Value {
        let iv = self.coerce(iv, it, Ty::I64);
        if it.is_unsigned() && it.int_width() == Some(64) {
            let d = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_uindex".to_string(),
                args: vec![iv],
            });
            return d;
        }
        iv
    }

    fn lower_index_set(
        &mut self,
        base: &Expr,
        index: &Expr,
        value: &Expr,
        span: Span,
    ) -> Result<(), LowerError> {
        let (bv, bty) = self.expr(base)?;
        match bty {
            Ty::List(elem) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce_index(iv, it);
                let (v, vt) = self.expr_collection_context(value, self.elem_ty(elem))?;
                self.check_index_value(vt, elem, true)?;
                let v = self.coerce(v, vt, elem.ty());
                let v = self.box_if_value(v, elem.ty());
                // --race (#684): a write of a shared heap list element, keyed on
                // (list header, index).
                self.emit_race_elem(bv, iv, "list element", true, span);
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_set_{}", elem.val_suffix()),
                    args: vec![bv, iv, v],
                });
                Ok(())
            }
            Ty::Dict(kk, vk) => {
                // Lower the key with the dict's key type as expected context, so
                // a bare `None` key (`d[None] = v` on a `dict[Optional[T], _]`)
                // lowers to the tagged Optional `None` rather than hitting the
                // generic unsupported-expression path (#704).
                let kv = self.expr_coerced(index, kk.ty(), zapc_diag::ErrorCode::ZAP4001)?;
                let kv = self.box_if_value(kv, kk.ty());
                let (v, vt) = self.expr_collection_context(value, self.elem_ty(vk))?;
                self.check_index_value(vt, vk, false)?;
                let v = self.coerce(v, vt, vk.ty());
                let v = self.box_if_value(v, vk.ty());
                let kfns = self.key_fn_args(kk)?;
                let mut cargs = vec![bv, kv, v];
                cargs.extend_from_slice(&kfns);
                // A float key canonicalizes + NaN-rejects in the runtime (#803), so
                // `d[k] = v` may raise a catchable ValueError on a NaN key.
                self.key_rt_call(
                    None,
                    format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    cargs,
                    kk,
                );
                Ok(())
            }
            Ty::Array(d, ndim) => {
                // Advanced-index assignment `a[mask] = ...` / `a[idx] = ...`
                // (#510, D-0175): a single array subscript scatters into the
                // target's storage. Detected by lowering the lone subscript once;
                // a scalar integer index keeps the existing single-slot store.
                if !matches!(
                    index,
                    Expr::Tuple(..) | Expr::Slice { .. } | Expr::SliceSpec { .. }
                ) {
                    let (iv, it) = self.expr(index)?;
                    if let Ty::Array(idt, ind) = it {
                        return self.lower_array_advanced_assign(bv, d, ndim, iv, idt, ind, value);
                    }
                    if it.int_width().is_none() {
                        return Err(err("array indices must be integers"));
                    }
                    let i = self.coerce_index(iv, it);
                    if ndim != 1 {
                        return Err(err(format!(
                            "assigning to a partial index of a {ndim}-dimensional array \
                             (a whole-row store) is not supported yet; index all {ndim} axes"
                        )));
                    }
                    let (v, vt) = self.expr(value)?;
                    if vt.int_width().is_none() && !vt.is_float() {
                        return Err(err("array element assignment requires a numeric scalar"));
                    }
                    let v = self.coerce(v, vt, d.ty());
                    self.emit(Inst::Call {
                        dest: None,
                        func: format!("zap_array_set_{}", d.suffix()),
                        args: vec![bv, i, v],
                    });
                    return Ok(());
                }
                let (view, last) = self.array_scalar_slot(bv, ndim, index)?;
                let (v, vt) = self.expr(value)?;
                if vt.int_width().is_none() && !vt.is_float() {
                    return Err(err("array element assignment requires a numeric scalar"));
                }
                let v = self.coerce(v, vt, d.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_array_set_{}", d.suffix()),
                    args: vec![view, last, v],
                });
                Ok(())
            }
            Ty::Struct(sid) => {
                // Indexed-assignment overloading (D-0047):
                // `a[i] = v` -> `a.__setitem__(i, v)`.
                let (iv, it) = self.expr(index)?;
                let (v, vt) = self.expr(value)?;
                self.call_dunder(sid, "__setitem__", vec![(bv, bty), (iv, it), (v, vt)])?;
                Ok(())
            }
            _ => Err(err(
                "indexed assignment is only supported on list, dict, and array values",
            )),
        }
    }

    /// Combine the current l-value `(cur, ty)` with `(rhs, rt)` for an augmented
    /// assignment (`+=`, `-=`, …). A user struct that defines the matching
    /// in-place dunder (`__iadd__`, …) mutates in place and returns itself
    /// (D-0047); otherwise the result derives from the binary operator. Shared
    /// by the name, subscript, and field aug-assignment paths (#750).
    fn aug_combine(
        &mut self,
        op: AstBin,
        cur: Value,
        ty: Ty,
        rhs: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        match (ty, inplace_op_dunder(op)) {
            (Ty::Struct(sid), Some(dunder))
                if self
                    .sigs
                    .contains_key(&format!("{}__{}", self.ctx.names[sid as usize], dunder)) =>
            {
                self.call_dunder(sid, dunder, vec![(cur, ty), (rhs, rt)])
            }
            _ => self.binop(op, cur, ty, rhs, rt),
        }
    }

    /// Read-modify-write one subscript element for `base[index] OP= value`
    /// (#750). `base` and `index` are each evaluated **exactly once**, then the
    /// element is read (with the same fault semantics as a plain `base[index]`
    /// read), combined with the RHS, and written back to the same slot.
    fn lower_aug_index(
        &mut self,
        base: &Expr,
        index: &Expr,
        op: AstBin,
        value: &Expr,
        span: Span,
    ) -> Result<(), LowerError> {
        let (bv, bty) = self.expr(base)?;
        match bty {
            Ty::List(elem) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce_index(iv, it);
                let ety = self.elem_ty(elem);
                // Read the current element — out-of-range raises, exactly like a
                // bare `xs[i]` read, so this is a `throwing_call`.
                self.emit_race_elem(bv, iv, "list element", false, span);
                let cur = self.new_value(ety);
                self.throwing_call(
                    Some(cur),
                    &format!("zap_list_get_{}", elem.val_suffix()),
                    vec![bv, iv],
                );
                let (rhs, rt) = self.expr(value)?;
                let (res, rty) = self.aug_combine(op, cur, ety, rhs, rt)?;
                self.check_index_value(rty, elem, true)?;
                let res = self.coerce(res, rty, elem.ty());
                let res = self.box_if_value(res, elem.ty());
                self.emit_race_elem(bv, iv, "list element", true, span);
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_set_{}", elem.val_suffix()),
                    args: vec![bv, iv, res],
                });
                Ok(())
            }
            Ty::Dict(kk, vk) => {
                // The key is evaluated once: transiently for the read, boxed for
                // the write-back.
                let kv = self.expr_coerced(index, kk.ty(), zapc_diag::ErrorCode::ZAP4001)?;
                let vty = self.elem_ty(vk);
                let kfns = self.key_fn_args(kk)?;
                let cur = self.new_value(vty);
                let mut gargs = vec![bv, kv];
                gargs.extend_from_slice(&kfns);
                // A float key canonicalizes + NaN-rejects in the runtime (#803), so
                // this read may raise on a NaN key; the write-back below reuses the
                // same (now non-NaN) key and cannot.
                self.key_rt_call(
                    Some(cur),
                    format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    gargs,
                    kk,
                );
                let (rhs, rt) = self.expr(value)?;
                let (res, rty) = self.aug_combine(op, cur, vty, rhs, rt)?;
                self.check_index_value(rty, vk, false)?;
                let res = self.coerce(res, rty, vk.ty());
                let res = self.box_if_value(res, vk.ty());
                let kvb = self.box_if_value(kv, kk.ty());
                let mut sargs = vec![bv, kvb, res];
                sargs.extend_from_slice(&kfns);
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    args: sargs,
                });
                Ok(())
            }
            Ty::Array(d, ndim) => {
                // The index (and any row folds) is resolved once to a scalar
                // slot, then read-modified-written through that rank-1 view.
                let (view, last) = self.array_scalar_slot(bv, ndim, index)?;
                let cur = self.new_value(d.ty());
                self.emit(Inst::Call {
                    dest: Some(cur),
                    func: format!("zap_array_get_{}", d.suffix()),
                    args: vec![view, last],
                });
                let (rhs, rt) = self.expr(value)?;
                let (res, rty) = self.aug_combine(op, cur, d.ty(), rhs, rt)?;
                if rty.int_width().is_none() && !rty.is_float() {
                    return Err(err("array element assignment requires a numeric scalar"));
                }
                let res = self.coerce(res, rty, d.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_array_set_{}", d.suffix()),
                    args: vec![view, last, res],
                });
                Ok(())
            }
            _ => Err(err(
                "augmented assignment to a subscript is only supported on list, dict, and array values",
            )),
        }
    }

    /// Read-modify-write a struct field for `base.name OP= value` (#750). `base`
    /// is evaluated **exactly once**.
    fn lower_aug_field(
        &mut self,
        base: &Expr,
        name: &str,
        op: AstBin,
        value: &Expr,
        span: Span,
    ) -> Result<(), LowerError> {
        let (bv, bty) = self.expr(base)?;
        let Ty::Struct(sid) = bty else {
            return Err(err("field augmented assignment on a non-struct value"));
        };
        let (idx, fty) = self
            .ctx
            .field(sid, name)
            .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
        self.emit_race_field(bv, sid, idx, name, false, span);
        let cur = self.new_value(fty);
        self.emit(Inst::FieldGet {
            dest: cur,
            base: bv,
            sid,
            idx,
        });
        let (rhs, rt) = self.expr(value)?;
        let (res, rty) = self.aug_combine(op, cur, fty, rhs, rt)?;
        let res = self.coerce(res, rty, fty);
        let res = self.copy_if_value(res, fty, value);
        self.emit_race_field(bv, sid, idx, name, true, span);
        self.emit(Inst::FieldSet {
            base: bv,
            sid,
            idx,
            val: res,
        });
        Ok(())
    }

    /// Store an already-evaluated value `(v, vt)` into an assignment `target`,
    /// without re-evaluating the value (chained assignment, #594). `value_expr`
    /// is the original right-hand side, used only for value-struct copy
    /// diagnostics (`copy_if_value`). This mirrors the single-`Assign` lowering
    /// above target-for-target, but reuses the one evaluated RHS so it is
    /// computed exactly once across the whole chain.
    fn store_computed(
        &mut self,
        target: &Expr,
        v: Value,
        vt: Ty,
        value_expr: &Expr,
    ) -> Result<(), LowerError> {
        if let Expr::Name(n, span) = target {
            // A local, or a module-level mutable global (D-0112, #662).
            let (ptr, ty) = self
                .name_lvalue(n)
                .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
            if let Ty::Func(t_idx) = ty {
                self.check_fn_coercion(&vt, t_idx, zapc_diag::ErrorCode::ZAP4001)?;
            }
            let cv = self.coerce(v, vt, ty);
            let cv = self.copy_if_value(cv, ty, value_expr);
            self.store_name(n, ptr, ty, cv, *span);
            Ok(())
        } else if let Expr::Attr { base, name, span } = target {
            let (bv, bty) = self.expr(base)?;
            let Ty::Struct(sid) = bty else {
                return Err(err("field assignment on a non-struct value"));
            };
            let (idx, fty) = self
                .ctx
                .field(sid, name)
                .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
            let cv = self.coerce(v, vt, fty);
            let cv = self.copy_if_value(cv, fty, value_expr);
            // --race (#684): a write of a shared heap struct field (chained form).
            self.emit_race_field(bv, sid, idx, name, true, *span);
            self.emit(Inst::FieldSet {
                base: bv,
                sid,
                idx,
                val: cv,
            });
            Ok(())
        } else if let Expr::Index { base, index, span } = target {
            self.store_index(base, index, v, vt, *span)
        } else if let Expr::Slice {
            base, lo, hi, step, ..
        } = target
        {
            self.store_slice(base, lo.as_deref(), hi.as_deref(), step.as_deref(), v, vt)
        } else {
            Err(err(
                "only variable, field, index, and slice assignment are supported by codegen yet",
            ))
        }
    }

    /// Lower a sliced assignment `xs[lo:hi:step] = ys` (#1005): the list region
    /// the slice selects is replaced by the already-evaluated right-hand list
    /// `(rv, rt)`. A step-1 slice splices (grow/shrink); an extended slice
    /// requires matching lengths (checked in the runtime, a catchable
    /// ValueError). Element-kind-agnostic: the runtime copies words and inherits
    /// pointer-ness, so this is one entry point for every `T`.
    fn store_slice(
        &mut self,
        base: &Expr,
        lo: Option<&Expr>,
        hi: Option<&Expr>,
        step: Option<&Expr>,
        rv: Value,
        rt: Ty,
    ) -> Result<(), LowerError> {
        let (bv, bty) = self.expr(base)?;
        let Ty::List(elem) = bty else {
            return Err(err(
                "a sliced assignment target `xs[lo:hi] = ys` is only supported on a list",
            ));
        };
        let Ty::List(relem) = rt else {
            return Err(err(
                "the right-hand side of a sliced assignment must be a list",
            ));
        };
        if elem.val_suffix() != relem.val_suffix() {
            return Err(err(
                "a sliced assignment requires the right-hand list to have the same element type",
            ));
        }
        let (lo_v, has_lo) = self.slice_bound(lo)?;
        let (hi_v, has_hi) = self.slice_bound(hi)?;
        let (st_v, has_st) = self.slice_bound(step)?;
        // May raise (extended-slice length mismatch) -> throwing_call.
        self.throwing_call(
            None,
            "zap_list_set_slice",
            vec![bv, rv, lo_v, hi_v, st_v, has_lo, has_hi, has_st],
        );
        Ok(())
    }

    /// Store an already-evaluated value `(v, vt)` at `base[index]` (chained
    /// assignment, #594). Identical to the index arm of [`Self::lower_index_set`]
    /// except the value word is supplied rather than re-evaluated.
    fn store_index(
        &mut self,
        base: &Expr,
        index: &Expr,
        v: Value,
        vt: Ty,
        span: Span,
    ) -> Result<(), LowerError> {
        let (bv, bty) = self.expr(base)?;
        match bty {
            Ty::List(elem) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce_index(iv, it);
                self.check_index_value(vt, elem, true)?;
                let v = self.coerce(v, vt, elem.ty());
                let v = self.box_if_value(v, elem.ty());
                // --race (#684): a write of a shared heap list element (chained).
                self.emit_race_elem(bv, iv, "list element", true, span);
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_set_{}", elem.val_suffix()),
                    args: vec![bv, iv, v],
                });
                Ok(())
            }
            Ty::Dict(kk, vk) => {
                let (kv, kt) = self.expr(index)?;
                let kv = self.coerce(kv, kt, kk.ty());
                let kv = self.box_if_value(kv, kk.ty());
                self.check_index_value(vt, vk, false)?;
                let v = self.coerce(v, vt, vk.ty());
                let v = self.box_if_value(v, vk.ty());
                let kfns = self.key_fn_args(kk)?;
                let mut cargs = vec![bv, kv, v];
                cargs.extend_from_slice(&kfns);
                // A float key canonicalizes + NaN-rejects in the runtime (#803), so
                // `d[k] = v` may raise a catchable ValueError on a NaN key.
                self.key_rt_call(
                    None,
                    format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    cargs,
                    kk,
                );
                Ok(())
            }
            Ty::Array(d, ndim) => {
                let (view, last) = self.array_scalar_slot(bv, ndim, index)?;
                if vt.int_width().is_none() && !vt.is_float() {
                    return Err(err("array element assignment requires a numeric scalar"));
                }
                let v = self.coerce(v, vt, d.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_array_set_{}", d.suffix()),
                    args: vec![view, last, v],
                });
                Ok(())
            }
            Ty::Struct(sid) => {
                let (iv, it) = self.expr(index)?;
                self.call_dunder(sid, "__setitem__", vec![(bv, bty), (iv, it), (v, vt)])?;
                Ok(())
            }
            _ => Err(err(
                "indexed assignment is only supported on list, dict, and array values",
            )),
        }
    }

    /// Lower `del base[index]` (#595): remove a dict entry by key or a list
    /// element by index. The removal reuses the collection's own storage
    /// operations through dedicated runtime entry points
    /// (`zap_dict_del_*` / `zap_list_del_at`), which share the probe/normalize
    /// code of the corresponding `set`/`get` so behavior matches element access:
    ///   * a list index is normalized (negatives count from the end) and bounds-
    ///     checked, raising the same `IndexError` as a read `xs[i]`;
    ///   * a missing dict key is a no-op, matching the current non-raising dict
    ///     read `d[k]` (a real `KeyError` is tracked under #22).
    fn lower_index_del(&mut self, base: &Expr, index: &Expr) -> Result<(), LowerError> {
        let (bv, bty) = self.expr(base)?;
        match bty {
            Ty::List(_) => {
                let (iv, it) = self.expr(index)?;
                let iv = self.coerce_index(iv, it);
                // An out-of-range index raises a catchable IndexError (#383), the
                // same as a read `xs[i]`, so this must be an `invoke` inside a
                // `try` for the handler to catch it.
                self.throwing_call(None, "zap_list_del_at", vec![bv, iv]);
                Ok(())
            }
            Ty::Dict(kk, _) => {
                let (kv, kt) = self.expr(index)?;
                let kv = self.coerce(kv, kt, kk.ty());
                let kv = self.box_if_value(kv, kk.ty());
                let kfns = self.key_fn_args(kk)?;
                let mut cargs = vec![bv, kv];
                cargs.extend_from_slice(&kfns);
                // A float key canonicalizes + NaN-rejects in the runtime (#803), so
                // `del d[k]` may raise a catchable ValueError on a NaN key.
                self.key_rt_call(None, format!("zap_dict_del_{}", kk.key_suffix()), cargs, kk);
                Ok(())
            }
            _ => Err(err(
                "del is only supported on dict and list subscripts (`del d[k]` / `del xs[i]`)",
            )),
        }
    }

    fn lower_list_method(
        &mut self,
        recv: Value,
        elem: Elem,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "append" => {
                if args.len() != 1 {
                    return Err(err("append() takes exactly one argument"));
                }
                let (v, vt) = self.expr_collection_context(&args[0].value, self.elem_ty(elem))?;
                let actual = list_elem_kind(vt, &self.ctx.nested)?;
                // A concrete conformer appended to a `list[Iface]` is boxed into
                // the interface fat pointer (#829); any other representation
                // mismatch is rejected.
                if actual != elem && !self.conforms_to_iface_elem(vt, elem, actual) {
                    return Err(err("append() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                let v = self.box_elem_for_store(v, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_push_{}", elem.val_suffix()),
                    args: vec![recv, v],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            // `xs.sort(key=..., reverse=...)` — stable, in place (ticket #336).
            "sort" => {
                let (keys, kind, reverse) = self.prepare_sort(recv, elem, "sort", args)?;
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_list_sort_by".to_string(),
                    args: vec![recv, keys, kind, reverse],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            // clear(): drop every element in place (#977).
            "clear" => {
                if !args.is_empty() {
                    return Err(err("clear() takes no arguments"));
                }
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_list_clear".to_string(),
                    args: vec![recv],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            // extend(other): append every element of another list[elem] (#977).
            "extend" => {
                if args.len() != 1 {
                    return Err(err("extend() takes exactly one argument"));
                }
                let (other, _) = self.expr(&args[0].value)?;
                let push = format!("zap_list_push_{}", elem.val_suffix());
                self.iterate_list(other, elem, move |s, v, _vt| {
                    s.emit(Inst::Call {
                        dest: None,
                        func: push.clone(),
                        args: vec![recv, v],
                    });
                    Ok(())
                })?;
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            // insert(index, value): shift-insert at a (saturating) index (#977).
            "insert" => {
                if args.len() != 2 {
                    return Err(err("insert() takes exactly two arguments (index, value)"));
                }
                let (iv, it) = self.expr(&args[0].value)?;
                let idx = self.coerce(iv, it, Ty::I64);
                let (v, vt) = self.expr_collection_context(&args[1].value, self.elem_ty(elem))?;
                let v = self.coerce(v, vt, elem.ty());
                let v = self.box_elem_for_store(v, elem.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_list_insert_{}", elem.val_suffix()),
                    args: vec![recv, idx, v],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            // count(x) -> int64: number of equal elements. A linear scan whose
            // per-element equality routes through `binop(Eq, ..)`, so it matches
            // `==`/`in` semantics (numeric joins, str by contents, struct __eq__),
            // the `in` template (#399, #977).
            "count" => {
                if args.len() != 1 {
                    return Err(err("count() takes exactly one argument"));
                }
                let (x, xt) = self.expr(&args[0].value)?;
                let cnt = self.alloca(Ty::I64);
                let zero = self.iconst(Ty::I64, 0);
                self.store(cnt, zero);
                let idx = self.alloca(Ty::I64);
                self.store(idx, zero);
                let cond_bb = self.new_block();
                let body_bb = self.new_block();
                let incr_bb = self.new_block();
                let end_bb = self.new_block();
                self.set_term(Term::Br(cond_bb));
                self.position(cond_bb);
                let cur = self.load(idx, Ty::I64);
                let len = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_list_len".to_string(),
                    args: vec![recv],
                });
                let c = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: c,
                    pred: CmpPred::Lt,
                    lhs: cur,
                    rhs: len,
                });
                self.set_term(Term::BrIf {
                    cond: c,
                    then_bb: body_bb,
                    else_bb: end_bb,
                });
                self.position(body_bb);
                let i = self.load(idx, Ty::I64);
                let e = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(e),
                    func: format!("zap_list_get_{}", elem.val_suffix()),
                    args: vec![recv, i],
                });
                let (eq, _) = self.binop(AstBin::Eq, e, elem.ty(), x, xt)?;
                let bump_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: eq,
                    then_bb: bump_bb,
                    else_bb: incr_bb,
                });
                self.position(bump_bb);
                let cv = self.load(cnt, Ty::I64);
                let one = self.iconst(Ty::I64, 1);
                let cv1 = self.new_value(Ty::I64);
                self.emit(Inst::Bin {
                    dest: cv1,
                    op: BinOp::Add,
                    lhs: cv,
                    rhs: one,
                });
                self.store(cnt, cv1);
                self.set_term(Term::Br(incr_bb));
                self.position(incr_bb);
                let cur2 = self.load(idx, Ty::I64);
                let one2 = self.iconst(Ty::I64, 1);
                let next = self.new_value(Ty::I64);
                self.emit(Inst::Bin {
                    dest: next,
                    op: BinOp::Add,
                    lhs: cur2,
                    rhs: one2,
                });
                self.store(idx, next);
                self.set_term(Term::Br(cond_bb));
                self.position(end_bb);
                let total = self.load(cnt, Ty::I64);
                Ok((total, Ty::I64))
            }
            // remove(x): delete the first element equal to `x`; raise ValueError
            // when none matches (CPython). Same scan as `count`, breaking on the
            // first hit (#977).
            "remove" => {
                if args.len() != 1 {
                    return Err(err("remove() takes exactly one argument"));
                }
                let (x, xt) = self.expr(&args[0].value)?;
                let found = self.alloca(Ty::I1);
                let ff = self.new_value(Ty::I1);
                self.emit(Inst::BConst {
                    dest: ff,
                    val: false,
                });
                self.store(found, ff);
                let idx = self.alloca(Ty::I64);
                let zero = self.iconst(Ty::I64, 0);
                self.store(idx, zero);
                let cond_bb = self.new_block();
                let body_bb = self.new_block();
                let incr_bb = self.new_block();
                let end_bb = self.new_block();
                self.set_term(Term::Br(cond_bb));
                self.position(cond_bb);
                let cur = self.load(idx, Ty::I64);
                let len = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_list_len".to_string(),
                    args: vec![recv],
                });
                let c = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: c,
                    pred: CmpPred::Lt,
                    lhs: cur,
                    rhs: len,
                });
                self.set_term(Term::BrIf {
                    cond: c,
                    then_bb: body_bb,
                    else_bb: end_bb,
                });
                self.position(body_bb);
                let i = self.load(idx, Ty::I64);
                let e = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(e),
                    func: format!("zap_list_get_{}", elem.val_suffix()),
                    args: vec![recv, i],
                });
                let (eq, _) = self.binop(AstBin::Eq, e, elem.ty(), x, xt)?;
                let del_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: eq,
                    then_bb: del_bb,
                    else_bb: incr_bb,
                });
                self.position(del_bb);
                let i2 = self.load(idx, Ty::I64);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_list_del_at".to_string(),
                    args: vec![recv, i2],
                });
                let tt = self.new_value(Ty::I1);
                self.emit(Inst::BConst {
                    dest: tt,
                    val: true,
                });
                self.store(found, tt);
                self.set_term(Term::Br(end_bb));
                self.position(incr_bb);
                let cur2 = self.load(idx, Ty::I64);
                let one = self.iconst(Ty::I64, 1);
                let next = self.new_value(Ty::I64);
                self.emit(Inst::Bin {
                    dest: next,
                    op: BinOp::Add,
                    lhs: cur2,
                    rhs: one,
                });
                self.store(idx, next);
                self.set_term(Term::Br(cond_bb));
                self.position(end_bb);
                let fv = self.load(found, Ty::I1);
                let raise_bb = self.new_block();
                let done_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: fv,
                    then_bb: done_bb,
                    else_bb: raise_bb,
                });
                self.position(raise_bb);
                let m = self.new_value(Ty::Ptr);
                self.emit(Inst::StrConst {
                    dest: m,
                    bytes: b"list.remove(x): x not in list".to_vec(),
                });
                let value_error = self.iconst(Ty::I64, 3); // ZAP_EXC_VALUE_ERROR
                self.throwing_call(None, "zap_panic_typed", vec![value_error, m]);
                if !self.terminated() {
                    self.set_term(Term::Unreachable);
                }
                self.position(done_bb);
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            // pop([i]) -> elem (#1095): remove and return the element at index
            // `i`, or the LAST element (O(1)) with no argument. A negative index
            // counts from the end. An empty list or an out-of-range index raises
            // a catchable IndexError in the runtime, so this must `invoke` the
            // pop helper for a `try`/`except` to catch it. The result reads back
            // as the element's full type (#625) and, being an `invoke` dest, is
            // GC-rooted exactly like a `zap_list_get_*` / subscript-read result.
            "pop" => {
                if args.len() > 1 {
                    return Err(err("pop() takes at most one argument (an index)"));
                }
                let (idx, has_index) = if let Some(a) = args.first() {
                    let (iv, it) = self.expr(&a.value)?;
                    (self.coerce_index(iv, it), self.iconst(Ty::I64, 1))
                } else {
                    (self.iconst(Ty::I64, 0), self.iconst(Ty::I64, 0))
                };
                let ety = self.elem_ty(elem);
                let d = self.new_value(ety);
                self.throwing_call(
                    Some(d),
                    &format!("zap_list_pop_{}", elem.val_suffix()),
                    vec![recv, idx, has_index],
                );
                Ok((d, ety))
            }
            // index(value[, start[, stop]]) -> int64 (#1095): the position of the
            // first element equal to `value` within `[start, stop)`; raises a
            // catchable ValueError when none matches (CPython raises ValueError,
            // not IndexError). The per-element equality routes through
            // `binop(Eq, ..)`, so it matches `==`/`in` (numeric joins, str by
            // contents, struct `__eq__`), exactly like `count`/`remove`. Optional
            // `start`/`stop` are clamped the CPython slice way (negatives count
            // from the end; both clamp to `[0, len]`), so the scan only ever
            // reads valid indices and never itself faults.
            "index" => {
                if args.is_empty() || args.len() > 3 {
                    return Err(err(
                        "index() takes one to three arguments (value[, start[, stop]])",
                    ));
                }
                let (x, xt) = self.expr(&args[0].value)?;
                let len = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_list_len".to_string(),
                    args: vec![recv],
                });
                let start = if let Some(a) = args.get(1) {
                    let (sv, st) = self.expr(&a.value)?;
                    let sv = self.coerce(sv, st, Ty::I64);
                    self.clamp_index_bound(sv, len)
                } else {
                    self.iconst(Ty::I64, 0)
                };
                let stop = if let Some(a) = args.get(2) {
                    let (sv, st) = self.expr(&a.value)?;
                    let sv = self.coerce(sv, st, Ty::I64);
                    self.clamp_index_bound(sv, len)
                } else {
                    len
                };
                let idx = self.alloca(Ty::I64);
                self.store(idx, start);
                let cond_bb = self.new_block();
                let body_bb = self.new_block();
                let incr_bb = self.new_block();
                let found_bb = self.new_block();
                let notfound_bb = self.new_block();
                self.set_term(Term::Br(cond_bb));
                self.position(cond_bb);
                let cur = self.load(idx, Ty::I64);
                let c = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: c,
                    pred: CmpPred::Lt,
                    lhs: cur,
                    rhs: stop,
                });
                self.set_term(Term::BrIf {
                    cond: c,
                    then_bb: body_bb,
                    else_bb: notfound_bb,
                });
                self.position(body_bb);
                let i = self.load(idx, Ty::I64);
                let e = self.new_value(elem.ty());
                self.emit(Inst::Call {
                    dest: Some(e),
                    func: format!("zap_list_get_{}", elem.val_suffix()),
                    args: vec![recv, i],
                });
                let (eq, _) = self.binop(AstBin::Eq, e, elem.ty(), x, xt)?;
                self.set_term(Term::BrIf {
                    cond: eq,
                    then_bb: found_bb,
                    else_bb: incr_bb,
                });
                self.position(incr_bb);
                let cur2 = self.load(idx, Ty::I64);
                let one = self.iconst(Ty::I64, 1);
                let next = self.new_value(Ty::I64);
                self.emit(Inst::Bin {
                    dest: next,
                    op: BinOp::Add,
                    lhs: cur2,
                    rhs: one,
                });
                self.store(idx, next);
                self.set_term(Term::Br(cond_bb));
                self.position(notfound_bb);
                let m = self.new_value(Ty::Ptr);
                self.emit(Inst::StrConst {
                    dest: m,
                    bytes: b"list.index(x): x not in list".to_vec(),
                });
                let value_error = self.iconst(Ty::I64, 3); // ZAP_EXC_VALUE_ERROR
                self.throwing_call(None, "zap_panic_typed", vec![value_error, m]);
                if !self.terminated() {
                    self.set_term(Term::Unreachable);
                }
                self.position(found_bb);
                let res = self.load(idx, Ty::I64);
                Ok((res, Ty::I64))
            }
            // reverse() (#1095): reverse the elements in place (CPython
            // `list.reverse`). Element-type-agnostic word swap in the runtime.
            "reverse" => {
                if !args.is_empty() {
                    return Err(err("reverse() takes no arguments"));
                }
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_list_reverse".to_string(),
                    args: vec![recv],
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            // copy() -> list[elem] (#1095): a shallow copy — a new list with the
            // same element words (shared referents), so mutating the copy's
            // membership does not touch the original, but reference elements are
            // shared (CPython `list.copy`). Reuses `zap_list_copy` (also used by
            // `sorted()`), whose result inherits `elem_ptr` for precise tracing.
            "copy" => {
                if !args.is_empty() {
                    return Err(err("copy() takes no arguments"));
                }
                let d = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_list_copy".to_string(),
                    args: vec![recv],
                });
                Ok((d, Ty::List(elem)))
            }
            other => Err(err(format!(
                "list has no method '{other}' (supported: append, sort, extend, \
                 insert, remove, pop, index, count, reverse, copy, clear)"
            ))),
        }
    }

    /// Clamp a `list.index(x, start, stop)` bound the CPython slice way (#1095):
    /// a negative bound counts from the end (`+= len`), and the result is clamped
    /// to `[0, len]` — the same forward-step clamp the runtime's `zap_slice_pos`
    /// applies to a slice bound. Emitting it in compiled code (rather than adding
    /// a runtime symbol) keeps the new runtime surface to just `pop`/`reverse`.
    fn clamp_index_bound(&mut self, v: Value, len: Value) -> Value {
        let slot = self.alloca(Ty::I64);
        self.store(slot, v);
        let zero = self.iconst(Ty::I64, 0);
        let is_neg = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: is_neg,
            pred: CmpPred::Lt,
            lhs: v,
            rhs: zero,
        });
        let neg_bb = self.new_block();
        let pos_bb = self.new_block();
        let join_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: is_neg,
            then_bb: neg_bb,
            else_bb: pos_bb,
        });
        // Negative: v += len, then floor at 0.
        self.position(neg_bb);
        let vln = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: vln,
            op: BinOp::Add,
            lhs: v,
            rhs: len,
        });
        let still_neg = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: still_neg,
            pred: CmpPred::Lt,
            lhs: vln,
            rhs: zero,
        });
        let floor_bb = self.new_block();
        let keep_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: still_neg,
            then_bb: floor_bb,
            else_bb: keep_bb,
        });
        self.position(floor_bb);
        self.store(slot, zero);
        self.set_term(Term::Br(join_bb));
        self.position(keep_bb);
        self.store(slot, vln);
        self.set_term(Term::Br(join_bb));
        // Non-negative: cap at len.
        self.position(pos_bb);
        let too_big = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: too_big,
            pred: CmpPred::Gt,
            lhs: v,
            rhs: len,
        });
        let cap_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: too_big,
            then_bb: cap_bb,
            else_bb: join_bb,
        });
        self.position(cap_bb);
        self.store(slot, len);
        self.set_term(Term::Br(join_bb));
        self.position(join_bb);
        self.load(slot, Ty::I64)
    }

    /// Build the shared arguments for a `zap_list_sort_by` call from the optional
    /// `key=` / `reverse=` keyword arguments of `sort()` / `sorted()` (ticket
    /// #336). Returns `(keys, cmp_kind, reverse)` where `keys` is the list whose
    /// words order the sort (the list itself when there is no `key=`), `cmp_kind`
    /// is `0` (signed int) or `1` (byte-wise str), and `reverse` is a `0/1` flag.
    ///
    /// `fname` is the caller's name for diagnostics. A `key=` projection is a
    /// first-class function value (I#163) whose return type selects `cmp_kind`;
    /// its key list is built in compiled code by [`Self::build_key_list`] so the
    /// growing list stays a GC root across the per-element calls.
    fn prepare_sort(
        &mut self,
        lst: Value,
        elem: Elem,
        fname: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Value, Value), LowerError> {
        let mut key_expr: Option<&Expr> = None;
        let mut reverse_expr: Option<&Expr> = None;
        for a in args {
            match a.name.as_deref() {
                Some("key") => key_expr = Some(&a.value),
                Some("reverse") => reverse_expr = Some(&a.value),
                Some(other) => {
                    return Err(err(format!(
                        "{fname}() got an unexpected keyword argument '{other}' \
                         (expected 'key' and/or 'reverse')"
                    )))
                }
                None => {
                    return Err(err(format!(
                        "{fname}() takes only the keyword arguments 'key' and 'reverse'"
                    )))
                }
            }
        }
        // `reverse` is a 0/1 flag; default ascending.
        let reverse = match reverse_expr {
            Some(e) => {
                let (v, vt) = self.expr(e)?;
                self.coerce(v, vt, Ty::I64)
            }
            None => self.iconst(Ty::I64, 0),
        };
        let orderable_kind = |k: Elem, what: &str| -> Result<i64, LowerError> {
            match k {
                // `char` orders as its `i64` codepoint word, like an int (#709).
                Elem::I64 | Elem::Bool | Elem::Char => Ok(0),
                Elem::Enum(_) => Ok(0),
                // `str` and `bytes` are `ZapStr` words, ordered byte-wise (#1074).
                Elem::Ptr | Elem::Bytes => Ok(1),
                // Ordering `f64` elements directly is a clean follow-up (the
                // runtime sort compares elements as `i64`/str words today); a
                // `list[float]` is still sortable via a `key=` projection.
                // A nested collection (#625) is not directly orderable (the
                // runtime compares elements as `i64`/str words), like `f64` and
                // structs; sort it via a `key=` projection.
                // A function value (#1348) has no runtime ordering, like a
                // struct / nested collection / complex; sort via a `key=`.
                Elem::F64 | Elem::Struct(_) | Elem::Nested(_) | Elem::Complex | Elem::Func(_) => {
                    Err(err(format!(
                        "{what} is not orderable; pass key= to sort by an orderable \
                         projection (an int or str)"
                    )))
                }
            }
        };
        let (keys, kind) = match key_expr {
            Some(e) => {
                let (kv, kt) = self.expr(e)?;
                let Ty::Func(sig_idx) = kt else {
                    return Err(err(format!(
                        "{fname}() key= must be a function value — a named function \
                         or a fn(T)->R binding"
                    )));
                };
                let ret = self.ctx.fn_sigs.borrow().get(sig_idx).ret;
                let kind = orderable_kind(
                    elem_kind(ret, &self.ctx.nested)?,
                    "the sort key's return value",
                )?;
                let keys = self.build_key_list(lst, elem, kv, sig_idx, ret)?;
                (keys, self.iconst(Ty::I64, kind))
            }
            None => {
                let kind = orderable_kind(elem, "this list's elements")?;
                (lst, self.iconst(Ty::I64, kind))
            }
        };
        Ok((keys, kind, reverse))
    }

    /// Emit a compiled loop that applies the `key` function to every element of
    /// `lst`, collecting the results into a fresh list returned for use as sort
    /// keys (ticket #336). Running in compiled code (not the C runtime) is what
    /// keeps the growing key list reachable as a GC root across each key call's
    /// safepoint. `ret` is the key function's return type (int or str).
    fn build_key_list(
        &mut self,
        lst: Value,
        elem: Elem,
        keyfn: Value,
        sig_idx: u32,
        ret: Ty,
    ) -> Result<Value, LowerError> {
        let kkind = elem_kind(ret, &self.ctx.nested)?;
        let keys = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(keys),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let idx = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let len = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_list_len".to_string(),
            args: vec![lst],
        });
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: len,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let e = self.new_value(elem.ty());
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst, i],
        });
        let (k, _kt) = self.emit_closure_call(keyfn, sig_idx, vec![e])?;
        self.emit(Inst::Call {
            dest: None,
            func: format!("zap_list_push_{}", kkind.val_suffix()),
            args: vec![keys, k],
        });
        self.set_term(Term::Br(incr_bb));

        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: one,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        Ok(keys)
    }

    /// Emit `a < b` (an `i1`) for two values of orderable kind `kind` (#1299):
    /// floats via `FCmp`, strings/bytes via `zap_str_cmp` then `< 0`, and
    /// integer-word kinds (int/bool/char/enum) via a signed `ICmp`.
    fn emit_order_lt(&mut self, a: Value, b: Value, kind: Elem) -> Result<Value, LowerError> {
        let d = self.new_value(Ty::I1);
        match kind {
            Elem::F64 => {
                self.emit(Inst::FCmp {
                    dest: d,
                    pred: CmpPred::Lt,
                    lhs: a,
                    rhs: b,
                });
            }
            Elem::Ptr | Elem::Bytes => {
                let cmp = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(cmp),
                    func: "zap_str_cmp".to_string(),
                    args: vec![a, b],
                });
                let zero = self.iconst(Ty::I64, 0);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Lt,
                    lhs: cmp,
                    rhs: zero,
                });
            }
            Elem::I64 | Elem::Bool | Elem::Char | Elem::Enum(_) => {
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Lt,
                    lhs: a,
                    rhs: b,
                });
            }
            other => {
                return Err(err(format!(
                    "min()/max() cannot order elements of kind {other:?}; pass key="
                )))
            }
        }
        Ok(d)
    }

    /// Iterable `min(xs)` / `max(xs)` (#1299), with optional `key=` projection
    /// and `default=`. Drains the list once, keeping the best element (and, with
    /// `key=`, its projected key). An empty list returns `default` when given,
    /// else raises a catchable `ValueError`. The best element/key live in
    /// allocas so they stay GC-reachable across the `key=` call's safepoint
    /// (same reasoning as `build_key_list`). Integer keys compare with a signed
    /// `icmp`, never through float.
    fn lower_minmax_iterable(
        &mut self,
        is_max: bool,
        seq: &Expr,
        key: Option<&Expr>,
        default: Option<&Expr>,
    ) -> Result<(Value, Ty), LowerError> {
        let (xs, xt) = self.expr(seq)?;
        let Ty::List(elem) = xt else {
            return Err(err("min()/max() iterable form expects a list"));
        };
        let elem_word = elem.ty();
        // The comparison kind: the key function's return kind when `key=` is
        // given, else the element kind.
        let (keyfn, cmp_kind): (Option<(Value, u32)>, Elem) = match key {
            Some(e) => {
                let (kv, kt) = self.expr(e)?;
                let Ty::Func(sig_idx) = kt else {
                    return Err(err(
                        "min()/max() key= must be a function value — a named function \
                         or a fn(T)->R binding",
                    ));
                };
                let ret = self.ctx.fn_sigs.borrow().get(sig_idx).ret;
                (Some((kv, sig_idx)), elem_kind(ret, &self.ctx.nested)?)
            }
            None => (None, elem),
        };

        // best element slot (the result), and — with key= — its key slot.
        let best = self.alloca(elem_word);
        let best_key = keyfn.map(|(_, sig_idx)| {
            let ret = self.ctx.fn_sigs.borrow().get(sig_idx).ret;
            self.alloca(ret)
        });

        let len = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_list_len".to_string(),
            args: vec![xs],
        });
        let zero = self.iconst(Ty::I64, 0);
        let is_empty = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: is_empty,
            pred: CmpPred::Eq,
            lhs: len,
            rhs: zero,
        });
        let empty_bb = self.new_block();
        let nonempty_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: is_empty,
            then_bb: empty_bb,
            else_bb: nonempty_bb,
        });

        // Empty: return default, or raise ValueError.
        self.position(empty_bb);
        if let Some(de) = default {
            let (dv, dt) = self.expr(de)?;
            let dv = self.coerce(dv, dt, elem_word);
            self.store(best, dv);
            self.set_term(Term::Br(end_bb));
        } else {
            let msg = self.strconst("min()/max() of an empty sequence");
            let verr = self.iconst(Ty::I64, 3); // ZAP_EXC_VALUE_ERROR
            self.throwing_call(None, "zap_panic_typed", vec![verr, msg]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }

        // Non-empty: seed best from element 0, then fold elements 1..len.
        self.position(nonempty_bb);
        let get = format!("zap_list_get_{}", elem.val_suffix());
        let e0 = self.new_value(elem_word);
        self.emit(Inst::Call {
            dest: Some(e0),
            func: get.clone(),
            args: vec![xs, zero],
        });
        self.store(best, e0);
        if let (Some((kv, sig_idx)), Some(bk)) = (keyfn, best_key) {
            let (k0, _) = self.emit_closure_call(kv, sig_idx, vec![e0])?;
            self.store(bk, k0);
        }
        let idx = self.alloca(Ty::I64);
        let one = self.iconst(Ty::I64, 1);
        self.store(idx, one);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let loop_end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: len,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: loop_end_bb,
        });
        self.position(body_bb);
        let i = self.load(idx, Ty::I64);
        let e = self.new_value(elem_word);
        self.emit(Inst::Call {
            dest: Some(e),
            func: get,
            args: vec![xs, i],
        });
        // Candidate key, and the current best key.
        let (cand_key, best_key_val) = match (keyfn, best_key) {
            (Some((kv, sig_idx)), Some(bk)) => {
                let (ck, kt) = self.emit_closure_call(kv, sig_idx, vec![e])?;
                (ck, self.load(bk, kt))
            }
            _ => {
                let cur_best = self.load(best, elem_word);
                (e, cur_best)
            }
        };
        // Update when the candidate beats the best: for `min`, candidate key is
        // less; for `max`, best key is less than the candidate key.
        let better = if is_max {
            self.emit_order_lt(best_key_val, cand_key, cmp_kind)?
        } else {
            self.emit_order_lt(cand_key, best_key_val, cmp_kind)?
        };
        let update_bb = self.new_block();
        let skip_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: better,
            then_bb: update_bb,
            else_bb: skip_bb,
        });
        self.position(update_bb);
        self.store(best, e);
        if let Some(bk) = best_key {
            self.store(bk, cand_key);
        }
        self.set_term(Term::Br(skip_bb));
        self.position(skip_bb);
        self.set_term(Term::Br(incr_bb));
        self.position(incr_bb);
        let cur2 = self.load(idx, Ty::I64);
        let onei = self.iconst(Ty::I64, 1);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: onei,
        });
        self.store(idx, next);
        self.set_term(Term::Br(cond_bb));
        self.position(loop_end_bb);
        self.set_term(Term::Br(end_bb));

        self.position(end_bb);
        let result = self.load(best, elem_word);
        Ok((result, elem_word))
    }

    /// Call a first-class function value (I#163) with already-lowered argument
    /// values, via [`Inst::CallClosure`]. A lower-level companion to
    /// [`Self::lower_closure_call`], which lowers AST argument expressions.
    fn emit_closure_call(
        &mut self,
        callee: Value,
        sig_idx: u32,
        argvals: Vec<Value>,
    ) -> Result<(Value, Ty), LowerError> {
        let sig = self.ctx.fn_sigs.borrow().get(sig_idx).clone();
        let dest = if sig.ret == Ty::Void {
            None
        } else {
            Some(self.new_value(sig.ret))
        };
        let unwind = self.landing_pads.last().copied();
        if let Some(lp) = unwind {
            self.used_lps.insert(lp);
        }
        self.emit(Inst::CallClosure {
            dest,
            callee,
            args: argvals,
            params: sig.params.clone(),
            ret: sig.ret,
            unwind,
        });
        let rv = dest.unwrap_or_else(|| self.iconst(Ty::I32, 0));
        Ok((
            rv,
            if sig.ret == Ty::Void {
                Ty::I32
            } else {
                sig.ret
            },
        ))
    }

    /// Lower a method call on an `Atomic[T]` box (I#176) to an inline atomic ZIR
    /// node over its cell. `recv` is already the box pointer; the cell is the sole
    /// `value` field. Sequentially consistent by default (the ordering is carried
    /// by the node so weaker orderings can be added later). `add`/`sub` are
    /// integer-only — a `bool` or reference cell rejects them.
    fn lower_atomic_method(
        &mut self,
        recv: Value,
        sid: u32,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let (idx, elem) = self
            .ctx
            .field(sid, "value")
            .ok_or_else(|| err("Atomic box has no cell field"))?;
        let order = AtomicOrd::SeqCst;
        // Lower a single coerced argument (the common shape).
        let one_arg = |s: &mut Self, args: &[ast::Arg]| -> Result<Value, LowerError> {
            if args.len() != 1 {
                return Err(err(format!("{name}() takes exactly one argument")));
            }
            let (v, vt) = s.expr(&args[0].value)?;
            Ok(s.coerce(v, vt, elem))
        };
        match name {
            "load" => {
                if !args.is_empty() {
                    return Err(err("load() takes no arguments"));
                }
                let dest = self.new_value(elem);
                self.emit(Inst::AtomicLoad {
                    dest,
                    base: recv,
                    sid,
                    idx,
                    elem,
                    order,
                });
                Ok((dest, elem))
            }
            "store" => {
                let v = one_arg(self, args)?;
                self.emit(Inst::AtomicStore {
                    base: recv,
                    sid,
                    idx,
                    val: v,
                    elem,
                    order,
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            "swap" => {
                let v = one_arg(self, args)?;
                let dest = self.new_value(elem);
                self.emit(Inst::AtomicRmw {
                    dest,
                    base: recv,
                    sid,
                    idx,
                    op: AtomicRmwOp::Xchg,
                    val: v,
                    elem,
                    order,
                });
                Ok((dest, elem))
            }
            "add" | "sub" => {
                // `char` is an integer-width cell (#709); it was accepted here as
                // `Ty::I32` before the IR split, so keep it accepted.
                if !matches!(elem, Ty::I8 | Ty::I16 | Ty::I32 | Ty::Char | Ty::I64) {
                    return Err(err(format!(
                        "{name}() is only available on an integer Atomic (not bool or a reference)"
                    )));
                }
                let v = one_arg(self, args)?;
                let op = if name == "add" {
                    AtomicRmwOp::Add
                } else {
                    AtomicRmwOp::Sub
                };
                let dest = self.new_value(elem);
                self.emit(Inst::AtomicRmw {
                    dest,
                    base: recv,
                    sid,
                    idx,
                    op,
                    val: v,
                    elem,
                    order,
                });
                Ok((dest, elem))
            }
            "compare_and_swap" => {
                if args.len() != 2 {
                    return Err(err("compare_and_swap() takes exactly two arguments"));
                }
                let (e, et) = self.expr(&args[0].value)?;
                let expected = self.coerce(e, et, elem);
                let (d, dt) = self.expr(&args[1].value)?;
                let desired = self.coerce(d, dt, elem);
                let dest = self.new_value(Ty::I1);
                self.emit(Inst::AtomicCas {
                    dest,
                    base: recv,
                    sid,
                    idx,
                    expected,
                    desired,
                    elem,
                    success: order,
                    failure: order,
                });
                Ok((dest, Ty::I1))
            }
            other => Err(err(format!(
                "Atomic has no method '{other}' (supported: load, store, add, sub, swap, compare_and_swap)"
            ))),
        }
    }

    /// Whether struct `sid` defines dunder method `dunder` (a registered,
    /// possibly monomorphized, signature exists). Used to decide operator
    /// dispatch — forward on the left, reflected on the right (#1180).
    fn struct_has_dunder(&self, sid: u32, dunder: &str) -> bool {
        let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
        self.sigs.contains_key(&mangled)
    }

    /// Whether struct `sid`'s forward operator dunder accepts an operand of type
    /// `arg` as its right operand (#1200), using the **monomorphized** signature
    /// (`self.sigs`). Parameter 0 is `self` (the left operand); parameter 1 is
    /// the right operand. This is the concrete accept/reject test sema could not
    /// perform for a generic left whose forward parameter is a type variable: by
    /// here the type variable has been substituted, so `self.sigs` holds the
    /// real parameter type. An arity-short (malformed) signature accepts nothing.
    fn forward_param_accepts(&self, sid: u32, dunder: &str, arg: Ty) -> bool {
        let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
        match self.sigs.get(&mangled) {
            Some((ptys, _)) => match ptys.get(1) {
                Some(param) => self.op_param_accepts(arg, *param),
                None => false,
            },
            None => false,
        }
    }

    /// Whether a value of type `arg` may be passed where the operator parameter
    /// type `param` is expected (#1200). Deliberately asymmetric with
    /// [`arg_repr_compatible`](Self::arg_repr_compatible): this drives operator
    /// *selection*, so it must return `false` precisely when the forward dunder
    /// genuinely cannot take the operand — otherwise the fall-through to the
    /// reflected dunder (or the clean error) would be missed, and a mismatched
    /// same-representation operand would be silently reinterpreted (the latent
    /// bug #1200 closes). It therefore rejects two *distinct* struct types even
    /// though both are pointers.
    ///
    /// It returns `false` only on a provable mismatch — a scalar/pointer boundary
    /// crossing, or two unrelated non-interface pointer types (distinct structs).
    /// Everywhere it is unsure (a dynamic `py` operand, a legitimate numeric
    /// widening, a concrete value where an interface parameter is expected, or
    /// any shape not enumerated) it returns `true`, keeping the forward commit so
    /// no sound dispatch is ever rerouted away.
    fn op_param_accepts(&self, arg: Ty, param: Ty) -> bool {
        if arg == param {
            return true;
        }
        // Dynamically-typed values stay permissive (keep forward).
        if matches!(arg, Ty::Py) || matches!(param, Ty::Py) {
            return true;
        }
        // Legitimate numeric widenings (the set `coerce` performs between
        // numbers, including boxing into `bigint`/`complex`): forward accepts.
        let numeric = |t: Ty| {
            t.int_width().is_some() || t.is_float() || matches!(t, Ty::Bigint | Ty::Complex)
        };
        if numeric(arg) && numeric(param) {
            return true;
        }
        // A concrete struct/enum where an interface parameter is expected: the
        // `struct -> interface` fat-pointer coercion (D-0016) is legitimate, so
        // keep the forward commit (conformance was validated upstream).
        if let Ty::Struct(psid) = param {
            if self.ctx.iid_of_fat_sid(psid).is_some() && matches!(arg, Ty::Struct(_) | Ty::Enum(_))
            {
                return true;
            }
        }
        // Provable mismatches → reject (reroute to reflected / clean error):
        //  - a scalar fed into a pointer parameter, or the reverse;
        //  - two *distinct* struct types (same representation, different identity
        //    — the `Duration + Delta` reinterpret).
        if arg.is_ptr() != param.is_ptr() {
            return false;
        }
        if matches!((arg, param), (Ty::Struct(_), Ty::Struct(_))) {
            return false;
        }
        // Anything else (e.g. two different collection shapes) is not something
        // we can prove the forward dunder rejects: keep the forward commit.
        true
    }

    /// Array reduction methods (#507, D-0170): `a.sum()`, `a.min()`, `a.max()`,
    /// `a.mean()`, `a.prod()`, each with an optional `axis=` keyword.
    ///
    /// Without `axis`, the whole array reduces to a scalar: `sum`/`min`/`max`/
    /// `prod` keep the dtype, `mean` yields a float (`f32` source → `f32`, else
    /// `f64`). With `axis=k`, one axis collapses: on a rank-1 array that is the
    /// full (scalar) reduction, and on a rank-`n` (n ≥ 2) array it produces a
    /// new rank-`(n-1)` array (`zap_array_reduce_axis` / `zap_array_mean_axis`).
    /// `min`/`max` raise a catchable `ValueError` on an empty array/axis, and an
    /// out-of-range `axis` raises too, so those forms lower through
    /// `throwing_call`; `sum`/`prod`/`mean` have defined empty results
    /// (`0`/`1`/`nan`) and never raise.
    fn lower_array_method(
        &mut self,
        recv: Value,
        d: Dtype,
        ndim: u8,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // Shape ops (#511): reshape / transpose / flatten are not reductions.
        match name {
            "reshape" => return self.lower_array_reshape(recv, d, ndim, args),
            "transpose" => {
                if !args.is_empty() {
                    return Err(err(
                        "transpose() takes no arguments yet (a permutation argument is a follow-up)",
                    ));
                }
                // Reverse all axes; a view (metadata only), rank preserved.
                let out = Ty::Array(d, ndim);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_transpose".to_string(),
                    args: vec![recv],
                });
                return Ok((dest, out));
            }
            "flatten" => {
                if !args.is_empty() {
                    return Err(err("flatten() takes no arguments"));
                }
                let out = Ty::Array(d, 1); // always rank-1 copy
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_flatten".to_string(),
                    args: vec![recv],
                });
                return Ok((dest, out));
            }
            // Complex support (#938): `a.conj()` -> complex array (conjugate; a
            // real receiver lifts to complex with imag 0), `a.abs()` -> f64 array
            // of magnitudes (|z| for complex, |x| for a real dtype).
            "conj" => {
                if !args.is_empty() {
                    return Err(err("conj() takes no arguments"));
                }
                let out = Ty::Array(Dtype::Complex, ndim);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_conj".to_string(),
                    args: vec![recv],
                });
                return Ok((dest, out));
            }
            "abs" => {
                if !args.is_empty() {
                    return Err(err("abs() takes no arguments"));
                }
                let out = Ty::Array(Dtype::F64, ndim);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_cabs".to_string(),
                    args: vec![recv],
                });
                return Ok((dest, out));
            }
            // Sort (#932): `a.sort(axis=, kind=)` sorts IN PLACE (returns nothing);
            // `a.argsort(axis=, kind=)` returns a new i64 index array.
            "sort" => return self.lower_array_sortish(recv, d, ndim, false, true, args),
            "argsort" => return self.lower_array_sortish(recv, d, ndim, true, false, args),
            // Searching / selection (#934): methods. `a.partition()` sorts in
            // place; the rest return a new array/scalar.
            "argmin" => return self.lower_array_search(recv, d, ndim, "argmin", false, args),
            "argmax" => return self.lower_array_search(recv, d, ndim, "argmax", false, args),
            "searchsorted" => {
                return self.lower_array_search(recv, d, ndim, "searchsorted", false, args)
            }
            "partition" => return self.lower_array_search(recv, d, ndim, "partition", true, args),
            "argpartition" => {
                return self.lower_array_search(recv, d, ndim, "argpartition", false, args)
            }
            _ => {}
        }
        // op code shared with the runtime axis kernel: 0=sum,1=min,2=max,3=prod.
        let op_code = match name {
            "sum" => 0i64,
            "min" => 1,
            "max" => 2,
            "prod" => 3,
            "mean" => 4, // mean has its own entry points (float result)
            other => {
                return Err(err(format!(
                    "'{other}' is not an array method; the array methods are \
                     reductions sum/min/max/mean/prod (optional axis=), shape \
                     ops reshape/transpose/flatten, and conj/abs"
                )))
            }
        };
        let is_mean = op_code == 4;
        let raises = matches!(op_code, 1 | 2); // min/max raise on empty
                                               // The float result dtype for `mean` (f32 source stays f32).
        let mean_dt = if d == Dtype::F32 {
            Dtype::F32
        } else {
            Dtype::F64
        };

        // Parse the single optional `axis=` keyword; no positional args.
        let mut axis: Option<&Expr> = None;
        for a in args {
            match &a.name {
                Some(k) if k == "axis" => {
                    if axis.is_some() {
                        return Err(err(format!("{name}() got axis= more than once")));
                    }
                    axis = Some(&a.value);
                }
                Some(k) => {
                    return Err(err(format!(
                        "{name}() got an unexpected keyword '{k}' (only axis= is supported)"
                    )))
                }
                None => {
                    return Err(err(format!(
                        "array {name}() takes no positional arguments; pass axis= as a keyword"
                    )))
                }
            }
        }

        // No axis, or axis on a rank-1 array -> a full reduction to a scalar.
        if axis.is_none() || ndim == 1 {
            // Evaluate (and discard) an axis expression on a 1-D array so a
            // runtime out-of-range axis still faults; its only valid value is 0
            // or -1, but we keep the full-reduction result either way.
            if let Some(ax) = axis {
                let (av, at) = self.expr(ax)?;
                let av = self.coerce(av, at, Ty::I64);
                self.throwing_call(None, "zap_array_axis_check", vec![recv, av]);
            }
            if is_mean {
                let raw = self.new_value(Ty::F64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: format!("zap_array_mean_{}", d.suffix()),
                    args: vec![recv],
                });
                let res = self.coerce(raw, Ty::F64, mean_dt.ty());
                return Ok((res, mean_dt.ty()));
            }
            let dest = self.new_value(d.ty());
            let func = format!("zap_array_{name}_{}", d.suffix());
            if raises {
                self.throwing_call(Some(dest), &func, vec![recv]);
            } else {
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func,
                    args: vec![recv],
                });
            }
            return Ok((dest, d.ty()));
        }

        // Axis reduction on a rank-(n>=2) array -> a rank-(n-1) array.
        let ax = axis.unwrap();
        let (av, at) = self.expr(ax)?;
        let av = self.coerce(av, at, Ty::I64);
        let out_rank = ndim - 1;
        if is_mean {
            let out_ty = Ty::Array(mean_dt, out_rank);
            let dest = self.new_value(out_ty);
            // Raises ValueError on an out-of-range axis.
            self.throwing_call(Some(dest), "zap_array_mean_axis", vec![recv, av]);
            return Ok((dest, out_ty));
        }
        let out_ty = Ty::Array(d, out_rank);
        let dest = self.new_value(out_ty);
        let opv = self.iconst(Ty::I64, op_code);
        // Raises ValueError on an out-of-range axis (min/max also on an empty axis).
        self.throwing_call(Some(dest), "zap_array_reduce_axis", vec![recv, av, opv]);
        Ok((dest, out_ty))
    }

    /// `sort` / `argsort` over a numeric array (#932, D-0190), as a method
    /// (`a.sort()` in place, `a.argsort()` -> indices) or a builtin (`sort(a)`
    /// copy, `argsort(a)`). `kw` holds the keyword arguments (`axis=`, `kind=`);
    /// the receiver array is already lowered. `in_place` is the sorting method
    /// form; `is_argsort` selects argsort. Unstable (introsort) is the default;
    /// `kind="stable"`/`"mergesort"` selects the stable floor. `axis=` is an int
    /// (default last axis) or `None` (flatten); the runtime raises ValueError on
    /// a bad axis. Complex has no ordering and is rejected.
    fn lower_array_sortish(
        &mut self,
        recv: Value,
        d: Dtype,
        ndim: u8,
        is_argsort: bool,
        in_place: bool,
        kw: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if matches!(d, Dtype::Complex) {
            return Err(err(
                "sort/argsort: complex arrays have no ordering (sort the real/imag parts)",
            ));
        }
        let mut stable = false;
        let mut axis_expr: Option<&Expr> = None;
        for a in kw {
            match a.name.as_deref() {
                Some("kind") => match &a.value {
                    Expr::Str {
                        value,
                        fstring: false,
                        ..
                    } => match value.as_str() {
                        "stable" | "mergesort" => stable = true,
                        "quicksort" | "heapsort" => stable = false,
                        _ => {
                            return Err(err(
                                "sort kind= must be \"quicksort\", \"stable\", \"mergesort\", or \"heapsort\"",
                            ))
                        }
                    },
                    _ => return Err(err("sort kind= must be a string literal")),
                },
                Some("axis") => axis_expr = Some(&a.value),
                Some(k) => {
                    return Err(err(format!(
                        "sort/argsort got an unexpected keyword '{k}' (only axis= and kind=)"
                    )))
                }
                None => {
                    return Err(err(
                        "array sort/argsort takes no extra positional arguments (pass axis=/kind=)",
                    ))
                }
            }
        }
        // axis: None -> flatten; an int -> that axis; absent -> last axis (-1).
        let (flatten, axis_val) = match axis_expr {
            Some(Expr::None(_)) => (true, self.iconst(Ty::I64, 0)),
            Some(e) => {
                let (v, t) = self.expr(e)?;
                (false, self.coerce(v, t, Ty::I64))
            }
            None => (false, self.iconst(Ty::I64, -1)),
        };
        let stable_v = self.iconst(Ty::I64, i64::from(stable));
        if in_place {
            if flatten {
                return Err(err(
                    "a.sort(axis=None) is not supported; use sort(a, axis=None)",
                ));
            }
            self.throwing_call(
                None,
                "zap_array_sort_inplace",
                vec![recv, axis_val, stable_v],
            );
            return Ok((self.iconst(Ty::I32, 0), Ty::I32));
        }
        let flatten_v = self.iconst(Ty::I64, i64::from(flatten));
        let out_dtype = if is_argsort { Dtype::I64 } else { d };
        let out_rank = if flatten { 1 } else { ndim };
        let out = Ty::Array(out_dtype, out_rank);
        let dest = self.new_value(out);
        let func = if is_argsort {
            "zap_array_argsort"
        } else {
            "zap_array_sorted"
        };
        self.throwing_call(Some(dest), func, vec![recv, axis_val, flatten_v, stable_v]);
        Ok((dest, out))
    }

    /// Build a fresh 1-element `array[d]` holding the already-lowered scalar `v`
    /// (used for a scalar `searchsorted` `v` / an int `partition` `kth`).
    fn build_scalar_array(&mut self, d: Dtype, v: Value, t: Ty) -> Value {
        let one = self.iconst(Ty::I64, 1);
        let arr = self.array_alloc_shaped(d, &[one]);
        let zero = self.iconst(Ty::I64, 0);
        let is_float = matches!(d, Dtype::F64 | Dtype::F32);
        let sv = self.coerce(v, t, if is_float { Ty::F64 } else { Ty::I64 });
        self.emit(Inst::Call {
            dest: None,
            func: format!("zap_array_set_{}", d.suffix()),
            args: vec![arr, zero, sv],
        });
        arr
    }

    /// A keyword argument's expression, if present.
    fn kw_arg<'b>(&self, args: &'b [ast::Arg], name: &str) -> Option<&'b Expr> {
        args.iter()
            .find(|a| a.name.as_deref() == Some(name))
            .map(|a| &a.value)
    }

    /// `argmin`/`argmax`/`searchsorted`/`partition`/`argpartition` over a numeric
    /// array (#934), as a method (receiver `recv`) or a builtin (first positional).
    /// `rest` is the remaining arguments. `in_place` marks the `a.partition()`
    /// method (sorts the receiver in place). Complex has no ordering → rejected.
    fn lower_array_search(
        &mut self,
        recv: Value,
        d: Dtype,
        ndim: u8,
        which: &str,
        in_place: bool,
        rest: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if matches!(d, Dtype::Complex) {
            return Err(err(format!(
                "{which}: complex arrays have no ordering (operate on real/imag parts)"
            )));
        }
        let positional: Vec<&Expr> = rest
            .iter()
            .filter(|a| a.name.is_none())
            .map(|a| &a.value)
            .collect();
        match which {
            "argmin" | "argmax" => {
                let is_max = i64::from(which == "argmax");
                let im = self.iconst(Ty::I64, is_max);
                // An axis on a rank-(>=2) array -> a rank-(n-1) index array; no
                // axis, or a rank-1 array -> a flattened scalar index.
                match self.kw_arg(rest, "axis") {
                    Some(ax) if ndim > 1 => {
                        let (v, t) = self.expr(ax)?;
                        let av = self.coerce(v, t, Ty::I64);
                        let out = Ty::Array(Dtype::I64, ndim - 1);
                        let dest = self.new_value(out);
                        self.throwing_call(
                            Some(dest),
                            "zap_array_argminmax_axis",
                            vec![recv, av, im],
                        );
                        Ok((dest, out))
                    }
                    axis => {
                        // A rank-1 axis is validated but still yields a scalar.
                        if let Some(ax) = axis {
                            let (v, t) = self.expr(ax)?;
                            let av = self.coerce(v, t, Ty::I64);
                            self.throwing_call(None, "zap_array_axis_check", vec![recv, av]);
                        }
                        let dest = self.new_value(Ty::I64);
                        self.throwing_call(Some(dest), "zap_array_argminmax", vec![recv, im]);
                        Ok((dest, Ty::I64))
                    }
                }
            }
            "searchsorted" => {
                if positional.len() != 1 {
                    return Err(err("searchsorted expects (v, side=...)"));
                }
                let side = match self.kw_arg(rest, "side") {
                    Some(Expr::Str {
                        value,
                        fstring: false,
                        ..
                    }) => match value.as_str() {
                        "left" => 0,
                        "right" => 1,
                        _ => return Err(err("searchsorted side= must be \"left\" or \"right\"")),
                    },
                    Some(_) => return Err(err("searchsorted side= must be a string literal")),
                    None => 0,
                };
                let sidev = self.iconst(Ty::I64, side);
                let (vv, vt) = self.expr(positional[0])?;
                if let Ty::Array(_, vnd) = vt {
                    let out = Ty::Array(Dtype::I64, vnd);
                    let dest = self.new_value(out);
                    self.emit(Inst::Call {
                        dest: Some(dest),
                        func: "zap_array_searchsorted".to_string(),
                        args: vec![recv, vv, sidev],
                    });
                    Ok((dest, out))
                } else {
                    // scalar v -> a 1-element array[d]; the result's element 0.
                    let varr = self.build_scalar_array(d, vv, vt);
                    let arr = self.new_value(Ty::Array(Dtype::I64, 1));
                    self.emit(Inst::Call {
                        dest: Some(arr),
                        func: "zap_array_searchsorted".to_string(),
                        args: vec![recv, varr, sidev],
                    });
                    let zero = self.iconst(Ty::I64, 0);
                    let dest = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(dest),
                        func: "zap_array_get_i64".to_string(),
                        args: vec![arr, zero],
                    });
                    Ok((dest, Ty::I64))
                }
            }
            "partition" | "argpartition" => {
                if positional.len() != 1 {
                    return Err(err(format!("{which} expects (kth, axis=...)")));
                }
                let (kv, kt) = self.expr(positional[0])?;
                // kth: an int (-> 1-element array[i64]) or an array[i64] sequence.
                let karr = if matches!(kt, Ty::Array(..)) {
                    kv
                } else {
                    self.build_scalar_array(Dtype::I64, kv, kt)
                };
                let axisv = match self.kw_arg(rest, "axis") {
                    Some(e) => {
                        let (v, t) = self.expr(e)?;
                        self.coerce(v, t, Ty::I64)
                    }
                    None => self.iconst(Ty::I64, -1),
                };
                if which == "argpartition" {
                    let out = Ty::Array(Dtype::I64, ndim);
                    let dest = self.new_value(out);
                    self.throwing_call(
                        Some(dest),
                        "zap_array_argpartition",
                        vec![recv, karr, axisv],
                    );
                    Ok((dest, out))
                } else if in_place {
                    self.throwing_call(
                        None,
                        "zap_array_partition_inplace",
                        vec![recv, karr, axisv],
                    );
                    Ok((self.iconst(Ty::I32, 0), Ty::I32))
                } else {
                    let out = Ty::Array(d, ndim);
                    let dest = self.new_value(out);
                    self.throwing_call(
                        Some(dest),
                        "zap_array_partitioned",
                        vec![recv, karr, axisv],
                    );
                    Ok((dest, out))
                }
            }
            _ => Err(err(format!(
                "'{which}' is not an array search/select method"
            ))),
        }
    }

    /// `unique(a, return_index=, return_inverse=, return_counts=)` (#934): the
    /// sorted unique values, or a tuple (values[, index][, inverse][, counts]) in
    /// NumPy's order when any `return_*` is set. Each output recomputes the sort.
    fn lower_unique(&mut self, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        let positional: Vec<&Expr> = args
            .iter()
            .filter(|a| a.name.is_none())
            .map(|a| &a.value)
            .collect();
        if positional.len() != 1 {
            return Err(err(
                "unique() takes the array as its single positional argument",
            ));
        }
        let (recv, rt) = self.expr(positional[0])?;
        let Ty::Array(d, _) = rt else {
            return Err(err("unique() requires a numeric array"));
        };
        if matches!(d, Dtype::Complex) {
            return Err(err("unique: complex arrays have no ordering"));
        }
        let flag = |this: &Self, name: &str| -> Result<bool, LowerError> {
            match this.kw_arg(args, name) {
                Some(Expr::Bool(value, _)) => Ok(*value),
                Some(_) => Err(err(format!("unique {name}= must be True or False"))),
                None => Ok(false),
            }
        };
        let ri = flag(self, "return_index")?;
        let rv = flag(self, "return_inverse")?;
        let rc = flag(self, "return_counts")?;
        let values_ty = Ty::Array(d, 1);
        let values = self.new_value(values_ty);
        self.emit(Inst::Call {
            dest: Some(values),
            func: "zap_array_unique_values".to_string(),
            args: vec![recv],
        });
        if !ri && !rv && !rc {
            return Ok((values, values_ty));
        }
        // Build the tuple (values, [index], [inverse], [counts]) in NumPy's order.
        let mut elems = vec![values_ty];
        let mut vals = vec![values];
        let mut add = |this: &mut Self, func: &str| {
            let v = this.new_value(Ty::Array(Dtype::I64, 1));
            this.emit(Inst::Call {
                dest: Some(v),
                func: func.to_string(),
                args: vec![recv],
            });
            elems.push(Ty::Array(Dtype::I64, 1));
            vals.push(v);
        };
        if ri {
            add(self, "zap_array_unique_index");
        }
        if rv {
            add(self, "zap_array_unique_inverse");
        }
        if rc {
            add(self, "zap_array_unique_counts");
        }
        let sid = self.ctx.tuples.borrow_mut().intern(elems);
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (idx, v) in vals.into_iter().enumerate() {
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: idx as u32,
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// Element-wise `+`/`-`/`*`/`/` on arrays, including scalar broadcast and
    /// mixed-dtype promotion (tickets #49, #506). Emits the [`Inst::ArrayBinOp`]
    /// node codegen vectorizes. Two arrays of different dtypes promote to the
    /// higher dtype ([`Dtype::promote`], so `i64 + f64 -> f64`); a scalar is
    /// weak ([`Dtype::promote_scalar`], so `uint8 + 1` stays `uint8`).
    ///
    /// `scalar op array` (#506): the commutative `+`/`*` are reordered to
    /// `array op scalar` (kernel computes `array[i] op scalar`); the
    /// non-commutative `-`/`/` are kept as scalar-on-left (`scalar_lhs`), so the
    /// kernel computes `scalar op array[i]`.
    ///
    /// `/` is NumPy **true division** (#506): the result dtype is always a float
    /// — an all-integer division lifts to `f64` (`int_array / 2 -> f64`), while a
    /// float operand keeps its width (`f32_array / 2 -> f32`). A zero divisor
    /// yields IEEE ±inf/nan, not a raise (D-0169). Floor division `//` is not yet
    /// supported on arrays.
    fn array_binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let irop = match op {
            Add => BinOp::Add,
            Sub => BinOp::Sub,
            Mul => BinOp::Mul,
            Div => BinOp::Div,
            _ => {
                return Err(err(
                    "only +, -, *, and / are supported on arrays yet (//, reductions beyond sum, etc. are deferred)",
                ))
            }
        };
        // A `complex` operand (a complex array, or a complex scalar against an
        // array) routes to the complex runtime helpers (#938): a complex element
        // is a 16-byte 2×f64 slot, not a scalar SIMD lane, so it never flows
        // through `Inst::ArrayBinOp`.
        let is_cx = |t: Ty| matches!(t, Ty::Complex) || matches!(t, Ty::Array(Dtype::Complex, _));
        if is_cx(lt) || is_cx(rt) {
            return self.complex_array_binop(op, l, lt, r, rt);
        }
        let is_div = matches!(op, Div);
        // NumPy true division always yields a float: an integer elementwise
        // result dtype lifts to `f64` (#506). Float dtypes are unchanged.
        let lift = |d: Dtype| {
            if is_div && !d.is_float() {
                Dtype::F64
            } else {
                d
            }
        };
        let is_scalar = |t: Ty| t.is_float() || t.int_width().is_some();
        match (lt, rt) {
            (Ty::Array(ld, lnd), Ty::Array(rd, rnd)) => {
                // Broadcasting (#501, D-0165): shapes are right-aligned and each
                // aligned axis must be equal, or one side 1 (stretched), or
                // missing (the lower-rank operand gets implicit leading 1s). The
                // result rank is the larger rank (static); the per-axis
                // compatibility and result dims are a runtime concern. A throwing
                // check raises a catchable ValueError on incompatible shapes
                // before the kernel runs; codegen then picks the flat or strided
                // kernel by a runtime same-shape test.
                let rank = lnd.max(rnd);
                self.throwing_call(None, "zap_array_broadcast_check", vec![l, r]);
                let out = lift(Dtype::promote(ld, rd));
                let dest = self.new_value(Ty::Array(out, rank));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: l,
                    lhs_dtype: ld,
                    rhs: r,
                    rhs_dtype: rd,
                    scalar: false,
                    scalar_lhs: false,
                });
                Ok((dest, Ty::Array(out, rank)))
            }
            (Ty::Array(ld, lnd), _) if is_scalar(rt) => {
                // Weak scalar (NEP 50): an int scalar keeps the array's dtype,
                // a float scalar forces `f64`. The scalar crosses to codegen as
                // its natural `i64`/`f64` word (`rhs_dtype`); codegen converts
                // it to `out` (possibly a narrowing `i64 -> u8`).
                let scalar_is_float = rt.is_float();
                let sdt = if scalar_is_float {
                    Dtype::F64
                } else {
                    Dtype::I64
                };
                let s = self.coerce(r, rt, sdt.ty());
                let out = lift(Dtype::promote_scalar(ld, scalar_is_float));
                let dest = self.new_value(Ty::Array(out, lnd));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: l,
                    lhs_dtype: ld,
                    rhs: s,
                    rhs_dtype: sdt,
                    scalar: true,
                    scalar_lhs: false,
                });
                Ok((dest, Ty::Array(out, lnd)))
            }
            (_, Ty::Array(rd, rnd)) if is_scalar(lt) => {
                // The commutative `+`/`*` reorder to `array op scalar` (the
                // kernel computes `array[i] op scalar`); the non-commutative
                // `-`/`/` are kept as scalar-on-left (`scalar_lhs`), so the
                // kernel computes `scalar op array[i]` (#506).
                let scalar_lhs = !matches!(op, Add | Mul);
                let scalar_is_float = lt.is_float();
                let sdt = if scalar_is_float {
                    Dtype::F64
                } else {
                    Dtype::I64
                };
                let s = self.coerce(l, lt, sdt.ty());
                let out = lift(Dtype::promote_scalar(rd, scalar_is_float));
                let dest = self.new_value(Ty::Array(out, rnd));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: r,
                    lhs_dtype: rd,
                    rhs: s,
                    rhs_dtype: sdt,
                    scalar: true,
                    scalar_lhs,
                });
                Ok((dest, Ty::Array(out, rnd)))
            }
            _ => Err(err(
                "unsupported array operand (element-wise ops take two arrays or an array and a numeric scalar)",
            )),
        }
    }

    /// Element-wise `+`/`-`/`*`/`/` where a `complex` operand is involved (#938),
    /// routed to the complex runtime helpers. `array ⊗ array` (either operand real
    /// or complex) → `zap_array_cbinop`; `array ⊗ scalar` → `zap_array_cbinop_cs`
    /// (a real scalar is boxed to a `complex` with a zero imaginary part, so one
    /// helper covers both). The result is always a `complex` array.
    fn complex_array_binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let opcode = match op {
            Add => 0,
            Sub => 1,
            Mul => 2,
            Div => 3,
            _ => return Err(err("only +, -, *, and / are supported on complex arrays")),
        };
        let opc = self.iconst(Ty::I64, opcode);
        let is_scalar = |t: Ty| t.is_float() || t.int_width().is_some() || matches!(t, Ty::Complex);
        // Box a scalar operand into a `complex` value (a real scalar → imag 0).
        let to_complex = |s: &mut FnLower, v: Value, t: Ty| -> Value {
            if matches!(t, Ty::Complex) {
                v
            } else {
                let f = s.coerce(v, t, Ty::F64);
                let zc = s.new_value(Ty::Complex);
                s.emit(Inst::Call {
                    dest: Some(zc),
                    func: "zap_complex_from_f64".to_string(),
                    args: vec![f],
                });
                zc
            }
        };
        match (lt, rt) {
            (Ty::Array(_, lnd), Ty::Array(_, rnd)) => {
                let rank = lnd.max(rnd);
                self.throwing_call(None, "zap_array_broadcast_check", vec![l, r]);
                let out = Ty::Array(Dtype::Complex, rank);
                let d = self.new_value(out);
                self.throwing_call(Some(d), "zap_array_cbinop", vec![l, r, opc]);
                Ok((d, out))
            }
            (Ty::Array(_, lnd), _) if is_scalar(rt) => {
                let zc = to_complex(self, r, rt);
                let scalar_lhs = self.iconst(Ty::I64, 0);
                let out = Ty::Array(Dtype::Complex, lnd);
                let d = self.new_value(out);
                self.throwing_call(Some(d), "zap_array_cbinop_cs", vec![l, zc, opc, scalar_lhs]);
                Ok((d, out))
            }
            (_, Ty::Array(_, rnd)) if is_scalar(lt) => {
                // `-`/`/` are non-commutative, so the scalar stays on the left.
                let scalar_lhs = self.iconst(Ty::I64, i64::from(!matches!(op, Add | Mul)));
                let zc = to_complex(self, l, lt);
                let out = Ty::Array(Dtype::Complex, rnd);
                let d = self.new_value(out);
                self.throwing_call(Some(d), "zap_array_cbinop_cs", vec![r, zc, opc, scalar_lhs]);
                Ok((d, out))
            }
            _ => Err(err(
                "unsupported complex array operand (element-wise ops take two arrays or an array and a numeric scalar)",
            )),
        }
    }

    /// Split a numeric scalar operand into the `(si, sf, is_float)` ABI words the
    /// array comparison kernel expects: a float scalar crosses as `sf` (`f64`)
    /// with `is_float = 1`; an int scalar as `si` (`i64`) with `is_float = 0`.
    /// The unused word is a zero constant.
    fn array_scalar_words(&mut self, v: Value, t: Ty) -> (Value, Value, Value) {
        if t.is_float() {
            let sf = self.coerce(v, t, Ty::F64);
            let si = self.iconst(Ty::I64, 0);
            let isf = self.iconst(Ty::I64, 1);
            (si, sf, isf)
        } else {
            let si = self.coerce(v, t, Ty::I64);
            let sf = self.fconst(Ty::F64, 0.0);
            let isf = self.iconst(Ty::I64, 0);
            (si, sf, isf)
        }
    }

    /// Element-wise comparison `a <op> b` / `a <op> scalar` → `array[bool]`
    /// (#510, D-0175), the mask source for boolean-mask indexing. Array⊗array
    /// broadcasts (result rank = max; the kernel validates shapes and raises a
    /// catchable `ValueError`, so it is a `throwing_call`); array⊗scalar keeps
    /// the array's shape. `scalar_lhs` records a scalar on the left so the kernel
    /// preserves operand order (`3 < a`), which matters for `<`/`<=`/`>`/`>=`.
    fn lower_array_cmp(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let pred: i64 = match op {
            Eq => 0,
            Ne => 1,
            Lt => 2,
            Le => 3,
            Gt => 4,
            Ge => 5,
            _ => return Err(err("internal: non-comparison op in lower_array_cmp")),
        };
        let pred_v = self.iconst(Ty::I64, pred);
        let is_scalar = |t: &Ty| t.is_float() || t.int_width().is_some();
        match (lt, rt) {
            (Ty::Array(_, lnd), Ty::Array(_, rnd)) => {
                let rank = lnd.max(rnd);
                let out = Ty::Array(Dtype::Bool, rank);
                let dest = self.new_value(out);
                self.throwing_call(Some(dest), "zap_array_cmp", vec![l, r, pred_v]);
                Ok((dest, out))
            }
            (Ty::Array(_, lnd), _) if is_scalar(&rt) => {
                let (si, sf, isf) = self.array_scalar_words(r, rt);
                let lhs_flag = self.iconst(Ty::I64, 0);
                let out = Ty::Array(Dtype::Bool, lnd);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_cmp_scalar".to_string(),
                    args: vec![l, si, sf, isf, pred_v, lhs_flag],
                });
                Ok((dest, out))
            }
            (_, Ty::Array(_, rnd)) if is_scalar(&lt) => {
                let (si, sf, isf) = self.array_scalar_words(l, lt);
                let lhs_flag = self.iconst(Ty::I64, 1);
                let out = Ty::Array(Dtype::Bool, rnd);
                let dest = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: "zap_array_cmp_scalar".to_string(),
                    args: vec![r, si, sf, isf, pred_v, lhs_flag],
                });
                Ok((dest, out))
            }
            _ => Err(err(
                "array comparison takes two arrays or an array and a numeric scalar",
            )),
        }
    }

    /// `mask1 & mask2` / `|` / `^` → `array[bool]` (#510, D-0175): combine two
    /// boolean masks (NumPy's logical `&`/`|`/`^` on bool dtype), with
    /// broadcasting. Only `array[bool]` operands are allowed — integer-array
    /// bitwise is a separate element-wise op, outside #510's indexing charter.
    /// The kernel validates broadcast shapes and raises, so it is a
    /// `throwing_call`.
    fn lower_array_logical(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let (Ty::Array(Dtype::Bool, lnd), Ty::Array(Dtype::Bool, rnd)) = (lt, rt) else {
            return Err(err(
                "'&' / '|' / '^' on arrays is only supported between boolean masks \
                 (array[bool]); integer-array bitwise operations are not supported yet",
            ));
        };
        let opc: i64 = match op {
            BitAnd => 0,
            BitOr => 1,
            BitXor => 2,
            _ => return Err(err("internal: non-bitwise op in lower_array_logical")),
        };
        let opv = self.iconst(Ty::I64, opc);
        let rank = lnd.max(rnd);
        let out = Ty::Array(Dtype::Bool, rank);
        let dest = self.new_value(out);
        self.throwing_call(Some(dest), "zap_array_logical", vec![l, r, opv]);
        Ok((dest, out))
    }

    /// Matrix multiply / dot product `a @ b` (and `dot(a, b)`) on numeric arrays
    /// (#509, D-0172, PEP 465). Both operands must be rank-1 or rank-2 arrays;
    /// the result dtype is the NEP-50 promotion of the operand dtypes.
    /// - 1-D · 1-D → a scalar dot product (`zap_array_dot{i,f}`);
    /// - 1-D · 2-D / 2-D · 1-D → a rank-1 array (the vector promoted to a row /
    ///   column and the singleton axis dropped);
    /// - 2-D · 2-D → a rank-2 matrix product.
    ///
    /// A shape mismatch raises a catchable `ValueError` from the runtime, so the
    /// calls are `throwing_call`s. Batched (≥ 3-D) matmul is a follow-up.
    /// `a.reshape(d0, d1, ...)` (#511, D-0174): a view over the compacted buffer
    /// with the given dims, one of which may be the literal `-1` (inferred at
    /// runtime). The result's static rank is the number of dim arguments. A
    /// total-size mismatch raises a catchable `ValueError`, so the reshape call
    /// is a `throwing_call`.
    fn lower_array_reshape(
        &mut self,
        recv: Value,
        d: Dtype,
        _ndim: u8,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        if args.is_empty() {
            return Err(err("reshape() needs at least one dimension"));
        }
        if args.len() > u8::MAX as usize {
            return Err(err("reshape() rank is too large"));
        }
        let mut dims = Vec::with_capacity(args.len());
        for a in args {
            if a.name.is_some() {
                return Err(err("reshape() takes positional integer dimensions"));
            }
            let (v, t) = self.expr(&a.value)?;
            if t.int_width().is_none() {
                return Err(err("reshape() dimensions must be integers"));
            }
            dims.push(self.coerce(v, t, Ty::I64));
        }
        let rank = dims.len() as u8;
        // Build the i64 dims buffer the runtime reads.
        let nd_v = self.iconst(Ty::I64, rank as i64);
        let buf = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(buf),
            func: "zap_i64buf".to_string(),
            args: vec![nd_v],
        });
        for (k, &dv) in dims.iter().enumerate() {
            let kv = self.iconst(Ty::I64, k as i64);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_i64set".to_string(),
                args: vec![buf, kv, dv],
            });
        }
        let out = Ty::Array(d, rank);
        let dest = self.new_value(out);
        // Raises ValueError on a size mismatch / bad -1.
        self.throwing_call(Some(dest), "zap_array_reshape", vec![recv, nd_v, buf]);
        Ok((dest, out))
    }

    /// `concatenate([a, …], axis=k)` / `stack([a, …], axis=k)` (#511, D-0174):
    /// join a `list` of arrays. The list is passed straight to the runtime (it
    /// is already GC-traced). `concatenate` keeps the input rank; `stack` adds a
    /// new axis (rank + 1). The result's static rank/dtype come from the list's
    /// element type `array[T]`. Raises `ValueError` on a shape/dtype/axis fault.
    fn lower_array_join(
        &mut self,
        fname: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let mut list_arg: Option<&Expr> = None;
        let mut axis_arg: Option<&Expr> = None;
        for a in args {
            match &a.name {
                None if list_arg.is_none() => list_arg = Some(&a.value),
                Some(k) if k == "axis" => axis_arg = Some(&a.value),
                _ => {
                    return Err(err(format!(
                        "{fname}() takes a list of arrays and an optional axis="
                    )))
                }
            }
        }
        let Some(list_expr) = list_arg else {
            return Err(err(format!("{fname}() needs a list of arrays")));
        };
        // A `list[array[T]]` erases its element dtype/rank to a pointer in the
        // collection ABI, so the result rank/dtype can't come from the list
        // type. Require a list literal and read them from the elements
        // directly, lowering each exactly once into a fresh pointer list.
        let Expr::List(elems, _) = list_expr else {
            return Err(err(format!(
                "{fname}() needs a list literal of arrays, e.g. {fname}([a, b])"
            )));
        };
        if elems.is_empty() {
            return Err(err(format!("{fname}() needs at least one array")));
        }
        let listv = self.new_value(Ty::List(Elem::Ptr));
        self.emit(Inst::Call {
            dest: Some(listv),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let mut dr: Option<(Dtype, u8)> = None;
        for e in elems {
            let (v, t) = self.expr(e)?;
            let Ty::Array(d, ndim) = t else {
                return Err(err(format!("{fname}() elements must be arrays")));
            };
            if dr.is_none() {
                dr = Some((d, ndim));
            }
            self.emit(Inst::Call {
                dest: None,
                func: "zap_list_push_ptr".to_string(),
                args: vec![listv, v],
            });
        }
        let (d, ndim) = dr.unwrap();
        // concatenate keeps the rank; stack adds a new axis.
        let out_rank = if fname == "stack" { ndim + 1 } else { ndim };
        let axis = match axis_arg {
            Some(e) => {
                let (v, t) = self.expr(e)?;
                if t.int_width().is_none() {
                    return Err(err(format!("{fname}() axis must be an integer")));
                }
                self.coerce(v, t, Ty::I64)
            }
            None => self.iconst(Ty::I64, 0),
        };
        let func = if fname == "stack" {
            "zap_array_stack"
        } else {
            "zap_array_concat"
        };
        let out = Ty::Array(d, out_rank);
        let dest = self.new_value(out);
        self.throwing_call(Some(dest), func, vec![listv, axis]);
        Ok((dest, out))
    }

    fn lower_array_matmul(
        &mut self,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        let (Ty::Array(ld, lnd), Ty::Array(rd, rnd)) = (lt, rt) else {
            return Err(err(
                "'@' (matrix multiply) requires two arrays; a scalar does not define '@'",
            ));
        };
        if lnd > 2 || rnd > 2 {
            return Err(err(
                "'@' (matrix multiply) supports only 1-D and 2-D arrays (batched matmul is a follow-up)",
            ));
        }
        let out = Dtype::promote(ld, rd);
        if lnd == 1 && rnd == 1 {
            // Dot product -> scalar. Accumulate in the wide word, then narrow to
            // the promoted dtype (like `sum`).
            let is_float = out.is_float();
            let (func, raw_ty) = if is_float {
                ("zap_array_dotf", Ty::F64)
            } else {
                ("zap_array_doti", Ty::I64)
            };
            let raw = self.new_value(raw_ty);
            self.throwing_call(Some(raw), func, vec![l, r]);
            let res = self.coerce(raw, raw_ty, out.ty());
            return Ok((res, out.ty()));
        }
        // An array result: rank 2 only when both operands are 2-D, else rank 1.
        let rank = if lnd == 2 && rnd == 2 { 2u8 } else { 1u8 };
        let out_ty = Ty::Array(out, rank);
        let dest = self.new_value(out_ty);
        let code = self.iconst(Ty::I64, out.code());
        self.throwing_call(Some(dest), "zap_array_matmul", vec![l, r, code]);
        Ok((dest, out_ty))
    }
}

fn is_name(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Name(n, _) if n == name)
}

/// True for a statically-negative integer literal — `Unary { Neg, Int }`, e.g.
/// `-5` (#1083). The lexer never folds the sign into the literal, so a bare
/// `Expr::Int` is always non-negative. Used to reject a negative `set.reserve`
/// hint at lowering (set methods are loosely typed in sema).
fn is_negative_int_literal(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Unary { op: AstUn::Neg, expr, .. } if matches!(expr.as_ref(), Expr::Int(_, _))
    )
}

/// Stable runtime ids for the built-in exception hierarchy (I#471). Keep these
/// in sync with the `ZAP_EXC_*` constants in `zaprt.c` and the table in §10.
fn exception_type_id_name(name: &str) -> Option<i64> {
    Some(match name {
        "BaseException" => 1,
        "Exception" => 2,
        "ValueError" => 3,
        "TypeError" => 4,
        "LookupError" => 5,
        "IndexError" => 6,
        "KeyError" => 7,
        "ArithmeticError" => 8,
        "ZeroDivisionError" => 9,
        "RuntimeError" => 10,
        "StopIteration" => 11,
        "Cancelled" => 12,
        "ExceptionGroup" => 13,
        "ScopeTimeout" => 14,
        "TimeoutError" => 15,
        "AssertionError" => 16,
        "EOFError" => 17,
        "MemoryError" => 18,
        "OSError" => 19,
        "FileNotFoundError" => 20,
        _ => return None,
    })
}

fn exception_type_id(ty: &ast::Type) -> Option<i64> {
    match ty {
        ast::Type::Named { name, args, .. } if args.is_empty() => {
            Some(exception_type_id_name(name).unwrap_or_else(|| custom_exception_type_id(name)))
        }
        _ => None,
    }
}

/// Scope-key under which a caught exception's *object* (not its message string)
/// is stashed alongside the ordinary `except ... as e` binding, so attribute
/// access `e.cause` / a later `raise ... from e` can reach the real exception
/// object (#604, D-0103). The embedded NUL keeps it from colliding with any
/// user-typable identifier.
fn exc_obj_key(name: &str) -> String {
    format!("\u{0}exc\u{0}{name}")
}

/// Stable per-name ids for user exception types. They use a disjoint range from
/// the built-ins and have `Exception` as their implicit parent in the runtime.
fn custom_exception_type_id(name: &str) -> i64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    ((hash & 0x1fff_ffff_ffff_ffff) + 1024) as i64
}

/// The free variables of a lambda body (I#163), in first-seen order: every name
/// referenced in `body` that is not a lambda parameter and not bound by an
/// inner binder (a nested lambda's parameters, a comprehension's loop variable).
/// The caller keeps only those that are actually bound in the enclosing scope;
/// the rest (top-level functions, imports, builtins) resolve directly in the
/// lifted body.
fn free_names_of(params: &[String], body: &Expr) -> Vec<String> {
    let mut bound: HashSet<String> = params.iter().cloned().collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    collect_free_names(body, &mut bound, &mut out, &mut seen);
    out
}

/// Build the body of a generator expression's anonymous generator function
/// (#596): the comprehension clause list `(elem for CLAUSES)` becomes nested
/// `for`/`if` statements wrapping an innermost `yield`. Clauses nest outer to
/// inner (left to right), and each clause's `if` filters nest inside its own
/// loop, filtering the clauses to its left — matching comprehension semantics
/// (#599).
fn build_genexpr_body(
    clauses: &[CompClause],
    yield_stmt: Stmt,
    span: zapc_syntax::span::Span,
) -> ast::Block {
    let mut body = ast::Block {
        stmts: vec![yield_stmt],
        span,
    };
    for c in clauses.iter().rev() {
        // The `if` filters live inside this clause's loop (innermost last).
        for cond in c.ifs.iter().rev() {
            body = ast::Block {
                stmts: vec![Stmt::If {
                    branches: vec![(cond.clone(), body)],
                    else_block: None,
                    span,
                }],
                span,
            };
        }
        body = ast::Block {
            stmts: vec![Stmt::For {
                target: Expr::Name(c.var.clone(), span),
                iter: (*c.iter).clone(),
                body,
                span,
            }],
            span,
        };
    }
    body
}

/// Walk `e`, recording names that are free given the currently-`bound` set.
/// A binder (nested lambda, comprehension loop var) extends `bound` only for the
/// sub-expressions in its scope, then restores it.
/// Collect free names across a comprehension clause list (#599), binding each
/// clause's loop variable before the clauses to its right (its `iter` is
/// evaluated with the earlier vars in scope). Returns the variables this call
/// freshly bound, so the caller can unbind them after visiting the body.
fn collect_free_names_comp_clauses(
    clauses: &[CompClause],
    bound: &mut HashSet<String>,
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) -> Vec<String> {
    let mut fresh = Vec::new();
    for c in clauses {
        collect_free_names(&c.iter, bound, out, seen);
        if bound.insert(c.var.clone()) {
            fresh.push(c.var.clone());
        }
        for f in &c.ifs {
            collect_free_names(f, bound, out, seen);
        }
    }
    fresh
}

fn unbind_comp_clauses(fresh: &[String], bound: &mut HashSet<String>) {
    for v in fresh {
        bound.remove(v);
    }
}

fn collect_free_names(
    e: &Expr,
    bound: &mut HashSet<String>,
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match e {
        Expr::Name(n, _) => {
            if !bound.contains(n) && seen.insert(n.clone()) {
                out.push(n.clone());
            }
        }
        Expr::Comptime { expr, .. } => collect_free_names(expr, bound, out, seen),
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Imaginary(..)
        | Expr::Str { .. }
        | Expr::Bool(..)
        | Expr::None(_) => {}
        Expr::Tuple(es, _) | Expr::List(es, _) | Expr::Set(es, _) => {
            for s in es {
                collect_free_names(s, bound, out, seen);
            }
        }
        Expr::Starred { value, .. } => collect_free_names(value, bound, out, seen),
        // `(name := value)` binds `name` locally (#600): collect the operand's
        // free names, then mark the target bound so later uses are not captured.
        Expr::Named { name, value, .. } => {
            collect_free_names(value, bound, out, seen);
            bound.insert(name.clone());
        }
        Expr::Dict(items, _) => {
            for it in items {
                match it {
                    DictItem::Pair(k, v) => {
                        collect_free_names(k, bound, out, seen);
                        collect_free_names(v, bound, out, seen);
                    }
                    DictItem::Spread(e) => collect_free_names(e, bound, out, seen),
                }
            }
        }
        Expr::ListComp { elem, clauses, .. }
        | Expr::SetComp { elem, clauses, .. }
        | Expr::GenExp { elem, clauses, .. } => {
            // Each clause's `iter` sees the previous clauses' vars; `elem` and
            // the filters see all of them (#599). A genexpr (#596) captures its
            // enclosing free variables the same way.
            let fresh = collect_free_names_comp_clauses(clauses, bound, out, seen);
            collect_free_names(elem, bound, out, seen);
            unbind_comp_clauses(&fresh, bound);
        }
        Expr::DictComp {
            key,
            value,
            clauses,
            ..
        } => {
            let fresh = collect_free_names_comp_clauses(clauses, bound, out, seen);
            collect_free_names(key, bound, out, seen);
            collect_free_names(value, bound, out, seen);
            unbind_comp_clauses(&fresh, bound);
        }
        Expr::Unary { expr, .. } | Expr::ChanRecv { chan: expr, .. } => {
            collect_free_names(expr, bound, out, seen);
        }
        Expr::Binary { lhs, rhs, .. } => {
            collect_free_names(lhs, bound, out, seen);
            collect_free_names(rhs, bound, out, seen);
        }
        Expr::Compare { first, rest, .. } => {
            collect_free_names(first, bound, out, seen);
            for (_, operand) in rest {
                collect_free_names(operand, bound, out, seen);
            }
        }
        Expr::Call { callee, args, .. } => {
            collect_free_names(callee, bound, out, seen);
            for a in args {
                collect_free_names(&a.value, bound, out, seen);
            }
        }
        Expr::Index { base, index, .. } => {
            collect_free_names(base, bound, out, seen);
            collect_free_names(index, bound, out, seen);
        }
        Expr::Slice {
            base, lo, hi, step, ..
        } => {
            collect_free_names(base, bound, out, seen);
            for o in [lo, hi, step].into_iter().flatten() {
                collect_free_names(o, bound, out, seen);
            }
        }
        Expr::SliceSpec { lo, hi, step, .. } => {
            for o in [lo, hi, step].into_iter().flatten() {
                collect_free_names(o, bound, out, seen);
            }
        }
        Expr::Attr { base, .. } => collect_free_names(base, bound, out, seen),
        Expr::Lambda { params, body, .. } => {
            // Nested lambda: its own parameters shadow within its body.
            let added: Vec<String> = params
                .iter()
                .filter(|p| bound.insert((*p).clone()))
                .cloned()
                .collect();
            collect_free_names(body, bound, out, seen);
            for p in added {
                bound.remove(&p);
            }
        }
        Expr::RegexMatch { subject, .. } | Expr::RegexSubst { subject, .. } => {
            collect_free_names(subject, bound, out, seen);
        }
        Expr::Yield { value, .. } => {
            if let Some(v) = value {
                collect_free_names(v, bound, out, seen);
            }
        }
        Expr::IfElse {
            cond, then, els, ..
        } => {
            collect_free_names(cond, bound, out, seen);
            collect_free_names(then, bound, out, seen);
            collect_free_names(els, bound, out, seen);
        }
    }
}

/// Comparison operators for which power-assert shows both operand values
/// (D-0054). Membership (`in`) and the boolean connectives are deliberately
/// excluded — their operand values read poorly side-by-side.
fn is_power_comparison(op: AstBin) -> bool {
    matches!(
        op,
        AstBin::Eq | AstBin::Ne | AstBin::Lt | AstBin::Le | AstBin::Gt | AstBin::Ge
    )
}

/// `StopIteration` or `StopIteration()` — the iterator exhaustion signal
/// (D-0048), accepted in either the bare-name or called form.
fn is_stop_iteration(e: &Expr) -> bool {
    match e {
        Expr::Name(n, _) => n == "StopIteration",
        Expr::Call { callee, .. } => is_name(callee, "StopIteration"),
        _ => false,
    }
}

/// The ZIR target type of an explicit numeric conversion call, by builtin name
/// (`int32(x)`, `float64(x)`, ...). `None` for any other callee.
fn numeric_conv_ty(name: &str) -> Option<Ty> {
    match name {
        "int32" | "i32" => Some(Ty::I32),
        "int64" | "i64" => Some(Ty::I64),
        // Unsigned conversions (#552). `usize` is pointer-width → `u64`.
        "uint8" | "u8" => Some(Ty::U8),
        "uint16" | "u16" => Some(Ty::U16),
        "uint32" | "u32" => Some(Ty::U32),
        "uint64" | "u64" => Some(Ty::U64),
        "usize" => Some(Ty::U64),
        "float32" | "f32" => Some(Ty::F32),
        "float64" | "f64" => Some(Ty::F64),
        _ => None,
    }
}

/// The regex flag bitmask passed to the runtime. Must stay in sync with the
/// `ZAP_RE_*` constants in `zaprt.c`. Unknown flag letters are ignored.
fn regex_flag_bits(flags: &str) -> i64 {
    let mut bits = 0i64;
    for c in flags.chars() {
        bits |= match c {
            'i' => 1,  // caseless
            'm' => 2,  // multiline ^$
            's' => 4,  // dotall
            'x' => 8,  // extended
            'g' => 16, // global (substitute-all / match-all)
            _ => 0,
        };
    }
    bits
}

/// Take exactly `n` hex digits from `chars`, returning their value, or `None`
/// if fewer than `n` hex digits are available (a malformed `\x`/`\u`/`\U`).
fn take_hex(chars: &mut std::str::Chars<'_>, n: usize) -> Option<u32> {
    let mut v: u32 = 0;
    for _ in 0..n {
        v = v * 16 + chars.next()?.to_digit(16)?;
    }
    Some(v)
}

/// Encode a Unicode codepoint as UTF-8 into `out`, erroring on a surrogate or an
/// out-of-range value (CPython raises at compile time; Zap `str` is UTF-8 and
/// cannot hold a lone surrogate).
fn push_codepoint(out: &mut Vec<u8>, cp: u32) -> Result<(), LowerError> {
    let ch = char::from_u32(cp).ok_or_else(|| {
        err(format!(
            "invalid Unicode escape: U+{cp:04X} is not a valid codepoint \
             (surrogates and values above U+10FFFF are not allowed)"
        ))
    })?;
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    Ok(())
}

/// A compile-time UPPER BOUND, in bytes, on the rendered width of one f-string
/// interpolation of static type `vt` (#1081), or `None` when the width is not
/// statically bounded. Used by `lower_fstring` to decide a `StrBuild`'s `bounded`
/// flag, which in turn lets codegen drop the GC root of a never-spilling stack
/// string. SOUNDNESS: the returned width must be a true upper bound on what
/// `render_str` can emit — over-estimating only forgoes the optimization, but an
/// under-estimate would let a run-time heap spill go unrooted (a use-after-free),
/// so anything uncertain (a float, an enum name, any spec'd / padded field, a
/// nested string) returns `None`.
fn fstring_render_max_len(vt: Ty, has_spec: bool) -> Option<u64> {
    if has_spec {
        // A format spec may pad to an arbitrary width / precision, so the result
        // is not statically bounded by the value's natural render.
        return None;
    }
    match vt {
        // Integers: the widest 64-bit render is 20 chars (i64::MIN is
        // "-9223372036854775808"; u64::MAX is "18446744073709551615"). 24 leaves
        // headroom for any sign / radix prefix without risking an under-estimate.
        Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 => Some(24),
        // Booleans render as "true" / "false".
        Ty::I1 => Some(8),
        // A single character is at most 4 UTF-8 bytes.
        Ty::Char => Some(4),
        // Floats, enum variant names, strings, bytes, structs, and everything
        // else have no tight static upper bound.
        _ => None,
    }
}

/// Process the escape sequences in a (non-raw) string or bytes literal (#374,
/// #751/#781). Besides the simple escapes, `\xHH` (two hex digits), `\uHHHH`,
/// and `\UHHHHHHHH` are decoded. In a `str` these denote a Unicode codepoint
/// (UTF-8 encoded); in a `bytes` literal `\xHH` is the raw byte and `\u`/`\U`
/// are not special (the backslash is kept, as in CPython). A malformed hex
/// escape is a compile error. (`\N{NAME}` named escapes are not yet supported;
/// they pass through unchanged.)
fn unescape(s: &str, is_bytes: bool) -> Result<Vec<u8>, LowerError> {
    let mut out = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('r') => out.push(b'\r'),
            Some('0') => out.push(0),
            Some('\\') => out.push(b'\\'),
            Some('\'') => out.push(b'\''),
            Some('"') => out.push(b'"'),
            // `\xHH`: a raw byte in a bytes literal, the codepoint U+00HH in a str.
            Some('x') => {
                let v = take_hex(&mut chars, 2)
                    .ok_or_else(|| err("invalid \\x escape: expected two hex digits"))?;
                if is_bytes {
                    out.push(v as u8);
                } else {
                    push_codepoint(&mut out, v)?;
                }
            }
            // `\uHHHH` / `\UHHHHHHHH`: a Unicode codepoint (str only). In a bytes
            // literal these are not escapes, so the backslash is kept.
            Some('u') if !is_bytes => {
                let cp = take_hex(&mut chars, 4)
                    .ok_or_else(|| err("invalid \\u escape: expected four hex digits"))?;
                push_codepoint(&mut out, cp)?;
            }
            Some('U') if !is_bytes => {
                let cp = take_hex(&mut chars, 8)
                    .ok_or_else(|| err("invalid \\U escape: expected eight hex digits"))?;
                push_codepoint(&mut out, cp)?;
            }
            Some(other) => {
                out.push(b'\\');
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    Ok(out)
}

/// Compute a value struct's C-style bitfield packing (I#173, D-0068), or `None`
/// if it is not a packed bitfield struct. A struct packs iff it is a `value
/// struct` and **every** field declares a bit width (sema enforces that
/// all-or-nothing rule and the per-field limits, so by the time we lower a
/// struct that got here is valid). Fields pack least-significant-first in
/// declaration order into the smallest `i8`/`i16`/`i32`/`i64` backing integer
/// that holds their total width.
fn packed_layout_of(s: &ast::StructDef) -> Option<PackedLayout> {
    if !s.is_value || s.fields.is_empty() || !s.fields.iter().all(|f| f.bits.is_some()) {
        return None;
    }
    let mut offset: u32 = 0;
    let mut bfs = Vec::with_capacity(s.fields.len());
    for f in &s.fields {
        let width = f.bits.unwrap_or(1).max(1); // sema guarantees Some and >= 1
        bfs.push(BitField {
            offset,
            width,
            signed: bitfield_is_signed(&f.ty),
        });
        offset = offset.saturating_add(width);
    }
    // Defensive: a valid struct totals <= 64 bits (sema rejects more). If an
    // invalid one slips past, do not pack rather than emit an out-of-range slice.
    if offset > 64 {
        return None;
    }
    let backing = if offset <= 8 {
        Ty::I8
    } else if offset <= 16 {
        Ty::I16
    } else if offset <= 32 {
        Ty::I32
    } else {
        Ty::I64
    };
    Some(PackedLayout {
        backing,
        fields: bfs,
    })
}

/// Whether a bitfield field reads back **sign-extended**: a signed integer
/// (`int32`/`int64`). `bool` and tier-1 enums are unsigned (zero-extended).
fn bitfield_is_signed(ty: &ast::Type) -> bool {
    matches!(
        ty,
        ast::Type::Named { name, .. }
            if matches!(name.as_str(), "int32" | "int64" | "i32" | "i64")
    )
}

/// Evaluate a tier-1 enum discriminant expression (`GREEN = 10`), which the
/// grammar allows to be an integer literal, optionally signed. Richer constant
/// expressions are a follow-up.
fn const_int_value(e: &ast::Expr) -> Option<i64> {
    match e {
        ast::Expr::Int(s, _) => Some(parse_int_lit(s)),
        ast::Expr::Unary {
            op: ast::UnOp::Neg,
            expr,
            ..
        } => const_int_value(expr).map(|v| -v),
        ast::Expr::Unary {
            op: ast::UnOp::Pos,
            expr,
            ..
        } => const_int_value(expr),
        _ => None,
    }
}

fn parse_int_lit(s: &str) -> i64 {
    parse_int_lit_opt(s).unwrap_or(0)
}

fn parse_int_lit_opt(s: &str) -> Option<i64> {
    let clean: String = s.chars().filter(|c| *c != '_').collect();
    if let Some(h) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        i64::from_str_radix(h, 16).ok()
    } else if let Some(o) = clean
        .strip_prefix("0o")
        .or_else(|| clean.strip_prefix("0O"))
    {
        i64::from_str_radix(o, 8).ok()
    } else if let Some(bn) = clean
        .strip_prefix("0b")
        .or_else(|| clean.strip_prefix("0B"))
    {
        i64::from_str_radix(bn, 2).ok()
    } else {
        clean.parse::<i64>().ok()
    }
}

#[cfg(test)]
mod tests;
