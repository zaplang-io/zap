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
}

impl TupleInterner {
    fn new(base: u32) -> TupleInterner {
        TupleInterner {
            base,
            layouts: Vec::new(),
            map: HashMap::new(),
        }
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
    /// Registered enums (tier-1 and tier-3), keyed by type name.
    enums: HashMap<String, EnumLow>,
    /// Python import aliases (ticket #27, D-0012): the in-scope name
    /// (`import python "numpy" as np` → `np`) mapped to the CPython module name
    /// (`numpy`). A reference to one of these names lowers to a `zap_py_import`.
    py: HashMap<String, String>,
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
    m.params
        .iter()
        .skip(1)
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
/// order (interface declaration order).
#[derive(Clone)]
struct InterfaceLow {
    name: String,
    fat_sid: u32,
    methods: Vec<IfaceMethodLow>,
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
    // Prepend the built-in prelude (`Signal` enum, `MemStats` struct) so the
    // signals surface lowers through the ordinary enum/struct paths. Must match
    // the set sema sees (both call the same helper), ticket #77.
    let module = zapc_syntax::parser::with_prelude(module);
    // Monomorphize user generics to concrete declarations (I#154, D-0017/D-0064)
    // before any id/type collection runs, so the rest of lowering and all of
    // codegen only ever see concrete types.
    let module = match crate::mono::monomorphize(&module) {
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
                interfaces.push(InterfaceLow {
                    name: i.name.clone(),
                    fat_sid,
                    methods,
                });
            }
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

    let mut ctx = StructCtx {
        ids,
        names,
        fields,
        value_ids,
        enums,
        py,
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
    };

    // Signatures: top-level functions and struct methods (mangled `S__m`).
    let mut sigs: HashMap<String, (Vec<Ty>, Ty)> = HashMap::new();
    // Per-function parameter metadata for default/keyword argument assembly
    // (D-0038, I#298). Collected for free functions, whose calls go through the
    // name-dispatch path that honors defaults and keyword reordering.
    let mut call_params: HashMap<String, Vec<ParamInfo>> = HashMap::new();
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
                    // `self` when it assembles the call.
                    call_params.insert(mangled.clone(), method_param_info(m));
                    sigs.insert(mangled, (params, ret));
                }
            }
            // Tier-3 enum methods (D-0046; used by generic `Optional[T]` etc.,
            // I#154). A tier-3 enum lowers to a tagged struct, so `self` is that
            // struct and a method call dispatches through the ordinary
            // struct-method path. Tier-1 enum methods are not lowered yet.
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
                        // for struct methods above — `self` dropped.
                        call_params.insert(mangled.clone(), method_param_info(m));
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
    let deco = DecoratorPlan::collect(module, &ctx, &mut errors);
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
            if ctx.has_globals() {
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
                    Ok(func) => out.funcs.push(func),
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
                    match lower_fn(m, name, Some(sid), &sigs, &ctx, &[], &[], &[]) {
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
                        match lower_fn(m, name, Some(sid), &sigs, &ctx, &[], &[], &[]) {
                            Ok(func) => out.funcs.push(func),
                            Err(e) => errors.push(e),
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
            // `uint8`/`u8` is currently only an `array[T]` dtype (ticket #49),
            // not a general scalar type; it resolves to `i8` so `dtype_of`
            // recognizes it in a type-annotation `array[uint8]`.
            "uint8" | "u8" => Ok(Ty::I8),
            "bool" => Ok(Ty::I1),
            // `char` is a distinct IR type (#709): physically an `i32` codepoint,
            // but kept apart from `int32` so the value layer can render it
            // Python-style.
            "char" => Ok(Ty::Char),
            "str" => Ok(Ty::Ptr),
            "bytes" => Ok(Ty::Bytes),
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
                Ok(Ty::Array(d))
            }
            "dict" => {
                let k = match args.first() {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                let v = match args.get(1) {
                    Some(a) => elem_of(a, env)?,
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
                // picks the typed send/recv runtime entry point.
                let e = match args.first() {
                    Some(a) => elem_of(a, env)?,
                    None => Elem::I64,
                };
                Ok(Ty::Chan(e))
            }
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
        Ty::Enum(id) => Ok(Elem::Enum(id)),
        Ty::Ptr => Ok(Elem::Ptr),
        other => Err(err(format!(
            "heap[{other}] is not supported yet (only integer and str elements)"
        ))),
    }
}

/// The array dtype for a `array[T]` element type (tickets #24, #49). Supported
/// dtypes: `f64`, `i64`, `uint8` (image data), and `bool` (masks). Other
/// numeric dtypes (`f32`, `i32`) are deferred.
fn dtype_of(t: &ast::Type, env: &TyEnv) -> Result<Dtype, LowerError> {
    match resolve_ty(t, env)? {
        Ty::F64 => Ok(Dtype::F64),
        Ty::I64 => Ok(Dtype::I64),
        Ty::I8 => Ok(Dtype::Uint8),
        Ty::I1 => Ok(Dtype::Bool),
        other => Err(err(format!(
            "array[{other}] is not supported yet (only array[f64], array[i64], array[uint8], array[bool])"
        ))),
    }
}

/// The array dtype named by a construction subscript like `array[f64]`, where
/// the dtype appears as a bare name in expression position.
fn dtype_from_name(name: &str) -> Result<Dtype, LowerError> {
    match name {
        "float" | "float64" | "f64" => Ok(Dtype::F64),
        "int" | "int64" | "i64" => Ok(Dtype::I64),
        "uint8" | "u8" => Ok(Dtype::Uint8),
        "bool" => Ok(Dtype::Bool),
        other => Err(err(format!(
            "array[{other}] is not supported yet (only array[f64], array[i64], array[uint8], array[bool])"
        ))),
    }
}

/// The channel element kind named by a construction subscript like
/// `channel[int64]`, where the element type appears as a bare name in
/// expression position (ticket #25, D-0031). Integers/bools become an `i64`
/// word; reference types (`str`, structs, collections) become a pointer word.
fn chan_elem_from_name(name: &str) -> Result<Elem, LowerError> {
    match name {
        "int" | "int64" | "i64" | "int32" | "i32" | "bool" | "char" => Ok(Elem::I64),
        "float" | "float64" | "f64" | "float32" | "f32" => Err(err(
            "channels of floating-point elements are not supported yet",
        )),
        // `str`, `bytes`, struct, and nested-collection elements are references.
        _ => Ok(Elem::Ptr),
    }
}

/// Classify a ZIR type into the element word kind collections store. Elements
/// are uniform 8-byte words (RUNTIME-ABI §"Collection layout"): integers/bools
/// go in as `i64`, reference types as pointers. Floats need a separate word
/// path and are deferred.
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
        Ty::List(_) | Ty::Dict(..) | Ty::Set(_) | Ty::Heap(_) | Ty::Chan(_) => {
            Ok(Elem::Nested(nested.borrow_mut().intern(t)))
        }
        Ty::Ptr | Ty::Bytes | Ty::Array(_) | Ty::Match | Ty::Func(_) | Ty::Py => Ok(Elem::Ptr),
        Ty::Bigint | Ty::Complex => Err(err(
            "collections of bigint or complex values are not supported yet",
        )),
        Ty::F32 | Ty::F64 => Err(err(
            "collections of floating-point elements are not supported yet",
        )),
        Ty::Void => Err(err("a collection element cannot be void")),
    }
}

/// Like [`elem_kind`], but admits an `f64` *list* element (I#569) as [`Elem::F64`].
/// `f32` list elements (ticket #49) and float elements of other collections stay
/// unsupported, so this is used only on the `list[T]` element paths.
fn list_elem_kind(t: Ty, nested: &RefCell<NestedInterner>) -> Result<Elem, LowerError> {
    match t {
        Ty::F64 => Ok(Elem::F64),
        other => elem_kind(other, nested),
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
    }
}

/// A lowering error carrying a specific diagnostic code (most lowering errors
/// use the generic [`err`] / ZAP6001 umbrella; a few, like the bare-`raise`
/// re-raise-outside-handler case, ZAP6002, have a dedicated code).
fn err_code(code: zapc_diag::ErrorCode, m: impl Into<String>) -> LowerError {
    LowerError {
        code,
        message: m.into(),
    }
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
/// unchanged.
const DECORATED_NO_CODE: &[&str] = &["deprecated"];

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

impl DecoratorPlan {
    fn collect(
        module: &ast::Module,
        ctx: &StructCtx,
        errors: &mut Vec<LowerError>,
    ) -> DecoratorPlan {
        // Names declared as metadata (annotations) or behavioral decorators.
        // These are what the reflection registry records; the built-in
        // compile-time decorators (`@derive` etc.) leave no runtime trace.
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
        let bit = gen_field_eq(&mut b, a, c, *fty)?;
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

/// Compare one field of a derived `eq`, returning an `i1`.
fn gen_field_eq(b: &mut FnLower, a: Value, c: Value, fty: Ty) -> Result<Value, LowerError> {
    let bit = b.new_value(Ty::I1);
    match fty {
        Ty::Ptr => {
            // Strings: content equality via the runtime (returns 1/0 as i64).
            let word = b.new_value(Ty::I64);
            b.emit(Inst::Call {
                dest: Some(word),
                func: "zap_str_eq".to_string(),
                args: vec![a, c],
            });
            let zero = b.iconst(Ty::I64, 0);
            b.emit(Inst::ICmp {
                dest: bit,
                pred: CmpPred::Ne,
                lhs: word,
                rhs: zero,
            });
        }
        Ty::F32 | Ty::F64 => {
            b.emit(Inst::FCmp {
                dest: bit,
                pred: CmpPred::Eq,
                lhs: a,
                rhs: c,
            });
        }
        t if t.int_width().is_some() => {
            b.emit(Inst::ICmp {
                dest: bit,
                pred: CmpPred::Eq,
                lhs: a,
                rhs: c,
            });
        }
        other => {
            return Err(err(format!(
                "@derive(Eq) does not support field type {other} yet \
                 (only scalars and str)"
            )));
        }
    }
    Ok(bit)
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
        let h = match *fty {
            Ty::Ptr => {
                let d = b.new_value(Ty::I64);
                b.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_hash".to_string(),
                    args: vec![fv],
                });
                d
            }
            t if t.int_width().is_some() => {
                let i = b.coerce(fv, t, Ty::I64);
                let d = b.new_value(Ty::I64);
                b.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_i64_hash".to_string(),
                    args: vec![i],
                });
                d
            }
            other => {
                return Err(err(format!(
                    "@derive(Hash) does not support field type {other} yet \
                     (only integers, bools, and str)"
                )));
            }
        };
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
        if is_main && b.ctx.has_globals() {
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
    /// A `with` manager: emit `exit_fn(mgr)` (the lowered `__exit__`).
    WithExit { mgr: Value, exit_fn: String },
    /// A `try ... finally`: re-lower the `finally` block.
    Finally(ast::Block),
    /// Balance a landing-pad `__cxa_begin_catch` on a structured early exit.
    EndCatch,
    /// Leaving a structured concurrency region cancels and awaits its children.
    ScopeCancel(Value),
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
                Cleanup::WithExit { mgr, exit_fn } => {
                    let (mgr, exit_fn) = (*mgr, exit_fn.clone());
                    self.throwing_call(None, &exit_fn, vec![mgr]);
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
            }
            if self.terminated() {
                break;
            }
        }
        self.cleanups = saved;
        self.landing_pads = saved_lps;
        Ok(())
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

    /// Lower a free-function call that carries `*`/`**` spread arguments (#597)
    /// or targets a callee with a `V **name` keyword-collector (#601).
    ///
    /// The call's arguments are assembled into the callee's declaration order:
    /// explicit positionals (and the elements of a statically-sized `*tuple`)
    /// bind to the fixed parameters in order; any overflow plus a runtime-length
    /// `*list`/`*set` spread are packed into the variadic `TYPE... name` list;
    /// keyword arguments bind to their named parameter, and any left over — plus
    /// the entries of a `**dict` spread — are collected into the `V **name`
    /// dict. Sema (`check_user_call_spread`) has already validated the shapes;
    /// the errors here are a defensive backstop.
    fn lower_free_call_assembled(
        &mut self,
        fname: &str,
        ptys: &[Ty],
        ret: Ty,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let ctx = self.ctx;
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

        // Assemble one value per parameter, in declaration order.
        let mut argvals: Vec<Value> = Vec::with_capacity(n);
        for pi in 0..n {
            if Some(pi) == var_idx {
                let overflow = &pos_srcs[n_fixed_pos..];
                let v = self.build_variadic_list(ptys[pi], overflow, seq_spread)?;
                argvals.push(v);
            } else if Some(pi) == kw_idx {
                let v = self.build_kwargs_dict(ptys[pi], &kw_for_collector, map_spread)?;
                argvals.push(v);
            } else {
                let fi = fixed.iter().position(|&x| x == pi).unwrap();
                let target = ptys[pi];
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

    // --- value <-> string conversions and f-strings (D-0025/D-0026) ---

    /// Render a scalar/char/bool/str value to a `str` (the `str(x)` path).
    /// f-string fields with no format spec share this exact lowering, so
    /// `str(42)` and `f"{42}"` produce byte-identical output.
    /// Top-level string rendering (`str(x)` / `print(x)` / an f-string field):
    /// the `str`-mode form of the general Display protocol (#274, D-0124), where
    /// a bare string/char/bytes value renders WITHOUT quotes. Equivalent to
    /// `render_value(v, vt, RenderMode::Str)`.
    fn render_str(&mut self, v: Value, vt: Ty) -> Result<Value, LowerError> {
        self.render_value(v, vt, RenderMode::Str)
    }

    /// Nested string rendering (a struct field, container element, or tuple
    /// element): the `repr`-mode form of the Display protocol (#274, D-0124),
    /// where a string renders Python-quoted (`'a'`), `bytes` as `b'…'`, and a
    /// `char` as its quoted one-character form — exactly as Python shows a value
    /// *inside* a container/`repr`. Every other type renders identically to
    /// `str` mode.
    fn render_repr(&mut self, v: Value, vt: Ty) -> Result<Value, LowerError> {
        self.render_value(v, vt, RenderMode::Repr)
    }

    /// The single value-rendering protocol (#274, D-0124). `mode` selects the
    /// Python `str`/`repr` distinction, which affects ONLY leaf string/char/bytes
    /// values: in `Str` mode a bare string renders unquoted (top-level
    /// `print`/`str`), in `Repr` mode it renders quoted (`'a'`), as Python shows
    /// a value nested inside a container or struct. Aggregates (structs, tuples,
    /// containers) always render their children in `Repr` mode regardless of how
    /// the aggregate itself was reached; `Optional` propagates the ambient mode to
    /// its payload.
    fn render_value(&mut self, v: Value, vt: Ty, mode: RenderMode) -> Result<Value, LowerError> {
        // `bytes`: `str` mode decodes the UTF-8 bytes (I#337, may raise on invalid
        // UTF-8, so it routes through `throwing_call`); `repr` mode renders the
        // Python `b'…'` byte-string form (no decode, never raises).
        if matches!(vt, Ty::Bytes) {
            let d = self.new_value(Ty::Ptr);
            match mode {
                RenderMode::Str => self.throwing_call(Some(d), "zap_bytes_to_str", vec![v]),
                RenderMode::Repr => self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bytes_repr".to_string(),
                    args: vec![v],
                }),
            }
            return Ok(d);
        }
        // `Optional[T]` renders its payload when `Some`, and the literal
        // `"None"` when `None` (#498, matching Python's `f"{d.get(k)}"`). The
        // payload is rendered recursively in the SAME mode, so a nested
        // `Optional[str]` element quotes its payload; a non-renderable payload
        // falls through to the error below.
        if let Some(opt) = optional_layout(self.ctx, vt) {
            let slot = self.alloca(Ty::Ptr);
            let is_none = gen_opt_is_none(self, &opt, v);
            let none_bb = self.new_block();
            let some_bb = self.new_block();
            let join_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond: is_none,
                then_bb: none_bb,
                else_bb: some_bb,
            });
            self.position(none_bb);
            let none_str = self.strconst("None");
            self.store(slot, none_str);
            self.set_term(Term::Br(join_bb));
            self.position(some_bb);
            let payload = gen_opt_payload(self, &opt, v);
            let some_str = self.render_value(payload, opt.payload_ty, mode)?;
            self.store(slot, some_str);
            self.set_term(Term::Br(join_bb));
            self.position(join_bb);
            return Ok(self.load(slot, Ty::Ptr));
        }
        let (func, arg) = match vt {
            // A bare `str`: unquoted in `str` mode (passthrough), Python-quoted in
            // `repr` mode (`'a'`, with the real quote-selection + escaping done by
            // the runtime helper).
            Ty::Ptr => match mode {
                RenderMode::Str => return Ok(v),
                RenderMode::Repr => ("zap_str_repr", v),
            },
            Ty::Bigint => {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bigint_to_str".to_string(),
                    args: vec![v],
                });
                return Ok(d);
            }
            Ty::Complex => {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_complex_to_str".to_string(),
                    args: vec![v],
                });
                return Ok(d);
            }
            // A user struct (#274, D-0124): render via its `__str__`/`__repr__` /
            // `@derive(Repr)` / the compiler default. This is what makes
            // `str(point)` / `f"{point}"` work, not just `print(point)` — closing
            // the pre-#274 asymmetry where only `print` reached the struct path.
            // `Optional[T]` (also a `Ty::Struct`) was already handled above; a
            // tuple or tier-3 enum routes through `struct_to_str`. An interface
            // value (its synthetic `{data, itable}` fat struct) renders by
            // **dynamic dispatch** to the concrete type's renderer through the
            // itable's renderer slot (#707, D-0124), since its concrete type is
            // erased — not through the static `struct_to_str`. The struct's
            // renderer always quotes its own fields, so the ambient `mode` does
            // not matter here.
            Ty::Struct(sid) => {
                if let Some(iid) = self.ctx.iid_of_fat_sid(sid) {
                    return self.render_iface_value(v, iid);
                }
                return self.struct_to_str(sid, v);
            }
            // A tier-1 enum (D-0018): the qualified canonical variant name,
            // `EnumName.VARIANT`, reusing the `.name` discriminant→name chain. An
            // unknown discriminant raises, exactly as `.name` does.
            Ty::Enum(id) => {
                let ename = self
                    .ctx
                    .enums
                    .values()
                    .find(|e| !e.is_sum && e.enum_id == id)
                    .map(|e| e.enum_name.clone())
                    .ok_or_else(|| err("enum identity is not registered"))?;
                let prefix = self.strconst(&format!("{ename}."));
                let (vname, _) = self.lower_enum_name(v, id)?;
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_concat".to_string(),
                    args: vec![prefix, vname],
                });
                return Ok(d);
            }
            // Native container rendering (#274, D-0124): `[v0, v1]`, `{k: v}`,
            // `{v0, v1}` with Python-exact empty forms `[]`, `{}`, `set()`.
            // Elements/keys/values render in `repr` mode (so string elements are
            // quoted), via a synthesized per-container-type renderer.
            Ty::List(_) | Ty::Dict(_, _) | Ty::Set(_) => {
                let f = self.container_repr_fn(vt)?;
                let d = self.new_value(Ty::Ptr);
                self.throwing_call(Some(d), &f, vec![v]);
                return Ok(d);
            }
            // A `char` (#709): `str` mode renders its one-codepoint string;
            // `repr` mode renders that string Python-quoted (`'c'`). An `int32`
            // is NOT a char — it renders as a decimal through the integer arm
            // below (fixing the pre-#709 conflation where an `int32` scalar /
            // field rendered as a character).
            Ty::Char => match mode {
                RenderMode::Str => ("zap_char_to_str", v),
                RenderMode::Repr => {
                    let s = self.new_value(Ty::Ptr);
                    self.emit(Inst::Call {
                        dest: Some(s),
                        func: "zap_char_to_str".to_string(),
                        args: vec![v],
                    });
                    ("zap_str_repr", s)
                }
            },
            Ty::I1 => ("zap_bool_to_str", self.coerce(v, vt, Ty::I64)),
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 => ("zap_i64_to_str", self.coerce(v, vt, Ty::I64)),
            Ty::F32 | Ty::F64 => ("zap_f64_to_str", self.as_f64(v, vt)),
            _ => return Err(err("cannot render this value as a string")),
        };
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args: vec![arg],
        });
        Ok(d)
    }

    /// Python-style truthiness: a number/char is falsey at zero, a str when
    /// empty, a bool is itself.
    fn value_to_bool(&mut self, v: Value, vt: Ty) -> Result<Value, LowerError> {
        match vt {
            Ty::I1 => Ok(v),
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::Char | Ty::I64 => {
                let z = self.iconst(vt, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: v,
                    rhs: z,
                });
                Ok(d)
            }
            Ty::F32 | Ty::F64 => {
                let fv = self.as_f64(v, vt);
                let z = self.fconst(Ty::F64, 0.0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::FCmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: fv,
                    rhs: z,
                });
                Ok(d)
            }
            // A non-empty `str` or `bytes` is truthy: compare byte length to 0.
            Ty::Ptr | Ty::Bytes => {
                let len = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_str_len".to_string(),
                    args: vec![v],
                });
                let z = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: len,
                    rhs: z,
                });
                Ok(d)
            }
            _ => Err(err("bool() expects a str, bytes, char, number or bool")),
        }
    }

    /// Lower an f-string (D-0026) into a left-folded `zap_str_concat` of its
    /// literal runs and formatted fields. The split and spec parsing are shared
    /// with the type checker via `zapc_syntax::fstring`.
    fn lower_fstring(
        &mut self,
        value: &str,
        raw: bool,
        span_start: u32,
        mono_fields: &[ast::Expr],
    ) -> Result<(Value, Ty), LowerError> {
        let base = span_start.saturating_add(2);
        let (parts, errors) = fstring::split_fstring(value, base);
        if let Some(e) = errors.into_iter().next() {
            // sema already reported these with proper spans; this is defensive.
            return Err(err(e.message));
        }
        // Monomorphization records the specialized field expressions here, since
        // it cannot see the ones re-parsed above (#498). Use them positionally
        // when present (their count matches the re-split fields), so inline
        // generics / `dict.get` in a field lower against concrete types. When
        // absent (nothing needed specializing, or the recording was skipped) the
        // freshly-parsed field expressions are used.
        let use_mono = mono_fields.len()
            == parts
                .iter()
                .filter(|p| matches!(p, FStringPart::Field { .. }))
                .count();
        let mut field_idx = 0usize;
        let mut acc: Option<Value> = None;
        for part in &parts {
            let piece = match part {
                FStringPart::Text(t) => {
                    let data = if raw {
                        t.clone().into_bytes()
                    } else {
                        unescape(t)
                    };
                    let d = self.new_value(Ty::Ptr);
                    self.emit(Inst::StrConst {
                        dest: d,
                        bytes: data,
                    });
                    d
                }
                FStringPart::Field { expr, spec, .. } => {
                    let field_expr = if use_mono {
                        &mono_fields[field_idx]
                    } else {
                        expr
                    };
                    field_idx += 1;
                    let (v, vt) = self.expr(field_expr)?;
                    match spec {
                        None => self.render_str(v, vt)?,
                        Some(fs) => self.format_value(v, vt, fs)?,
                    }
                }
            };
            acc = Some(match acc {
                None => piece,
                Some(prev) => {
                    let d = self.new_value(Ty::Ptr);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_str_concat".to_string(),
                        args: vec![prev, piece],
                    });
                    d
                }
            });
        }
        let result = match acc {
            Some(v) => v,
            None => {
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::StrConst {
                    dest: d,
                    bytes: Vec::new(),
                });
                d
            }
        };
        Ok((result, Ty::Ptr))
    }

    /// Render a value through a format spec (D-0026). Dispatches to the runtime
    /// `zap_fmt_int`/`zap_fmt_flt`/`zap_fmt_str` with the spec decomposed into
    /// integer constants.
    fn format_value(&mut self, v: Value, vt: Ty, spec: &FormatSpec) -> Result<Value, LowerError> {
        let align_code = match spec.align {
            Some(Align::Left) => '<' as i64,
            Some(Align::Right) => '>' as i64,
            Some(Align::Center) => '^' as i64,
            Some(Align::SignAware) => '=' as i64,
            None => 0,
        };
        let sign_code = match spec.sign {
            '+' => 1,
            ' ' => 2,
            _ => 0,
        };
        let fill_code = spec.fill as i64;
        let width = spec.width.map(|w| w as i64).unwrap_or(-1);
        let prec = spec.precision.map(|p| p as i64).unwrap_or(-1);

        // Choose the rendering family: an explicit type wins; otherwise the
        // value's own kind decides (int/float numeric, char/bool/str textual).
        let want_int = match spec.ty {
            Some(t) if FormatSpec::is_int_type(t) => true,
            Some(t) if FormatSpec::is_float_type(t) || t == 's' => false,
            _ => matches!(vt, Ty::I8 | Ty::I16 | Ty::I64),
        };
        let want_flt = match spec.ty {
            Some(t) if FormatSpec::is_float_type(t) => true,
            Some(_) => false,
            None => matches!(vt, Ty::F32 | Ty::F64),
        };

        if want_int {
            let (base, upper) = match spec.ty {
                Some('b') => (2, 0),
                Some('o') => (8, 0),
                Some('x') => (16, 0),
                Some('X') => (16, 1),
                _ => (10, 0),
            };
            let iv = self.coerce(v, vt, Ty::I64);
            let b = self.iconst(Ty::I64, base);
            let up = self.iconst(Ty::I64, upper);
            let alt = self.iconst(Ty::I64, spec.alt as i64);
            let sign = self.iconst(Ty::I64, sign_code);
            let w = self.iconst(Ty::I64, width);
            let fill = self.iconst(Ty::I64, fill_code);
            let al = self.iconst(Ty::I64, align_code);
            let zero = self.iconst(Ty::I64, spec.zero as i64);
            let d = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_fmt_int".to_string(),
                args: vec![iv, b, up, alt, sign, w, fill, al, zero],
            });
            return Ok(d);
        }

        if want_flt {
            let ftype = match spec.ty {
                Some('e') => 'e' as i64,
                Some('E') => 'E' as i64,
                Some('f') | Some('F') => 'f' as i64,
                _ => 'g' as i64, // 'g'/'G'/none default to compact
            };
            let fv = self.as_f64(v, vt);
            let ft = self.iconst(Ty::I64, ftype);
            let sign = self.iconst(Ty::I64, sign_code);
            let w = self.iconst(Ty::I64, width);
            let p = self.iconst(Ty::I64, prec);
            let fill = self.iconst(Ty::I64, fill_code);
            let al = self.iconst(Ty::I64, align_code);
            let zero = self.iconst(Ty::I64, spec.zero as i64);
            let d = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_fmt_flt".to_string(),
                args: vec![fv, ft, sign, w, p, fill, al, zero],
            });
            return Ok(d);
        }

        // Textual: render char/bool/str to a string, then pad/truncate.
        let s = self.render_str(v, vt)?;
        let w = self.iconst(Ty::I64, width);
        let p = self.iconst(Ty::I64, prec);
        let fill = self.iconst(Ty::I64, fill_code);
        let al = self.iconst(Ty::I64, align_code);
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(d),
            func: "zap_fmt_str".to_string(),
            args: vec![s, w, p, fill, al],
        });
        Ok(d)
    }

    // --- decorators & reflection (ticket #16, D-0036) ---
    //
    // This section is self-contained: it builds a string constant object, emits
    // the reflection-registry bootstrap, and lowers the reflection queries. All
    // of it is item-level/runtime-support work, isolated from expression and
    // numeric lowering.

    /// A fresh string object holding `s` (via the runtime string constructor).
    fn strconst(&mut self, s: &str) -> Value {
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::StrConst {
            dest: d,
            bytes: s.as_bytes().to_vec(),
        });
        d
    }

    /// Emit `zap_reflect_register(type, name)` for every (type, decorator/
    /// annotation) pair. Called once at the top of `main`.
    fn emit_reflect_registrations(&mut self, meta: &[(String, String)]) {
        for (ty, name) in meta {
            let tv = self.strconst(ty);
            let nv = self.strconst(name);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_reflect_register".to_string(),
                args: vec![tv, nv],
            });
        }
    }

    /// Runtime data-race detector bootstrap (`--race`, #678, epic #665, D-0114).
    /// Emitted once at the top of `main`, ONLY under `--race` (`ctx.race`): so a
    /// non-`--race` build's `main` prologue is byte-identical to today, exactly
    /// like the slice-1 access hooks (every call carries the `__zap_race_`
    /// prefix, so the "race ZIR minus the race hooks == off ZIR" invariant
    /// still holds). Two calls:
    ///
    /// - `__zap_race_enable()` arms the happens-before engine (its spawn /
    ///   `Mutex` release-acquire hooks live in the runtime and are dormant until
    ///   this runs — before any `spawn` in the user body).
    /// - `__zap_race_register_global(slot_idx, name)` records each module
    ///   global's name under its slot index, so a race report can resolve the
    ///   `name_id` the access hooks pass (slice 1) back to a human name. Every
    ///   global is registered (the id is its declaration index), not only the
    ///   racy ones, so any id a hook passes resolves.
    fn emit_race_bootstrap(&mut self) {
        self.emit(Inst::Call {
            dest: None,
            func: "__zap_race_enable".to_string(),
            args: vec![],
        });
        // Snapshot (idx, name) first so the `globals` borrow is released before
        // `strconst`/`emit` (which do not touch it, but keep the borrow tight).
        let names: Vec<(usize, String)> = self
            .ctx
            .globals
            .borrow()
            .iter()
            .enumerate()
            .map(|(i, g)| (i, g.name.clone()))
            .collect();
        for (idx, name) in names {
            let id = self.iconst(Ty::I64, idx as i64);
            let nv = self.strconst(&name);
            self.emit(Inst::Call {
                dest: None,
                func: "__zap_race_register_global".to_string(),
                args: vec![id, nv],
            });
        }
    }

    /// Emit the signature-reflection registry population (I#458, D-0089): for
    /// every reflectable function/method, one `zap_reflect_param_register` per
    /// parameter (in declaration order) plus one `zap_reflect_sig_register` for
    /// the precomputed readable signature. Called once at the top of `main`.
    fn emit_param_reflect_registrations(&mut self, meta: &[FnReflect]) {
        for fr in meta {
            let owner = self.strconst(&fr.owner);
            for p in &fr.params {
                let name = self.strconst(&p.name);
                let tyd = self.strconst(&p.type_disp);
                let hasdef = self.iconst(Ty::I64, if p.has_default { 1 } else { 0 });
                let annos = self.strconst(&p.annos.join(", "));
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_reflect_param_register".to_string(),
                    args: vec![owner, name, tyd, hasdef, annos],
                });
            }
            let sig = self.strconst(&fr.sig);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_reflect_sig_register".to_string(),
                args: vec![owner, sig],
            });
        }
    }

    /// Emit `zap_signal_on(signo, &handler)` for every `@on_signal(Signal.X)`
    /// handler (ticket #77). Called once at the top of `main`. The runtime
    /// subscribes to the signal and spawns a detached coroutine that invokes the
    /// handler on each delivery (so it runs in full Zap context).
    fn emit_on_signal_registrations(&mut self, meta: &[(i64, String)]) {
        for (signo, fn_name) in meta {
            let s = self.iconst(Ty::I64, *signo);
            let fv = self.new_value(Ty::Ptr);
            self.emit(Inst::FuncAddr {
                dest: fv,
                func: fn_name.clone(),
            });
            self.emit(Inst::Call {
                dest: None,
                func: "zap_signal_on".to_string(),
                args: vec![s, fv],
            });
        }
    }

    /// Lower `reflect_has(type, name) -> bool` / `reflect_names(type) -> str`.
    fn lower_reflect_query(
        &mut self,
        fname: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // Built-in `@shareable` reflection (#664, D-0113): the concurrency
        // primitives and concurrent collections carry the marker implicitly, so
        // they have no runtime registry entry. When both arguments are string
        // literals naming such a type and the `"shareable"` marker, fold the
        // query to a compile-time `true`. This keeps `reflect_has(Atomic,
        // "shareable")` answerable without changing the `main` prologue for
        // programs that use no reflection (golden-stable). Any other query —
        // including a user `@shareable` struct, which IS in the runtime registry
        // — falls through to the normal runtime call below.
        if fname == "reflect_has" && args.len() == 2 {
            if let (ast::Expr::Str { value: ty, .. }, ast::Expr::Str { value: name, .. }) =
                (&args[0].value, &args[1].value)
            {
                if name == zapc_syntax::SHAREABLE_MARKER
                    && zapc_syntax::BUILTIN_SHAREABLE_TYPES.contains(&ty.as_str())
                {
                    return Ok((self.iconst(Ty::I1, 1), Ty::I1));
                }
            }
        }
        let mut argvals = Vec::new();
        for a in args {
            let (v, vt) = self.expr(&a.value)?;
            if vt != Ty::Ptr {
                return Err(err(format!("{fname}() expects str arguments")));
            }
            argvals.push(v);
        }
        if fname == "reflect_names" {
            if argvals.len() != 1 {
                return Err(err("reflect_names(type) takes exactly one argument"));
            }
            let d = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_reflect_names".to_string(),
                args: argvals,
            });
            return Ok((d, Ty::Ptr));
        }
        // reflect_has: the runtime returns 1/0 as i64; normalize to a bool.
        if argvals.len() != 2 {
            return Err(err("reflect_has(type, name) takes exactly two arguments"));
        }
        let word = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(word),
            func: "zap_reflect_has".to_string(),
            args: argvals,
        });
        let zero = self.iconst(Ty::I64, 0);
        let d = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: d,
            pred: CmpPred::Ne,
            lhs: word,
            rhs: zero,
        });
        Ok((d, Ty::I1))
    }

    /// Lower the signature-reflection queries (I#458, D-0089):
    /// `reflect_params(owner)`, `reflect_sig(owner)`, `reflect_param_type(owner,
    /// param)`, `reflect_param_annotations(owner, param)` (all `-> str`), and
    /// `reflect_param_has_default(owner, param) -> bool`. Each forwards string
    /// arguments to its runtime entry point; the only one returning an `i64` is
    /// normalized to a `bool`.
    fn lower_reflect_param_query(
        &mut self,
        fname: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let arity = if fname == "reflect_params" || fname == "reflect_sig" {
            1
        } else {
            2
        };
        if args.len() != arity {
            return Err(err(format!("{fname}() takes exactly {arity} argument(s)")));
        }
        let mut argvals = Vec::new();
        for a in args {
            let (v, vt) = self.expr(&a.value)?;
            if vt != Ty::Ptr {
                return Err(err(format!("{fname}() expects str arguments")));
            }
            argvals.push(v);
        }
        let rtfn = match fname {
            "reflect_params" => "zap_reflect_params",
            "reflect_sig" => "zap_reflect_sig",
            "reflect_param_type" => "zap_reflect_param_type",
            "reflect_param_annotations" => "zap_reflect_param_annotations",
            "reflect_param_has_default" => "zap_reflect_param_has_default",
            _ => return Err(err(format!("unknown reflection query '{fname}'"))),
        };
        if fname == "reflect_param_has_default" {
            // The runtime returns 1/0 as i64; normalize to a bool.
            let word = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(word),
                func: rtfn.to_string(),
                args: argvals,
            });
            let zero = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: word,
                rhs: zero,
            });
            return Ok((d, Ty::I1));
        }
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(d),
            func: rtfn.to_string(),
            args: argvals,
        });
        Ok((d, Ty::Ptr))
    }

    // --- statements ---

    fn stmt(&mut self, st: &Stmt) -> Result<(), LowerError> {
        match st {
            // `when target.*` conditional-compilation blocks are expanded by
            // `zapc_syntax::cfg::resolve_cfg` before lowering (the driver resolves
            // against the active target; sema re-resolves for the host). One
            // reaching lowering is an internal invariant violation (I#139).
            Stmt::Cfg(_) => Err(err(
                "internal error: unresolved `when target.*` block reached \
                 lowering (expected cfg resolution first)",
            )),
            Stmt::Let {
                ty, name, value, ..
            } => {
                // With a `fn(...)` annotation, a `lambda` initializer is lowered
                // against that type (I#163) — its parameter types come from the
                // annotation. Otherwise lower-then-coerce as usual.
                let (v, declared) = match ty {
                    Some(t) => {
                        let declared = resolve_ty(t, &self.ctx.tyenv())?;
                        (
                            self.expr_coerced(value, declared, zapc_diag::ErrorCode::ZAP4001)?,
                            declared,
                        )
                    }
                    None => {
                        let (v, vt) = self.expr(value)?;
                        (v, vt)
                    }
                };
                // Value structs are copied on binding (D-0051): `T b = a` makes
                // `b` an independent instance of the value aggregate.
                let v = self.copy_if_value(v, declared, value);
                // `decl_slot` is a plain alloca+declare outside a generator; in a
                // generator it records the local (discovery) or reuses the state
                // struct-backed per-call slot (emit), so the binding persists
                // across `yield`s (I#67).
                let slot = self.decl_slot(name, declared);
                self.store(slot, v);
                Ok(())
            }
            Stmt::Assign { target, value, .. } => {
                if let Expr::Name(n, span) = target {
                    let (v, vt) = self.expr(value)?;
                    // A local slot, or a module-level mutable global's slot
                    // (D-0112, #662) — stored into identically.
                    let (ptr, ty) = self
                        .name_lvalue(n)
                        .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                    // Reassigning a `fn(...)`-typed binding is still a coercion
                    // into a function slot, so it must match structurally (I#465).
                    if let Ty::Func(t_idx) = ty {
                        self.check_fn_coercion(&vt, t_idx, zapc_diag::ErrorCode::ZAP4001)?;
                    }
                    let v = self.coerce(v, vt, ty);
                    // Value structs copy on assignment (D-0051).
                    let v = self.copy_if_value(v, ty, value);
                    self.store_name(n, ptr, ty, v, *span);
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
                    // A `fn(...)` field accepts a lambda / bare function value
                    // (I#163), lowered against the field type.
                    let v = self.expr_coerced(value, fty, zapc_diag::ErrorCode::ZAP4001)?;
                    // Storing a value struct into a field copies it (D-0051), so
                    // the field holds an independent instance.
                    let v = self.copy_if_value(v, fty, value);
                    // --race (#684): a write of a shared heap struct field.
                    self.emit_race_field(bv, sid, idx, name, true, *span);
                    self.emit(Inst::FieldSet {
                        base: bv,
                        sid,
                        idx,
                        val: v,
                    });
                    Ok(())
                } else if let Expr::Index { base, index, span } = target {
                    self.lower_index_set(base, index, value, *span)
                } else {
                    Err(err(
                        "only variable, field, and index assignment are supported by codegen yet",
                    ))
                }
            }
            Stmt::AssignChain { targets, value, .. } => {
                // Chained assignment `t0 = t1 = ... = value` (#594). The RHS is
                // evaluated EXACTLY ONCE and then stored into each target, left
                // to right (Python order). Per-target `base`/`index` sub-
                // expressions are still evaluated at each store, so a subscript
                // target's own side effects happen in source order after the RHS.
                let (v, vt) = match targets.iter().find_map(|t| self.assignment_target_ty(t)) {
                    Some(ty) => self.expr_collection_context(value, ty)?,
                    None => self.expr(value)?,
                };
                for target in targets {
                    self.store_computed(target, v, vt, value)?;
                }
                Ok(())
            }
            Stmt::Del { target, .. } => {
                // `del d[k]` / `del xs[i]` (#595). The parser guarantees the
                // target is a subscript; removal reuses the dict/list element
                // operations (see `lower_index_del`).
                let Expr::Index { base, index, .. } = target else {
                    return Err(err("del requires a subscript target"));
                };
                self.lower_index_del(base, index)
            }
            Stmt::AugAssign {
                target, op, value, ..
            } => {
                if let Expr::Name(n, span) = target {
                    // A local, or a module-level mutable global (D-0112, #662):
                    // read-modify-write through the same slot.
                    let (ptr, ty) = self
                        .name_lvalue(n)
                        .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                    let cur = self.load(ptr, ty);
                    let (rhs, rt) = self.expr(value)?;
                    // In-place augmented assignment (D-0047): if the LHS is a user
                    // struct that defines the matching in-place dunder, `a OP= b`
                    // calls `a.__iOP__(b)`, which mutates the operand in place and
                    // returns it — no fresh allocation as the binary `__OP__` form
                    // would need. Otherwise fall back to deriving from the binary
                    // operator (`a = a.__OP__(b)`). Reference structs share
                    // identity (D-0023), so the in-place mutation is the intended
                    // visible effect; the store-back is a no-op rebind for them and
                    // keeps the value-struct / scalar paths correct.
                    let (res, rty) = match (ty, inplace_op_dunder(*op)) {
                        (Ty::Struct(sid), Some(dunder))
                            if self.sigs.contains_key(&format!(
                                "{}__{}",
                                self.ctx.names[sid as usize], dunder
                            )) =>
                        {
                            self.call_dunder(sid, dunder, vec![(cur, ty), (rhs, rt)])?
                        }
                        _ => self.binop(*op, cur, ty, rhs, rt)?,
                    };
                    let res = self.coerce(res, rty, ty);
                    self.store_name(n, ptr, ty, res, *span);
                    Ok(())
                } else {
                    Err(err(
                        "only simple variable aug-assignment is supported by codegen yet",
                    ))
                }
            }
            Stmt::Expr(e) => {
                // A statement-position `yield` drives the generator state machine
                // (I#67); every other position is deferred (see `expr`).
                if let Expr::Yield { value, .. } = e {
                    return self.lower_yield_stmt(value.as_deref());
                }
                self.expr(e)?;
                Ok(())
            }
            Stmt::If {
                branches,
                else_block,
                ..
            } => self.lower_if(branches, else_block.as_ref()),
            Stmt::While { cond, body, .. } => self.lower_while(cond, body),
            Stmt::DoWhile { body, cond, .. } => self.lower_do_while(body, cond),
            Stmt::For {
                target, iter, body, ..
            } => self.lower_for(target, iter, body),
            // Inside a generator, `return` ends iteration (D-0048/PEP 479): jump
            // to the shared exhaustion block (sema rejects `return <value>`).
            Stmt::Return { .. } if self.gen_entry.is_some() => {
                let done = self.gen_entry.as_ref().unwrap().done_block;
                self.set_term(Term::Br(done));
                Ok(())
            }
            Stmt::Return { value, .. } => {
                match value {
                    Some(e) => {
                        let ret = self.func.ret;
                        // `return <lambda>` in a `fn(...)`-returning function is
                        // lowered against the return type (I#163) — this is how a
                        // function builds and returns a closure over its locals.
                        let v = if ret == Ty::Void {
                            self.expr(e)?.0
                        } else {
                            self.expr_coerced(e, ret, zapc_diag::ErrorCode::ZAP4003)?
                        };
                        // Returning a value struct yields an instance independent
                        // of the callee's locals (D-0051). An inline-eligible
                        // aggregate is returned by value (register/sret), which is
                        // already that copy; a non-inline value struct is deep-
                        // copied here. See `copy_for_pass`.
                        let v = self.copy_for_pass(v, ret, e);
                        // Run `with`/`try-finally` cleanups before returning so
                        // `__exit__`/`finally` execute on the early-return path
                        // (I#185, D-0069). The return value is evaluated first
                        // (Python order).
                        self.run_cleanups(0)?;
                        if !self.terminated() {
                            self.set_term(Term::Ret(Some(v)));
                        }
                    }
                    None => {
                        self.run_cleanups(0)?;
                        if !self.terminated() {
                            self.set_term(Term::Ret(None));
                        }
                    }
                }
                Ok(())
            }
            Stmt::Pass(_) => {
                // `pass` is a no-op (#591): it lowers to no IR at all.
                Ok(())
            }
            Stmt::Break(_) => {
                let (brk, base) = {
                    let f = self
                        .loops
                        .last()
                        .ok_or_else(|| err("'break' outside a loop"))?;
                    (f.brk, f.cleanup_base)
                };
                self.run_cleanups(base)?;
                if !self.terminated() {
                    self.set_term(Term::Br(brk));
                }
                Ok(())
            }
            Stmt::Continue(_) => {
                let (cont, base) = {
                    let f = self
                        .loops
                        .last()
                        .ok_or_else(|| err("'continue' outside a loop"))?;
                    (f.cont, f.cleanup_base)
                };
                self.run_cleanups(base)?;
                if !self.terminated() {
                    self.set_term(Term::Br(cont));
                }
                Ok(())
            }
            Stmt::Raise { value, cause, .. } => self.lower_raise(value.as_ref(), cause.as_ref()),
            Stmt::Assert {
                cond,
                msg,
                cond_src,
                ..
            } => self.lower_assert(cond, msg.as_ref(), cond_src),
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => self.lower_try(body, excepts, finally.as_ref()),
            Stmt::Spawn { call, detached, .. } => self.lower_spawn(call, *detached),
            Stmt::Scope { opts, body, .. } => self.lower_scope(opts, body),
            Stmt::Match {
                scrutinee, arms, ..
            } => self.lower_match(scrutinee, arms),
            // Tuple-destructuring declaration / assignment (D-0049): lower the
            // value to its tuple aggregate, then bind each target via `FieldGet`.
            Stmt::LetTuple { target, value, .. } => {
                let (v, vt) = self.expr(value)?;
                self.lower_bind_target(target, v, vt, true)
            }
            Stmt::AssignTuple { target, value, .. } => {
                let (v, vt) = self.expr(value)?;
                self.lower_bind_target(target, v, vt, false)
            }
            Stmt::With { items, body, .. } => self.lower_with(items, body),
        }
    }

    /// Bind a destructuring target to a value (D-0049). `is_decl` chooses between
    /// introducing new locals (declaration) and storing into existing lvalues
    /// (reassignment). A `Tuple` target reads each element out of the aggregate
    /// with `FieldGet`; a `*rest` rebuilds the remaining elements into a sub-tuple.
    fn lower_bind_target(
        &mut self,
        t: &BindTarget,
        val: Value,
        ty: Ty,
        is_decl: bool,
    ) -> Result<(), LowerError> {
        match t {
            BindTarget::Elem(e) => self.lower_bind_leaf(e, val, ty, is_decl),
            BindTarget::Rest(_, _) => {
                Err(err("'*rest' is only valid as an element of a tuple target"))
            }
            BindTarget::Tuple(elems, _) => {
                let Ty::Struct(sid) = ty else {
                    return Err(err("cannot destructure a non-tuple value"));
                };
                let field_tys = self
                    .ctx
                    .tuples
                    .borrow()
                    .fields(sid)
                    .ok_or_else(|| err("cannot destructure a non-tuple value"))?;
                self.lower_bind_tuple(elems, val, sid, &field_tys, is_decl)
            }
        }
    }

    /// Bind the elements of a tuple target against the aggregate's fields,
    /// honoring at most one `*rest` (which captures the remaining fields as a
    /// freshly built sub-tuple). Arity is validated in sema.
    fn lower_bind_tuple(
        &mut self,
        elems: &[BindTarget],
        base: Value,
        sid: u32,
        field_tys: &[Ty],
        is_decl: bool,
    ) -> Result<(), LowerError> {
        let rest_pos = elems.iter().position(|e| matches!(e, BindTarget::Rest(..)));
        match rest_pos {
            None => {
                if elems.len() != field_tys.len() {
                    return Err(err(format!(
                        "tuple destructuring expects {} element(s), found {}",
                        field_tys.len(),
                        elems.len()
                    )));
                }
                for (idx, e) in elems.iter().enumerate() {
                    let fty = field_tys[idx];
                    let fv = self.field_get(base, sid, idx as u32, fty);
                    self.lower_bind_target(e, fv, fty, is_decl)?;
                }
                Ok(())
            }
            Some(r) => {
                let after = elems.len() - r - 1;
                if field_tys.len() + 1 < elems.len() {
                    return Err(err("tuple destructuring with '*rest' needs more elements"));
                }
                let tail_start = field_tys.len() - after;
                // Fixed elements before the rest.
                for (idx, e) in elems[..r].iter().enumerate() {
                    let fty = field_tys[idx];
                    let fv = self.field_get(base, sid, idx as u32, fty);
                    self.lower_bind_target(e, fv, fty, is_decl)?;
                }
                // `*rest`: rebuild the middle fields into a new sub-tuple.
                let rest_tys: Vec<Ty> = field_tys[r..tail_start].to_vec();
                let rest_sid = self.ctx.tuples.borrow_mut().intern(rest_tys.clone());
                let rest_val = self.new_value(Ty::Struct(rest_sid));
                self.emit(Inst::StructNew {
                    dest: rest_val,
                    sid: rest_sid,
                });
                for (j, fty) in rest_tys.iter().enumerate() {
                    let fv = self.field_get(base, sid, (r + j) as u32, *fty);
                    self.emit(Inst::FieldSet {
                        base: rest_val,
                        sid: rest_sid,
                        idx: j as u32,
                        val: fv,
                    });
                }
                if let BindTarget::Rest(inner, _) = &elems[r] {
                    self.lower_bind_target(inner, rest_val, Ty::Struct(rest_sid), is_decl)?;
                }
                // Fixed elements after the rest.
                for (k, e) in elems[r + 1..].iter().enumerate() {
                    let idx = tail_start + k;
                    let fty = field_tys[idx];
                    let fv = self.field_get(base, sid, idx as u32, fty);
                    self.lower_bind_target(e, fv, fty, is_decl)?;
                }
                Ok(())
            }
        }
    }

    /// Emit a `FieldGet` yielding field `idx` of tuple/struct `sid`.
    fn field_get(&mut self, base: Value, sid: u32, idx: u32, fty: Ty) -> Value {
        let d = self.new_value(fty);
        self.emit(Inst::FieldGet {
            dest: d,
            base,
            sid,
            idx,
        });
        d
    }

    /// Bind one leaf of a destructuring target: a new local (declaration) or an
    /// existing lvalue (reassignment — a name or a struct field).
    fn lower_bind_leaf(
        &mut self,
        e: &Expr,
        val: Value,
        ty: Ty,
        is_decl: bool,
    ) -> Result<(), LowerError> {
        if is_decl {
            let Expr::Name(name, _) = e else {
                return Err(err("a declaration target must be a name"));
            };
            let slot = self.alloca(ty);
            self.store(slot, val);
            self.declare(name, slot, ty);
            return Ok(());
        }
        match e {
            Expr::Name(n, span) => {
                // A local, or a module-level mutable global (D-0112, #662).
                let (ptr, dt) = self
                    .name_lvalue(n)
                    .ok_or_else(|| err(format!("unknown variable '{n}'")))?;
                let v = self.coerce(val, ty, dt);
                self.store_name(n, ptr, dt, v, *span);
                Ok(())
            }
            Expr::Attr { base, name, .. } => {
                let (bv, bty) = self.expr(base)?;
                let Ty::Struct(sid) = bty else {
                    return Err(err("field assignment on a non-struct value"));
                };
                let (idx, fty) = self
                    .ctx
                    .field(sid, name)
                    .ok_or_else(|| err(format!("no field '{name}' on struct")))?;
                let v = self.coerce(val, ty, fty);
                self.emit(Inst::FieldSet {
                    base: bv,
                    sid,
                    idx,
                    val: v,
                });
                Ok(())
            }
            _ => Err(err(
                "only name and field targets are supported in tuple destructuring assignment yet",
            )),
        }
    }

    /// Lower `spawn <call>` (ticket #25, D-0013; callable forms I#475). The
    /// operand must be a call; its arguments (and, for a method, the receiver)
    /// are evaluated eagerly here, in the spawning coroutine — so the capture
    /// happens-before the child runs (D-0034) — coerced to the callee's
    /// parameter types, copied for value-struct safety, and handed to
    /// [`Inst::Spawn`], which codegen turns into a GC-registered coroutine.
    /// Fire-and-forget: the spawn yields no handle; ownership is the innermost
    /// `scope:` (or the root scope, or nothing for `detached` — D-0044).
    ///
    /// Three callable forms are supported, all with **positional arguments
    /// only** (a deliberate v1 restriction, D-0118):
    /// 1. a named top-level function — `spawn worker(ch)`;
    /// 2. a concrete user-struct method — `spawn server.handle(conn)` — desugared
    ///    to a named spawn of the mangled method with the receiver as argument 0;
    /// 3. a first-class function value — `spawn f(x)` where `f` is a variable,
    ///    field, or expression of `fn(...)->...` type (a bound lambda, closure,
    ///    or function-value, I#163) — lowered through a generated per-signature
    ///    thunk that performs the indirect [`Inst::CallClosure`].
    ///
    /// A *bare lambda literal* callee (`spawn (lambda: ...)()`) is rejected
    /// (`ZAP5076`): a lambda has no type without a function-typed context, which
    /// a spawn site does not supply (the same limit as a normal call position);
    /// bind it to a name first. Keyword/`**`/`*` arguments, an arity mismatch, a
    /// non-struct method receiver, and a non-callable operand are also rejected.
    fn lower_spawn(&mut self, call: &Expr, detached: bool) -> Result<(), LowerError> {
        let Expr::Call { callee, args, .. } = call else {
            return Err(err("spawn operand must be a call expression (D-0013)"));
        };
        // Positional-only across every spawn form (D-0118): keyword and spread
        // arguments are not captured by `spawn` in v1.
        if args.iter().any(|a| a.name.is_some())
            || args
                .iter()
                .any(|a| !matches!(a.spread, ast::ArgSpread::None))
        {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn arguments must all be supplied positionally (no keyword or spread arguments)",
            ));
        }
        // Ownership (D-0044): `spawn detached` opts out of all scopes; otherwise
        // the innermost enclosing `scope:` owns it, falling back to the implicit
        // root scope at the top level.
        let owner = if detached {
            Owner::Detached
        } else if let Some(&s) = self.scope_stack.last() {
            Owner::Scope(s)
        } else {
            Owner::Root
        };

        // Form 1 — a named top-level function (the fast path): emit a direct
        // `Inst::Spawn` on the function symbol, no thunk. A name shadowed by a
        // local falls through to the function-value form below.
        if let Expr::Name(fname, _) = callee.as_ref() {
            if self.lookup(fname).is_none() {
                let (ptys, _ret) = self.sigs.get(fname).cloned().ok_or_else(|| {
                    err_code(
                        zapc_diag::ErrorCode::ZAP5076,
                        format!("spawn of unknown function '{fname}'"),
                    )
                })?;
                if args.len() != ptys.len() {
                    return Err(err_code(
                        zapc_diag::ErrorCode::ZAP5076,
                        format!(
                            "spawn of '{fname}' expects {} positional argument(s), found {}",
                            ptys.len(),
                            args.len()
                        ),
                    ));
                }
                let mut argvals = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let (v, vt) = self.expr(&a.value)?;
                    let target = *ptys.get(i).unwrap_or(&vt);
                    let v = self.coerce(v, vt, target);
                    // A value struct captured by a coroutine is copied (D-0051)
                    // so it outlives the spawning frame; an inline-eligible
                    // aggregate is stored by value into the GC closure (codegen),
                    // so no explicit copy is emitted. See `copy_for_pass`.
                    let v = self.copy_for_pass(v, target, &a.value);
                    argvals.push(v);
                }
                self.emit(Inst::Spawn {
                    func: fname.clone(),
                    args: argvals,
                    owner,
                    name: None,
                });
                return Ok(());
            }
        }

        // Form 2 — a concrete user-struct method call `recv.method(args)`.
        if let Expr::Attr { base, name, .. } = callee.as_ref() {
            return self.lower_spawn_method(base, name, args, owner);
        }

        // Form 3 — a first-class function value (bound lambda / closure /
        // function-value expression).
        self.lower_spawn_fnvalue(callee, args, owner)
    }

    /// Lower form 2 of `spawn` (I#475): a method call `recv.method(args)` on a
    /// concrete user struct. The receiver is captured as argument 0 — copied for
    /// value-struct safety exactly like any other captured argument, so a value
    /// receiver outlives the spawning frame and a reference receiver is a shared
    /// GC pointer the spawn env's precise descriptor traces. Only a plain,
    /// statically-resolved struct method qualifies; a built-in-container, string,
    /// Python, interface (dynamic dispatch), atomic, or generator receiver is
    /// rejected with `ZAP5076`.
    fn lower_spawn_method(
        &mut self,
        base: &Expr,
        name: &str,
        args: &[ast::Arg],
        owner: Owner,
    ) -> Result<(), LowerError> {
        let (recv, bty) = self.expr(base)?;
        let Ty::Struct(sid) = bty else {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn of a method call is only supported on a user-struct receiver",
            ));
        };
        if is_atomic_struct(self.ctx, sid) || self.ctx.iid_of_fat_sid(sid).is_some() {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn of a method on an atomic or interface value is not supported",
            ));
        }
        let mangled = format!("{}__{}", self.ctx.names[sid as usize], name);
        if self.ctx.generators.contains_key(&mangled) {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                format!("spawn of a generator method '{name}' is not supported"),
            ));
        }
        let (ptys, _ret) = self.sigs.get(&mangled).cloned().ok_or_else(|| {
            err_code(
                zapc_diag::ErrorCode::ZAP5076,
                format!(
                    "spawn of unknown method '{name}' on struct '{}'",
                    self.ctx.names[sid as usize]
                ),
            )
        })?;
        // `ptys[0]` is the implicit `self`; the explicit arguments fill the rest,
        // positionally and in full (no defaults — spawn is positional-only).
        if args.len() + 1 != ptys.len() {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                format!(
                    "spawn of '{}.{name}' expects {} positional argument(s), found {}",
                    self.ctx.names[sid as usize],
                    ptys.len().saturating_sub(1),
                    args.len()
                ),
            ));
        }
        let self_ty = *ptys.first().unwrap_or(&bty);
        let recv = self.copy_for_pass(recv, self_ty, base);
        let mut argvals = Vec::with_capacity(args.len() + 1);
        argvals.push(recv);
        for (i, a) in args.iter().enumerate() {
            let (v, vt) = self.expr(&a.value)?;
            let target = *ptys.get(i + 1).unwrap_or(&vt);
            let v = self.coerce(v, vt, target);
            let v = self.copy_for_pass(v, target, &a.value);
            argvals.push(v);
        }
        self.emit(Inst::Spawn {
            func: mangled,
            args: argvals,
            owner,
            name: Some(format!("{}.{name}", self.ctx.names[sid as usize])),
        });
        Ok(())
    }

    /// Lower form 3 of `spawn` (I#475): a first-class function value callee
    /// (`spawn f(x)` where `f : fn(...)->...`). The callee and arguments are
    /// evaluated eagerly in the parent; the call is performed in the child
    /// through a generated per-signature thunk `$spawnthunk$<sig>` that runs a
    /// single [`Inst::CallClosure`]. The thunk is a plain top-level function, so
    /// the existing named-spawn trampoline hosts it as the coroutine body and its
    /// catch-all landing pad routes a child failure to the owning scope exactly
    /// as for a named spawn. The closure record is the first captured value, a GC
    /// pointer the spawn env's precise descriptor traces.
    fn lower_spawn_fnvalue(
        &mut self,
        callee: &Expr,
        args: &[ast::Arg],
        owner: Owner,
    ) -> Result<(), LowerError> {
        if matches!(callee, Expr::Lambda { .. }) {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn of a bare lambda literal is not supported; bind it to a name first \
                 (e.g. `auto f = lambda: ...; spawn f()`)",
            ));
        }
        let (cv, ct) = self.expr(callee)?;
        let Ty::Func(sig_idx) = ct else {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                "spawn requires a named function, a user-struct method call, or a \
                 first-class function value",
            ));
        };
        let sig = self.ctx.fn_sigs.borrow().get(sig_idx).clone();
        if args.len() != sig.params.len() {
            return Err(err_code(
                zapc_diag::ErrorCode::ZAP5076,
                format!(
                    "spawn of this function value expects {} positional argument(s), found {}",
                    sig.params.len(),
                    args.len()
                ),
            ));
        }
        let mut argvals = Vec::with_capacity(args.len() + 1);
        // The closure record is captured first (env field 0, a traced pointer).
        argvals.push(cv);
        for (i, a) in args.iter().enumerate() {
            let target = sig.params[i];
            let v = self.expr_coerced(&a.value, target, zapc_diag::ErrorCode::ZAP4021)?;
            let v = self.copy_for_pass(v, target, &a.value);
            argvals.push(v);
        }
        let thunk = self.spawn_thunk_for(sig_idx);
        let display = match callee {
            Expr::Name(n, _) => n.clone(),
            _ => "<function value>".to_string(),
        };
        self.emit(Inst::Spawn {
            func: thunk,
            args: argvals,
            owner,
            name: Some(display),
        });
        Ok(())
    }

    /// Generate (once per interned signature) the `spawn` thunk
    /// `$spawnthunk$<sig>(closure, args…)` whose body is a single indirect
    /// [`Inst::CallClosure`] on the captured closure record, then `Ret` (the
    /// result is discarded — `spawn` is fire-and-forget). The `CallClosure` has
    /// no landing pad so a raise propagates out of the thunk to the coroutine's
    /// trampoline, which records the failure on the owning scope (I#475). Returns
    /// the thunk's symbol, reused across every `spawn` of that signature.
    fn spawn_thunk_for(&self, sig_idx: u32) -> String {
        if let Some(w) = self.ctx.spawn_thunks.borrow().get(&sig_idx) {
            return w.clone();
        }
        let sig = self.ctx.fn_sigs.borrow().get(sig_idx).clone();
        let wname = format!("$spawnthunk${sig_idx}");
        let mut params_z = Vec::with_capacity(sig.params.len() + 1);
        params_z.push(Ty::Func(sig_idx));
        params_z.extend_from_slice(&sig.params);
        let mut lf = FnLower::new(
            wname.clone(),
            params_z,
            Ty::Void,
            false,
            false,
            self.sigs,
            self.ctx,
        );
        // ZIR value 0 is the closure record; values 1..=n are the call arguments.
        let callee: Value = 0;
        let argvals: Vec<Value> = (1..=sig.params.len() as u32).collect();
        lf.emit(Inst::CallClosure {
            dest: None,
            callee,
            args: argvals,
            params: sig.params.clone(),
            ret: sig.ret,
            unwind: None,
        });
        lf.set_term(Term::Ret(None));
        self.ctx.generated.borrow_mut().push(lf.func);
        self.ctx
            .spawn_thunks
            .borrow_mut()
            .insert(sig_idx, wname.clone());
        wname
    }

    /// Lower a `scope:` block (D-0044/D-0045, structured concurrency): a lexical
    /// region that owns every `spawn` inside it and joins — or, on failure/early
    /// exit, cancels then awaits — them at the dedent.
    ///
    /// Shape (mirrors `lower_try`'s landing-pad handling): a fresh scope handle is
    /// opened with `zap_scope_enter(on_error)`, pushed onto `scope_stack` so inner
    /// `spawn`s bind to it, and the body is lowered under a landing pad. On normal
    /// completion `zap_scope_exit` is the join barrier — it waits for all owned
    /// coroutines and re-raises their failure (a single exception, or an aggregate
    /// `ExceptionGroup` for several concurrent failures, D-0045). If the body
    /// itself raises (or leaves early by a `raise`), the landing pad calls
    /// `zap_scope_exit_cancel` to cancel and await the children, then re-raises the
    /// parent's in-flight exception (rule 4; the parent's failure wins over the
    /// children's, which are cancelled).
    fn lower_scope(&mut self, opts: &[ast::Arg], body: &ast::Block) -> Result<(), LowerError> {
        let on_error = self.scope_on_error(opts)?;
        let policy = self.iconst(Ty::I64, on_error);
        let handle = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(handle),
            func: "zap_scope_enter".to_string(),
            args: vec![policy],
        });

        if let Some(parent) = self.scope_stack.last().copied() {
            self.emit(Inst::Call {
                dest: None,
                func: "zap_scope_set_parent".to_string(),
                args: vec![handle, parent],
            });
        }
        let mut cached_grace = None;
        for option in opts {
            if option.name.as_deref() != Some("timeout") {
                continue;
            }
            // `timeout` is a nonnegative integer number of milliseconds (no unit
            // suffix; D-0094). Negative/overflow is rejected at runtime (ValueError).
            let (duration, ty) = self.expr(&option.value)?;
            if ty.int_width().is_none() {
                return Err(err(
                    "scope timeout requires an integer duration in milliseconds",
                ));
            }
            let duration = self.coerce(duration, ty, Ty::I64);
            let grace = match cached_grace {
                Some(value) => value,
                None => {
                    let value = match self.scope_grace(opts)? {
                        Some(value) => value,
                        None => self.iconst(Ty::I64, 0),
                    };
                    cached_grace = Some(value);
                    value
                }
            };
            self.throwing_call(None, "zap_scope_timeout", vec![handle, duration, grace]);
        }

        // `cancel_on=[Signal.X, ...]` + optional `grace=<ms>` (ticket #77,
        // D-0058): arm this scope so a listed signal cancels its tree, with an
        // optional hard-exit deadline. The subscription is removed inside
        // `zap_scope_exit`/`_cancel` (after the join), so it stays active while
        // the scope awaits its children.
        let cancel_mask = self.scope_cancel_mask(opts)?;
        if cancel_mask != 0 {
            let grace = match cached_grace {
                Some(value) => value,
                None => match self.scope_grace(opts)? {
                    Some(value) => value,
                    None => self.iconst(Ty::I64, 0),
                },
            };
            let maskv = self.iconst(Ty::I64, cancel_mask);
            self.emit(Inst::Call {
                dest: None,
                func: "zap_scope_cancel_on".to_string(),
                args: vec![handle, maskv, grace],
            });
        }

        let lp = self.new_block();
        let after = self.new_block();

        // --- owned region: a raise inside unwinds to `lp` (cancel + re-raise) ---
        self.landing_pads.push(lp);
        self.scope_stack.push(handle);
        self.cleanups.push(Cleanup::ScopeCancel(handle));
        self.push_scope();
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        self.scope_stack.pop();
        self.cleanups.pop();
        self.landing_pads.pop();

        // Normal completion: the join barrier. `zap_scope_exit` awaits the owned
        // coroutines and re-raises their failure, so it is a throwing call (an
        // aggregate may unwind to an enclosing `try`).
        if !self.terminated() {
            self.throwing_call(None, "zap_scope_exit", vec![handle]);
            self.ensure_br(after);
        }

        // If the body emitted no throwing call it cannot raise, so the cancel
        // landing pad is dead (LLVM requires a `landingpad` to have an `invoke`
        // predecessor) — skip it.
        if !self.used_lps.contains(&lp) {
            self.position(after);
            return Ok(());
        }

        // --- cancel landing pad: the body raised; cancel + await children, then
        // re-raise (rule 4). `zap_scope_exit_cancel_exc` returns which exception
        // surfaces: a recorded child failure outranks a block-level `ScopeTimeout`
        // or `Cancelled`, and when *this* scope's own deadline cancelled the body
        // the caller sees `ScopeTimeout` rather than the raw `Cancelled` (I#475).
        self.position(lp);
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::LandingPad { dest: exc });
        let surfaced = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(surfaced),
            func: "zap_scope_exit_cancel_exc".to_string(),
            args: vec![handle, exc],
        });
        if !self.terminated() {
            self.emit_end_catch();
            self.throwing_call(None, "zap_raise", vec![surfaced]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }

        self.position(after);
        Ok(())
    }

    /// The `on_error` policy constant for a `scope(...)` (D-0044): `0` = Cancel
    /// (the default — a failure cancels siblings), `1` = Wait (a failure waits for
    /// siblings instead of cancelling). Validated in sema; re-checked here so a
    /// bad option is a lowering error rather than a silent default.
    fn scope_on_error(&self, opts: &[ast::Arg]) -> Result<i64, LowerError> {
        let mut policy = 0; // OnError.Cancel
        for a in opts {
            match a.name.as_deref() {
                Some("on_error") => {
                    if let Expr::Attr { base, name, .. } = &a.value {
                        if matches!(base.as_ref(), Expr::Name(n, _) if n == "OnError") {
                            policy = match name.as_str() {
                                "Cancel" => 0,
                                "Wait" => 1,
                                _ => {
                                    return Err(err(
                                        "scope on_error expects OnError.Cancel or OnError.Wait",
                                    ))
                                }
                            };
                            continue;
                        }
                    }
                    return Err(err("scope on_error expects OnError.Cancel or OnError.Wait"));
                }
                Some("timeout") => continue,
                // Handled separately by `scope_cancel_on` (ticket #77).
                Some("cancel_on") | Some("grace") => continue,
                Some(other) => return Err(err(format!("scope() has no option '{other}'"))),
                None => return Err(err("scope() options must be keyword arguments")),
            }
        }
        Ok(policy)
    }

    /// The `cancel_on` signal bitmask for a `scope(cancel_on=[Signal.X, ...])`
    /// (ticket #77, D-0058): bit N set for each listed signal number. Returns 0
    /// when the scope has no `cancel_on`. The `Signal` members are tier-1
    /// discriminants that ARE the OS signal numbers.
    fn scope_cancel_mask(&self, opts: &[ast::Arg]) -> Result<i64, LowerError> {
        let mut mask: i64 = 0;
        for a in opts {
            if a.name.as_deref() != Some("cancel_on") {
                continue;
            }
            let Expr::List(elems, _) = &a.value else {
                return Err(err(
                    "scope cancel_on expects a list, e.g. cancel_on=[Signal.SIGINT]",
                ));
            };
            for e in elems {
                let signo = match e {
                    Expr::Attr { base, name, .. } if matches!(base.as_ref(), Expr::Name(n, _) if n == "Signal") => {
                        self.ctx
                            .enums
                            .get("Signal")
                            .and_then(|en| en.member_value(name))
                            .ok_or_else(|| err(format!("unknown Signal member '{name}'")))?
                    }
                    _ => {
                        return Err(err(
                            "scope cancel_on expects Signal members, e.g. Signal.SIGINT",
                        ))
                    }
                };
                if (1..63).contains(&signo) {
                    mask |= 1i64 << signo;
                }
            }
        }
        Ok(mask)
    }

    /// The `grace=<ms>` hard-exit deadline expression for a `scope(...)`, lowered
    /// to an i64 value (ticket #77); `None` when absent (no deadline).
    fn scope_grace(&mut self, opts: &[ast::Arg]) -> Result<Option<Value>, LowerError> {
        for a in opts {
            if a.name.as_deref() == Some("grace") {
                let (v, vt) = self.expr(&a.value)?;
                return Ok(Some(self.coerce(v, vt, Ty::I64)));
            }
        }
        Ok(None)
    }

    /// Balance the `__cxa_begin_catch` performed on landing-pad entry. Called on
    /// each path that leaves a handler normally or re-raises.
    fn emit_end_catch(&mut self) {
        self.emit(Inst::Call {
            dest: None,
            func: "zap_eh_end_catch".to_string(),
            args: Vec::new(),
        });
    }

    /// `raise EXPR` begins unwinding with a typed exception value. Raising a
    /// string uses the compatibility `zap_panic` entry, which wraps it in the
    /// base `Exception` type.
    fn lower_raise(
        &mut self,
        value: Option<&Expr>,
        cause: Option<&Expr>,
    ) -> Result<(), LowerError> {
        let Some(e) = value else {
            // Bare `raise` (#657, D-0116): re-raise the exception the
            // dynamically-enclosing `except` handler is handling, re-throwing
            // the *same* in-flight object so its type and identity are
            // preserved (traceback fidelity rides on the I#475 green-stack
            // unwinder and is a follow-up). A `from <cause>` is syntactically
            // impossible without a value, so there is none to attach.
            let Some(&exc) = self.active_exc.last() else {
                return Err(err_code(
                    zapc_diag::ErrorCode::ZAP6002,
                    "bare 'raise' is only valid inside an 'except' handler, \
                     where it re-raises the exception being handled; there is \
                     no active exception to re-raise here",
                ));
            };
            self.throwing_call(None, "zap_raise", vec![exc]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
            return Ok(());
        };
        // `raise StopIteration` / `raise StopIteration()` (the Python iterator
        // end-of-iteration signal, D-0048) lowers to a panic carrying a sentinel
        // message the iteration-protocol landing pad recognizes.
        if is_stop_iteration(e) {
            let msg = self.new_value(Ty::Ptr);
            self.emit(Inst::StrConst {
                dest: msg,
                bytes: b"StopIteration".to_vec(),
            });
            let ty = self.iconst(Ty::I64, 11);
            let exc = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(exc),
                func: "zap_exception_new".to_string(),
                args: vec![ty, msg],
            });
            return self.finish_raise(exc, cause);
        }
        if let Expr::Call { callee, args, .. } = e {
            if let Expr::Name(name, _) = callee.as_ref() {
                let custom_exception = self.ctx.ids.contains_key(name)
                    && !self.ctx.enums.contains_key(name)
                    && !self.sigs.contains_key(name);
                if let Some(type_id) = exception_type_id_name(name)
                    .or_else(|| custom_exception.then(|| custom_exception_type_id(name)))
                {
                    if args.len() != 1 || args[0].name.is_some() {
                        return Err(err(format!("{name}() takes one positional message")));
                    }
                    let (message, ty) = self.expr(&args[0].value)?;
                    if ty != Ty::Ptr {
                        return Err(err(format!("{name}() message must be a str")));
                    }
                    let id = self.iconst(Ty::I64, type_id);
                    let exc = self.new_value(Ty::Ptr);
                    self.emit(Inst::Call {
                        dest: Some(exc),
                        func: "zap_exception_new".to_string(),
                        args: vec![id, message],
                    });
                    return self.finish_raise(exc, cause);
                }
            }
        }
        let (v, vt) = self.expr(e)?;
        if vt != Ty::Ptr {
            return Err(err(
                "raise currently expects a str value (the exception message)",
            ));
        }
        // `raise "message"` with no `from` keeps the legacy `zap_panic` shape
        // (the runtime wraps the string in a base `Exception`). A `from <cause>`
        // needs a concrete exception object to carry the cause slot, so build
        // one explicitly and route it through the shared raise path.
        if cause.is_none() {
            self.throwing_call(None, "zap_panic", vec![v]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
            return Ok(());
        }
        let id = self.iconst(Ty::I64, 2); // ZAP_EXC_EXCEPTION
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(exc),
            func: "zap_exception_new".to_string(),
            args: vec![id, v],
        });
        self.finish_raise(exc, cause)
    }

    /// Attach the optional `from <cause>` chain to the exception object `exc`
    /// (#604, D-0103), then begin unwinding with it. The cause is stored in the
    /// exception's `cause` slot so the uncaught renderer can print the cause's
    /// own section above the new one; it is also readable from Zap as
    /// `<caught>.cause`.
    fn finish_raise(&mut self, exc: Value, cause: Option<&Expr>) -> Result<(), LowerError> {
        if let Some(c) = cause {
            let cobj = self.lower_cause(c)?;
            self.emit(Inst::Call {
                dest: None,
                func: "zap_exception_set_cause".to_string(),
                args: vec![exc, cobj],
            });
        }
        self.throwing_call(None, "zap_raise", vec![exc]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        Ok(())
    }

    /// Evaluate a `from <cause>` operand to an exception object. When the cause
    /// names a caught exception binding, its real in-flight exception object is
    /// reused (preserving the cause's own type id and chain); otherwise the
    /// operand must be a str message, which is wrapped in a base `Exception`.
    fn lower_cause(&mut self, cause: &Expr) -> Result<Value, LowerError> {
        if let Expr::Name(n, _) = cause {
            if let Some((obj_slot, _)) = self.lookup(&exc_obj_key(n)) {
                return Ok(self.load(obj_slot, Ty::Ptr));
            }
        }
        let (cv, ct) = self.expr(cause)?;
        if ct != Ty::Ptr {
            return Err(err(
                "raise ... from <cause>: the cause must be a caught exception or a str message",
            ));
        }
        let id = self.iconst(Ty::I64, 2); // ZAP_EXC_EXCEPTION
        let obj = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(obj),
            func: "zap_exception_new".to_string(),
            args: vec![id, cv],
        });
        Ok(obj)
    }

    /// `assert cond [, msg]` (D-0054). Always evaluated in v1: the condition is
    /// tested, and on failure a *power-assert* message — the condition's source
    /// text plus its operand values — is built and raised through the ordinary
    /// `zap_panic` path (so an enclosing `try`/`except`, and the `zap test`
    /// runner, catch it like any other exception).
    ///
    /// For a top-level comparison (`a == b`, `a < b`, …) each operand is
    /// evaluated exactly once and its value retained, so the diagnostic can show
    /// both sides without re-evaluating (no double side effects). Other
    /// conditions fall back to showing the whole expression's truthiness.
    fn lower_assert(
        &mut self,
        cond: &Expr,
        msg: Option<&Expr>,
        cond_src: &str,
    ) -> Result<(), LowerError> {
        // Decompose a comparison so both operand values can be reported.
        let decomposed = match cond {
            Expr::Binary { op, lhs, rhs, .. } if is_power_comparison(*op) => {
                let (lv, lt) = self.expr(lhs)?;
                let (rv, rt) = self.expr(rhs)?;
                let (bit, _) = self.binop(*op, lv, lt, rv, rt)?;
                Some((bit, (lv, lt), (rv, rt)))
            }
            _ => None,
        };
        let bit = match decomposed {
            Some((bit, ..)) => bit,
            None => {
                let (cv, ct) = self.expr(cond)?;
                self.truthy(cv, ct)
            }
        };

        let fail = self.new_block();
        let after = self.new_block();
        self.set_term(Term::BrIf {
            cond: bit,
            then_bb: after,
            else_bb: fail,
        });

        // --- failure path: build the message and raise it ---
        self.position(fail);
        let mut acc = self.strconst("assertion failed");
        if let Some(m) = msg {
            let (mv, mt) = self.expr(m)?;
            // A str message is used verbatim; anything else is stringified.
            let ms = if mt == Ty::Ptr {
                mv
            } else {
                self.value_to_str(mv, mt)
            };
            let sep = self.strconst(": ");
            acc = gen_concat(self, acc, sep);
            acc = gen_concat(self, acc, ms);
        }
        let srcline = self.strconst(&format!("\n  assert {cond_src}"));
        acc = gen_concat(self, acc, srcline);
        if let Some((_, (lv, lt), (rv, rt))) = decomposed {
            let ls = self.value_to_str(lv, lt);
            let rs = self.value_to_str(rv, rt);
            let lpfx = self.strconst("\n  left:  ");
            acc = gen_concat(self, acc, lpfx);
            acc = gen_concat(self, acc, ls);
            let rpfx = self.strconst("\n  right: ");
            acc = gen_concat(self, acc, rpfx);
            acc = gen_concat(self, acc, rs);
        }
        self.throwing_call(None, "zap_panic", vec![acc]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        self.position(after);
        Ok(())
    }

    /// Best-effort stringification of a value for power-assert diagnostics.
    /// Returns a `Ptr` (ZapStr). Never fails: an unsupported type renders as a
    /// `<type>` placeholder so `assert` always lowers.
    fn value_to_str(&mut self, v: Value, t: Ty) -> Value {
        match t {
            // Quote strings so empty/whitespace values are visible.
            Ty::Ptr => {
                let q = self.strconst("\"");
                let a = gen_concat(self, q, v);
                let q2 = self.strconst("\"");
                gen_concat(self, a, q2)
            }
            Ty::I1 => {
                let i64v = self.coerce(v, Ty::I1, Ty::I64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bool_to_str".to_string(),
                    args: vec![i64v],
                });
                d
            }
            t if t.int_width().is_some() => {
                let i64v = self.coerce(v, t, Ty::I64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_i64_to_str".to_string(),
                    args: vec![i64v],
                });
                d
            }
            t if t.is_float() => {
                let f64v = self.coerce(v, t, Ty::F64);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_f64_to_str".to_string(),
                    args: vec![f64v],
                });
                d
            }
            Ty::Struct(sid) => {
                // Use the struct's own `__str__`/`__repr__` when it has one;
                // otherwise fall back to a type-name placeholder (never fail).
                for dunder in ["__str__", "__repr__"] {
                    let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
                    if self.sigs.contains_key(&mangled) {
                        let d = self.new_value(Ty::Ptr);
                        self.throwing_call(Some(d), &mangled, vec![v]);
                        return d;
                    }
                }
                self.strconst(&format!("<{}>", self.ctx.names[sid as usize]))
            }
            _ => self.strconst("<value>"),
        }
    }

    /// `with items: BODY` (D-0027). Only the `with expect_raises(ErrType):`
    /// exception-assertion form is supported (D-0054): BODY must raise, or the
    /// `with` itself fails. The general context-manager protocol
    /// (`__enter__`/`__exit__`) is a documented follow-up.
    ///
    /// Desugars to the equivalent `try`/flag/`raise`:
    /// ```text
    /// int __er_N = 0
    /// try: BODY
    /// except: __er_N = 1
    /// if __er_N == 0: raise "expected <ErrType> ... but none was raised"
    /// ```
    /// so only a matching raise inside BODY is swallowed and a missing or
    /// mismatched one propagates (and is caught by an enclosing test).
    /// `with ITEM { , ITEM } : BODY` (D-0027, D-0069). The test-only
    /// `with expect_raises(ErrType):` (D-0054) keeps its bespoke desugaring;
    /// every other form drives the general `__enter__`/`__exit__` context-manager
    /// protocol (I#185). Multiple managers nest left-to-right (Python).
    fn lower_with(&mut self, items: &[ast::WithItem], body: &ast::Block) -> Result<(), LowerError> {
        if items.len() == 1 {
            if let Expr::Call { callee, .. } = &items[0].expr {
                if is_name(callee, "expect_raises") {
                    return self.lower_expect_raises(&items[0].expr, body);
                }
            }
        }
        self.push_scope();
        let r = self.lower_with_items(items, body, 0);
        self.pop_scope();
        r
    }

    /// The context-manager protocol for one `with` item (I#185, D-0069):
    /// evaluate the manager once, call `__enter__` (binding `as NAME` to its
    /// result), lower the rest (the next item, or `BODY` for the last), then run
    /// `__exit__` on every exit path — normal fall-through, panic/unwind (via a
    /// landing pad that re-raises, chaining to any enclosing manager), and the
    /// structured early exits (via the cleanup stack walked by
    /// `return`/`break`/`continue`).
    fn lower_with_items(
        &mut self,
        items: &[ast::WithItem],
        body: &ast::Block,
        idx: usize,
    ) -> Result<(), LowerError> {
        if idx == items.len() {
            for st in &body.stmts {
                self.stmt(st)?;
            }
            return Ok(());
        }
        let item = &items[idx];
        let (mgr, mty) = self.expr(&item.expr)?;
        let Ty::Struct(sid) = mty else {
            return Err(err(
                "`with` manager must be a value implementing the context-manager \
                 protocol (`__enter__`/`__exit__`)",
            ));
        };
        let name = self.ctx.names[sid as usize].clone();
        let enter_m = format!("{}__{}", name, "__enter__");
        let exit_m = format!("{}__{}", name, "__exit__");
        let Some((_p, enter_ret)) = self.sigs.get(&enter_m).cloned() else {
            return Err(err(format!(
                "type '{name}' is not a context manager: define '__enter__(self)' \
                 and '__exit__(self)' (I#185)"
            )));
        };
        if !self.sigs.contains_key(&exit_m) {
            return Err(err(format!(
                "context manager '{name}' defines '__enter__' but not '__exit__' (I#185)"
            )));
        }

        // `NAME = mgr.__enter__()` on entry (result discarded without `as`).
        let entered = self.new_value(enter_ret);
        self.throwing_call(Some(entered), &enter_m, vec![mgr]);
        if let Some(alias) = &item.alias {
            let slot = self.alloca(enter_ret);
            self.store(slot, entered);
            self.declare(alias, slot, enter_ret);
        }

        let lp = self.new_block();
        let after = self.new_block();

        // Body: calls unwind to `lp`; an early exit runs `__exit__` via the stack.
        self.landing_pads.push(lp);
        self.cleanups.push(Cleanup::WithExit {
            mgr,
            exit_fn: exit_m.clone(),
        });
        self.push_scope();
        self.lower_with_items(items, body, idx + 1)?;
        self.pop_scope();
        self.cleanups.pop();
        self.landing_pads.pop();

        // Normal fall-through: run `__exit__`, continue past the `with`.
        if !self.terminated() {
            self.throwing_call(None, &exit_m, vec![mgr]);
            self.ensure_br(after);
        }

        // Body could not raise → no landing pad needed.
        if !self.used_lps.contains(&lp) {
            self.position(after);
            return Ok(());
        }

        // Unwind path: run `__exit__`, then re-raise. With `lp` already popped,
        // the re-raise (and an `__exit__` that itself raises) routes to the
        // enclosing manager/`try`, so cleanup chains on exceptions.
        self.position(lp);
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::LandingPad { dest: exc });
        self.emit_end_catch();
        self.throwing_call(None, &exit_m, vec![mgr]);
        if !self.terminated() {
            self.throwing_call(None, "zap_raise", vec![exc]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }
        self.position(after);
        Ok(())
    }

    /// `with expect_raises(ErrType):` (D-0054) — the test-framework assertion
    /// that `BODY` raises. Desugars to a flag + `try/except` + post-check,
    /// reusing the existing try/if/raise lowering.
    fn lower_expect_raises(&mut self, call: &Expr, body: &ast::Block) -> Result<(), LowerError> {
        let Expr::Call { args, .. } = call else {
            return Err(err("expect_raises must be called"));
        };
        let ty_name = args
            .first()
            .map(|a| ast::render_expr(&a.value))
            .unwrap_or_else(|| "an exception".to_string());
        let caught_ty = args.first().and_then(|a| match &a.value {
            Expr::Name(name, span) => Some(ast::Type::Named {
                name: name.clone(),
                args: Vec::new(),
                span: *span,
            }),
            _ => None,
        });

        // Build the desugared statements as AST and lower them, reusing the
        // existing try / if / raise lowering.
        let sp = body.span;
        let flag = format!("__er_{}", self.new_with_id());
        let miss = format!("expected {ty_name} to be raised, but the with-body returned normally");

        let decl = Stmt::Let {
            is_const: false,
            ty: None,
            name: flag.clone(),
            value: Expr::Int("0".to_string(), sp),
            span: sp,
        };
        let set_flag = Stmt::Assign {
            target: Expr::Name(flag.clone(), sp),
            value: Expr::Int("1".to_string(), sp),
            span: sp,
        };
        let try_stmt = Stmt::Try {
            body: body.clone(),
            excepts: vec![ast::ExceptClause {
                ty: caught_ty,
                binding: None,
                body: ast::Block {
                    stmts: vec![set_flag],
                    span: sp,
                },
                is_star: false,
                span: sp,
            }],
            finally: None,
            span: sp,
        };
        let check = Stmt::If {
            branches: vec![(
                Expr::Binary {
                    op: AstBin::Eq,
                    lhs: Box::new(Expr::Name(flag.clone(), sp)),
                    rhs: Box::new(Expr::Int("0".to_string(), sp)),
                    span: sp,
                },
                ast::Block {
                    stmts: vec![Stmt::Raise {
                        value: Some(Expr::Str {
                            value: miss,
                            raw: true,
                            bytes: false,
                            fstring: false,
                            span: sp,
                            fstring_fields: Vec::new(),
                        }),
                        cause: None,
                        span: sp,
                    }],
                    span: sp,
                },
            )],
            else_block: None,
            span: sp,
        };

        self.push_scope();
        self.stmt(&decl)?;
        self.stmt(&try_stmt)?;
        self.stmt(&check)?;
        self.pop_scope();
        Ok(())
    }

    /// A monotonically increasing id so each `with` desugaring uses a distinct
    /// flag variable (safe even when `with`s nest).
    fn new_with_id(&mut self) -> u32 {
        let id = self.with_counter;
        self.with_counter += 1;
        id
    }

    /// `try: BODY except [Type [as e]]: HANDLER finally: FIN` with typed,
    /// source-ordered dispatch. Calls in BODY become `invoke`s whose unwind edge
    /// targets a landing-pad block. Every clause gets a chance to match before
    /// the exception is propagated; `finally` runs on normal, handled, and
    /// unmatched paths.
    fn lower_try(
        &mut self,
        body: &ast::Block,
        excepts: &[ast::ExceptClause],
        finally: Option<&ast::Block>,
    ) -> Result<(), LowerError> {
        let lp = self.new_block();
        let after = self.new_block();

        // --- protected body: calls unwind to `lp` ---
        self.landing_pads.push(lp);
        // Register `finally` as a cleanup so an early `return`/`break`/`continue`
        // inside the body runs it before jumping (I#185, D-0069 — the gap I#22
        // deferred). The unwind and normal paths emit `finally` explicitly below.
        let has_finally_cleanup = finally.is_some();
        if let Some(f) = finally {
            self.cleanups.push(Cleanup::Finally(f.clone()));
        }
        self.push_scope();
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        if has_finally_cleanup {
            self.cleanups.pop();
        }
        self.landing_pads.pop();
        // Normal completion: run `finally`, then continue past the `try`.
        if !self.terminated() {
            if let Some(f) = finally {
                self.lower_block(f)?;
            }
            self.ensure_br(after);
        }

        // If the body emitted no throwing call it cannot raise, so the handler
        // is dead — skip the landing pad entirely (LLVM requires a `landingpad`
        // block to have an `invoke` predecessor).
        if !self.used_lps.contains(&lp) {
            self.position(after);
            return Ok(());
        }

        // --- landing pad: entered while unwinding ---
        self.position(lp);
        let exc = self.new_value(Ty::Ptr);
        self.emit(Inst::LandingPad { dest: exc });

        // `except*` group handling (#605, D-0129) takes a distinct path: every
        // matching clause runs (unlike plain `except`, where only the first
        // match runs), each peeling its sub-exceptions out of the in-flight
        // group, and the unmatched remainder auto-re-raises after all clauses.
        // sema has already enforced that the clauses are all-`except*` (so this
        // branch covers every clause) and that each carries a type.
        if excepts.first().is_some_and(|c| c.is_star) {
            return self.lower_try_star(exc, excepts, finally, after);
        }

        let mut has_catch_all = false;
        for clause in excepts {
            let matched = self.new_block();
            let type_id = clause.ty.as_ref().and_then(exception_type_id);
            let next = type_id.map(|_| self.new_block());
            if let Some(type_id) = type_id {
                let expected = self.iconst(Ty::I64, type_id);
                let is_a = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(is_a),
                    func: "zap_exception_is_a".to_string(),
                    args: vec![exc, expected],
                });
                let cond = self.new_value(Ty::I1);
                let zero = self.iconst(Ty::I64, 0);
                self.emit(Inst::ICmp {
                    dest: cond,
                    pred: CmpPred::Ne,
                    lhs: is_a,
                    rhs: zero,
                });
                self.set_term(Term::BrIf {
                    cond,
                    then_bb: matched,
                    else_bb: next.unwrap(),
                });
            } else {
                self.set_term(Term::Br(matched));
                has_catch_all = true;
            }

            self.position(matched);
            self.push_scope();
            if let Some(bind) = &clause.binding {
                let message = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(message),
                    func: "zap_exception_message".to_string(),
                    args: vec![exc],
                });
                let slot = self.alloca(Ty::Ptr);
                self.store(slot, message);
                self.declare(bind, slot, Ty::Ptr);
                // Stash the exception *object* under a scoped, non-typable key so
                // `e.cause` and `raise ... from e` can reach the real exception
                // (its type id and chain), not just the message string (#604).
                let obj_slot = self.alloca(Ty::Ptr);
                self.store(obj_slot, exc);
                self.declare(&exc_obj_key(bind), obj_slot, Ty::Ptr);
            }
            let handler_lp = self.new_block();
            self.landing_pads.push(handler_lp);
            let cleanup_depth = self.cleanups.len();
            if let Some(f) = finally {
                self.cleanups.push(Cleanup::Finally(f.clone()));
            }
            self.cleanups.push(Cleanup::EndCatch);
            // Make the caught exception the active one for the duration of this
            // handler body, so a bare `raise` (re-raise, #657) re-throws it.
            // Nested constructs lowered inline (an inner `try`/`finally`) keep
            // this entry visible, matching Python's dynamic "currently handled
            // exception"; a sibling `else`/`finally` is lowered outside this
            // window, where the stack is already popped.
            self.active_exc.push(exc);
            for st in &clause.body.stmts {
                self.stmt(st)?;
            }
            self.active_exc.pop();
            self.landing_pads.pop();
            let handler_throws = self.used_lps.contains(&handler_lp);
            self.cleanups.truncate(cleanup_depth);
            self.pop_scope();
            if !self.terminated() {
                self.emit_end_catch();
                if let Some(f) = finally {
                    self.lower_block(f)?;
                }
                self.ensure_br(after);
            }
            if handler_throws {
                // A new exception raised by the handler replaces the caught one,
                // but `finally` still runs. Close both active C++ catches before
                // executing cleanup so a failure in `finally` propagates cleanly.
                self.position(handler_lp);
                let replacement = self.new_value(Ty::Ptr);
                self.emit(Inst::LandingPad { dest: replacement });
                self.emit_end_catch();
                self.emit_end_catch();
                if let Some(f) = finally {
                    self.lower_block(f)?;
                }
                if !self.terminated() {
                    self.throwing_call(None, "zap_raise", vec![replacement]);
                    if !self.terminated() {
                        self.set_term(Term::Unreachable);
                    }
                }
            }
            if has_catch_all {
                break;
            }
            self.position(next.unwrap());
        }

        // A finally-only try, or an exception for which no typed clause matched:
        // run cleanup and rethrow the same typed exception value.
        if !has_catch_all {
            self.emit_end_catch();
            if let Some(f) = finally {
                self.lower_block(f)?;
            }
            self.throwing_call(None, "zap_raise", vec![exc]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }

        self.position(after);
        Ok(())
    }

    /// Lower the `except*` clauses of a `try` (#605, D-0129). The landing pad has
    /// already run `begin_catch`, binding the in-flight exception object to
    /// `exc`. Unlike plain `except` (first-match-only), **every** `except*`
    /// clause whose type matches some sub-exception runs: each peels its matches
    /// out of a shared mutable "remaining" working set into a sub-group, binds
    /// its `as NAME` to that group, and runs its handler; control then falls to
    /// the *next* clause (not out of the `try`). After the last clause, whatever
    /// is left in `remaining` auto-re-raises — the single remaining exception
    /// bare (so a plain enclosing `except E` catches it, keeping Zap's
    /// bare-for-single rule), or an `ExceptionGroup` when two or more remain.
    ///
    /// The one `begin_catch` is balanced by exactly one `end_catch` on the way
    /// out, on both the re-raise and the clean-exit edge (and by the per-clause
    /// `Cleanup::EndCatch` on any early `return`/`break`/`raise` inside a
    /// handler). A handler that itself raises replaces the in-flight exception
    /// and abandons the remainder, mirroring the plain-`except` handler-throw
    /// path (its landing pad closes both catches, runs `finally`, and re-raises).
    fn lower_try_star(
        &mut self,
        exc: Value,
        excepts: &[ast::ExceptClause],
        finally: Option<&ast::Block>,
        after: BlockId,
    ) -> Result<(), LowerError> {
        // The mutable working set of still-unhandled sub-exceptions: the group's
        // children, or a fresh one-element list `[exc]` for a bare exception (so
        // a single failure matches `except* E`, treating it as a one-element
        // group — D-0129). Defined in the landing pad, which dominates every
        // clause block, so the value flows to each `split` below.
        let remaining = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(remaining),
            func: "zap_eg_remaining".to_string(),
            args: vec![exc],
        });

        for clause in excepts {
            // sema guarantees an `except*` clause names a built-in or user
            // exception type; recover its runtime type id for the split.
            let Some(type_id) = clause.ty.as_ref().and_then(exception_type_id) else {
                return Err(err(
                    "'except*' requires a named exception type (#605)".to_string()
                ));
            };
            // Peel every remaining sub-exception matching this clause's type into
            // a fresh sub-group (always a group, even for a single match), and
            // remove them from `remaining`. A null result means nothing matched,
            // so this clause's handler is skipped.
            let id = self.iconst(Ty::I64, type_id);
            let matched = self.new_value(Ty::Ptr);
            self.emit(Inst::Call {
                dest: Some(matched),
                func: "zap_eg_split".to_string(),
                args: vec![remaining, id],
            });
            let no_match = self.new_value(Ty::I1);
            self.emit(Inst::IsNull {
                dest: no_match,
                val: matched,
            });
            let body_bb = self.new_block();
            let next_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond: no_match,
                then_bb: next_bb,
                else_bb: body_bb,
            });

            self.position(body_bb);
            self.push_scope();
            if let Some(bind) = &clause.binding {
                // `except* E as eg` binds `eg` to the sub-GROUP. Its value is the
                // group's rendered message string (like `except E as e`); the
                // group object is stashed under the non-typable key so
                // `eg.exceptions` can iterate the real sub-exceptions (#605),
                // exactly as `e.cause` reaches the stashed object (#604).
                let message = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(message),
                    func: "zap_exception_message".to_string(),
                    args: vec![matched],
                });
                let slot = self.alloca(Ty::Ptr);
                self.store(slot, message);
                self.declare(bind, slot, Ty::Ptr);
                let obj_slot = self.alloca(Ty::Ptr);
                self.store(obj_slot, matched);
                self.declare(&exc_obj_key(bind), obj_slot, Ty::Ptr);
            }
            // A raise inside the handler unwinds to `handler_lp`. Register
            // `finally` (if any) then `end_catch` as cleanups so an early
            // `return`/`break`/`continue` runs `end_catch` first, then `finally`
            // — the single `begin_catch` is closed exactly once on every exit.
            let handler_lp = self.new_block();
            self.landing_pads.push(handler_lp);
            let cleanup_depth = self.cleanups.len();
            if let Some(f) = finally {
                self.cleanups.push(Cleanup::Finally(f.clone()));
            }
            self.cleanups.push(Cleanup::EndCatch);
            // The matched sub-group is the active exception for the handler, so a
            // bare `raise` (#657) re-throws it.
            self.active_exc.push(matched);
            for st in &clause.body.stmts {
                self.stmt(st)?;
            }
            self.active_exc.pop();
            self.landing_pads.pop();
            let handler_throws = self.used_lps.contains(&handler_lp);
            self.cleanups.truncate(cleanup_depth);
            self.pop_scope();
            // Normal completion falls through to the NEXT clause's split (not out
            // of the `try`): another `except*` clause may still match.
            self.ensure_br(next_bb);
            if handler_throws {
                // A new exception raised by the handler replaces the caught group
                // and abandons the remainder. Close the handler's catch and the
                // outer `try` catch, run `finally`, then re-raise.
                self.position(handler_lp);
                let replacement = self.new_value(Ty::Ptr);
                self.emit(Inst::LandingPad { dest: replacement });
                self.emit_end_catch();
                self.emit_end_catch();
                if let Some(f) = finally {
                    self.lower_block(f)?;
                }
                if !self.terminated() {
                    self.throwing_call(None, "zap_raise", vec![replacement]);
                    if !self.terminated() {
                        self.set_term(Term::Unreachable);
                    }
                }
            }
            self.position(next_bb);
        }

        // After all clauses: re-raise whatever is still unhandled. `finalize`
        // returns null (nothing left), the single remaining exception (re-raised
        // bare), or a fresh `ExceptionGroup` of the two-or-more remainder.
        let rem = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(rem),
            func: "zap_eg_finalize_remainder".to_string(),
            args: vec![remaining],
        });
        let nothing_left = self.new_value(Ty::I1);
        self.emit(Inst::IsNull {
            dest: nothing_left,
            val: rem,
        });
        let reraise_bb = self.new_block();
        let done_bb = self.new_block();
        self.set_term(Term::BrIf {
            cond: nothing_left,
            then_bb: done_bb,
            else_bb: reraise_bb,
        });

        // Unhandled remainder: close the catch, run `finally`, re-raise.
        self.position(reraise_bb);
        self.emit_end_catch();
        if let Some(f) = finally {
            self.lower_block(f)?;
        }
        if !self.terminated() {
            self.throwing_call(None, "zap_raise", vec![rem]);
            if !self.terminated() {
                self.set_term(Term::Unreachable);
            }
        }

        // Clean exit: close the catch, run `finally`, continue past the `try`.
        self.position(done_bb);
        self.emit_end_catch();
        if let Some(f) = finally {
            self.lower_block(f)?;
        }
        self.ensure_br(after);

        self.position(after);
        Ok(())
    }

    fn lower_block(&mut self, b: &ast::Block) -> Result<(), LowerError> {
        self.push_scope();
        for st in &b.stmts {
            self.stmt(st)?;
        }
        self.pop_scope();
        Ok(())
    }

    fn lower_if(
        &mut self,
        branches: &[(Expr, ast::Block)],
        else_block: Option<&ast::Block>,
    ) -> Result<(), LowerError> {
        let merge = self.new_block();
        for (cond, body) in branches {
            let (cv, cty) = self.expr(cond)?;
            let c = self.truthy(cv, cty);
            let then_bb = self.new_block();
            let next_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond: c,
                then_bb,
                else_bb: next_bb,
            });
            self.position(then_bb);
            self.lower_block(body)?;
            self.ensure_br(merge);
            self.position(next_bb);
        }
        if let Some(eb) = else_block {
            self.lower_block(eb)?;
        }
        self.ensure_br(merge);
        self.position(merge);
        Ok(())
    }

    fn lower_while(&mut self, cond: &Expr, body: &ast::Block) -> Result<(), LowerError> {
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let (cv, cty) = self.expr(cond)?;
        let c = self.truthy(cv, cty);
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        self.push_loop(cond_bb, end_bb);
        self.lower_block(body)?;
        self.loops.pop();
        self.ensure_br(cond_bb);
        self.position(end_bb);
        Ok(())
    }

    fn lower_do_while(&mut self, body: &ast::Block, cond: &Expr) -> Result<(), LowerError> {
        let body_bb = self.new_block();
        let cond_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(body_bb));
        self.position(body_bb);
        self.push_loop(cond_bb, end_bb);
        self.lower_block(body)?;
        self.loops.pop();
        self.ensure_br(cond_bb);
        self.position(cond_bb);
        let (cv, cty) = self.expr(cond)?;
        let c = self.truthy(cv, cty);
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(end_bb);
        Ok(())
    }

    /// Resolve the iterator for a struct-typed iterable (D-0048): call
    /// `__iter__` (the `Iterable` interface) to get a fresh iterator, or, if the
    /// value is itself an iterator (it defines `__next__` but no `__iter__`), use
    /// it directly. Returns the iterator value and its struct id.
    fn setup_iter(&mut self, obj: Value, sid: u32) -> Result<(Value, u32), LowerError> {
        // A generator expression's state struct (#596) is its own iterator (it
        // has a `__next__` but no `__iter__`), and its sid is tuple-interned so
        // it is not in `ctx.names`. Recognize it before the name lookup.
        if self.ctx.genexpr_iters.borrow().contains_key(&sid) {
            return Ok((obj, sid));
        }
        let name = self.ctx.names[sid as usize].clone();
        let iter_m = format!("{}__{}", name, "__iter__");
        if let Some((_p, ret)) = self.sigs.get(&iter_m).cloned() {
            let Ty::Struct(iter_sid) = ret else {
                return Err(err(format!(
                    "'{name}.__iter__' must return an iterator struct (the 'Iterable' \
                     interface, D-0048)"
                )));
            };
            let d = self.new_value(ret);
            self.throwing_call(Some(d), &iter_m, vec![obj]);
            Ok((d, iter_sid))
        } else if self.sigs.contains_key(&format!("{}__{}", name, "__next__")) {
            Ok((obj, sid)) // the value is already an iterator
        } else {
            Err(err(format!(
                "type '{name}' is not iterable: define '__iter__' (the 'Iterable' \
                 interface, D-0048)"
            )))
        }
    }

    /// Classify an iterator's `__next__` (D-0048) by its return signature: the
    /// comma-ok form `-> (T, bool)` (preferred) versus the Python/`StopIteration`
    /// form `-> T`. Returns the element type and, for comma-ok, the `(T, bool)`
    /// tuple's struct id.
    fn next_form(&self, iter_sid: u32) -> Result<NextForm, LowerError> {
        // Generator expressions (#596) carry their `__next__` signature in the
        // side table rather than `sigs`/`ctx.names`; they are always the comma-ok
        // form `-> (T, bool)`.
        if let Some(gx) = self.ctx.genexpr_iters.borrow().get(&iter_sid) {
            return Ok(NextForm::CommaOk {
                tuple_sid: gx.next_tuple_sid,
                elem: gx.elem,
            });
        }
        let name = &self.ctx.names[iter_sid as usize];
        let m = format!("{}__{}", name, "__next__");
        let (_p, ret) = self.sigs.get(&m).cloned().ok_or_else(|| {
            err(format!(
                "iterator '{name}' must define '__next__' (the 'Iterator' interface, D-0048)"
            ))
        })?;
        if let Ty::Struct(tsid) = ret {
            if let Some(fields) = self.ctx.tuples.borrow().fields(tsid) {
                if fields.len() == 2 && fields[1] == Ty::I1 {
                    return Ok(NextForm::CommaOk {
                        tuple_sid: tsid,
                        elem: fields[0],
                    });
                }
            }
        }
        Ok(NextForm::StopIter { elem: ret })
    }

    /// Drive the iteration protocol (D-0048) over struct-typed iterable `obj`
    /// (`sid`): obtain the iterator via [`setup_iter`], then loop calling
    /// `__next__`, invoking `body` with each element value/type. `body` owns the
    /// loop variable / accumulation; a `for` body's `break`/`continue` target the
    /// loop end / the next `__next__`.
    fn drive_iteration(
        &mut self,
        obj: Value,
        sid: u32,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        self.push_scope();
        let (it, iter_sid) = self.setup_iter(obj, sid)?;
        // A genexpr iterator (#596) resolves its `__next__` through the side
        // table, not the `ctx.names`-based mangling (its sid is tuple-interned).
        let next_m = match self.ctx.genexpr_iters.borrow().get(&iter_sid) {
            Some(gx) => gx.next_fn.clone(),
            None => format!("{}__{}", self.ctx.names[iter_sid as usize], "__next__"),
        };
        match self.next_form(iter_sid)? {
            NextForm::CommaOk { tuple_sid, elem } => {
                let head = self.new_block();
                let body_bb = self.new_block();
                let end = self.new_block();
                self.set_term(Term::Br(head));
                self.position(head);
                let res = self.new_value(Ty::Struct(tuple_sid));
                self.throwing_call(Some(res), &next_m, vec![it]);
                let v = self.new_value(elem);
                self.emit(Inst::FieldGet {
                    dest: v,
                    base: res,
                    sid: tuple_sid,
                    idx: 0,
                });
                let ok = self.new_value(Ty::I1);
                self.emit(Inst::FieldGet {
                    dest: ok,
                    base: res,
                    sid: tuple_sid,
                    idx: 1,
                });
                self.set_term(Term::BrIf {
                    cond: ok,
                    then_bb: body_bb,
                    else_bb: end,
                });
                self.position(body_bb);
                self.push_loop(head, end);
                body(self, v, elem)?;
                self.loops.pop();
                self.ensure_br(head);
                self.position(end);
            }
            NextForm::StopIter { elem } => {
                // The Python form: `__next__` returns the element directly and
                // raises `StopIteration` at exhaustion. Guard the call with a
                // landing pad that ends the loop on `StopIteration` and re-raises
                // anything else (D-0048; zero-cost table EH, D-0007).
                let head = self.new_block();
                let body_bb = self.new_block();
                let lp = self.new_block();
                let reraise = self.new_block();
                let end = self.new_block();
                self.set_term(Term::Br(head));
                self.position(head);
                self.landing_pads.push(lp);
                let v = self.new_value(elem);
                self.throwing_call(Some(v), &next_m, vec![it]);
                self.landing_pads.pop();
                self.set_term(Term::Br(body_bb));
                self.position(body_bb);
                self.push_loop(head, end);
                body(self, v, elem)?;
                self.loops.pop();
                self.ensure_br(head);
                // Landing pad: match the typed StopIteration signal.
                self.position(lp);
                let exc = self.new_value(Ty::Ptr);
                self.emit(Inst::LandingPad { dest: exc });
                let stop_id = self.iconst(Ty::I64, 11);
                let eq = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(eq),
                    func: "zap_exception_is_a".to_string(),
                    args: vec![exc, stop_id],
                });
                let zero = self.iconst(Ty::I64, 0);
                let is_stop = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: is_stop,
                    pred: CmpPred::Ne,
                    lhs: eq,
                    rhs: zero,
                });
                self.emit_end_catch();
                self.set_term(Term::BrIf {
                    cond: is_stop,
                    then_bb: end,
                    else_bb: reraise,
                });
                self.position(reraise);
                self.throwing_call(None, "zap_raise", vec![exc]);
                if !self.terminated() {
                    self.set_term(Term::Unreachable);
                }
                self.position(end);
            }
        }
        self.pop_scope();
        Ok(())
    }

    /// `sum(iterable)` over anything that is not a numeric `array` (#596): a
    /// lazy generator / generator expression (a `Ty::Struct` iterator) or a
    /// `list`. Drains the iterable once through the D-0048 protocol, summing
    /// into an accumulator of the element type — so a genexpr builds no
    /// intermediate list. The already-lowered iterable value `v` (type `vt`) is
    /// reused; `arg` is kept only for diagnostics.
    fn lower_sum_iterable(
        &mut self,
        v: Value,
        vt: Ty,
        _arg: &Expr,
    ) -> Result<(Value, Ty), LowerError> {
        let elem_ty = match vt {
            Ty::Struct(sid) => match self.next_form(sid)? {
                NextForm::CommaOk { elem, .. } => elem,
                NextForm::StopIter { elem } => elem,
            },
            Ty::List(e) => e.ty(),
            other => {
                return Err(err(format!(
                    "sum() expects an array, list, or iterator value, found {other:?}"
                )))
            }
        };
        let acc = self.alloca(elem_ty);
        let zero = if elem_ty.is_float() {
            self.fconst(elem_ty, 0.0)
        } else {
            self.iconst(elem_ty, 0)
        };
        self.store(acc, zero);
        let add = move |s: &mut FnLower<'a>, ev: Value, et: Ty| -> Result<(), LowerError> {
            let cur = s.load(acc, elem_ty);
            let ev = s.coerce(ev, et, elem_ty);
            let next = s.new_value(elem_ty);
            s.emit(Inst::Bin {
                dest: next,
                op: BinOp::Add,
                lhs: cur,
                rhs: ev,
            });
            s.store(acc, next);
            Ok(())
        };
        match vt {
            Ty::Struct(sid) => self.drive_iteration(v, sid, add)?,
            Ty::List(e) => self.iterate_list(v, e, add)?,
            _ => unreachable!("element type already classified above"),
        }
        let result = self.load(acc, elem_ty);
        Ok((result, elem_ty))
    }

    /// Iterate any supported iterable `iter`, calling `body(self, value, ty)` for
    /// each element: `range(...)`, a `list`, a `set`, or a user struct via the
    /// D-0048 protocol. Used by comprehensions (and the struct case of `for`).
    fn iterate(
        &mut self,
        iter: &Expr,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        // `range(...)`: a counting loop over `[start, stop)` by `step`.
        if let Expr::Call { callee, args, .. } = iter {
            if is_name(callee, "range") {
                let mut vs = Vec::new();
                for a in args {
                    let (v, t) = self.expr(&a.value)?;
                    vs.push(self.coerce(v, t, Ty::I64));
                }
                let (start, stop, step) = match vs.len() {
                    1 => (self.iconst(Ty::I64, 0), vs[0], self.iconst(Ty::I64, 1)),
                    2 => (vs[0], vs[1], self.iconst(Ty::I64, 1)),
                    3 => (vs[0], vs[1], vs[2]),
                    n => return Err(err(format!("range() takes 1-3 arguments, got {n}"))),
                };
                self.push_scope();
                let var = self.alloca(Ty::I64);
                self.store(var, start);
                let cond_bb = self.new_block();
                let body_bb = self.new_block();
                let incr_bb = self.new_block();
                let end_bb = self.new_block();
                self.set_term(Term::Br(cond_bb));
                self.position(cond_bb);
                let cur = self.load(var, Ty::I64);
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
                    else_bb: end_bb,
                });
                self.position(body_bb);
                let cur_v = self.load(var, Ty::I64);
                self.push_loop(incr_bb, end_bb);
                body(self, cur_v, Ty::I64)?;
                self.loops.pop();
                self.ensure_br(incr_bb);
                self.position(incr_bb);
                let cur2 = self.load(var, Ty::I64);
                let next = self.new_value(Ty::I64);
                self.emit(Inst::Bin {
                    dest: next,
                    op: BinOp::Add,
                    lhs: cur2,
                    rhs: step,
                });
                self.store(var, next);
                self.set_term(Term::Br(cond_bb));
                self.position(end_bb);
                self.pop_scope();
                return Ok(());
            }
        }
        let (val, ty) = self.expr(iter)?;
        match ty {
            Ty::List(elem) => self.iterate_list(val, elem, body),
            Ty::Set(elem) => {
                let items = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_set_items".to_string(),
                    args: vec![val],
                });
                self.iterate_list(items, elem, body)
            }
            Ty::Heap(elem) => {
                // Iteration yields elements in priority (min-first) order without
                // draining the heap: `zap_heap_items` returns a sorted snapshot
                // list, which the list iterator then walks (D-0057).
                let items = self.new_value(Ty::List(elem));
                self.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_heap_items".to_string(),
                    args: vec![val],
                });
                self.iterate_list(items, elem, body)
            }
            // A `str` (a pointer handle in ZIR) iterates its codepoints as
            // `char`s (D-0025), walking the UTF-8 buffer once — so comprehensions
            // and `set(...)`/`set[T](...)` accept a string operand.
            Ty::Ptr => self.iterate_str(val, body),
            Ty::Struct(sid) => self.drive_iteration(val, sid, body),
            other => Err(err(format!(
                "value of type {other:?} is not iterable (D-0048)"
            ))),
        }
    }

    /// Iterate a `str`'s codepoints, invoking `body` with each `char` (an `i32`
    /// codepoint). The closure form of [`Self::lower_for_str`]: it walks the
    /// UTF-8 buffer by byte cursor, re-reading the length each step. Used by the
    /// general iteration protocol ([`Self::iterate`]) for comprehensions and the
    /// `set` constructors over a string.
    fn iterate_str(
        &mut self,
        s: Value,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        self.push_scope();
        let off = self.alloca(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(off, zero);
        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();
        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(off, Ty::I64);
        let nbytes = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(nbytes),
            func: "zap_str_len".to_string(),
            args: vec![s],
        });
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: nbytes,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });
        self.position(body_bb);
        let cur_off = self.load(off, Ty::I64);
        // A codepoint read from a string is a `char` (#709), physically an i32.
        let cp = self.new_value(Ty::Char);
        self.emit(Inst::Call {
            dest: Some(cp),
            func: "zap_str_cp_at_byte".to_string(),
            args: vec![s, cur_off],
        });
        self.push_loop(incr_bb, end_bb);
        body(self, cp, Ty::Char)?;
        self.loops.pop();
        self.ensure_br(incr_bb);
        self.position(incr_bb);
        let cur2 = self.load(off, Ty::I64);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(next),
            func: "zap_str_adv_byte".to_string(),
            args: vec![s, cur2],
        });
        self.store(off, next);
        self.set_term(Term::Br(cond_bb));
        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// Iterate a `list[T]` value by index, re-reading the length each iteration.
    fn iterate_list(
        &mut self,
        lst: Value,
        elem: Elem,
        mut body: impl FnMut(&mut FnLower<'a>, Value, Ty) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        self.push_scope();
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
        // A nested-collection element reads back as its real type (#625).
        let ety = self.elem_ty(elem);
        let e = self.new_value(ety);
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst, i],
        });
        self.push_loop(incr_bb, end_bb);
        body(self, e, ety)?;
        self.loops.pop();
        self.ensure_br(incr_bb);
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
        self.pop_scope();
        Ok(())
    }

    /// Desugar a comprehension clause list into nested loops (#599). Each clause
    /// iterates its source through the general protocol, binds its loop variable
    /// in a fresh scope, and applies its `if` filters there (so each filter
    /// constrains the clauses to its left) before recursing into the remaining
    /// clauses; `emit` runs once at the innermost level per surviving tuple of
    /// bindings. A single clause with one filter reproduces the pre-#599 shape
    /// (one `iterate` wrapping one `comp_guard`).
    fn lower_comp_clauses(
        &mut self,
        clauses: &[CompClause],
        emit: &mut dyn FnMut(&mut FnLower<'a>) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        match clauses.split_first() {
            None => emit(self),
            Some((clause, rest)) => self.iterate(&clause.iter, |s, v, ty| {
                s.push_scope();
                let slot = s.alloca(ty);
                s.store(slot, v);
                s.declare(&clause.var, slot, ty);
                s.guard_all(&clause.ifs, &mut |s| s.lower_comp_clauses(rest, emit))?;
                s.pop_scope();
                Ok(())
            }),
        }
    }

    /// Run `work` guarded by each of `conds` as nested `if` filters (#599): the
    /// filters are evaluated outer-to-inner, so `work` runs only when all of
    /// them hold. An empty `conds` runs `work` unconditionally.
    fn guard_all(
        &mut self,
        conds: &[Expr],
        work: &mut dyn FnMut(&mut FnLower<'a>) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        match conds.split_first() {
            None => work(self),
            Some((c, rest)) => self.comp_guard(Some(c), |s| s.guard_all(rest, work)),
        }
    }

    /// `[elem for CLAUSES]` (D-0048, #599): build a fresh list, desugar the
    /// clauses to nested loops, and push `elem` at the innermost level.
    fn lower_list_comp(
        &mut self,
        elem_e: &Expr,
        clauses: &[CompClause],
    ) -> Result<(Value, Ty), LowerError> {
        let list = self.new_value(Ty::List(Elem::I64));
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let kind = std::cell::Cell::new(None);
        let mut emit = |s: &mut FnLower<'a>| -> Result<(), LowerError> {
            let (ev, et) = s.expr(elem_e)?;
            let ek = list_elem_kind(et, &s.ctx.nested)?;
            kind.set(Some(ek));
            let ev = s.coerce(ev, et, ek.ty());
            let ev = s.box_if_value(ev, ek.ty());
            s.emit(Inst::Call {
                dest: None,
                func: format!("zap_list_push_{}", ek.val_suffix()),
                args: vec![list, ev],
            });
            Ok(())
        };
        // Record the scope enclosing this comprehension so a walrus target in a
        // filter binds there (#600), surviving the per-clause scopes.
        self.comp_enclosing.push(self.scopes.len());
        let comp_result = self.lower_comp_clauses(clauses, &mut emit);
        self.comp_enclosing.pop();
        comp_result?;
        let elem = kind.get().unwrap_or(Elem::I64);
        self.func.value_types[list as usize] = Ty::List(elem);
        Ok((list, Ty::List(elem)))
    }

    /// `{elem for CLAUSES}` (D-0048, #599): like [`lower_list_comp`] into a fresh
    /// set.
    fn lower_set_comp(
        &mut self,
        elem_e: &Expr,
        clauses: &[CompClause],
    ) -> Result<(Value, Ty), LowerError> {
        let set = self.new_value(Ty::Set(Elem::I64));
        self.emit(Inst::Call {
            dest: Some(set),
            func: "zap_set_new".to_string(),
            args: vec![],
        });
        let kind = std::cell::Cell::new(None);
        let mut emit = |s: &mut FnLower<'a>| -> Result<(), LowerError> {
            let (ev, et) = s.expr(elem_e)?;
            let ek = elem_kind(et, &s.ctx.nested)?;
            kind.set(Some(ek));
            let ev = s.coerce(ev, et, ek.ty());
            let ev = s.box_if_value(ev, ek.ty());
            let kfns = s.key_fn_args(ek)?;
            let mut args = vec![set, ev];
            args.extend_from_slice(&kfns);
            s.emit(Inst::Call {
                dest: None,
                func: format!("zap_set_add_{}", ek.key_suffix()),
                args,
            });
            Ok(())
        };
        // Record the scope enclosing this comprehension so a walrus target in a
        // filter binds there (#600), surviving the per-clause scopes.
        self.comp_enclosing.push(self.scopes.len());
        let comp_result = self.lower_comp_clauses(clauses, &mut emit);
        self.comp_enclosing.pop();
        comp_result?;
        let elem = kind.get().unwrap_or(Elem::I64);
        self.func.value_types[set as usize] = Ty::Set(elem);
        Ok((set, Ty::Set(elem)))
    }

    /// `(elem for CLAUSES)` — a bare **generator expression** (#596, epic #614
    /// D4). Unlike the comprehensions above it does not materialize a
    /// collection: it evaluates to a lazy `Iterator` (D-0048) built from the
    /// existing generator state-machine machinery (I#67).
    ///
    /// Because a genexpr captures its enclosing free variables — whose types are
    /// only known here, while lowering the enclosing body — its state struct
    /// cannot be a named struct registered in the module pre-pass. Instead the
    /// genexpr is desugared on the fly into an anonymous generator function
    /// whose parameters are exactly the captured free variables, which is then
    /// fed through the shared [`discover_generator`]/[`lower_generator_next`]:
    ///
    /// ```text
    /// def $genexpr$N(cap0, cap1, ...):
    ///     for v0 in iter0:
    ///         if filt0: ... for vk in iterk: ... yield elem
    /// ```
    ///
    /// The result value is its state struct, constructed with the captured
    /// values stored into the parameter fields and `$state == 0` — exactly like
    /// a generator call. The struct id is tuple-interned (not named) and forced
    /// non-inline/heap (GC-traced, like a closure environment); a side table
    /// ([`StructCtx::genexpr_iters`]) lets the iteration hooks treat it as its
    /// own iterator so `for`/`sum`/etc. consume it through the ordinary D-0048
    /// protocol. The loop variables stay inside the generator (they never leak
    /// to the enclosing scope), matching comprehensions and Python.
    fn lower_genexpr(
        &mut self,
        elem_e: &Expr,
        clauses: &[CompClause],
    ) -> Result<(Value, Ty), LowerError> {
        let dummy = zapc_syntax::span::Span::new(0, 0);
        // 1. Free names of the genexpr, with the loop variables excluded.
        let free = {
            let mut bound: HashSet<String> = HashSet::new();
            let mut out: Vec<String> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            let _fresh = collect_free_names_comp_clauses(clauses, &mut bound, &mut out, &mut seen);
            collect_free_names(elem_e, &mut bound, &mut out, &mut seen);
            out
        };
        // 2. Keep the free names bound in the enclosing scope; those are the
        //    captured parameters. Names resolved elsewhere (top-level functions,
        //    `range`, builtins) are left to resolve inside the generator body.
        let mut cap_names: Vec<String> = Vec::new();
        let mut cap_tys: Vec<Ty> = Vec::new();
        let mut cap_slots: Vec<Value> = Vec::new();
        for name in &free {
            if let Some((slot, ty)) = self.lookup(name) {
                cap_names.push(name.clone());
                cap_tys.push(ty);
                cap_slots.push(slot);
            }
        }
        // 3. Synthesize the anonymous generator function.
        let fname = {
            let mut c = self.ctx.genexpr_counter.borrow_mut();
            let n = *c;
            *c += 1;
            format!("$genexpr${n}")
        };
        let params: Vec<ast::Param> = cap_names
            .iter()
            .map(|n| ast::Param {
                decorators: Vec::new(),
                ty: None,
                name: n.clone(),
                default: None,
                variadic: false,
                kwargs: false,
                keyword_only: false,
                positional_only: false,
                span: dummy,
            })
            .collect();
        let yield_stmt = Stmt::Expr(Expr::Yield {
            value: Some(Box::new(elem_e.clone())),
            span: dummy,
        });
        let body = build_genexpr_body(clauses, yield_stmt, dummy);
        let synth = ast::Function {
            decorators: Vec::new(),
            name: fname.clone(),
            generics: Vec::new(),
            params,
            ret: None,
            body,
            span: dummy,
        };
        // 4. Discover body locals + element type through the shared generator
        //    machinery, with a signature table that knows the captured params.
        let mut local_sigs = self.sigs.clone();
        local_sigs.insert(fname.clone(), (cap_tys.clone(), Ty::Void));
        let (locals, elem_ty) = discover_generator(&synth, &fname, &local_sigs, self.ctx)?;
        // 5. Build the state struct: `$state` + captured params + body locals.
        let mut vars: Vec<(String, Ty)> = cap_names
            .iter()
            .cloned()
            .zip(cap_tys.iter().cloned())
            .collect();
        let n_params = vars.len();
        vars.extend(locals);
        let mut layout = vec![Ty::I64];
        layout.extend(vars.iter().map(|(_, t)| *t));
        let sid = self.ctx.tuples.borrow_mut().push_layout(layout);
        // Force the state struct to a non-inline reference (heap) struct — it
        // outlives the constructing frame and is mutated in place across
        // `__next__` calls, exactly like a generator state struct / closure env.
        self.ctx.closure_env_sids.borrow_mut().push(sid);
        let next_tuple_sid = self.ctx.tuples.borrow_mut().intern(vec![elem_ty, Ty::I1]);
        let gi = GeneratorInfo {
            name: fname.clone(),
            sid,
            elem: elem_ty,
            next_tuple_sid,
            vars,
            n_params,
        };
        // 6. Emit the state machine `__next__`; register the iterator side entry.
        let next_func = lower_generator_next(&synth, &gi, &local_sigs, self.ctx)?;
        self.ctx.generated.borrow_mut().push(next_func);
        let next_fn = format!(
            "{}____next__",
            zapc_syntax::genanalysis::gen_state_name(&fname)
        );
        self.ctx.genexpr_iters.borrow_mut().insert(
            sid,
            GenExprIter {
                next_fn,
                next_tuple_sid,
                elem: elem_ty,
            },
        );
        // 7. Construct the iterator, capturing the free-variable values into the
        //    parameter fields (`$state` defaults to 0 from `StructNew`).
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        for (i, (slot, ty)) in cap_slots.iter().zip(cap_tys.iter()).enumerate() {
            let v = self.load(*slot, *ty);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: gen_var_field(i),
                val: v,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// `{key: value for CLAUSES}` (D-0048, #599): like [`lower_list_comp`] into a
    /// fresh dict.
    fn lower_dict_comp(
        &mut self,
        key_e: &Expr,
        val_e: &Expr,
        clauses: &[CompClause],
    ) -> Result<(Value, Ty), LowerError> {
        let dict = self.new_value(Ty::Dict(Elem::I64, Elem::I64));
        self.emit(Inst::Call {
            dest: Some(dict),
            func: "zap_dict_new".to_string(),
            args: vec![],
        });
        let kinds = std::cell::Cell::new(None);
        let mut emit = |s: &mut FnLower<'a>| -> Result<(), LowerError> {
            let (kv, kt) = s.expr(key_e)?;
            let (vv, vt) = s.expr(val_e)?;
            let kk = elem_kind(kt, &s.ctx.nested)?;
            let vk = elem_kind(vt, &s.ctx.nested)?;
            kinds.set(Some((kk, vk)));
            let kv = s.coerce(kv, kt, kk.ty());
            let vv = s.coerce(vv, vt, vk.ty());
            let kv = s.box_if_value(kv, kk.ty());
            let vv = s.box_if_value(vv, vk.ty());
            let kfns = s.key_fn_args(kk)?;
            let mut args = vec![dict, kv, vv];
            args.extend_from_slice(&kfns);
            s.emit(Inst::Call {
                dest: None,
                func: format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                args,
            });
            Ok(())
        };
        // Record the scope enclosing this comprehension so a walrus target in a
        // filter binds there (#600), surviving the per-clause scopes.
        self.comp_enclosing.push(self.scopes.len());
        let comp_result = self.lower_comp_clauses(clauses, &mut emit);
        self.comp_enclosing.pop();
        comp_result?;
        let (kk, vk) = kinds.get().unwrap_or((Elem::I64, Elem::I64));
        self.func.value_types[dict as usize] = Ty::Dict(kk, vk);
        Ok((dict, Ty::Dict(kk, vk)))
    }

    /// Emit `work` either unconditionally or guarded by `if cond:` (the optional
    /// comprehension filter).
    fn comp_guard(
        &mut self,
        cond: Option<&Expr>,
        work: impl FnOnce(&mut FnLower<'a>) -> Result<(), LowerError>,
    ) -> Result<(), LowerError> {
        match cond {
            None => work(self),
            Some(c) => {
                let (cv, cty) = self.expr(c)?;
                let cc = self.truthy(cv, cty);
                let then_bb = self.new_block();
                let cont_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: cc,
                    then_bb,
                    else_bb: cont_bb,
                });
                self.position(then_bb);
                work(self)?;
                self.ensure_br(cont_bb);
                self.position(cont_bb);
                Ok(())
            }
        }
    }

    fn lower_for(
        &mut self,
        target: &Expr,
        iter: &Expr,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let (start, stop, step) = match iter {
            Expr::Call { callee, args, .. } if is_name(callee, "range") => {
                let mut vs = Vec::new();
                for a in args {
                    let (v, t) = self.expr(&a.value)?;
                    vs.push(self.coerce(v, t, Ty::I64));
                }
                match vs.len() {
                    1 => (self.iconst(Ty::I64, 0), vs[0], self.iconst(Ty::I64, 1)),
                    2 => (vs[0], vs[1], self.iconst(Ty::I64, 1)),
                    3 => (vs[0], vs[1], vs[2]),
                    n => return Err(err(format!("range() takes 1-3 arguments, got {n}"))),
                }
            }
            _ => {
                if let Expr::Name(n, _) = iter {
                    if let Some(info) = self.ctx.enums.get(n).cloned() {
                        if !info.is_sum {
                            return self.lower_for_enum(target, &info, body);
                        }
                    }
                }
                // `for x in <list>` — iterate by index over the list's length.
                let (lst, lty) = self.expr(iter)?;
                if let Ty::List(elem) = lty {
                    return self.lower_for_list(target, lst, elem, body);
                }
                if let Ty::Set(elem) = lty {
                    return self.lower_for_set(target, lst, elem, body);
                }
                if let Ty::Dict(kk, _vk) = lty {
                    return self.lower_for_dict(target, lst, kk, body);
                }
                if let Ty::Heap(elem) = lty {
                    return self.lower_for_heap(target, lst, elem, body);
                }
                // `for c in s` where `s : str` — iterate codepoints (`char`),
                // walking the UTF-8 buffer once (D-0025).
                if lty == Ty::Ptr {
                    return self.lower_for_str(target, lst, body);
                }
                // `for x in <user iterable>` — the general D-0048 protocol.
                if let Ty::Struct(sid) = lty {
                    let name = match target {
                        Expr::Name(n, _) => n.clone(),
                        _ => {
                            return Err(err(
                                "only a simple loop variable is supported by codegen yet",
                            ))
                        }
                    };
                    return self.drive_iteration(lst, sid, |s, v, ty| {
                        let slot = s.alloca(ty);
                        s.store(slot, v);
                        s.declare(&name, slot, ty);
                        for st in &body.stmts {
                            s.stmt(st)?;
                        }
                        Ok(())
                    });
                }
                return Err(err(
                    "only `for ... in range(...)`, `for ... in <list>`, `for ... in <set>`, \
                     `for ... in <dict>`, and `for ... in <user iterable>` are supported by \
                     codegen yet",
                ));
            }
        };
        let name = match target {
            Expr::Name(n, _) => n.clone(),
            _ => {
                return Err(err(
                    "only a simple loop variable is supported by codegen yet",
                ))
            }
        };

        self.push_scope();
        // The loop variable *is* the induction counter here; hoist it so it
        // survives a `yield` in the body, and spill the `stop`/`step` bounds —
        // loop-invariant SSA values used in the cond/incr blocks, which the
        // resume re-entry does not dominate (I#453). Outside a generator this
        // is a plain alloca and the bounds stay SSA.
        let var = self.decl_slot(&name, Ty::I64);
        self.store(var, start);
        let stop_slot = self.spill_invariant(stop, Ty::I64);
        let step_slot = self.spill_invariant(step, Ty::I64);

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(var, Ty::I64);
        let stop_c = self.reload_invariant(stop_slot, stop, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: stop_c,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        self.push_loop(incr_bb, end_bb);
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.loops.pop();
        self.ensure_br(incr_bb);

        self.position(incr_bb);
        let cur2 = self.load(var, Ty::I64);
        let step_i = self.reload_invariant(step_slot, step, Ty::I64);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Bin {
            dest: next,
            op: BinOp::Add,
            lhs: cur2,
            rhs: step_i,
        });
        self.store(var, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// Lower `for member in Enum` as an ordered list of the enum's canonical
    /// discriminants. Duplicate discriminants are aliases and are omitted from
    /// iteration, matching the first-declaration canonical name used by `.name`.
    fn lower_for_enum(
        &mut self,
        target: &Expr,
        info: &EnumLow,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let elem = Elem::Enum(info.enum_id);
        let list_ty = Ty::List(elem);
        let list = self.new_value(list_ty);
        self.emit(Inst::Call {
            dest: Some(list),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        let mut seen = HashSet::new();
        for (_, value) in &info.members {
            if seen.insert(*value) {
                let member = self.iconst(Ty::Enum(info.enum_id), *value);
                self.emit(Inst::Call {
                    dest: None,
                    func: "zap_list_push_i64".to_string(),
                    args: vec![list, member],
                });
            }
        }
        self.lower_for_list(target, list, elem, body)
    }

    /// Resolve a tier-1 enum discriminant to its canonical source name. Explicit
    /// integer-to-enum conversions can produce unknown discriminants, which
    /// raise a catchable runtime fault instead of returning a fabricated name.
    fn lower_enum_name(&mut self, value: Value, enum_id: u32) -> Result<(Value, Ty), LowerError> {
        let info = self
            .ctx
            .enums
            .values()
            .find(|info| !info.is_sum && info.enum_id == enum_id)
            .cloned()
            .ok_or_else(|| err("enum identity is not registered"))?;
        let result_slot = self.alloca(Ty::Ptr);
        let merge = self.new_block();
        let mut check = self.cur;
        let mut seen = HashSet::new();
        for (name, discriminant) in &info.members {
            if !seen.insert(*discriminant) {
                continue;
            }
            self.position(check);
            let expected = self.iconst(Ty::Enum(enum_id), *discriminant);
            let matches = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: matches,
                pred: CmpPred::Eq,
                lhs: value,
                rhs: expected,
            });
            let found = self.new_block();
            let next = self.new_block();
            self.set_term(Term::BrIf {
                cond: matches,
                then_bb: found,
                else_bb: next,
            });
            self.position(found);
            let string = self.new_value(Ty::Ptr);
            self.emit(Inst::StrConst {
                dest: string,
                bytes: name.as_bytes().to_vec(),
            });
            self.store(result_slot, string);
            self.ensure_br(merge);
            check = next;
        }
        self.position(check);
        let message = self.new_value(Ty::Ptr);
        self.emit(Inst::StrConst {
            dest: message,
            bytes: format!("invalid value for enum {}", info.enum_name).into_bytes(),
        });
        self.throwing_call(None, "zap_panic", vec![message]);
        if !self.terminated() {
            self.set_term(Term::Unreachable);
        }
        self.position(merge);
        Ok((self.load(result_slot, Ty::Ptr), Ty::Ptr))
    }

    /// `for x in xs` where `xs : list[T]`. Re-reads the length each iteration so
    /// a body that appends behaves like a live iterator.
    fn lower_for_list(
        &mut self,
        target: &Expr,
        lst: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        // The loop target is a simple name, or a tuple destructured from a tuple
        // element (`for k, v in d.items()`, ticket #299). A name binds once to a
        // hoisted slot stored each iteration; a tuple rebinds its names from the
        // element's fields inside the loop body.
        match target {
            Expr::Name(_, _) | Expr::Tuple(_, _) => {}
            _ => {
                return Err(err(
                    "only a simple loop variable is supported by codegen yet",
                ))
            }
        }

        self.push_scope();
        // Inside a generator body the induction index and the iterable handle
        // must live in hoisted state-struct slots so they survive a `yield` in
        // the loop body (I#453): otherwise the index is a per-call alloca that
        // reads uninitialized on resume, and the handle — an SSA value defined
        // here at loop setup — does not dominate the resume re-entry. Outside a
        // generator these stay a plain index alloca and the SSA handle.
        let idx = self.for_control_slot(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(idx, zero);
        let lst_slot = self.spill_invariant(lst, Ty::List(elem));
        // A nested-collection element binds with its real type (#625) so the loop
        // body dispatches on the true `list`/`dict`/... rather than a `str` pointer.
        let ety = self.elem_ty(elem);
        let single_var = if let Expr::Name(n, _) = target {
            // The loop variable is live across a `yield` placed after its use in
            // the body, so hoist it too (a plain alloca outside a generator).
            Some(self.decl_slot(n, ety))
        } else {
            None
        };

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(idx, Ty::I64);
        let len = self.new_value(Ty::I64);
        let lst_c = self.reload_invariant(lst_slot, lst, Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(len),
            func: "zap_list_len".to_string(),
            args: vec![lst_c],
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
        let e = self.new_value(ety);
        let lst_b = self.reload_invariant(lst_slot, lst, Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(e),
            func: format!("zap_list_get_{}", elem.val_suffix()),
            args: vec![lst_b, i],
        });
        match target {
            Expr::Name(_, _) => self.store(single_var.expect("name slot"), e),
            Expr::Tuple(elems, _) => self.bind_tuple_loop_target(elems, e, ety)?,
            _ => unreachable!("validated above"),
        }
        self.push_loop(incr_bb, end_bb);
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.loops.pop();
        self.ensure_br(incr_bb);

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
        self.pop_scope();
        Ok(())
    }

    /// Bind a tuple loop target (`for k, v in ...`, ticket #299) to the current
    /// element: the element must be a statically-known tuple of matching arity,
    /// and each sub-target a simple name. Each name gets a fresh slot per
    /// iteration carrying its field's value (and precise type).
    fn bind_tuple_loop_target(
        &mut self,
        elems: &[Expr],
        elem_val: Value,
        elem_ty: Ty,
    ) -> Result<(), LowerError> {
        let Ty::Struct(sid) = elem_ty else {
            return Err(err(
                "a tuple loop target requires a tuple element (e.g. `for k, v in d.items()`)",
            ));
        };
        let field_tys = self
            .ctx
            .tuples
            .borrow()
            .fields(sid)
            .ok_or_else(|| err("a tuple loop target requires a tuple element"))?;
        if elems.len() != field_tys.len() {
            return Err(err(format!(
                "tuple loop target expects {} variable(s), found {}",
                field_tys.len(),
                elems.len()
            )));
        }
        for (idx, e) in elems.iter().enumerate() {
            let Expr::Name(n, _) = e else {
                return Err(err(
                    "only simple names are supported in a tuple loop target",
                ));
            };
            let fty = field_tys[idx];
            let fv = self.field_get(elem_val, sid, idx as u32, fty);
            let slot = self.alloca(fty);
            self.store(slot, fv);
            self.declare(n, slot, fty);
        }
        Ok(())
    }

    /// `for c in s` where `s : str` (lowered to `Ptr`). Walks the UTF-8 buffer
    /// by byte offset, yielding each codepoint as a `char` (`i32`) — one pass,
    /// O(n) regardless of the ASCII flag (D-0025).
    fn lower_for_str(
        &mut self,
        target: &Expr,
        s: Value,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let name = match target {
            Expr::Name(n, _) => n.clone(),
            _ => {
                return Err(err(
                    "only a simple loop variable is supported by codegen yet",
                ))
            }
        };

        self.push_scope();
        // Byte cursor into the UTF-8 buffer, and the total byte length. Inside a
        // generator the cursor, the string handle, and the length must survive a
        // `yield` in the body, so the cursor is a hoisted slot and the handle /
        // length are spilled and reloaded (I#453); outside a generator the
        // cursor is a plain alloca and both stay SSA.
        let off = self.for_control_slot(Ty::I64);
        let zero = self.iconst(Ty::I64, 0);
        self.store(off, zero);
        let nbytes = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(nbytes),
            func: "zap_str_len".to_string(),
            args: vec![s],
        });
        let nbytes_slot = self.spill_invariant(nbytes, Ty::I64);
        let s_slot = self.spill_invariant(s, Ty::Ptr);
        // Iterating a string binds each codepoint as a `char` (#709).
        let var = self.decl_slot(&name, Ty::Char);

        let cond_bb = self.new_block();
        let body_bb = self.new_block();
        let incr_bb = self.new_block();
        let end_bb = self.new_block();

        self.set_term(Term::Br(cond_bb));
        self.position(cond_bb);
        let cur = self.load(off, Ty::I64);
        let nbytes_c = self.reload_invariant(nbytes_slot, nbytes, Ty::I64);
        let c = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: c,
            pred: CmpPred::Lt,
            lhs: cur,
            rhs: nbytes_c,
        });
        self.set_term(Term::BrIf {
            cond: c,
            then_bb: body_bb,
            else_bb: end_bb,
        });

        self.position(body_bb);
        let cur_off = self.load(off, Ty::I64);
        let s_b = self.reload_invariant(s_slot, s, Ty::Ptr);
        let cp = self.new_value(Ty::Char);
        self.emit(Inst::Call {
            dest: Some(cp),
            func: "zap_str_cp_at_byte".to_string(),
            args: vec![s_b, cur_off],
        });
        self.store(var, cp);
        self.push_loop(incr_bb, end_bb);
        for st in &body.stmts {
            self.stmt(st)?;
        }
        self.loops.pop();
        self.ensure_br(incr_bb);

        self.position(incr_bb);
        let cur2 = self.load(off, Ty::I64);
        let s_i = self.reload_invariant(s_slot, s, Ty::Ptr);
        let next = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(next),
            func: "zap_str_adv_byte".to_string(),
            args: vec![s_i, cur2],
        });
        self.store(off, next);
        self.set_term(Term::Br(cond_bb));

        self.position(end_bb);
        self.pop_scope();
        Ok(())
    }

    /// `for x in s` where `s : set[T]`. Snapshots the set's live elements into a
    /// fresh list (sets are unordered; iteration order is unspecified) and then
    /// iterates that list.
    fn lower_for_set(
        &mut self,
        target: &Expr,
        set_val: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let items = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_set_items".to_string(),
            args: vec![set_val],
        });
        self.lower_for_list(target, items, elem, body)
    }

    /// `for k in d` where `d : dict[K, V]` (ticket #299). Snapshots the dict's
    /// keys into a fresh list (order unspecified — hash order) and iterates that
    /// list, exactly like `set`. `for k, v in d.items()` is the key/value form.
    fn lower_for_dict(
        &mut self,
        target: &Expr,
        dict_val: Value,
        key: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let keys = self.new_value(Ty::List(key));
        self.emit(Inst::Call {
            dest: Some(keys),
            func: format!("zap_dict_keys_{}", key.val_suffix()),
            args: vec![dict_val],
        });
        self.lower_for_list(target, keys, key, body)
    }

    /// `for x in <heap>` (D-0057): snapshot the heap into a list in priority
    /// (root-first) order via `zap_heap_items`, then iterate that list. The
    /// snapshot leaves the live heap untouched.
    fn lower_for_heap(
        &mut self,
        target: &Expr,
        heap_val: Value,
        elem: Elem,
        body: &ast::Block,
    ) -> Result<(), LowerError> {
        let items = self.new_value(Ty::List(elem));
        self.emit(Inst::Call {
            dest: Some(items),
            func: "zap_heap_items".to_string(),
            args: vec![heap_val],
        });
        self.lower_for_list(target, items, elem, body)
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
                    unescape(value)
                };
                // `b"..."` and `"..."` share the `ZapStr` object; only the static
                // type differs so byte-wise ops route correctly (I#337). The
                // literal's bytes are the UTF-8 encoding of the escaped text —
                // `unescape` has no `\xNN` form, so arbitrary non-UTF-8 bytes use
                // `bytes([...])`.
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
                if matches!(t, Ty::Array(_)) && matches!(op, AstUn::Neg | AstUn::BitNot) {
                    return Err(err("element-wise unary array operations are unsupported"));
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
            // `x is None` / `x is not None` (#611, D18): the common null/empty
            // check. The bare `None` operand has no standalone lowering, so this
            // is intercepted here (where the other operand's type is known)
            // rather than in `binop`, and emits the same empty-variant / null
            // test as `.is_none()` and `match case Optional.None`.
            Expr::Binary { op, lhs, rhs, .. }
                if matches!(op, AstBin::Is | AstBin::IsNot)
                    && (matches!(lhs.as_ref(), Expr::None(_))
                        || matches!(rhs.as_ref(), Expr::None(_))) =>
            {
                self.lower_is_none(matches!(op, AstBin::IsNot), lhs, rhs)
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
                self.binop(*op, l, lt, r, rt)
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
                if matches!(bty, Ty::Array(_)) {
                    return Err(err(format!("array attribute '{name}' is unsupported; arrays are one-dimensional and expose length via len(a)")));
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
                let (cv, cty) = self.expr(cond)?;
                let c = self.truthy(cv, cty);
                let (a, at) = self.expr(then)?;
                let (b, _bt) = self.expr(els)?;
                let d = self.new_value(at);
                self.emit(Inst::Select {
                    dest: d,
                    cond: c,
                    a,
                    b,
                });
                Ok((d, at))
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
            if let Expr::Name(tyname, _) = base.as_ref() {
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
                    if let Some(r) = modules::lower_builtin_module_call(self, tyname, name, args)? {
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
            if is_name(base, "channel") {
                return self.lower_chan_new(index, args);
            }
            if is_name(base, "heap") {
                return self.lower_heap_new(index, args);
            }
            if is_name(base, "set") {
                return self.lower_set_new(index, args);
            }
        }
        if let Expr::Name(fname, _) = callee {
            if fname == "sum" {
                if args.len() != 1 {
                    return Err(err("sum() takes exactly one argument"));
                }
                let (v, vt) = self.expr(&args[0].value)?;
                // An `array[T]` sums through its dedicated runtime reduction.
                if let Ty::Array(d) = vt {
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
                // genexpr builds no intermediate list). The accumulator type is
                // the element type; `+` is specialized by codegen per operand
                // type, so this handles both integer and float elements.
                return self.lower_sum_iterable(v, vt, &args[0].value);
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
                    } else if matches!(
                        vt,
                        Ty::Struct(_) | Ty::List(_) | Ty::Dict(_, _) | Ty::Set(_)
                    ) || vt == Ty::Complex
                        || vt == Ty::Bigint
                        || vt.is_float()
                        || vt == Ty::I1
                        || vt == Ty::Char
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
                // Size overloading (D-0047): `len(a)` -> `a.__len__()`.
                if let Ty::Struct(sid) = vt {
                    return self.call_dunder(sid, "__len__", vec![(v, vt)]);
                }
                let func = match vt {
                    Ty::List(_) => "zap_list_len",
                    Ty::Dict(..) => "zap_dict_len",
                    Ty::Set(_) => "zap_set_len",
                    Ty::Heap(_) => "zap_heap_len",
                    Ty::Array(_) => "zap_array_len",
                    // `str` and `bytes` both lower to `Ptr`. `len` is the byte
                    // length here: it is correct for `bytes` (indexed by byte),
                    // and for ASCII `str` it equals the codepoint count. Making
                    // `len(str)` codepoint-based (D-0025) requires telling `str`
                    // and `bytes` apart in the IR, where they share `Ptr` — a
                    // load-bearing change deferred for maintainer ratification
                    // (ticket #284). Indexing/slicing/iteration are already
                    // codepoint-based, so this only differs for non-ASCII `str`.
                    Ty::Ptr => "zap_str_len",
                    // `bytes`: byte count (I#337). `zap_str_len` returns the
                    // ZapStr length, which is the byte count for a `bytes`.
                    Ty::Bytes => "zap_str_len",
                    _ => {
                        return Err(err(
                            "len() expects a list, dict, set, heap, array, str, or bytes",
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
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_str_parse_float".to_string(),
                        args: vec![v],
                    });
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
                    return self.lower_free_call_assembled(fname, &ptys, ret, args);
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
            if let Some((slot, Ty::Struct(sid))) = self.lookup(fname) {
                let recv = self.load(slot, Ty::Struct(sid));
                let mut dargs = vec![(recv, Ty::Struct(sid))];
                for a in args {
                    dargs.push(self.expr(&a.value)?);
                }
                return self.call_dunder(sid, "__call__", dargs);
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
            if bty == Ty::Match {
                return self.lower_match_method(recv, name, args);
            }
            if bty == Ty::Py {
                // `obj.method(args)` on a Python handle (ticket #27, D-0012):
                // read the bound attribute, then call it with marshaled args.
                let callable = self.lower_py_getattr(recv, name);
                return self.lower_py_call(callable, args);
            }
            // `str` methods (ticket #284): `str` lowers to `Ptr`, so a method
            // call on a `Ptr` receiver is a string method (sema has already
            // rejected any other `Ptr`-typed method receiver).
            if bty == Ty::Ptr {
                return self.lower_str_method(recv, name, args);
            }
            let Ty::Struct(sid) = bty else {
                return Err(err("method call on a non-struct value"));
            };
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
            // A generator method call `obj.gen(args)` (I#429) constructs the
            // compiler-generated state struct — capturing the receiver in `self`'s
            // field and the arguments in the remaining parameter fields, starting
            // at `$state == 0` — rather than calling a function; the body was
            // compiled into the state struct's `__next__` (like a free generator,
            // I#67).
            if let Some(gi) = self.ctx.generators.get(&mangled).cloned() {
                return self.construct_generator_method(&gi, recv, args);
            }
            let (ptys, ret) = self
                .sigs
                .get(&mangled)
                .cloned()
                .ok_or_else(|| err(format!("no method '{name}' on that struct")))?;
            // `self` is passed by reference (a value-type mutating method acts
            // on the receiver variable, Swift-style); the remaining arguments
            // are copied when they are value structs (D-0051). An inline-eligible
            // argument is copied by the register/sret ABI, so no explicit copy is
            // emitted for it (see `copy_for_pass`).
            let mut argvals = vec![recv]; // self
                                          // Default arguments and out-of-order keyword arguments on methods
                                          // (I#481, parity with free functions I#298): assemble the explicit
                                          // arguments into declaration order, filling omitted defaulted
                                          // parameters, before lowering. The recorded `call_params` drop the
                                          // implicit `self`, so a plan slot `i` corresponds to parameter type
                                          // `ptys[i + 1]`. A variadic method (or one with no recorded metadata,
                                          // e.g. a derived method) keeps the positional lowering path.
            let ctx = self.ctx;
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

    // --- first-class function values (I#163) ---

    /// The shared `{code_ptr}` closure-record struct id for a captureless
    /// function value, interned once and reused.
    fn captureless_env_sid(&self) -> u32 {
        if let Some(sid) = *self.ctx.captureless_env.borrow() {
            return sid;
        }
        let sid = self.ctx.tuples.borrow_mut().push_layout(vec![Ty::Ptr]);
        self.ctx.closure_env_sids.borrow_mut().push(sid);
        *self.ctx.captureless_env.borrow_mut() = Some(sid);
        sid
    }

    /// The closure-record struct id `{code_ptr, cap_tys…}` for a value capturing
    /// `cap_tys` (the captureless one when empty).
    fn env_sid_for(&self, cap_tys: &[Ty]) -> u32 {
        if cap_tys.is_empty() {
            return self.captureless_env_sid();
        }
        let mut fields = Vec::with_capacity(cap_tys.len() + 1);
        fields.push(Ty::Ptr);
        fields.extend_from_slice(cap_tys);
        let sid = self.ctx.tuples.borrow_mut().push_layout(fields);
        self.ctx.closure_env_sids.borrow_mut().push(sid);
        sid
    }

    /// A fresh unique name for a compiler-generated closure function (I#163).
    fn fresh_closure_name(&self) -> String {
        let mut c = self.ctx.closure_counter.borrow_mut();
        let n = *c;
        *c += 1;
        format!("$closure${n}")
    }

    /// Build a first-class function value from a bare top-level function
    /// (I#163): generate (once) a wrapper whose code pointer takes the implicit
    /// env and calls the function, then emit the captureless closure record.
    fn fnval_of_name(&mut self, fname: &str) -> Result<(Value, Ty), LowerError> {
        let (params, ret) = self
            .sigs
            .get(fname)
            .cloned()
            .ok_or_else(|| err(format!("unknown function '{fname}'")))?;
        let sig_idx = self.ctx.fn_sigs.borrow_mut().intern(params.clone(), ret);
        let (wname, env_sid) = self.make_fnval_wrapper(fname, &params, ret);
        let dest = self.new_value(Ty::Func(sig_idx));
        self.emit(Inst::ClosureMake {
            dest,
            func: wname,
            env_sid,
            captures: Vec::new(),
        });
        Ok((dest, Ty::Func(sig_idx)))
    }

    /// Generate (once per source function) the wrapper `<f>$fnval(env, args…)`
    /// that ignores the env and calls `f(args…)` — the code pointer for a bare
    /// top-level function used as a value. Returns `(wrapper_name, env_sid)`.
    fn make_fnval_wrapper(&self, fname: &str, params: &[Ty], ret: Ty) -> (String, u32) {
        let env_sid = self.captureless_env_sid();
        if let Some(w) = self.ctx.fnval_wrappers.borrow().get(fname) {
            return (w.clone(), env_sid);
        }
        let wname = format!("{fname}$fnval");
        let mut params_z = Vec::with_capacity(params.len() + 1);
        params_z.push(Ty::Struct(env_sid));
        params_z.extend_from_slice(params);
        let mut lf = FnLower::new(
            wname.clone(),
            params_z,
            ret,
            false,
            false,
            self.sigs,
            self.ctx,
        );
        // Values 1..=n are the incoming args (value 0 is the ignored env).
        let argvals: Vec<Value> = (1..=params.len() as u32).collect();
        let dest = if ret == Ty::Void {
            None
        } else {
            Some(lf.new_value(ret))
        };
        lf.emit(Inst::Call {
            dest,
            func: fname.to_string(),
            args: argvals,
        });
        lf.set_term(Term::Ret(dest));
        self.ctx.generated.borrow_mut().push(lf.func);
        self.ctx
            .fnval_wrappers
            .borrow_mut()
            .insert(fname.to_string(), wname.clone());
        (wname, env_sid)
    }

    /// Lower a `lambda` to a first-class function value (I#163), given the
    /// expected signature `sig_idx` (the lambda's parameter types come from it —
    /// Zap lambdas are untyped on the surface, like Python's). Free variables
    /// bound in the enclosing scope are captured into a GC-managed closure
    /// record; the body becomes a lifted top-level function that reloads them.
    fn lower_lambda(
        &mut self,
        lam_params: &[String],
        body: &Expr,
        sig_idx: u32,
    ) -> Result<(Value, Ty), LowerError> {
        let sig = self.ctx.fn_sigs.borrow().get(sig_idx).clone();
        if lam_params.len() != sig.params.len() {
            return Err(err(format!(
                "this lambda has {} parameter(s) but its expected type takes {}",
                lam_params.len(),
                sig.params.len()
            )));
        }
        // Capture the free variables that are bound in the current scope; names
        // resolved elsewhere (top-level functions, imports, builtins) are left
        // to resolve directly inside the lifted body.
        let free = free_names_of(lam_params, body);
        let mut names = Vec::new();
        let mut cap_vals = Vec::new();
        let mut cap_tys = Vec::new();
        for name in &free {
            if let Some((slot, ty)) = self.lookup(name) {
                names.push(name.clone());
                cap_vals.push(self.load(slot, ty));
                cap_tys.push(ty);
            }
        }
        let env_sid = self.env_sid_for(&cap_tys);
        let fname = self.fresh_closure_name();
        let lifted = self.build_lifted_lambda(
            &fname,
            env_sid,
            &names,
            &cap_tys,
            lam_params,
            &sig.params,
            sig.ret,
            body,
        )?;
        self.ctx.generated.borrow_mut().push(lifted);
        let dest = self.new_value(Ty::Func(sig_idx));
        self.emit(Inst::ClosureMake {
            dest,
            func: fname,
            env_sid,
            captures: cap_vals,
        });
        Ok((dest, Ty::Func(sig_idx)))
    }

    /// Build the lifted top-level function for a lambda body (I#163): its ZIR
    /// param 0 is the closure record, from which the captured variables are
    /// reloaded; params 1.. are the lambda's parameters. The body expression is
    /// lowered and returned (coerced to the declared return type).
    #[allow(clippy::too_many_arguments)]
    fn build_lifted_lambda(
        &self,
        name: &str,
        env_sid: u32,
        captured: &[String],
        cap_tys: &[Ty],
        lam_params: &[String],
        param_tys: &[Ty],
        ret: Ty,
        body: &Expr,
    ) -> Result<Func, LowerError> {
        let mut params_z = Vec::with_capacity(param_tys.len() + 1);
        params_z.push(Ty::Struct(env_sid));
        params_z.extend_from_slice(param_tys);
        let mut lf = FnLower::new(
            name.to_string(),
            params_z,
            ret,
            false,
            false,
            self.sigs,
            self.ctx,
        );
        // Reload captured free variables from the closure record (param 0,
        // fields 1..) into fresh locals under their source names.
        let env_val: Value = 0;
        for (i, (cname, cty)) in captured.iter().zip(cap_tys).enumerate() {
            let got = lf.new_value(*cty);
            lf.emit(Inst::FieldGet {
                dest: got,
                base: env_val,
                sid: env_sid,
                idx: (i + 1) as u32,
            });
            let slot = lf.alloca(*cty);
            lf.store(slot, got);
            lf.declare(cname, slot, *cty);
        }
        // Bind the lambda parameters (ZIR values 1..=n).
        for (i, pname) in lam_params.iter().enumerate() {
            let pty = param_tys[i];
            let slot = lf.alloca(pty);
            lf.store(slot, (i + 1) as Value);
            lf.declare(pname, slot, pty);
        }
        let (v, vt) = lf.expr(body)?;
        if ret == Ty::Void {
            lf.set_term(Term::Ret(None));
        } else {
            let rv = lf.coerce(v, vt, ret);
            lf.set_term(Term::Ret(Some(rv)));
        }
        Ok(lf.func)
    }

    /// Collection literals take their element kinds from the destination when
    /// known (#637). In particular, an empty `{}` must not default to
    /// dict[int64, int64] when appended to list[dict[str, bool]]. Recurse through
    /// nested literals, but leave named values alone: a real type mismatch must
    /// still be rejected, never erased by relabelling its pointer.
    fn expr_collection_context(&mut self, e: &Expr, target: Ty) -> Result<(Value, Ty), LowerError> {
        match (e, target) {
            (Expr::List(items, _), Ty::List(elem)) => {
                self.lower_list_lit_expected(items, Some(elem))
            }
            (Expr::Dict(items, _), Ty::Dict(key, value)) => {
                self.lower_dict_lit_expected(items, Some((key, value)))
            }
            (
                Expr::IfElse {
                    cond, then, els, ..
                },
                Ty::List(_) | Ty::Dict(..),
            ) => {
                let (cv, cty) = self.expr(cond)?;
                let c = self.truthy(cv, cty);
                let (a, at) = self.expr_collection_context(then, target)?;
                let (b, bt) = self.expr_collection_context(els, target)?;
                if at != target || bt != target {
                    return Err(err(format!(
                        "conditional collection branches must match expected `{}` (found `{}` and `{}`)",
                        self.render_ty(&target), self.render_ty(&at), self.render_ty(&bt)
                    )));
                }
                let d = self.new_value(target);
                self.emit(Inst::Select {
                    dest: d,
                    cond: c,
                    a,
                    b,
                });
                Ok((d, target))
            }
            _ => self.expr(e),
        }
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
        // `None` coerced to a reference-typed slot. Mono rewrites `None` into
        // `Optional[T].None` wherever it threads an expected type (bindings,
        // returns — Fork I/O), but *not* for call arguments (`rewrite_call` lowers
        // each argument with no expected type) nor for an omitted parameter's
        // defaulted `None`. So a bare `Expr::None` can still reach here against an
        // `Optional[T]` target, and that target's `None` is NOT a raw null pointer:
        // a scalar-payload `Optional` tags its discriminant, so a null would make
        // `is_some()`/`is_none()` dereference nil at native runtime (#564).
        // Construct the canonical value instead — `gen_opt_none` yields the tagged
        // `None` for a scalar payload and the null pointer for a niche/pointer
        // payload, so niche `Optional`s are unchanged.
        if matches!(e, Expr::None(_)) && target.is_ptr() {
            if let Some(opt) = optional_layout(self.ctx, target) {
                return Ok(gen_opt_none(self, &opt));
            }
            // A plain reference type (a struct instance, `str`, or a collection):
            // its null value IS the null pointer. Member access on it then throws
            // (D-0007, EXC-4), enforced by the null-guard at the access site. Value
            // structs are not nullable (D-0014), so `None` is rejected for them.
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
            return Ok(());
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
        Err(LowerError { code, message })
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

    fn construct(&mut self, sid: u32, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
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
            let (v, vt) = self.expr(&a.value)?;
            let v = self.coerce(v, vt, fty);
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
            let (v, vt) = self.expr(&default)?;
            let v = self.coerce(v, vt, fty);
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
        Ok((dest, Ty::Struct(sid)))
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

    // --- enums (ticket #51, D-0018/D-0046) ---
    //
    // A tier-1 enum is its unboxed backing `i64`; a member access lowers to the
    // discriminant constant. A tier-3 enum is a GC-allocated tagged struct
    // `{ tag:i32, payload… }`; a variant construction allocates it, stores the
    // tag, and stores each payload field — reusing the struct machinery, so a
    // recursive variant field is simply a pointer to a heap enum value (which
    // breaks the otherwise-infinite size, D-0046). `match` branches on the
    // backing integer (tier 1) or the tag field (tier 3).

    /// Lower a qualified member/variant access with no call: `Color.RED`
    /// (tier-1 constant) or `Shape.Unit` (tier-3 payload-free variant).
    fn lower_enum_member(
        &mut self,
        enum_name: &str,
        member: &str,
    ) -> Result<(Value, Ty), LowerError> {
        let info = self
            .ctx
            .enums
            .get(enum_name)
            .cloned()
            .expect("caller checked this is an enum");
        if !info.is_sum {
            let val = info
                .member_value(member)
                .ok_or_else(|| err(format!("enum '{enum_name}' has no member '{member}'")))?;
            let ty = Ty::Enum(info.enum_id);
            return Ok((self.iconst(ty, val), ty));
        }
        // Tier-3: a bare variant must be payload-free (data-carrying variants are
        // constructed through `call`).
        let v = info
            .variant(member)
            .ok_or_else(|| err(format!("enum '{enum_name}' has no variant '{member}'")))?
            .clone();
        if !v.fields.is_empty() {
            return Err(err(format!(
                "variant '{enum_name}.{member}' carries data; construct it with \
                 '{enum_name}.{member}(...)'"
            )));
        }
        let sid = info.sid.expect("tier-3 enum has a struct id");
        // Niche optimization (I#154, D-0064 #4): the payload-free variant of a
        // niche enum is the null pointer (`Optional.None`), typed as the enum so
        // method dispatch still resolves.
        if info.niche.is_some() {
            let dest = self.new_value(Ty::Struct(sid));
            self.emit(Inst::NullConst { dest });
            return Ok((dest, Ty::Struct(sid)));
        }
        self.build_variant(sid, &v, &[])
    }

    /// Lower a qualified variant construction `Enum.Variant(args)` (D-0046):
    /// allocate the tagged struct, store the tag, and store each payload field
    /// (positional or named). Returns the enum value (a `Struct` pointer).
    fn lower_variant_construct(
        &mut self,
        enum_name: &str,
        variant: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let info = self
            .ctx
            .enums
            .get(enum_name)
            .cloned()
            .expect("caller checked this is an enum");
        if !info.is_sum {
            return Err(err(format!(
                "'{enum_name}' is a tier-1 enum; '{enum_name}.{variant}(...)' is not a \
                 constructor"
            )));
        }
        let v = info
            .variant(variant)
            .ok_or_else(|| err(format!("enum '{enum_name}' has no variant '{variant}'")))?
            .clone();
        let sid = info.sid.expect("tier-3 enum has a struct id");
        // Niche optimization (I#154, D-0064 #4): the one-field variant of a niche
        // enum (`Optional.Some(p)`) IS the payload pointer — no allocation, no tag.
        // The value keeps the enum's ZIR type so method dispatch resolves; it is
        // physically the payload pointer (both are opaque pointers to codegen).
        if let Some(n) = &info.niche {
            let payload = n.payload;
            if variant == n.some_variant {
                if args.len() != 1 {
                    return Err(err(format!(
                        "niche variant '{enum_name}.{variant}' takes exactly one value"
                    )));
                }
                let (val, vt) = self.expr(&args[0].value)?;
                let val = self.coerce(val, vt, payload);
                return Ok((val, Ty::Struct(sid)));
            }
        }
        self.build_variant(sid, &v, args)
    }

    /// Allocate a tier-3 tagged struct for `variant` and fill the tag slot and
    /// its payload fields from `args` (named or positional, declaration order).
    fn build_variant(
        &mut self,
        sid: u32,
        variant: &VariantLow,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let dest = self.new_value(Ty::Struct(sid));
        self.emit(Inst::StructNew { dest, sid });
        // Tag slot (field 0).
        let tag = self.iconst(Ty::I32, variant.tag);
        self.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: 0,
            val: tag,
        });
        // Payload fields.
        let mut pos = 0usize;
        for a in args {
            let (fidx, fty) = match &a.name {
                Some(fname) => variant
                    .fields
                    .iter()
                    .find(|(n, _, _)| n == fname)
                    .map(|(_, idx, t)| (*idx, *t))
                    .ok_or_else(|| {
                        err(format!("variant '{}' has no field '{fname}'", variant.name))
                    })?,
                None => {
                    let f = variant
                        .fields
                        .get(pos)
                        .ok_or_else(|| err("too many fields in variant constructor"))?;
                    pos += 1;
                    (f.1, f.2)
                }
            };
            let (val, vt) = self.expr(&a.value)?;
            let val = self.coerce(val, vt, fty);
            self.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: fidx,
                val,
            });
        }
        Ok((dest, Ty::Struct(sid)))
    }

    /// Lower a `match` statement (D-0040/D-0046). The scrutinee is evaluated
    /// once; arms are tested top-to-bottom like an `if`/`elif` chain. The test
    /// compares the backing integer (tier-1 enum / literal) or the tag field
    /// (tier-3 enum); a matching arm binds its captures/payload fields, checks
    /// any guard, and runs its body. Exhaustiveness is already enforced by sema.
    fn lower_match(&mut self, scrutinee: &Expr, arms: &[ast::MatchArm]) -> Result<(), LowerError> {
        let (sv, sty) = self.expr(scrutinee)?;
        let merge = self.new_block();
        for arm in arms {
            // A statically incompatible tuple shape cannot reach its bindings.
            if let Pattern::Tuple(patterns, _) = &arm.pattern {
                let fields = if let Ty::Struct(sid) = sty {
                    self.ctx.tuples.borrow().fields(sid)
                } else {
                    None
                };
                let rest = patterns.iter().any(|p| matches!(p, Pattern::Rest { .. }));
                if fields.map_or(true, |fields| {
                    if rest {
                        fields.len() + 1 < patterns.len()
                    } else {
                        fields.len() != patterns.len()
                    }
                }) {
                    continue;
                }
            }
            let (cond, _catch_all) = self.pattern_cond(sv, sty, &arm.pattern)?;
            let then_bb = self.new_block();
            let next_bb = self.new_block();
            self.set_term(Term::BrIf {
                cond,
                then_bb,
                else_bb: next_bb,
            });
            self.position(then_bb);
            self.push_scope();
            self.bind_pattern(sv, sty, &arm.pattern)?;
            if let Some(g) = &arm.guard {
                // A guard runs after binding; on failure fall through to the
                // next arm (it does not count toward exhaustiveness — sema).
                let (gv, gty) = self.expr(g)?;
                let gc = self.truthy(gv, gty);
                let body_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: gc,
                    then_bb: body_bb,
                    else_bb: next_bb,
                });
                self.position(body_bb);
            }
            self.lower_block(&arm.body)?;
            self.ensure_br(merge);
            self.pop_scope();
            self.position(next_bb);
        }
        // Fall-through (no arm matched): for a non-enum scrutinee this is the
        // Python no-op; for an exhaustive enum sema guarantees this is dead.
        self.ensure_br(merge);
        self.position(merge);
        Ok(())
    }

    /// Emit the test for one pattern against the scrutinee and return its `i1`
    /// condition plus whether the pattern is an irrefutable catch-all.
    fn pattern_cond(
        &mut self,
        sv: Value,
        sty: Ty,
        pat: &Pattern,
    ) -> Result<(Value, bool), LowerError> {
        match pat {
            Pattern::Wildcard(_) | Pattern::Capture { .. } => Ok((self.bconst(true), true)),
            Pattern::Literal(e) => {
                let (lv, lt) = self.expr(e)?;
                let (c, _) = self.binop(AstBin::Eq, sv, sty, lv, lt)?;
                Ok((c, false))
            }
            Pattern::Or(ps, _) => {
                let mut acc: Option<Value> = None;
                let mut catch_all = false;
                for sub in ps {
                    let (c, ca) = self.pattern_cond(sv, sty, sub)?;
                    catch_all |= ca;
                    acc = Some(match acc {
                        None => c,
                        Some(prev) => {
                            let d = self.new_value(Ty::I1);
                            self.emit(Inst::Bin {
                                dest: d,
                                op: BinOp::Or,
                                lhs: prev,
                                rhs: c,
                            });
                            d
                        }
                    });
                }
                Ok((acc.unwrap_or_else(|| self.bconst(false)), catch_all))
            }
            Pattern::As { pattern, .. } => {
                // `<pattern> as name` tests exactly the inner pattern; the
                // binding is added in `bind_pattern` (D11, #603).
                self.pattern_cond(sv, sty, pattern)
            }
            Pattern::Struct {
                path,
                args,
                span: _,
            } => self.variant_cond(sv, sty, path, args.as_deref()),
            Pattern::Tuple(patterns, _) => {
                let Ty::Struct(sid) = sty else {
                    return Ok((self.bconst(false), false));
                };
                let fields = self.ctx.tuples.borrow().fields(sid);
                let rest = patterns
                    .iter()
                    .position(|p| matches!(p, Pattern::Rest { .. }));
                let exact = rest.is_none();
                let Some(fields) = fields.filter(|fields| {
                    if exact {
                        fields.len() == patterns.len()
                    } else {
                        fields.len() + 1 >= patterns.len()
                    }
                }) else {
                    return Ok((self.bconst(false), false));
                };
                let required = patterns.len() - usize::from(rest.is_some());
                let len_ok = self.bconst(if exact {
                    fields.len() == required
                } else {
                    fields.len() >= required
                });
                let mut result = self.bconst(true);
                for (index, pattern) in patterns.iter().enumerate() {
                    if matches!(pattern, Pattern::Rest { .. }) {
                        continue;
                    }
                    let field_index = if rest.is_some_and(|r| index > r) {
                        fields.len() - (patterns.len() - index)
                    } else {
                        index
                    };
                    let field_ty = fields[field_index];
                    let value = self.new_value(field_ty);
                    self.emit(Inst::FieldGet {
                        dest: value,
                        base: sv,
                        sid,
                        idx: field_index as u32,
                    });
                    let (condition, _) = self.pattern_cond(value, field_ty, pattern)?;
                    let combined = self.new_value(Ty::I1);
                    self.emit(Inst::Bin {
                        dest: combined,
                        op: BinOp::And,
                        lhs: result,
                        rhs: condition,
                    });
                    result = combined;
                }
                let combined = self.new_value(Ty::I1);
                self.emit(Inst::Bin {
                    dest: combined,
                    op: BinOp::And,
                    lhs: result,
                    rhs: len_ok,
                });
                result = combined;
                Ok((result, false))
            }
            Pattern::Sequence(patterns, _) => {
                let Ty::List(elem) = sty else {
                    return Ok((self.bconst(false), false));
                };
                let rest = patterns
                    .iter()
                    .position(|p| matches!(p, Pattern::Rest { .. }));
                let len = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_list_len".to_string(),
                    args: vec![sv],
                });
                let needed = self.iconst(
                    Ty::I64,
                    (patterns.len() - usize::from(rest.is_some())) as i64,
                );
                let len_ok = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: len_ok,
                    pred: if rest.is_some() {
                        CmpPred::Ge
                    } else {
                        CmpPred::Eq
                    },
                    lhs: len,
                    rhs: needed,
                });
                let result = self.guarded_pattern(len_ok, |this| {
                    let mut result = this.bconst(true);
                    for (index, pattern) in patterns.iter().enumerate() {
                        if matches!(pattern, Pattern::Rest { .. }) {
                            continue;
                        }
                        let idx = if rest.is_some_and(|r| index > r) {
                            let off = patterns.len() - index;
                            let n = this.iconst(Ty::I64, off as i64);
                            let v = this.new_value(Ty::I64);
                            this.emit(Inst::Bin {
                                dest: v,
                                op: BinOp::Sub,
                                lhs: len,
                                rhs: n,
                            });
                            v
                        } else {
                            this.iconst(Ty::I64, index as i64)
                        };
                        let ety = this.elem_ty(elem);
                        let value = this.new_value(ety);
                        this.emit(Inst::Call {
                            dest: Some(value),
                            func: format!("zap_list_get_{}", elem.val_suffix()),
                            args: vec![sv, idx],
                        });
                        let (condition, _) = this.pattern_cond(value, ety, pattern)?;
                        let combined = this.new_value(Ty::I1);
                        this.emit(Inst::Bin {
                            dest: combined,
                            op: BinOp::And,
                            lhs: result,
                            rhs: condition,
                        });
                        result = combined;
                    }
                    Ok(result)
                })?;
                Ok((result, false))
            }
            Pattern::Rest { .. } => Ok((self.bconst(true), true)),
        }
    }

    /// Evaluate a nested test only after its enclosing shape/length matched.
    fn guarded_pattern(
        &mut self,
        condition: Value,
        test: impl FnOnce(&mut Self) -> Result<Value, LowerError>,
    ) -> Result<Value, LowerError> {
        let slot = self.alloca(Ty::I1);
        let no = self.bconst(false);
        self.store(slot, no);
        let yes_bb = self.new_block();
        let end = self.new_block();
        self.set_term(Term::BrIf {
            cond: condition,
            then_bb: yes_bb,
            else_bb: end,
        });
        self.position(yes_bb);
        let result = test(self)?;
        self.store(slot, result);
        self.ensure_br(end);
        self.position(end);
        Ok(self.load(slot, Ty::I1))
    }

    fn pattern_fields(
        &self,
        sid: u32,
        args: &[PatArg],
    ) -> Result<Vec<(u32, Ty, Pattern)>, LowerError> {
        let mut pos = 0;
        args.iter()
            .map(|a| {
                let (idx, ty) = if let Some(name) = &a.name {
                    self.ctx
                        .field(sid, name)
                        .ok_or_else(|| err(format!("unknown pattern field '{name}'")))?
                } else {
                    let idx = pos;
                    pos += 1;
                    let ty = self.ctx.fields[sid as usize]
                        .get(idx)
                        .ok_or_else(|| err("too many pattern fields"))?
                        .1;
                    (idx as u32, ty)
                };
                Ok((idx, ty, a.pattern.clone()))
            })
            .collect()
    }

    fn pattern_field_conditions(
        &mut self,
        sv: Value,
        sid: u32,
        fields: &[(u32, Ty, Pattern)],
    ) -> Result<Value, LowerError> {
        let mut result = self.bconst(true);
        for (idx, ty, pattern) in fields {
            result = self.guarded_pattern(result, |this| {
                let value = this.field_get(sv, sid, *idx, *ty);
                Ok(this.pattern_cond(value, *ty, pattern)?.0)
            })?;
        }
        Ok(result)
    }

    /// Test the outer constructor before inspecting any nested payload.
    fn variant_cond(
        &mut self,
        sv: Value,
        sty: Ty,
        path: &[String],
        args: Option<&[PatArg]>,
    ) -> Result<(Value, bool), LowerError> {
        if path.len() == 1 {
            let sid = *self
                .ctx
                .ids
                .get(&path[0])
                .ok_or_else(|| err("unknown struct pattern"))?;
            if sty != Ty::Struct(sid) {
                return Ok((self.bconst(false), false));
            }
            let fields = self.pattern_fields(sid, args.unwrap_or(&[]))?;
            let nonnull = if self.ctx.is_value_struct(sid) {
                self.bconst(true)
            } else {
                let null = self.new_value(Ty::I1);
                self.emit(Inst::IsNull {
                    dest: null,
                    val: sv,
                });
                let value = self.new_value(Ty::I1);
                self.emit(Inst::Un {
                    dest: value,
                    op: UnOp::Not,
                    val: null,
                });
                value
            };
            let condition = self.guarded_pattern(nonnull, |this| {
                this.pattern_field_conditions(sv, sid, &fields)
            })?;
            return Ok((condition, false));
        }
        if path.len() != 2 {
            return Err(err("invalid constructor pattern"));
        }
        let info = self
            .ctx
            .enums
            .get(&path[0])
            .cloned()
            .ok_or_else(|| err("unknown enum pattern"))?;
        if !info.is_sum {
            let val = info
                .member_value(&path[1])
                .ok_or_else(|| err("unknown enum member"))?;
            let k = self.iconst(Ty::I64, val);
            return Ok((self.binop(AstBin::Eq, sv, sty, k, Ty::I64)?.0, false));
        }
        let v = info
            .variant(&path[1])
            .ok_or_else(|| err("unknown enum variant"))?;
        let sid = info.sid.expect("sum enum layout");
        if let Some(n) = &info.niche {
            let null = self.new_value(Ty::I1);
            self.emit(Inst::IsNull {
                dest: null,
                val: sv,
            });
            if path[1] == n.none_variant {
                return Ok((null, false));
            }
            let nonnull = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: nonnull,
                op: UnOp::Not,
                val: null,
            });
            let condition = self.guarded_pattern(nonnull, |this| {
                if let Some(a) = args.and_then(|a| a.first()) {
                    Ok(this.pattern_cond(sv, n.payload, &a.pattern)?.0)
                } else {
                    Ok(this.bconst(true))
                }
            })?;
            return Ok((condition, false));
        }
        let mut fields = Vec::new();
        let mut pos = 0;
        for a in args.unwrap_or(&[]) {
            let (_, idx, ty) = if let Some(name) = &a.name {
                v.fields.iter().find(|(n, _, _)| n == name)
            } else {
                let field = v.fields.get(pos);
                pos += 1;
                field
            }
            .ok_or_else(|| err("unknown variant pattern field"))?;
            fields.push((*idx, *ty, a.pattern.clone()));
        }
        let tag = self.field_get(sv, sid, 0, Ty::I32);
        let expected = self.iconst(Ty::I32, v.tag);
        let condition = self.binop(AstBin::Eq, tag, Ty::I32, expected, Ty::I32)?.0;
        let condition = self.guarded_pattern(condition, |this| {
            this.pattern_field_conditions(sv, sid, &fields)
        })?;
        Ok((condition, false))
    }

    /// Declare the bindings a matched pattern introduces into the current scope.
    fn bind_pattern(&mut self, sv: Value, sty: Ty, pat: &Pattern) -> Result<(), LowerError> {
        match pat {
            Pattern::Capture { name, .. } => {
                let slot = self.alloca(sty);
                self.store(slot, sv);
                self.declare(name, slot, sty);
                Ok(())
            }
            Pattern::As { pattern, name, .. } => {
                // Bind the inner pattern's own captures first, then bind `name`
                // to the whole matched value — same slot/type as a bare capture
                // of the scrutinee (D11, #603).
                self.bind_pattern(sv, sty, pattern)?;
                let slot = self.alloca(sty);
                self.store(slot, sv);
                self.declare(name, slot, sty);
                Ok(())
            }
            Pattern::Tuple(patterns, _) => {
                let Ty::Struct(sid) = sty else {
                    return Ok(());
                };
                let Some(fields) = self.ctx.tuples.borrow().fields(sid) else {
                    return Ok(());
                };
                let rest = patterns
                    .iter()
                    .position(|p| matches!(p, Pattern::Rest { .. }));
                if if rest.is_some() {
                    fields.len() + 1 < patterns.len()
                } else {
                    fields.len() != patterns.len()
                } {
                    return Ok(());
                }
                let mut index = 0usize;
                for (i, pattern) in patterns.iter().enumerate() {
                    if let Pattern::Rest {
                        name: Some(name), ..
                    } = pattern
                    {
                        let after = patterns.len() - i - 1;
                        let end = fields.len().saturating_sub(after);
                        let types = fields[index..end].to_vec();
                        let sid2 = self.ctx.tuples.borrow_mut().intern(types.clone());
                        let value = self.new_value(Ty::Struct(sid2));
                        self.emit(Inst::StructNew {
                            dest: value,
                            sid: sid2,
                        });
                        for (j, ty) in types.iter().enumerate() {
                            let item = self.field_get(sv, sid, (index + j) as u32, *ty);
                            self.emit(Inst::FieldSet {
                                base: value,
                                sid: sid2,
                                idx: j as u32,
                                val: item,
                            });
                        }
                        let slot = self.alloca(Ty::Struct(sid2));
                        self.store(slot, value);
                        self.declare(name, slot, Ty::Struct(sid2));
                        index = end;
                    } else if !matches!(pattern, Pattern::Rest { .. }) {
                        if rest.is_some_and(|r| i > r) {
                            index = fields.len().saturating_sub(patterns.len() - i);
                        }
                        let ty = fields[index];
                        let item = self.field_get(sv, sid, index as u32, ty);
                        self.bind_pattern(item, ty, pattern)?;
                        index += 1;
                    }
                }
                Ok(())
            }
            Pattern::Sequence(patterns, _) => {
                let Ty::List(elem) = sty else {
                    return Ok(());
                };
                let len = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_list_len".into(),
                    args: vec![sv],
                });
                let rest = patterns
                    .iter()
                    .position(|p| matches!(p, Pattern::Rest { .. }));
                for (i, pattern) in patterns.iter().enumerate() {
                    if let Pattern::Rest {
                        name: Some(name), ..
                    } = pattern
                    {
                        let lo = self.iconst(Ty::I64, i as i64);
                        let hi = self.iconst(Ty::I64, -((patterns.len() - i - 1) as i64));
                        let step = self.iconst(Ty::I64, 1);
                        let flag = self.iconst(Ty::I64, 1);
                        let no = self.iconst(Ty::I64, 0);
                        let value = self.new_value(Ty::List(elem));
                        self.emit(Inst::Call {
                            dest: Some(value),
                            func: "zap_list_slice".into(),
                            args: vec![
                                sv,
                                lo,
                                hi,
                                step,
                                flag,
                                if i + 1 == patterns.len() { no } else { flag },
                                no,
                            ],
                        });
                        let slot = self.alloca(Ty::List(elem));
                        self.store(slot, value);
                        self.declare(name, slot, Ty::List(elem));
                    } else if !matches!(pattern, Pattern::Rest { .. }) {
                        let idx = if rest.is_some_and(|r| i > r) {
                            let off = self.iconst(Ty::I64, (patterns.len() - i) as i64);
                            let v = self.new_value(Ty::I64);
                            self.emit(Inst::Bin {
                                dest: v,
                                op: BinOp::Sub,
                                lhs: len,
                                rhs: off,
                            });
                            v
                        } else {
                            self.iconst(Ty::I64, i as i64)
                        };
                        let ety = self.elem_ty(elem);
                        let item = self.new_value(ety);
                        self.emit(Inst::Call {
                            dest: Some(item),
                            func: format!("zap_list_get_{}", elem.val_suffix()),
                            args: vec![sv, idx],
                        });
                        self.bind_pattern(item, ety, pattern)?;
                    }
                }
                Ok(())
            }
            Pattern::Rest {
                name: Some(name), ..
            } => {
                let slot = self.alloca(sty);
                self.store(slot, sv);
                self.declare(name, slot, sty);
                Ok(())
            }
            Pattern::Or(patterns, _) => {
                let entry = self.cur;
                let end = self.new_block();
                let mut shared: HashMap<String, (Value, Ty)> = HashMap::new();
                for pattern in patterns {
                    let condition = self.pattern_cond(sv, sty, pattern)?.0;
                    let matched = self.new_block();
                    let next = self.new_block();
                    self.set_term(Term::BrIf {
                        cond: condition,
                        then_bb: matched,
                        else_bb: next,
                    });
                    self.position(matched);
                    self.push_scope();
                    self.bind_pattern(sv, sty, pattern)?;
                    let mut bindings: Vec<_> = self.scopes.pop().unwrap().into_iter().collect();
                    bindings.sort_by(|a, b| a.0.cmp(&b.0));
                    for (name, (local, ty)) in bindings {
                        let slot = if let Some((slot, expected)) = shared.get(&name) {
                            if *expected != ty {
                                return Err(err("or-pattern bindings must have matching types"));
                            }
                            *slot
                        } else {
                            let slot = self.new_value(Ty::Ptr);
                            self.func.blocks[entry as usize].insts.push(Inst::Alloca {
                                dest: slot,
                                pointee: ty,
                            });
                            shared.insert(name, (slot, ty));
                            slot
                        };
                        let value = self.load(local, ty);
                        self.store(slot, value);
                    }
                    self.ensure_br(end);
                    self.position(next);
                }
                self.set_term(Term::Unreachable);
                self.position(end);
                for (name, (slot, ty)) in shared {
                    self.declare(&name, slot, ty);
                }
                Ok(())
            }
            Pattern::Struct {
                path,
                args: Some(args),
                ..
            } if path.len() == 1 => {
                let sid = *self
                    .ctx
                    .ids
                    .get(&path[0])
                    .ok_or_else(|| err("unknown struct pattern"))?;
                for (idx, ty, pattern) in self.pattern_fields(sid, args)? {
                    self.bind_field(sv, sid, idx, ty, &pattern)?;
                }
                Ok(())
            }
            Pattern::Struct {
                path,
                args: Some(args),
                ..
            } if path.len() == 2 => {
                let info = self
                    .ctx
                    .enums
                    .get(&path[0])
                    .cloned()
                    .ok_or_else(|| err(format!("'{}' is not an enum", path[0])))?;
                // Niche optimization (I#154, D-0064 #4): `Some(x)` binds `x` to the
                // pointer itself (reinterpreted as the payload type); `None` binds
                // nothing. There is no tagged struct to read a field from.
                if let Some(n) = &info.niche {
                    if path[1] == n.some_variant {
                        if let Some(a) = args.first() {
                            self.bind_pattern(sv, n.payload, &a.pattern)?;
                        }
                    }
                    return Ok(());
                }
                let Some(v) = info.variant(&path[1]).cloned() else {
                    return Ok(());
                };
                let sid = info.sid.expect("tier-3 enum has a struct id");
                let mut pos = 0usize;
                for a in args {
                    let field = match &a.name {
                        Some(fname) => v.fields.iter().find(|(n, _, _)| n == fname).cloned(),
                        None => {
                            let f = v.fields.get(pos).cloned();
                            pos += 1;
                            f
                        }
                    };
                    let Some((_, fidx, fty)) = field else {
                        continue;
                    };
                    self.bind_field(sv, sid, fidx, fty, &a.pattern)?;
                }
                Ok(())
            }
            // Payload-free qualified constant (`Color.RED`, `Shape.Unit`) and
            // other refutable leaves bind nothing.
            _ => Ok(()),
        }
    }

    /// Bind one payload field's value to the capture in its sub-pattern.
    fn bind_field(
        &mut self,
        base: Value,
        sid: u32,
        fidx: u32,
        fty: Ty,
        sub: &Pattern,
    ) -> Result<(), LowerError> {
        match sub {
            Pattern::Wildcard(_) => Ok(()),
            _ => {
                let fv = self.new_value(fty);
                self.emit(Inst::FieldGet {
                    dest: fv,
                    base,
                    sid,
                    idx: fidx,
                });
                self.bind_pattern(fv, fty, sub)
            }
        }
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

    /// `array[dtype]([e0, e1, ...])` — allocate an array of the literal's length
    /// and fill it element-by-element. N-D literals, non-literal arguments
    /// (`array[f64](xs)`), and `zeros`/`arange` constructors are deferred.
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
            return Err(err("array[T](...) takes one positional flat list literal"));
        }
        let Expr::List(elems, _) = &args[0].value else {
            return Err(err(
                "array[T](...) takes a list literal, e.g. array[f64]([1.0, 2.0, 3.0]) \
                 (N-D and non-literal construction are not supported yet)",
            ));
        };
        let code = self.iconst(Ty::I64, dtype.code());
        let n = self.iconst(Ty::I64, elems.len() as i64);
        let arr = self.new_value(Ty::Array(dtype));
        self.emit(Inst::Call {
            dest: Some(arr),
            func: "zap_array_new".to_string(),
            args: vec![code, n],
        });
        let setter = format!("zap_array_set_{}", dtype.suffix());
        for (i, e) in elems.iter().enumerate() {
            let (v, vt) = self.expr(e)?;
            if vt.int_width().is_none() && !vt.is_float() {
                return Err(err(
                    "array construction requires numeric scalars in one flat list literal",
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
        Ok((arr, Ty::Array(dtype)))
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
        // A channel stores its elements as uniform 8-byte words (D-0031), so a
        // bare scalar/reference name classifies directly. A **tuple** element is
        // a value aggregate: delivering one across the channel ABI needs
        // boxing/unboxing this slice does not implement (unlike a set/dict key,
        // which hashes a tuple through its key functions), so it is rejected
        // cleanly here with a specific message rather than the generic
        // "expects an element type name" (#703; tuple channels are a follow-up).
        let elem = match elem_expr {
            Expr::Name(ename, _) => chan_elem_from_name(ename)?,
            Expr::Tuple(..) => {
                return Err(err(
                    "channels of tuple elements are not supported yet (a tuple is a \
                     value aggregate; channel elements are uniform words)",
                ))
            }
            _ => {
                return Err(err(
                    "channel[...] expects an element type name, e.g. channel[int64]",
                ))
            }
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
                    if list_elem_kind(t, &s.ctx.nested)? != e {
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
            let v = s.box_if_value(v, ek.ty());
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
                            if elem_kind(kt, &self.ctx.nested)? != kk
                                || elem_kind(vt, &self.ctx.nested)? != vk
                            {
                                return Err(err(
                                    "dict keys/values must all have the same representation",
                                ));
                            }
                            (kk, vk)
                        }
                        None => {
                            let kk = elem_kind(kt, &self.ctx.nested)?;
                            let vk = elem_kind(vt, &self.ctx.nested)?;
                            kinds.set(Some((kk, vk)));
                            (kk, vk)
                        }
                    };
                    let kv = self.coerce(kv, kt, kk.ty());
                    let vv = self.coerce(vv, vt, vk.ty());
                    let kv = self.box_if_value(kv, kk.ty());
                    let vv = self.box_if_value(vv, vk.ty());
                    let kfns = self.key_fn_args(kk)?;
                    let mut cargs = vec![dict, kv, vv];
                    cargs.extend_from_slice(&kfns);
                    self.emit(Inst::Call {
                        dest: None,
                        func: format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                        args: cargs,
                    });
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
            s.emit(Inst::Call {
                dest: None,
                func: format!("zap_set_add_{}", ek.key_suffix()),
                args: cargs,
            });
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
        match args.len() {
            0 => {
                let set = self.new_value(Ty::Set(elem));
                self.emit(Inst::Call {
                    dest: Some(set),
                    func: "zap_set_new".to_string(),
                    args: vec![],
                });
                Ok((set, Ty::Set(elem)))
            }
            1 if args[0].name.is_none() => self.lower_set_from_iterable(&args[0].value, Some(elem)),
            _ => Err(err(
                "set[T](...) takes at most one positional iterable argument",
            )),
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
            s.emit(Inst::Call {
                dest: None,
                func: format!("zap_set_add_{}", ek.key_suffix()),
                args: cargs,
            });
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
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_set_add_{}", elem.key_suffix()),
                    args: cargs,
                });
                Ok((self.iconst(Ty::I32, 0), Ty::I32))
            }
            other => Err(err(format!("set has no method '{other}' (supported: add)"))),
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
                "dict has no method '{other}' (supported: keys, values, items, copy, get)"
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

        // The key, coerced to the dict's key kind, plus the struct-key hash/eq
        // function pointers (empty for scalar/string keys), shared by the
        // presence probe and the value fetch.
        let (kv, kt) = self.expr(&args[0].value)?;
        let key = self.coerce(kv, kt, kk.ty());
        let kfns = self.key_fn_args(kk)?;

        // present = (contains(d, key) != 0)
        let word = self.new_value(Ty::I64);
        let mut cargs = vec![recv, key];
        cargs.extend_from_slice(&kfns);
        self.emit(Inst::Call {
            dest: Some(word),
            func: format!("zap_dict_contains_{}", kk.key_suffix()),
            args: cargs,
        });
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
        let mut gargs = vec![recv, key];
        gargs.extend_from_slice(&kfns);
        self.emit(Inst::Call {
            dest: Some(raw),
            func: format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix()),
            args: gargs,
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

        // The key, coerced to the dict's key kind, plus the struct-key hash/eq
        // function pointers (empty for scalar/string keys), shared by the
        // presence probe and the value fetch.
        let (kv, kt) = self.expr(&args[0].value)?;
        let key = self.coerce(kv, kt, kk.ty());
        let kfns = self.key_fn_args(kk)?;

        // The default, coerced to the dict's value kind — the same `V` the fetch
        // yields, so both arms agree on the slot's type.
        let (dv, dt) = self.expr(&args[1].value)?;
        let default = self.coerce(dv, dt, vk.ty());

        // present = (contains(d, key) != 0)
        let word = self.new_value(Ty::I64);
        let mut cargs = vec![recv, key];
        cargs.extend_from_slice(&kfns);
        self.emit(Inst::Call {
            dest: Some(word),
            func: format!("zap_dict_contains_{}", kk.key_suffix()),
            args: cargs,
        });
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
        let mut gargs = vec![recv, key];
        gargs.extend_from_slice(&kfns);
        self.emit(Inst::Call {
            dest: Some(raw),
            func: format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix()),
            args: gargs,
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
        for a in args {
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
                None => {
                    return Err(err(
                        "heap[T](...) takes only the keyword argument 'cmp' (e.g. cmp=\"max\")",
                    ))
                }
            }
        }
        let kind = self.iconst(Ty::I64, if elem == Elem::I64 { 0 } else { 1 });
        let d = self.iconst(Ty::I64, dir);
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
                if elem_kind(vt, &self.ctx.nested)? != elem {
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
            other => Err(err(format!(
                "heap has no method '{other}' (supported: push, pop, peek)"
            ))),
        }
    }

    /// Lower a tuple literal (D-0049): a by-value anonymous-struct aggregate. The
    /// element-type sequence interns to a struct id, then `StructNew` + one
    /// `FieldSet` per element build the value — exactly the struct path, so
    /// codegen needs no tuple-specific handling.
    fn lower_tuple_lit(&mut self, elems: &[Expr]) -> Result<(Value, Ty), LowerError> {
        let mut vals = Vec::with_capacity(elems.len());
        let mut tys = Vec::with_capacity(elems.len());
        for e in elems {
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

    fn lower_index_get(
        &mut self,
        base: &Expr,
        index: &Expr,
        span: Span,
    ) -> Result<(Value, Ty), LowerError> {
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
                let iv = self.coerce(iv, it, Ty::I64);
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
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    args: cargs,
                });
                Ok((d, vty))
            }
            Ty::Array(d) => {
                // One-dimensional access; reject multi-axis indices before coercion.
                let (iv, it) = self.expr(index)?;
                if it.int_width().is_none() {
                    return Err(err("array indexing requires one integer index; multi-axis indexing is unsupported"));
                }
                let iv = self.coerce(iv, it, Ty::I64);
                let dest = self.new_value(d.ty());
                self.emit(Inst::Call {
                    dest: Some(dest),
                    func: format!("zap_array_get_{}", d.suffix()),
                    args: vec![bv, iv],
                });
                Ok((dest, d.ty()))
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
                let iv = self.coerce(iv, it, Ty::I64);
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
                let iv = self.coerce(iv, it, Ty::I64);
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
            // `array[T]` slice -> a VIEW sharing the buffer (D-0030). A view is
            // contiguous, so only an absent or literal-`1` step is supported; a
            // strided view would need strides the 1-D array has not got (a
            // documented D-0030 follow-up).
            Ty::Array(dt) => {
                if let Some(s) = step {
                    if const_int_value(s) != Some(1) {
                        return Err(err(
                            "a stepped array slice is not supported yet (an array slice is a \
                             contiguous view and strided views are a D-0030 follow-up)",
                        ));
                    }
                }
                let (lo_v, has_lo) = self.array_slice_bound(lo)?;
                let (hi_v, has_hi) = self.array_slice_bound(hi)?;
                let out = Ty::Array(dt);
                let d = self.new_value(out);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_array_slice".to_string(),
                    args: vec![bv, lo_v, hi_v, has_lo, has_hi],
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
        // Indexed float stores already widen integers via `coerce`.
        if list && expected == Elem::F64 && actual.int_width().is_some() {
            return Ok(());
        }
        let kind = if list {
            list_elem_kind(actual, &self.ctx.nested)?
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
                let iv = self.coerce(iv, it, Ty::I64);
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
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    args: cargs,
                });
                Ok(())
            }
            Ty::Array(d) => {
                let (iv, it) = self.expr(index)?;
                if it.int_width().is_none() {
                    return Err(err("array indexing requires one integer index; multi-axis indexing is unsupported"));
                }
                let iv = self.coerce(iv, it, Ty::I64);
                let (v, vt) = self.expr(value)?;
                if vt.int_width().is_none() && !vt.is_float() {
                    return Err(err("array element assignment requires a numeric scalar"));
                }
                let v = self.coerce(v, vt, d.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_array_set_{}", d.suffix()),
                    args: vec![bv, iv, v],
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
        } else {
            Err(err(
                "only variable, field, and index assignment are supported by codegen yet",
            ))
        }
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
                let iv = self.coerce(iv, it, Ty::I64);
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
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_dict_set_{}_{}", kk.key_suffix(), vk.val_suffix()),
                    args: cargs,
                });
                Ok(())
            }
            Ty::Array(d) => {
                let (iv, it) = self.expr(index)?;
                if it.int_width().is_none() {
                    return Err(err("array indexing requires one integer index; multi-axis indexing is unsupported"));
                }
                let iv = self.coerce(iv, it, Ty::I64);
                if vt.int_width().is_none() && !vt.is_float() {
                    return Err(err("array element assignment requires a numeric scalar"));
                }
                let v = self.coerce(v, vt, d.ty());
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_array_set_{}", d.suffix()),
                    args: vec![bv, iv, v],
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
                let iv = self.coerce(iv, it, Ty::I64);
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
                self.emit(Inst::Call {
                    dest: None,
                    func: format!("zap_dict_del_{}", kk.key_suffix()),
                    args: cargs,
                });
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
                if list_elem_kind(vt, &self.ctx.nested)? != elem {
                    return Err(err("append() value has the wrong element type"));
                }
                let v = self.coerce(v, vt, elem.ty());
                let v = self.box_if_value(v, elem.ty());
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
            other => Err(err(format!(
                "list has no method '{other}' (supported: append, sort)"
            ))),
        }
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
                Elem::Ptr => Ok(1),
                // Ordering `f64` elements directly is a clean follow-up (the
                // runtime sort compares elements as `i64`/str words today); a
                // `list[float]` is still sortable via a `key=` projection.
                // A nested collection (#625) is not directly orderable (the
                // runtime compares elements as `i64`/str words), like `f64` and
                // structs; sort it via a `key=` projection.
                Elem::F64 | Elem::Struct(_) | Elem::Nested(_) => Err(err(format!(
                    "{what} is not orderable; pass key= to sort by an orderable \
                     projection (an int or str)"
                ))),
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

    // --- regex (ticket #23, D-0010) ------------------------------------------
    //
    // `m/.../`, `s/.../.../`, `=~`/`!~` and the match object all lower to calls
    // into the PCRE2-backed runtime (`zap_regex_*`); no new ZIR instruction is
    // needed (the string/collection pattern). Literal patterns are emitted as
    // string constants and compiled by the runtime on each call — a compiled-
    // pattern cache is future work (design/0001 step 4). A match object is a
    // GC'd heap value that owns its captured groups, so capture state is
    // coroutine-safe by construction (no magic globals, D-0010).

    /// `subject =~ m/pattern/flags` → a match object (`Ty::Match`, null = no
    /// match), or, for `!~`, an `i1` that is true when there is no match.
    fn lower_regex_match(
        &mut self,
        subject: &Expr,
        pattern: &str,
        flags: &str,
        negated: bool,
    ) -> Result<(Value, Ty), LowerError> {
        let (sv, _st) = self.expr(subject)?;
        let pv = self.strconst(pattern);
        let fv = self.iconst(Ty::I64, regex_flag_bits(flags));
        let m = self.new_value(Ty::Match);
        self.emit(Inst::Call {
            dest: Some(m),
            func: "zap_regex_match".to_string(),
            args: vec![sv, pv, fv],
        });
        if !negated {
            return Ok((m, Ty::Match));
        }
        // `!~`: true exactly when the match object is null.
        let matched = self.new_value(Ty::I64);
        self.emit(Inst::Call {
            dest: Some(matched),
            func: "zap_ptr_nonnull".to_string(),
            args: vec![m],
        });
        let zero = self.iconst(Ty::I64, 0);
        let d = self.new_value(Ty::I1);
        self.emit(Inst::ICmp {
            dest: d,
            pred: CmpPred::Eq,
            lhs: matched,
            rhs: zero,
        });
        Ok((d, Ty::I1))
    }

    /// `subject =~ s/pattern/replacement/flags` → a new string (the subject is
    /// never mutated). The `g` flag replaces all occurrences, else the first.
    fn lower_regex_subst(
        &mut self,
        subject: &Expr,
        pattern: &str,
        replacement: &str,
        flags: &str,
    ) -> Result<(Value, Ty), LowerError> {
        let (sv, _st) = self.expr(subject)?;
        let pv = self.strconst(pattern);
        let rv = self.strconst(replacement);
        let fv = self.iconst(Ty::I64, regex_flag_bits(flags));
        let d = self.new_value(Ty::Ptr);
        self.emit(Inst::Call {
            dest: Some(d),
            func: "zap_regex_subst".to_string(),
            args: vec![sv, pv, rv, fv],
        });
        Ok((d, Ty::Ptr))
    }

    /// `str` methods (ticket #284, D-0025). The receiver `recv` is the string
    /// (`Ptr`). Sema has already type-checked the method name, arity, and
    /// argument types, so this is a mechanical lowering to the `zap_str_*`
    /// runtime entry points. Case folding / whitespace are ASCII in v1.
    fn lower_str_method(
        &mut self,
        recv: Value,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        // Lower a single `str`-typed argument (its ZIR type is `Ptr`).
        let one_str = |s: &mut Self| -> Result<Value, LowerError> {
            if args.len() != 1 {
                return Err(err(format!("str.{name}() takes exactly one argument")));
            }
            let (v, _t) = s.expr(&args[0].value)?;
            Ok(v)
        };
        match name {
            "upper" | "lower" => {
                if !args.is_empty() {
                    return Err(err(format!("str.{name}() takes no arguments")));
                }
                let func = if name == "upper" {
                    "zap_str_upper"
                } else {
                    "zap_str_lower"
                };
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![recv],
                });
                Ok((d, Ty::Ptr))
            }
            "strip" | "lstrip" | "rstrip" => {
                if !args.is_empty() {
                    return Err(err(format!("str.{name}() takes no arguments")));
                }
                let mode = match name {
                    "strip" => 0,
                    "lstrip" => 1,
                    _ => 2,
                };
                let m = self.iconst(Ty::I64, mode);
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_strip".to_string(),
                    args: vec![recv, m],
                });
                Ok((d, Ty::Ptr))
            }
            "startswith" | "endswith" | "contains" => {
                let a = one_str(self)?;
                let func = match name {
                    "startswith" => "zap_str_startswith",
                    "endswith" => "zap_str_endswith",
                    _ => "zap_str_contains",
                };
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: func.to_string(),
                    args: vec![recv, a],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: raw,
                    rhs: zero,
                });
                Ok((d, Ty::I1))
            }
            "find" | "index" => {
                let a = one_str(self)?;
                let func = if name == "find" {
                    "zap_str_find"
                } else {
                    "zap_str_index_of"
                };
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![recv, a],
                });
                Ok((d, Ty::I64))
            }
            "replace" => {
                if args.len() != 2 {
                    return Err(err("str.replace() takes exactly two arguments"));
                }
                let (o, _) = self.expr(&args[0].value)?;
                let (n, _) = self.expr(&args[1].value)?;
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_replace".to_string(),
                    args: vec![recv, o, n],
                });
                Ok((d, Ty::Ptr))
            }
            "split" => {
                let a = one_str(self)?;
                let d = self.new_value(Ty::List(Elem::Ptr));
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_split".to_string(),
                    args: vec![recv, a],
                });
                Ok((d, Ty::List(Elem::Ptr)))
            }
            "join" => {
                // `sep.join(parts)`: the receiver is the separator; the single
                // argument is a `list[str]`.
                if args.len() != 1 {
                    return Err(err("str.join() takes exactly one argument"));
                }
                let (lv, _lt) = self.expr(&args[0].value)?;
                let d = self.new_value(Ty::Ptr);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_join".to_string(),
                    args: vec![recv, lv],
                });
                Ok((d, Ty::Ptr))
            }
            "encode" => {
                // UTF-8 byte values as a `list[int]` — the reachable byte view
                // for SigV4 until the full `bytes` type lands.
                if !args.is_empty() {
                    return Err(err("str.encode() takes no arguments"));
                }
                let d = self.new_value(Ty::List(Elem::I64));
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_encode".to_string(),
                    args: vec![recv],
                });
                Ok((d, Ty::List(Elem::I64)))
            }
            _ => Err(err(format!("str has no method '{name}'"))),
        }
    }

    /// Methods on a match object: `m.group(n)` (positional, 0 = whole match) and
    /// `m.group("name")` (named capture). Both return a `str` (empty if the
    /// group did not participate).
    fn lower_match_method(
        &mut self,
        recv: Value,
        name: &str,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        match name {
            "group" => {
                if args.len() != 1 {
                    return Err(err("group() takes exactly one argument"));
                }
                let (arg, aty) = self.expr(&args[0].value)?;
                let d = self.new_value(Ty::Ptr);
                if aty == Ty::Ptr {
                    // Named group: `m.group("year")`.
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_regex_group_name".to_string(),
                        args: vec![recv, arg],
                    });
                } else {
                    // Positional group: `m.group(0)`.
                    let idx = self.coerce(arg, aty, Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_regex_group".to_string(),
                        args: vec![recv, idx],
                    });
                }
                Ok((d, Ty::Ptr))
            }
            other => Err(err(format!(
                "a match object has no method '{other}' (supported: group)"
            ))),
        }
    }

    // --- Python interop (ticket #27, D-0012, design/0003-python-interop.md) ---
    //
    // `import python "mod" as m` makes `m` a Python module handle; attribute
    // access and calls dispatch through the embedded CPython C-API, and values
    // marshal across the boundary. Nothing here needs a new ZIR instruction (the
    // strings/collections/regex pattern): every operation lowers to a
    // `zap_py_*` runtime call. A Python value flows as `Ty::Py` — a GC-managed
    // handle that owns one CPython reference (decref'd by a finalizer). Calls
    // that can surface a Python exception use `throwing_call`, so a raised
    // Python error becomes a Zap exception catchable with `try`/`except`
    // (D-0007): inside a `try` it is an `invoke` to the handler, else it unwinds
    // to the caller / top level like any other `zap_panic`.

    /// `import python "module"` → a module handle. Importing is idempotent
    /// (CPython caches modules), so each reference re-imports cheaply.
    fn lower_py_import(&mut self, module: &str) -> Value {
        let name = self.strconst(module);
        let d = self.new_value(Ty::Py);
        self.throwing_call(Some(d), "zap_py_import", vec![name]);
        d
    }

    /// `obj.attr` on a Python handle → the attribute value (another handle).
    fn lower_py_getattr(&mut self, base: Value, name: &str) -> Value {
        let nv = self.strconst(name);
        let d = self.new_value(Ty::Py);
        self.throwing_call(Some(d), "zap_py_getattr", vec![base, nv]);
        d
    }

    /// Call a Python callable with the given Zap argument expressions. Arguments
    /// are marshaled to Python values and collected into a `list[ptr]` the
    /// runtime turns into a call tuple (any arity; keyword arguments are a
    /// documented follow-up).
    fn lower_py_call(
        &mut self,
        callable: Value,
        args: &[ast::Arg],
    ) -> Result<(Value, Ty), LowerError> {
        let argl = self.new_value(Ty::List(Elem::Ptr));
        self.emit(Inst::Call {
            dest: Some(argl),
            func: "zap_list_new".to_string(),
            args: vec![],
        });
        for a in args {
            if a.name.is_some() {
                return Err(err(
                    "keyword arguments to Python calls are not supported yet",
                ));
            }
            let p = self.marshal_to_py(&a.value)?;
            self.emit(Inst::Call {
                dest: None,
                func: "zap_list_push_ptr".to_string(),
                args: vec![argl, p],
            });
        }
        let d = self.new_value(Ty::Py);
        self.throwing_call(Some(d), "zap_py_call", vec![callable, argl]);
        Ok((d, Ty::Py))
    }

    /// Marshal one Zap value into a Python handle (ticket #27 marshaling table).
    /// Scalars and `str` convert by value; a value already on the Python side is
    /// passed through. Collections and other types are a documented follow-up.
    fn marshal_to_py(&mut self, e: &Expr) -> Result<Value, LowerError> {
        let (v, vt) = self.expr(e)?;
        let (helper, arg) = if vt == Ty::Py {
            return Ok(v);
        } else if vt == Ty::I1 {
            ("zap_py_from_bool", self.coerce(v, vt, Ty::I64))
        } else if vt.int_width().is_some() {
            ("zap_py_from_i64", self.coerce(v, vt, Ty::I64))
        } else if vt.is_float() {
            ("zap_py_from_f64", self.as_f64(v, vt))
        } else if vt == Ty::Ptr {
            ("zap_py_from_str", v)
        } else {
            return Err(err(
                "cannot marshal this value to Python yet (supported: int, float, bool, str, and Python handles)",
            ));
        };
        let d = self.new_value(Ty::Py);
        self.emit(Inst::Call {
            dest: Some(d),
            func: helper.to_string(),
            args: vec![arg],
        });
        Ok(d)
    }

    /// Call a struct's dunder method (operator overloading, D-0047). `args` are
    /// `(value, type)` pairs starting with the receiver (`self`); each is coerced
    /// to the registered parameter type. A missing dunder is a clean error (sema
    /// normally catches it first, but the ir crate is exercised without sema).
    fn call_dunder(
        &mut self,
        sid: u32,
        dunder: &str,
        args: Vec<(Value, Ty)>,
    ) -> Result<(Value, Ty), LowerError> {
        let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
        let (ptys, ret) = self.sigs.get(&mangled).cloned().ok_or_else(|| {
            err(format!(
                "type '{}' does not define '{}' (required by this operator; D-0047)",
                self.ctx.names[sid as usize], dunder
            ))
        })?;
        let mut argvals = Vec::with_capacity(args.len());
        for (i, (v, vt)) in args.into_iter().enumerate() {
            let target = *ptys.get(i).unwrap_or(&vt);
            argvals.push(self.coerce(v, vt, target));
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

    /// The extra runtime arguments for a collection *key* of the given element
    /// kind: for a struct key (D-0047), the `__hash__` and `__eq__` function
    /// pointers the `_fn` entry points take; for any other key, none. A struct
    /// key without both dunders is a clean error (needs `@derive(Hash, Eq)` or
    /// hand-written methods).
    fn key_fn_args(&mut self, elem: Elem) -> Result<Vec<Value>, LowerError> {
        // A nested collection (#625) is unhashable: a `list`/`dict`/`set` cannot be
        // a dict key or a set element. Without this guard it would silently take
        // the pointer-key (`str`) path and hash the collection's pointer as if it
        // were a `ZapStr` — a silent miscompile. Fail loudly instead.
        if let Elem::Nested(id) = elem {
            let ty = self.ctx.nested.borrow().get(id);
            return Err(err(format!(
                "a {ty} is unhashable and cannot be used as a dict key or a set \
                 element (collections have no stable hash)"
            )));
        }
        let Elem::Struct(sid) = elem else {
            return Ok(vec![]);
        };
        // A tuple key (#680, D-0049): tuples intern as struct ids at or past
        // `TupleInterner.base` (= `names.len()`), so they have no `ctx.names`
        // entry — indexing `names[sid]` here was the ICE. A tuple is hashable
        // exactly when all its elements are (CPython parity), so synthesize its
        // structural `__hash__`/`__eq__` on demand and route it through the same
        // function-pointer key path as a named `@derive(Hash, Eq)` struct.
        let (hashn, eqn) = if self.ctx.tuples.borrow().is_tuple(sid) {
            self.tuple_key_fns(sid)?
        } else if optional_layout(self.ctx, Ty::Struct(sid)).is_some() {
            // An `Optional[T]` key (#704): `None` and a present value are both
            // hashable (CPython parity). Optional lowers as a tagged/niche enum
            // value, so — like a tuple — synthesize its structural
            // `__hash__`/`__eq__` and route it through the fn-key path. A
            // sentinel hash for `None`, delegation to `T` for `Some`.
            self.optional_key_fns(sid)?
        } else {
            let name = self.ctx.names[sid as usize].clone();
            let hashn = format!("{name}__{}", "__hash__");
            let eqn = format!("{name}__{}", "__eq__");
            if !self.sigs.contains_key(&hashn) {
                return Err(err(format!(
                    "type '{name}' needs '__hash__' to be a dict/set key \
                     (use @derive(Hash) or define it; D-0047)"
                )));
            }
            if !self.sigs.contains_key(&eqn) {
                return Err(err(format!(
                    "type '{name}' needs '__eq__' to be a dict/set key \
                     (use @derive(Eq) or define it; D-0047)"
                )));
            }
            (hashn, eqn)
        };
        let hv = self.new_value(Ty::Ptr);
        self.emit(Inst::FuncAddr {
            dest: hv,
            func: hashn,
        });
        let ev = self.new_value(Ty::Ptr);
        self.emit(Inst::FuncAddr {
            dest: ev,
            func: eqn,
        });
        Ok(vec![hv, ev])
    }

    /// Synthesize (once per tuple id, #680 / D-0049) the structural `__hash__`
    /// and `__eq__` for a tuple used as a dict key or set element, returning
    /// their symbol names. A tuple is an anonymous value-struct aggregate, so this
    /// mirrors `@derive(Hash, Eq)` over the interned element layout. The
    /// `__hash__(t) -> i64` seeds `0` and folds each element's hash in with
    /// `zap_hash_combine` — `zap_i64_hash` for integer/bool/char/enum words,
    /// `zap_str_hash` for `str`/`bytes`, and the element's own `__hash__` for a
    /// nested tuple or hashable struct — while `__eq__(a, b) -> i1` ANDs the
    /// per-element comparisons element-for-element.
    ///
    /// A tuple is hashable exactly when every element is (CPython parity); an
    /// unhashable element (float, a nested collection, or a struct lacking
    /// `__hash__`/`__eq__`) is rejected with a clean coded diagnostic — never an
    /// ICE. The generated functions are appended to [`StructCtx::generated`] and
    /// referenced by `Inst::FuncAddr`, like the named-struct key path; they need
    /// no signature registration (they are only ever called indirectly by the
    /// collection runtime). Reused across every key use of the same tuple layout.
    fn tuple_key_fns(&mut self, sid: u32) -> Result<(String, String), LowerError> {
        if let Some(pair) = self.ctx.synth_keyfns.borrow().get(&sid) {
            return Ok(pair.clone());
        }
        let fields = self
            .ctx
            .tuples
            .borrow()
            .fields(sid)
            .expect("tuple_key_fns called on a non-tuple struct id");
        // Resolve each element's per-element key handling first, so an unhashable
        // element fails cleanly before any function is emitted. For a nested
        // aggregate element this recurses to generate that element's key
        // functions; for a named struct it requires derived/defined dunders.
        let plan: Vec<ElemKey> = fields
            .iter()
            .map(|fty| self.elem_key_plan(*fty))
            .collect::<Result<_, _>>()?;

        let hashn = format!("$tuple${sid}$hash");
        let eqn = format!("$tuple${sid}$eq");

        // __hash__(tuple) -> i64: fold per-element hashes, like gen_hash_method.
        {
            let mut b = FnLower::new(
                hashn.clone(),
                vec![Ty::Struct(sid)],
                Ty::I64,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let mut acc = b.iconst(Ty::I64, 0);
            for (idx, (fty, key)) in fields.iter().zip(&plan).enumerate() {
                let fv = b.new_value(*fty);
                b.emit(Inst::FieldGet {
                    dest: fv,
                    base: 0,
                    sid,
                    idx: idx as u32,
                });
                let h = match key {
                    ElemKey::Str => {
                        let d = b.new_value(Ty::I64);
                        b.emit(Inst::Call {
                            dest: Some(d),
                            func: "zap_str_hash".to_string(),
                            args: vec![fv],
                        });
                        d
                    }
                    ElemKey::Int => {
                        let i = b.coerce(fv, *fty, Ty::I64);
                        let d = b.new_value(Ty::I64);
                        b.emit(Inst::Call {
                            dest: Some(d),
                            func: "zap_i64_hash".to_string(),
                            args: vec![i],
                        });
                        d
                    }
                    ElemKey::Struct(ch, _) => {
                        let d = b.new_value(Ty::I64);
                        b.emit(Inst::Call {
                            dest: Some(d),
                            func: ch.clone(),
                            args: vec![fv],
                        });
                        d
                    }
                };
                let next = b.new_value(Ty::I64);
                b.emit(Inst::Call {
                    dest: Some(next),
                    func: "zap_hash_combine".to_string(),
                    args: vec![acc, h],
                });
                acc = next;
            }
            b.set_term(Term::Ret(Some(acc)));
            self.ctx.generated.borrow_mut().push(b.func);
        }

        // __eq__(a, b) -> i1: AND the per-element comparisons, like gen_eq_method.
        {
            let mut b = FnLower::new(
                eqn.clone(),
                vec![Ty::Struct(sid), Ty::Struct(sid)],
                Ty::I1,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let mut acc = b.iconst(Ty::I1, 1);
            for (idx, (fty, key)) in fields.iter().zip(&plan).enumerate() {
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
                let bit = match key {
                    ElemKey::Str => {
                        let word = b.new_value(Ty::I64);
                        b.emit(Inst::Call {
                            dest: Some(word),
                            func: "zap_str_eq".to_string(),
                            args: vec![a, c],
                        });
                        let zero = b.iconst(Ty::I64, 0);
                        let bit = b.new_value(Ty::I1);
                        b.emit(Inst::ICmp {
                            dest: bit,
                            pred: CmpPred::Ne,
                            lhs: word,
                            rhs: zero,
                        });
                        bit
                    }
                    ElemKey::Int => {
                        let bit = b.new_value(Ty::I1);
                        b.emit(Inst::ICmp {
                            dest: bit,
                            pred: CmpPred::Eq,
                            lhs: a,
                            rhs: c,
                        });
                        bit
                    }
                    ElemKey::Struct(_, ce) => {
                        let bit = b.new_value(Ty::I1);
                        b.emit(Inst::Call {
                            dest: Some(bit),
                            func: ce.clone(),
                            args: vec![a, c],
                        });
                        bit
                    }
                };
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
            self.ctx.generated.borrow_mut().push(b.func);
        }

        let pair = (hashn, eqn);
        self.ctx.synth_keyfns.borrow_mut().insert(sid, pair.clone());
        Ok(pair)
    }

    /// Classify one element/payload type for the structural key path — shared by
    /// a tuple's per-field handling (#680, D-0049) and an `Optional[T]`'s payload
    /// (#704, D-0122). An unhashable element yields a clean coded diagnostic,
    /// never an ICE; an aggregate element (a nested tuple, an `Optional[T]`, or a
    /// `@derive(Hash, Eq)` struct) resolves to its own `(__hash__, __eq__)`,
    /// recursing to synthesize them on demand.
    fn elem_key_plan(&mut self, fty: Ty) -> Result<ElemKey, LowerError> {
        Ok(match fty {
            Ty::Ptr | Ty::Bytes => ElemKey::Str,
            t if t.int_width().is_some() => ElemKey::Int,
            Ty::Struct(csid) => {
                if self.ctx.tuples.borrow().is_tuple(csid) {
                    let (ch, ce) = self.tuple_key_fns(csid)?;
                    ElemKey::Struct(ch, ce)
                } else if optional_layout(self.ctx, Ty::Struct(csid)).is_some() {
                    let (ch, ce) = self.optional_key_fns(csid)?;
                    ElemKey::Struct(ch, ce)
                } else {
                    let name = self.ctx.names[csid as usize].clone();
                    let ch = format!("{name}____hash__");
                    let ce = format!("{name}____eq__");
                    if !self.sigs.contains_key(&ch) || !self.sigs.contains_key(&ce) {
                        return Err(err(format!(
                            "a '{name}' element needs '__hash__' and '__eq__' to be part of \
                             a dict key or set element (use @derive(Hash, Eq) or define \
                             them; D-0047, D-0049)"
                        )));
                    }
                    ElemKey::Struct(ch, ce)
                }
            }
            other => {
                return Err(err(format!(
                    "a '{other}' element is not hashable and cannot be a dict key or set \
                     element (only integers, bools, char, enums, str, bytes, and \
                     tuples/Optionals/structs of those; D-0049, D-0122)"
                )))
            }
        })
    }

    /// Synthesize (once per `Optional[T]` instance id, #704 / D-0122) the
    /// structural `__hash__` and `__eq__` for an `Optional[T]` used as a dict key
    /// or set element, returning their symbol names. CPython parity: `None` and a
    /// present value are both hashable, so — like a tuple — the Optional routes
    /// through the function-pointer key path (D-0047). `__hash__(o) -> i64`
    /// returns a fixed sentinel for `None` and delegates to `T`'s hashing for
    /// `Some(x)`; `__eq__(a, b) -> i1` is true iff both are `None`, or both are
    /// `Some` with equal payloads — so `None` never compares equal to any present
    /// value (a hash collision between the sentinel and a payload hash is
    /// therefore harmless). An unhashable `T` (e.g. `Optional[float]`) is rejected
    /// with the same clean coded diagnostic as elsewhere, never an ICE. The
    /// generated functions append to [`StructCtx::generated`] and are referenced
    /// by `Inst::FuncAddr`, needing no signature registration (they are only ever
    /// called indirectly by the collection runtime). Reused across every key use
    /// of the same Optional instance, and the tagged/niche layout is handled by
    /// the shared `gen_opt_*` constructors (#564, D-0064).
    fn optional_key_fns(&mut self, sid: u32) -> Result<(String, String), LowerError> {
        if let Some(pair) = self.ctx.synth_keyfns.borrow().get(&sid) {
            return Ok(pair.clone());
        }
        let opt = optional_layout(self.ctx, Ty::Struct(sid))
            .expect("optional_key_fns called on a non-Optional struct id");
        // Classify the payload first, so an unhashable `T` fails cleanly before
        // any function is emitted. A `Some` payload that is itself a tuple or
        // Optional recurses to synthesize that layout's own key functions.
        let plan = self.elem_key_plan(opt.payload_ty)?;

        let hashn = format!("$opt${sid}$hash");
        let eqn = format!("$opt${sid}$eq");
        // A fixed, arbitrary nonzero hash for `None` (ASCII "none"). It need not
        // avoid payload hashes: `__eq__` keeps `None` distinct from every present
        // value, so a bucket collision only chains.
        const NONE_HASH: i64 = 0x6E6F_6E65;

        // __hash__(opt) -> i64: sentinel for `None`, delegate to `T` for `Some`.
        {
            let mut b = FnLower::new(
                hashn.clone(),
                vec![Ty::Struct(sid)],
                Ty::I64,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let fv: Value = 0; // the Optional parameter
            let slot = b.alloca(Ty::I64);
            let is_none = gen_opt_is_none(&mut b, &opt, fv);
            let none_bb = b.new_block();
            let some_bb = b.new_block();
            let end_bb = b.new_block();
            b.set_term(Term::BrIf {
                cond: is_none,
                then_bb: none_bb,
                else_bb: some_bb,
            });
            b.position(none_bb);
            let sentinel = b.iconst(Ty::I64, NONE_HASH);
            b.store(slot, sentinel);
            b.set_term(Term::Br(end_bb));
            b.position(some_bb);
            let payload = gen_opt_payload(&mut b, &opt, fv);
            let h = match &plan {
                ElemKey::Str => {
                    let d = b.new_value(Ty::I64);
                    b.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_str_hash".to_string(),
                        args: vec![payload],
                    });
                    d
                }
                ElemKey::Int => {
                    let i = b.coerce(payload, opt.payload_ty, Ty::I64);
                    let d = b.new_value(Ty::I64);
                    b.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_i64_hash".to_string(),
                        args: vec![i],
                    });
                    d
                }
                ElemKey::Struct(ch, _) => {
                    let d = b.new_value(Ty::I64);
                    b.emit(Inst::Call {
                        dest: Some(d),
                        func: ch.clone(),
                        args: vec![payload],
                    });
                    d
                }
            };
            b.store(slot, h);
            b.set_term(Term::Br(end_bb));
            b.position(end_bb);
            let acc = b.load(slot, Ty::I64);
            b.set_term(Term::Ret(Some(acc)));
            self.ctx.generated.borrow_mut().push(b.func);
        }

        // __eq__(a, b) -> i1: both None -> true; one None -> false; both Some ->
        // compare payloads. The IR has no phi, so the result merges through a slot.
        {
            let mut b = FnLower::new(
                eqn.clone(),
                vec![Ty::Struct(sid), Ty::Struct(sid)],
                Ty::I1,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            let a: Value = 0;
            let c: Value = 1;
            let slot = b.alloca(Ty::I1);
            let a_none = gen_opt_is_none(&mut b, &opt, a);
            let b_none = gen_opt_is_none(&mut b, &opt, c);
            let a_none_bb = b.new_block();
            let a_some_bb = b.new_block();
            let end_bb = b.new_block();
            b.set_term(Term::BrIf {
                cond: a_none,
                then_bb: a_none_bb,
                else_bb: a_some_bb,
            });
            // a is None: equal iff b is also None.
            b.position(a_none_bb);
            b.store(slot, b_none);
            b.set_term(Term::Br(end_bb));
            // a is Some: equal iff b is Some and the payloads compare equal.
            b.position(a_some_bb);
            let b_none_bb = b.new_block();
            let both_some_bb = b.new_block();
            b.set_term(Term::BrIf {
                cond: b_none,
                then_bb: b_none_bb,
                else_bb: both_some_bb,
            });
            b.position(b_none_bb);
            let f = b.bconst(false);
            b.store(slot, f);
            b.set_term(Term::Br(end_bb));
            b.position(both_some_bb);
            let pa = gen_opt_payload(&mut b, &opt, a);
            let pc = gen_opt_payload(&mut b, &opt, c);
            let bit = match &plan {
                ElemKey::Str => {
                    let word = b.new_value(Ty::I64);
                    b.emit(Inst::Call {
                        dest: Some(word),
                        func: "zap_str_eq".to_string(),
                        args: vec![pa, pc],
                    });
                    let zero = b.iconst(Ty::I64, 0);
                    let bit = b.new_value(Ty::I1);
                    b.emit(Inst::ICmp {
                        dest: bit,
                        pred: CmpPred::Ne,
                        lhs: word,
                        rhs: zero,
                    });
                    bit
                }
                ElemKey::Int => {
                    let bit = b.new_value(Ty::I1);
                    b.emit(Inst::ICmp {
                        dest: bit,
                        pred: CmpPred::Eq,
                        lhs: pa,
                        rhs: pc,
                    });
                    bit
                }
                ElemKey::Struct(_, ce) => {
                    let bit = b.new_value(Ty::I1);
                    b.emit(Inst::Call {
                        dest: Some(bit),
                        func: ce.clone(),
                        args: vec![pa, pc],
                    });
                    bit
                }
            };
            b.store(slot, bit);
            b.set_term(Term::Br(end_bb));
            b.position(end_bb);
            let r = b.load(slot, Ty::I1);
            b.set_term(Term::Ret(Some(r)));
            self.ctx.generated.borrow_mut().push(b.func);
        }

        let pair = (hashn, eqn);
        self.ctx.synth_keyfns.borrow_mut().insert(sid, pair.clone());
        Ok(pair)
    }

    /// Render a struct value to a string, for `print` / `str` / f-string / REPL
    /// contexts (D-0047; the general rendering protocol #274, D-0124). Resolves
    /// the type's renderer with [`Self::struct_repr_fn`] — an explicit `__str__`
    /// (preferred) or `__repr__`, else `@derive(Repr)`, else the compiler's
    /// default field-wise form `TypeName(field=value, …)` — and invokes it. The
    /// call is `throwing` because a user-defined `__str__`/`__repr__` (or a
    /// `bytes` field in the default render) may raise.
    fn struct_to_str(&mut self, sid: u32, v: Value) -> Result<Value, LowerError> {
        let f = self.struct_repr_fn(sid)?;
        let d = self.new_value(Ty::Ptr);
        self.throwing_call(Some(d), &f, vec![v]);
        Ok(d)
    }

    /// Render an **interface value** `v` of interface `iid` (#707, D-0124). The
    /// concrete type behind the fat pointer is erased at this site, so rendering
    /// cannot resolve a static renderer symbol the way [`Self::struct_to_str`]
    /// does; it emits an [`Inst::IfaceRender`] that dynamically dispatches through
    /// the itable's renderer slot to the concrete type's own renderer (recorded in
    /// [`StructCtx::iface_renderers`] at each concrete->interface coercion). The
    /// renderer may raise (a user `__str__`, or a `bytes` field decode), so it
    /// routes through the active landing pad exactly like [`Self::throwing_call`]:
    /// the [`Inst::IfaceRenderInvoke`] form inside a `try`, the plain
    /// [`Inst::IfaceRender`] otherwise. Always yields a `str`.
    fn render_iface_value(&mut self, iface: Value, iid: u32) -> Result<Value, LowerError> {
        let dest = self.new_value(Ty::Ptr);
        if let Some(&lp) = self.landing_pads.last() {
            self.used_lps.insert(lp);
            self.emit(Inst::IfaceRenderInvoke {
                dest,
                iface,
                iid,
                unwind: lp,
            });
        } else {
            self.emit(Inst::IfaceRender { dest, iface, iid });
        }
        Ok(dest)
    }

    /// Resolve the symbol name of the function that renders a value of struct id
    /// `sid` to a string, synthesizing the compiler default on demand (#274,
    /// D-0124). Resolution order (first match wins), so an explicit override
    /// always beats the default:
    ///
    /// 1. a tuple layout → the Python `(v0, v1, …)` renderer (synthesized once);
    /// 2. a hand-written `__str__`;
    /// 3. a hand-written or `@derive(Repr)` `__repr__`;
    /// 4. a tier-3 data-carrying enum (D-0046) → the tag-dispatch structural
    ///    renderer `Enum.Variant(field=value, …)` (#706, synthesized once);
    /// 5. otherwise the compiler-synthesized default `TypeName(field=value, …)`
    ///    renderer (synthesized once, memoized in [`StructCtx::synth_reprfns`]).
    ///
    /// Only an interface value's fat struct still has no default and is rejected
    /// with a clean coded error pointing at the #707 follow-up — never an ICE or a
    /// nonsense render over synthetic `$data`/`$itable` slots.
    fn struct_repr_fn(&mut self, sid: u32) -> Result<String, LowerError> {
        // Tuples have no `ctx.names` entry (their ids sit past `names.len()`), so
        // they MUST be handled before any `names[sid]` lookup — the same ordering
        // the tuple key-fn path relies on (#680).
        if self.ctx.tuples.borrow().is_tuple(sid) {
            if let Some(name) = self.ctx.synth_reprfns.borrow().get(&sid) {
                return Ok(name.clone());
            }
            let name = format!("$tuple${sid}$repr");
            // Memoize BEFORE emitting the body so a (hypothetically) self-referring
            // layout terminates; tuples cannot actually nest themselves, but this
            // keeps the invariant uniform with the struct path below.
            self.ctx
                .synth_reprfns
                .borrow_mut()
                .insert(sid, name.clone());
            let mut b = FnLower::new(
                name.clone(),
                vec![Ty::Struct(sid)],
                Ty::Ptr,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            if let Err(e) = gen_tuple_repr_body(&mut b, sid) {
                // Body generation failed (e.g. an unrenderable element type):
                // drop the memo so the half-built name never resolves to a missing
                // function if errors are collected rather than fatal.
                self.ctx.synth_reprfns.borrow_mut().remove(&sid);
                return Err(e);
            }
            self.ctx.generated.borrow_mut().push(b.func);
            return Ok(name);
        }
        // An interface value's synthetic `{data, itable}` fat struct has no static
        // renderer: its concrete type is erased, so it renders by **dynamic
        // dispatch** through the itable's renderer slot (#707, D-0124), which
        // [`FnLower::render_value`] emits as an [`Inst::IfaceRender`] *before*
        // reaching `struct_to_str`/here. Reaching this branch therefore means a fat
        // struct was handed to the static-renderer path directly — not a render
        // site — so it stays a clean coded error (never an ICE or a nonsense render
        // over the synthetic `$data`/`$itable` slots) rather than a reachable
        // user-facing limitation.
        if self.ctx.fat_to_iid.contains_key(&sid) {
            return Err(err(format!(
                "internal: interface value ('{}') reached the static renderer path; \
                 an interface value renders via dynamic dispatch (Inst::IfaceRender), \
                 not a static renderer symbol (#707, D-0124)",
                self.ctx.names[sid as usize]
            )));
        }
        // An explicit `__str__` (preferred) or `__repr__` / `@derive(Repr)` wins
        // over the default — for a tier-3 enum too (its methods mangle to the
        // same `{Name}__{dunder}`), so this is tested BEFORE the enum's default
        // structural renderer below.
        for dunder in ["__str__", "__repr__"] {
            let mangled = format!("{}__{}", self.ctx.names[sid as usize], dunder);
            if self.sigs.contains_key(&mangled) {
                return Ok(mangled);
            }
        }
        // A tier-3 data-carrying enum (D-0046) lowers as a tagged struct
        // `{ $tag:i32, payload… }` (or a niche `Optional`-shaped pointer).
        // Render it structurally — `Enum.Variant(field=value, …)` for a
        // data-carrying variant, `Enum.Variant` for a payload-free one — via a
        // per-enum tag-dispatch renderer synthesized on demand (#706, D-0124
        // extended to D-0046), mirroring the default-struct synthesis below but
        // keyed on the tag like the tier-1 `.name` chain.
        if self
            .ctx
            .enums
            .values()
            .any(|e| e.is_sum && e.sid == Some(sid))
        {
            if let Some(name) = self.ctx.synth_reprfns.borrow().get(&sid) {
                return Ok(name.clone());
            }
            let name = format!("{}$repr$enum", self.ctx.names[sid as usize]);
            // Memoize BEFORE emitting the body so a recursive variant field whose
            // type is this same enum (`enum Expr: Add(Expr l, Expr r)`) resolves to
            // this name as a plain call — compile-time synthesis terminates (one
            // function per enum).
            self.ctx
                .synth_reprfns
                .borrow_mut()
                .insert(sid, name.clone());
            let mut b = FnLower::new(
                name.clone(),
                vec![Ty::Struct(sid)],
                Ty::Ptr,
                false,
                true,
                self.sigs,
                self.ctx,
            );
            if let Err(e) = gen_enum_repr_body(&mut b, sid) {
                // Drop the memo on failure (e.g. an unrenderable payload type), so
                // a collected-error build never calls a half-built, never-emitted
                // function.
                self.ctx.synth_reprfns.borrow_mut().remove(&sid);
                return Err(e);
            }
            self.ctx.generated.borrow_mut().push(b.func);
            return Ok(name);
        }
        // The compiler-synthesized default field-wise renderer.
        if let Some(name) = self.ctx.synth_reprfns.borrow().get(&sid) {
            return Ok(name.clone());
        }
        let name = format!("{}$repr$default", self.ctx.names[sid as usize]);
        // Memoize BEFORE emitting the body: a field whose type is this same struct
        // (directly or through a mutual cycle) resolves to this name as a plain
        // call, so compile-time synthesis terminates (one function per struct).
        self.ctx
            .synth_reprfns
            .borrow_mut()
            .insert(sid, name.clone());
        let mut b = FnLower::new(
            name.clone(),
            vec![Ty::Struct(sid)],
            Ty::Ptr,
            false,
            true,
            self.sigs,
            self.ctx,
        );
        if let Err(e) = gen_struct_repr_body(&mut b, sid) {
            // Drop the memo on failure (e.g. an unrenderable field type), so a
            // collected-error build never calls a half-built, never-emitted fn.
            self.ctx.synth_reprfns.borrow_mut().remove(&sid);
            return Err(e);
        }
        self.ctx.generated.borrow_mut().push(b.func);
        Ok(name)
    }

    /// Resolve the symbol name of the function that renders a `list`/`dict`/`set`
    /// value of ZIR type `vt` to its Python string form (#274, D-0124),
    /// synthesizing it on demand (memoized per container type):
    /// `[v0, v1]` / `{k0: v0, k1: v1}` / `{v0, v1}`, with the Python-exact empty
    /// forms `[]`, `{}`, and `set()`. Every element/key/value renders in `repr`
    /// mode (so string elements are quoted), through the shared value renderer —
    /// so containers nest (`[[1], [2]]`, `{'k': [1, 2]}`) and carry structs,
    /// tuples, enums, and `Optional` uniformly. The renderer reuses the tested
    /// list-iteration (`iterate_list`) and dict key/value entry points, so it is
    /// not a bespoke runtime path.
    fn container_repr_fn(&mut self, vt: Ty) -> Result<String, LowerError> {
        let key = format!("{vt:?}");
        if let Some(name) = self.ctx.synth_container_reprfns.borrow().get(&key) {
            return Ok(name.clone());
        }
        let name = format!("$crepr${}", self.ctx.synth_container_reprfns.borrow().len());
        let mut b = FnLower::new(
            name.clone(),
            vec![vt],
            Ty::Ptr,
            false,
            true,
            self.sigs,
            self.ctx,
        );
        // The container is the single `self` parameter: value 0.
        let c: Value = 0;
        match &vt {
            Ty::List(elem) => {
                let elem = *elem;
                let acc = b.alloca(Ty::Ptr);
                let first = b.alloca(Ty::I64);
                let open = b.strconst("[");
                b.store(acc, open);
                let one = b.iconst(Ty::I64, 1);
                b.store(first, one);
                b.iterate_list(c, elem, |s, e, _ety| {
                    gen_seq_sep(s, acc, first);
                    let es = render_elem_repr(s, e, elem)?;
                    let a = s.load(acc, Ty::Ptr);
                    let a2 = gen_concat(s, a, es);
                    s.store(acc, a2);
                    Ok(())
                })?;
                let a = b.load(acc, Ty::Ptr);
                let close = b.strconst("]");
                let a2 = gen_concat(&mut b, a, close);
                b.set_term(Term::Ret(Some(a2)));
            }
            Ty::Set(elem) => {
                let elem = *elem;
                // `zap_set_items` snapshots the set's elements as a `list[elem]`
                // in stable iteration order (#608/#609), reused by the list loop.
                let items = b.new_value(Ty::List(elem));
                b.emit(Inst::Call {
                    dest: Some(items),
                    func: "zap_set_items".to_string(),
                    args: vec![c],
                });
                let len = b.new_value(Ty::I64);
                b.emit(Inst::Call {
                    dest: Some(len),
                    func: "zap_list_len".to_string(),
                    args: vec![items],
                });
                let zero = b.iconst(Ty::I64, 0);
                let is_empty = b.new_value(Ty::I1);
                b.emit(Inst::ICmp {
                    dest: is_empty,
                    pred: CmpPred::Eq,
                    lhs: len,
                    rhs: zero,
                });
                let result = b.alloca(Ty::Ptr);
                let empty_bb = b.new_block();
                let nonempty_bb = b.new_block();
                let done_bb = b.new_block();
                b.set_term(Term::BrIf {
                    cond: is_empty,
                    then_bb: empty_bb,
                    else_bb: nonempty_bb,
                });
                // Python renders the empty set as `set()`, NOT `{}` (which is an
                // empty dict).
                b.position(empty_bb);
                let empty_str = b.strconst("set()");
                b.store(result, empty_str);
                b.set_term(Term::Br(done_bb));
                b.position(nonempty_bb);
                let acc = b.alloca(Ty::Ptr);
                let first = b.alloca(Ty::I64);
                let open = b.strconst("{");
                b.store(acc, open);
                let one = b.iconst(Ty::I64, 1);
                b.store(first, one);
                b.iterate_list(items, elem, |s, e, _ety| {
                    gen_seq_sep(s, acc, first);
                    let es = render_elem_repr(s, e, elem)?;
                    let a = s.load(acc, Ty::Ptr);
                    let a2 = gen_concat(s, a, es);
                    s.store(acc, a2);
                    Ok(())
                })?;
                let a = b.load(acc, Ty::Ptr);
                let close = b.strconst("}");
                let a2 = gen_concat(&mut b, a, close);
                b.store(result, a2);
                b.set_term(Term::Br(done_bb));
                b.position(done_bb);
                let out = b.load(result, Ty::Ptr);
                b.set_term(Term::Ret(Some(out)));
            }
            Ty::Dict(kk, vk) => {
                let (kk, vk) = (*kk, *vk);
                let acc = b.alloca(Ty::Ptr);
                let first = b.alloca(Ty::I64);
                let open = b.strconst("{");
                b.store(acc, open);
                let one = b.iconst(Ty::I64, 1);
                b.store(first, one);
                // Snapshot keys (stable order), then fetch each value with the
                // typed dict getter — the same entry points the dict literal /
                // merge paths use; a struct key supplies its `__hash__`/`__eq__`
                // via `key_fn_args`.
                let keys = b.new_value(Ty::List(kk));
                b.emit(Inst::Call {
                    dest: Some(keys),
                    func: format!("zap_dict_keys_{}", kk.val_suffix()),
                    args: vec![c],
                });
                let kfns = b.key_fn_args(kk)?;
                let get_fn = format!("zap_dict_get_{}_{}", kk.key_suffix(), vk.val_suffix());
                let vty = b.elem_ty(vk);
                b.iterate_list(keys, kk, |s, key, _kty| {
                    gen_seq_sep(s, acc, first);
                    // key repr
                    let ks = render_elem_repr(s, key, kk)?;
                    let a = s.load(acc, Ty::Ptr);
                    let a2 = gen_concat(s, a, ks);
                    let colon = s.strconst(": ");
                    let a3 = gen_concat(s, a2, colon);
                    s.store(acc, a3);
                    // value: look it up by key, then repr it
                    let val = s.new_value(vty);
                    let mut gargs = vec![c, key];
                    gargs.extend_from_slice(&kfns);
                    s.emit(Inst::Call {
                        dest: Some(val),
                        func: get_fn.clone(),
                        args: gargs,
                    });
                    let vs = render_elem_repr(s, val, vk)?;
                    let a4 = s.load(acc, Ty::Ptr);
                    let a5 = gen_concat(s, a4, vs);
                    s.store(acc, a5);
                    Ok(())
                })?;
                let a = b.load(acc, Ty::Ptr);
                let close = b.strconst("}");
                let a2 = gen_concat(&mut b, a, close);
                b.set_term(Term::Ret(Some(a2)));
            }
            _ => return Err(err("container_repr_fn called on a non-container type")),
        }
        self.ctx.generated.borrow_mut().push(b.func);
        self.ctx
            .synth_container_reprfns
            .borrow_mut()
            .insert(key, name.clone());
        Ok(name)
    }

    /// Desugar a binary operator whose left operand is a user struct to the
    /// matching dunder (D-0047). Equality/relational operators derive from
    /// `__eq__` / `__cmp__`; `and`/`or` use truthiness; the rest call their
    /// arithmetic/bitwise dunder directly and take its return type.
    fn struct_binop(
        &mut self,
        op: AstBin,
        sid: u32,
        l: Value,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;
        let lt = Ty::Struct(sid);
        match op {
            Eq | Ne => {
                let (bit, _) = self.call_dunder(sid, "__eq__", vec![(l, lt), (r, rt)])?;
                if matches!(op, Ne) {
                    let d = self.new_value(Ty::I1);
                    self.emit(Inst::Un {
                        dest: d,
                        op: UnOp::Not,
                        val: bit,
                    });
                    Ok((d, Ty::I1))
                } else {
                    Ok((bit, Ty::I1))
                }
            }
            Lt | Le | Gt | Ge => {
                // `a < b` -> `a.__cmp__(b) < 0`, and likewise for the others.
                let (cmp, _) = self.call_dunder(sid, "__cmp__", vec![(l, lt), (r, rt)])?;
                let pred = match op {
                    Lt => CmpPred::Lt,
                    Le => CmpPred::Le,
                    Gt => CmpPred::Gt,
                    Ge => CmpPred::Ge,
                    _ => unreachable!(),
                };
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: cmp,
                    rhs: zero,
                });
                Ok((d, Ty::I1))
            }
            And | Or => {
                let lb = self.truthy(l, lt);
                let rb = self.truthy(r, rt);
                let irop = if matches!(op, And) {
                    BinOp::And
                } else {
                    BinOp::Or
                };
                let d = self.new_value(Ty::I1);
                self.emit(Inst::Bin {
                    dest: d,
                    op: irop,
                    lhs: lb,
                    rhs: rb,
                });
                Ok((d, Ty::I1))
            }
            Send => Err(err("'<-' (channel send) is not an overloadable operator")),
            In => unreachable!("'in' dispatches on the right operand"),
            _ => {
                let dunder = match op {
                    Add => "__add__",
                    Sub => "__sub__",
                    Mul => "__mul__",
                    Div => "__truediv__",
                    FloorDiv => "__floordiv__",
                    Mod => "__mod__",
                    Pow => "__pow__",
                    BitAnd => "__and__",
                    BitOr => "__or__",
                    BitXor => "__xor__",
                    Shl => "__lshift__",
                    Shr => "__rshift__",
                    _ => unreachable!(),
                };
                self.call_dunder(sid, dunder, vec![(l, lt), (r, rt)])
            }
        }
    }

    /// Lower `x is None` / `x is not None` (#611, D18). Exactly one operand is a
    /// literal `None` (the caller guarantees it); the other is lowered and
    /// tested for emptiness. An `Optional[T]` reuses the niche/tagged
    /// empty-variant check (`gen_opt_is_none`); a plain reference `struct` is
    /// None exactly when its pointer is null. `None is None` folds to a
    /// constant. `is not` negates the result.
    fn lower_is_none(
        &mut self,
        is_not: bool,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<(Value, Ty), LowerError> {
        let l_none = matches!(lhs, Expr::None(_));
        let r_none = matches!(rhs, Expr::None(_));
        if l_none && r_none {
            // `None is None` is True; `None is not None` is False.
            return Ok((self.bconst(!is_not), Ty::I1));
        }
        let other = if l_none { rhs } else { lhs };
        let (v, t) = self.expr(other)?;
        let is_none = if let Some(opt) = optional_layout(self.ctx, t) {
            gen_opt_is_none(self, &opt, v)
        } else {
            // Reference struct (or other reference pointer): null is None. Sema
            // (`is_nullable_ty`) has already rejected non-nullable operands.
            let d = self.new_value(Ty::I1);
            self.emit(Inst::IsNull { dest: d, val: v });
            d
        };
        if is_not {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: is_none,
            });
            Ok((d, Ty::I1))
        } else {
            Ok((is_none, Ty::I1))
        }
    }

    fn binop(
        &mut self,
        op: AstBin,
        l: Value,
        lt: Ty,
        r: Value,
        rt: Ty,
    ) -> Result<(Value, Ty), LowerError> {
        use AstBin::*;

        // `not in` (#593): negated membership. Reuse the `in` lowering verbatim
        // (the membership probe that dispatches on the right operand) and negate
        // its boolean result, so `x not in xs` lowers identically to
        // `not (x in xs)` — no separate membership path, no codegen surface.
        if matches!(op, NotIn) {
            let (b, bt) = self.binop(In, l, lt, r, rt)?;
            let bi = self.truthy(b, bt);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::Un {
                dest: d,
                op: UnOp::Not,
                val: bi,
            });
            return Ok((d, Ty::I1));
        }

        // `is` / `is not` (#611, D18): reference identity (same allocation).
        // Both operands are reference values — sema (`check_identity`) rejects
        // value types, and a literal `None` operand is handled earlier in the
        // `Expr::Binary` lowerer (it has the static types, so it emits the
        // null/empty-variant check). Reaching here both sides are concrete
        // reference pointers, so compare them directly; this is also the path a
        // chained `a is b` link takes. Codegen's `ICmp` accepts pointer
        // operands (it `ptrtoint`s them), so no element-kind dispatch is needed.
        if matches!(op, Is | IsNot) {
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: if matches!(op, Is) {
                    CmpPred::Eq
                } else {
                    CmpPred::Ne
                },
                lhs: l,
                rhs: r,
            });
            return Ok((d, Ty::I1));
        }

        // Channel send `ch <- v` (ticket #25, D-0031): handled before the
        // generic reference-type guard below, since the channel operand is a
        // reference type. Blocks until the value can be delivered; the element
        // kind picks the typed runtime entry point. Used as a statement, so the
        // result is a unit-like `i32 0`.
        if matches!(op, Send) {
            let Ty::Chan(elem) = lt else {
                return Err(err("the left operand of '<-' (send) must be a channel"));
            };
            let v = self.coerce(r, rt, elem.ty());
            self.throwing_call(
                None,
                &format!("zap_chan_send_{}", elem.val_suffix()),
                vec![l, v],
            );
            return Ok((self.iconst(Ty::I32, 0), Ty::I32));
        }

        // Element-wise numeric-array ops (ticket #24): same-shape `a + b` and
        // scalar broadcast `a + 2.0`, lowered to a SIMD kernel in codegen.
        if matches!(lt, Ty::Array(_)) || matches!(rt, Ty::Array(_)) {
            return self.array_binop(op, l, lt, r, rt);
        }

        if lt == Ty::Complex || rt == Ty::Complex {
            let l = self.coerce(l, lt, Ty::Complex);
            let r = self.coerce(r, rt, Ty::Complex);
            if matches!(op, Eq | Ne) {
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: "zap_complex_eq".to_string(),
                    args: vec![l, r],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: if matches!(op, Eq) {
                        CmpPred::Ne
                    } else {
                        CmpPred::Eq
                    },
                    lhs: raw,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
            let func = match op {
                Add => "zap_complex_add",
                Sub => "zap_complex_sub",
                Mul => "zap_complex_mul",
                Div => "zap_complex_div",
                Pow => "zap_complex_pow",
                _ => return Err(err("unsupported operation on complex values")),
            };
            let d = self.new_value(Ty::Complex);
            if matches!(op, Div) {
                self.throwing_call(Some(d), func, vec![l, r]);
            } else {
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![l, r],
                });
            }
            return Ok((d, Ty::Complex));
        }

        if (lt == Ty::Bigint || rt == Ty::Bigint) && !lt.is_float() && !rt.is_float() {
            if matches!(op, Div) {
                let lf = self.as_f64(l, lt);
                let rf = self.as_f64(r, rt);
                let d = self.new_value(Ty::F64);
                self.emit(Inst::Bin {
                    dest: d,
                    op: BinOp::Div,
                    lhs: lf,
                    rhs: rf,
                });
                return Ok((d, Ty::F64));
            }
            let l = self.coerce(l, lt, Ty::Bigint);
            let r = self.coerce(r, rt, Ty::Bigint);
            if matches!(op, Eq | Ne | Lt | Le | Gt | Ge) {
                let cmp = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(cmp),
                    func: "zap_bigint_cmp".to_string(),
                    args: vec![l, r],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                let pred = match op {
                    Eq => CmpPred::Eq,
                    Ne => CmpPred::Ne,
                    Lt => CmpPred::Lt,
                    Le => CmpPred::Le,
                    Gt => CmpPred::Gt,
                    Ge => CmpPred::Ge,
                    _ => unreachable!(),
                };
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: cmp,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
            let (func, can_throw) = match op {
                Add => ("zap_bigint_add", false),
                Sub => ("zap_bigint_sub", false),
                Mul => ("zap_bigint_mul", false),
                FloorDiv => ("zap_bigint_floordiv", true),
                Mod => ("zap_bigint_mod", true),
                Pow => ("zap_bigint_pow", true),
                _ => return Err(err("unsupported operation on bigint values")),
            };
            let d = self.new_value(Ty::Bigint);
            if can_throw {
                self.throwing_call(Some(d), func, vec![l, r]);
            } else {
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: func.to_string(),
                    args: vec![l, r],
                });
            }
            return Ok((d, Ty::Bigint));
        }

        // Element-wise tuple comparison (D-0049): `t1 == t2` / `t1 != t2` compares
        // positionally, recursing into each element (so nested tuples compare
        // element-wise too). A tuple is an anonymous struct but not a *user*
        // struct, so this must precede operator-overloading dispatch below (a
        // tuple has no dunder methods); other operators on tuples are unsupported
        // and fall through to the struct path, which reports a clear error.
        if matches!(op, Eq | Ne) {
            if let Ty::Struct(ls) = lt {
                if self.ctx.tuples.borrow().is_tuple(ls) {
                    return self.tuple_cmp(op, l, ls, r, rt);
                }
            }
        }

        // Operator overloading on user structs (D-0047): `a op b` desugars to the
        // matching dunder method, resolved like any struct method call. `in`
        // dispatches on the right operand (`x in a` -> `a.__contains__(x)`); every
        // other operator dispatches on the left.
        if let (Ty::Struct(sid), false) = (lt, matches!(op, In)) {
            return self.struct_binop(op, sid, l, r, rt);
        }
        if matches!(op, In) {
            if let Ty::Struct(sid) = rt {
                let contains = format!("{}__{}", self.ctx.names[sid as usize], "__contains__");
                if self.sigs.contains_key(&contains) {
                    let (ret, rty) =
                        self.call_dunder(sid, "__contains__", vec![(r, rt), (l, lt)])?;
                    return Ok((ret, rty));
                }
                // No `__contains__`: membership falls back to iterating the
                // container and comparing each element for equality (D-0048).
                let result = self.alloca(Ty::I1);
                let f = self.new_value(Ty::I1);
                self.emit(Inst::BConst {
                    dest: f,
                    val: false,
                });
                self.store(result, f);
                self.drive_iteration(r, sid, |s, ev, ety| {
                    let (eq, _) = s.binop(AstBin::Eq, ev, ety, l, lt)?;
                    let then_bb = s.new_block();
                    let cont_bb = s.new_block();
                    s.set_term(Term::BrIf {
                        cond: eq,
                        then_bb,
                        else_bb: cont_bb,
                    });
                    s.position(then_bb);
                    let t = s.new_value(Ty::I1);
                    s.emit(Inst::BConst { dest: t, val: true });
                    s.store(result, t);
                    let brk = s.loops.last().expect("in-iteration loop").brk;
                    s.set_term(Term::Br(brk));
                    s.position(cont_bb);
                    Ok(())
                })?;
                let d = self.load(result, Ty::I1);
                return Ok((d, Ty::I1));
            }
        }

        // Substring membership: `sub in s` -> `zap_str_contains` (ticket #284).
        // Both operands lower to `Ptr` (str); sema has checked the types.
        if matches!(op, In) && lt == Ty::Ptr && rt == Ty::Ptr {
            let raw = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(raw),
                func: "zap_str_contains".to_string(),
                args: vec![r, l],
            });
            let zero = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: raw,
                rhs: zero,
            });
            return Ok((d, Ty::I1));
        }

        // Key membership: `k in <dict>` -> `zap_dict_contains_*`, normalized to a
        // bool (ticket #299). The key kind selects the entry point; a struct key
        // passes its `__hash__`/`__eq__` as the trailing function pointers.
        if matches!(op, In) {
            if let Ty::Dict(kk, _vk) = rt {
                let l = self.coerce(l, lt, kk.ty());
                let kfns = self.key_fn_args(kk)?;
                let word = self.new_value(Ty::I64);
                let mut cargs = vec![r, l];
                cargs.extend_from_slice(&kfns);
                self.emit(Inst::Call {
                    dest: Some(word),
                    func: format!("zap_dict_contains_{}", kk.key_suffix()),
                    args: cargs,
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: CmpPred::Ne,
                    lhs: word,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
        }

        // List membership: `x in <list>` — a list has no hash index, so this is a
        // linear scan comparing each element to `x` for equality (ticket #399). The
        // comparison routes through `binop(Eq, ..)`, so elements use the same
        // equality semantics as `==` (numeric joins, `str` by contents, a struct's
        // `__eq__`). Lowered in-IR like a `for` loop, breaking on the first match.
        if matches!(op, In) {
            if let Ty::List(elem) = rt {
                let result = self.alloca(Ty::I1);
                let f = self.new_value(Ty::I1);
                self.emit(Inst::BConst {
                    dest: f,
                    val: false,
                });
                self.store(result, f);
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
                    args: vec![r],
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
                    args: vec![r, i],
                });
                let (eq, _) = self.binop(AstBin::Eq, e, elem.ty(), l, lt)?;
                let found_bb = self.new_block();
                self.set_term(Term::BrIf {
                    cond: eq,
                    then_bb: found_bb,
                    else_bb: incr_bb,
                });

                self.position(found_bb);
                let t = self.new_value(Ty::I1);
                self.emit(Inst::BConst { dest: t, val: true });
                self.store(result, t);
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
                let d = self.load(result, Ty::I1);
                return Ok((d, Ty::I1));
            }
        }

        // Membership: `x in <set>` -> `zap_set_contains_*`, normalized to a bool.
        if matches!(op, In) {
            let Ty::Set(elem) = rt else {
                return Err(err("'in' is only supported with a set on the right"));
            };
            if elem_kind(lt, &self.ctx.nested)? != elem {
                return Err(err("'in' element has the wrong type for this set"));
            }
            let l = self.coerce(l, lt, elem.ty());
            let kfns = self.key_fn_args(elem)?;
            let word = self.new_value(Ty::I64);
            let mut cargs = vec![r, l];
            cargs.extend_from_slice(&kfns);
            self.emit(Inst::Call {
                dest: Some(word),
                func: format!("zap_set_contains_{}", elem.key_suffix()),
                args: cargs,
            });
            let zero = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: word,
                rhs: zero,
            });
            return Ok((d, Ty::I1));
        }

        // `bytes` operands (I#337): `+` (concatenation -> bytes), `==`/`!=`
        // (content equality), and ordering `< <= > >=` (byte-wise), reusing the
        // `ZapStr` byte-wise runtime. Both operands share the physical repr, so
        // the comparison helpers are identical to `str`'s; `+` yields `bytes`.
        // Sema has checked that both sides are `bytes`.
        if lt == Ty::Bytes && rt == Ty::Bytes {
            if matches!(op, Add) {
                let d = self.new_value(Ty::Bytes);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_str_concat".to_string(),
                    args: vec![l, r],
                });
                return Ok((d, Ty::Bytes));
            }
            if matches!(op, Eq | Ne) {
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: "zap_str_eq".to_string(),
                    args: vec![l, r],
                });
                let zero = self.iconst(Ty::I64, 0);
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred: if matches!(op, Eq) {
                        CmpPred::Ne
                    } else {
                        CmpPred::Eq
                    },
                    lhs: raw,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
            if matches!(op, Lt | Le | Gt | Ge) {
                let raw = self.new_value(Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(raw),
                    func: "zap_str_cmp".to_string(),
                    args: vec![l, r],
                });
                let zero = self.iconst(Ty::I64, 0);
                let pred = match op {
                    Lt => CmpPred::Lt,
                    Le => CmpPred::Le,
                    Gt => CmpPred::Gt,
                    Ge => CmpPred::Ge,
                    _ => unreachable!(),
                };
                let d = self.new_value(Ty::I1);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: raw,
                    rhs: zero,
                });
                return Ok((d, Ty::I1));
            }
            return Err(err(
                "unsupported operation on bytes (bytes supports +, ==, !=, <, <=, >, >=)",
            ));
        }

        // Reference-typed operands: `str` (lowered to `Ptr`) supports `+`
        // (concatenation), `==`/`!=` (content equality), and ordering
        // `< <= > >=` (lexicographic by codepoint == byte-wise for UTF-8,
        // D-0025). Everything else on a reference type is unsupported.
        if matches!(lt, Ty::Ptr) || matches!(rt, Ty::Ptr) {
            if lt == Ty::Ptr && rt == Ty::Ptr {
                if matches!(op, Add) {
                    let d = self.new_value(Ty::Ptr);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_str_concat".to_string(),
                        args: vec![l, r],
                    });
                    return Ok((d, Ty::Ptr));
                }
                if matches!(op, Eq | Ne) {
                    // `zap_str_eq` returns 1/0; `!=` negates via comparing to 0.
                    let raw = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(raw),
                        func: "zap_str_eq".to_string(),
                        args: vec![l, r],
                    });
                    let zero = self.iconst(Ty::I64, 0);
                    let d = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: d,
                        pred: if matches!(op, Eq) {
                            CmpPred::Ne
                        } else {
                            CmpPred::Eq
                        },
                        lhs: raw,
                        rhs: zero,
                    });
                    return Ok((d, Ty::I1));
                }
                if matches!(op, Lt | Le | Gt | Ge) {
                    // `zap_str_cmp` returns -1/0/1; compare it against 0.
                    let raw = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(raw),
                        func: "zap_str_cmp".to_string(),
                        args: vec![l, r],
                    });
                    let zero = self.iconst(Ty::I64, 0);
                    let pred = match op {
                        Lt => CmpPred::Lt,
                        Le => CmpPred::Le,
                        Gt => CmpPred::Gt,
                        Ge => CmpPred::Ge,
                        _ => unreachable!(),
                    };
                    let d = self.new_value(Ty::I1);
                    self.emit(Inst::ICmp {
                        dest: d,
                        pred,
                        lhs: raw,
                        rhs: zero,
                    });
                    return Ok((d, Ty::I1));
                }
            }
            return Err(err(
                "unsupported operation on a reference type (str supports +, ==, !=, <, <=, >, >=)",
            ));
        }

        let is_float = lt.is_float() || rt.is_float();

        if matches!(op, Eq | Ne | Lt | Le | Gt | Ge) {
            let pred = match op {
                Eq => CmpPred::Eq,
                Ne => CmpPred::Ne,
                Lt => CmpPred::Lt,
                Le => CmpPred::Le,
                Gt => CmpPred::Gt,
                Ge => CmpPred::Ge,
                _ => unreachable!(),
            };
            let d = self.new_value(Ty::I1);
            if is_float {
                let lf = self.as_f64(l, lt);
                let rf = self.as_f64(r, rt);
                self.emit(Inst::FCmp {
                    dest: d,
                    pred,
                    lhs: lf,
                    rhs: rf,
                });
            } else {
                // Integer comparisons are done at i64 so mixed-width operands
                // (e.g. `int32 < int64`, or a literal against an int32) share a
                // type (D-0019 all-signed tower).
                let li = self.coerce(l, lt, Ty::I64);
                let ri = self.coerce(r, rt, Ty::I64);
                self.emit(Inst::ICmp {
                    dest: d,
                    pred,
                    lhs: li,
                    rhs: ri,
                });
            }
            return Ok((d, Ty::I1));
        }

        match op {
            Add | Sub | Mul => {
                let irop = match op {
                    Add => BinOp::Add,
                    Sub => BinOp::Sub,
                    Mul => BinOp::Mul,
                    _ => unreachable!(),
                };
                if is_float {
                    // Float arithmetic computes at f64 (operands widened), per
                    // the numeric join in sema (any float operand -> float64).
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Bin {
                        dest: d,
                        op: irop,
                        lhs: lf,
                        rhs: rf,
                    });
                    Ok((d, Ty::F64))
                } else {
                    // Integer arithmetic computes at i64 (operands widened), so
                    // `int32`/untyped-literal operands share a width (D-0019).
                    let li = self.coerce(l, lt, Ty::I64);
                    let ri = self.coerce(r, rt, Ty::I64);
                    let d = self.new_value(Ty::I64);
                    self.emit(Inst::Bin {
                        dest: d,
                        op: irop,
                        lhs: li,
                        rhs: ri,
                    });
                    Ok((d, Ty::I64))
                }
            }
            Mod => {
                // Modulo (D-0024) takes the sign of the divisor (Python
                // floored modulo), consistent with floored `//` so that
                // `a == (a // b) * b + (a % b)` holds. Floats use LLVM `frem`
                // directly; integers go through a runtime helper that corrects
                // C's truncated remainder, mirroring `zap_ifloordiv`.
                if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Bin {
                        dest: d,
                        op: BinOp::Rem,
                        lhs: lf,
                        rhs: rf,
                    });
                    Ok((d, Ty::F64))
                } else {
                    let li = self.coerce(l, lt, Ty::I64);
                    let ri = self.coerce(r, rt, Ty::I64);
                    let d = self.new_value(Ty::I64);
                    // Modulo by zero is a catchable built-in fault (EXC-3,
                    // D-0007): `zap_imod` raises through `zap_panic`, so this is
                    // a `throwing_call` (an `invoke` inside a `try`) rather than
                    // a plain `call`, which could not reach the landing pad.
                    self.throwing_call(Some(d), "zap_imod", vec![li, ri]);
                    Ok((d, Ty::I64))
                }
            }
            FloorDiv => {
                // Floor division (D-0024) rounds toward negative infinity,
                // unlike LLVM `sdiv` (toward zero) / `fdiv` (no rounding). The
                // toward-negative-infinity adjustment lives in a runtime helper
                // so codegen stays mechanical and the semantics are in one place.
                if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_ffloordiv".to_string(),
                        args: vec![lf, rf],
                    });
                    Ok((d, Ty::F64))
                } else {
                    let li = self.coerce(l, lt, Ty::I64);
                    let ri = self.coerce(r, rt, Ty::I64);
                    let d = self.new_value(Ty::I64);
                    // Divide by zero is a catchable built-in fault (EXC-3,
                    // D-0007): `zap_ifloordiv` raises through `zap_panic`, so
                    // this is a `throwing_call` (an `invoke` inside a `try`)
                    // rather than a plain `call` that could not reach the pad.
                    self.throwing_call(Some(d), "zap_ifloordiv", vec![li, ri]);
                    Ok((d, Ty::I64))
                }
            }
            Pow => {
                // `**` over the numeric tower (D-0019). A floating-point base or
                // exponent goes through libm `pow` (f64); the all-integer case
                // uses a binary-exponentiation runtime helper and keeps an
                // integer result per the static type (a negative exponent yields
                // the floor of the true power — 0 for |base| > 1).
                if is_float {
                    let lf = self.as_f64(l, lt);
                    let rf = self.as_f64(r, rt);
                    let d = self.new_value(Ty::F64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_powf".to_string(),
                        args: vec![lf, rf],
                    });
                    Ok((d, Ty::F64))
                } else {
                    let li = self.coerce(l, lt, Ty::I64);
                    let ri = self.coerce(r, rt, Ty::I64);
                    let d = self.new_value(Ty::I64);
                    self.emit(Inst::Call {
                        dest: Some(d),
                        func: "zap_ipow".to_string(),
                        args: vec![li, ri],
                    });
                    Ok((d, Ty::I64))
                }
            }
            Div => {
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
            BitAnd | BitOr | BitXor | Shl | Shr => {
                let irop = match op {
                    BitAnd => BinOp::And,
                    BitOr => BinOp::Or,
                    BitXor => BinOp::Xor,
                    Shl => BinOp::Shl,
                    Shr => BinOp::Shr,
                    _ => unreachable!(),
                };
                let li = self.coerce(l, lt, Ty::I64);
                let ri = self.coerce(r, rt, Ty::I64);
                let d = self.new_value(Ty::I64);
                self.emit(Inst::Bin {
                    dest: d,
                    op: irop,
                    lhs: li,
                    rhs: ri,
                });
                Ok((d, Ty::I64))
            }
            And | Or => {
                let lb = self.truthy(l, lt);
                let rb = self.truthy(r, rt);
                let irop = if matches!(op, And) {
                    BinOp::And
                } else {
                    BinOp::Or
                };
                let d = self.new_value(Ty::I1);
                self.emit(Inst::Bin {
                    dest: d,
                    op: irop,
                    lhs: lb,
                    rhs: rb,
                });
                Ok((d, Ty::I1))
            }
            In => Err(err("'in' is not supported by codegen yet")),
            NotIn => unreachable!("'not in' is rewritten to a negated 'in' in binop()"),
            // `Send` is intercepted at the top of `binop` (before the generic
            // reference-type guard), so it never reaches here.
            Send => unreachable!("channel send handled above"),
            // `is` / `is not` (#611) are intercepted at the top of `binop`.
            Is | IsNot => unreachable!("'is'/'is not' handled above"),
            Eq | Ne | Lt | Le | Gt | Ge => unreachable!(),
        }
    }

    /// Element-wise `+`/`-`/`*` on arrays, including scalar broadcast and
    /// mixed-dtype promotion (ticket #49). Emits the [`Inst::ArrayBinOp`] node
    /// codegen vectorizes. Two arrays of different dtypes promote to the higher
    /// dtype ([`Dtype::promote`], so `i64 + f64 -> f64`); a scalar is weak
    /// ([`Dtype::promote_scalar`], so `uint8 + 1` stays `uint8`). `scalar op
    /// array` is only accepted for the commutative `+`/`*` (reordered to `array
    /// op scalar`), so the SIMD kernel always computes `array[i] op scalar`.
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
            _ => {
                return Err(err(
                    "only +, -, and * are supported on arrays yet (/, reductions beyond sum, etc. are deferred)",
                ))
            }
        };
        let is_scalar = |t: Ty| t.is_float() || t.int_width().is_some();
        match (lt, rt) {
            (Ty::Array(ld), Ty::Array(rd)) => {
                self.throwing_call(None, "zap_array_check_shape", vec![l, r]);
                let out = Dtype::promote(ld, rd);
                let dest = self.new_value(Ty::Array(out));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: l,
                    lhs_dtype: ld,
                    rhs: r,
                    rhs_dtype: rd,
                    scalar: false,
                });
                Ok((dest, Ty::Array(out)))
            }
            (Ty::Array(ld), _) if is_scalar(rt) => {
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
                let out = Dtype::promote_scalar(ld, scalar_is_float);
                let dest = self.new_value(Ty::Array(out));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: l,
                    lhs_dtype: ld,
                    rhs: s,
                    rhs_dtype: sdt,
                    scalar: true,
                });
                Ok((dest, Ty::Array(out)))
            }
            (_, Ty::Array(rd)) if is_scalar(lt) => {
                if !matches!(op, Add | Mul) {
                    return Err(err(
                        "scalar-on-the-left broadcast is only supported for + and * (write `a - scalar`, not `scalar - a`)",
                    ));
                }
                let scalar_is_float = lt.is_float();
                let sdt = if scalar_is_float {
                    Dtype::F64
                } else {
                    Dtype::I64
                };
                let s = self.coerce(l, lt, sdt.ty());
                let out = Dtype::promote_scalar(rd, scalar_is_float);
                let dest = self.new_value(Ty::Array(out));
                self.emit(Inst::ArrayBinOp {
                    dest,
                    op: irop,
                    out,
                    lhs: r,
                    lhs_dtype: rd,
                    rhs: s,
                    rhs_dtype: sdt,
                    scalar: true,
                });
                Ok((dest, Ty::Array(out)))
            }
            _ => Err(err(
                "unsupported array operand (element-wise ops take two arrays or an array and a numeric scalar)",
            )),
        }
    }

    // --- coercions / helpers ---

    fn coerce(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        if from == to {
            return v;
        }
        if to == Ty::Bigint {
            let d = self.new_value(Ty::Bigint);
            if from.int_width().is_some() {
                let arg = self.coerce(v, from, Ty::I64);
                self.emit(Inst::Call {
                    dest: Some(d),
                    func: "zap_bigint_from_i64".to_string(),
                    args: vec![arg],
                });
            } else if from.is_float() {
                let arg = self.as_f64(v, from);
                self.throwing_call(Some(d), "zap_bigint_from_f64", vec![arg]);
            } else {
                return v;
            }
            return d;
        }
        if from == Ty::Bigint && to == Ty::Complex {
            let real = self.coerce(v, from, Ty::F64);
            let d = self.new_value(Ty::Complex);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_complex_from_f64".to_string(),
                args: vec![real],
            });
            return d;
        }
        if from == Ty::Bigint && to.is_float() {
            let f64v = self.new_value(Ty::F64);
            self.emit(Inst::Call {
                dest: Some(f64v),
                func: "zap_bigint_to_f64".to_string(),
                args: vec![v],
            });
            return if to == Ty::F64 {
                f64v
            } else {
                self.coerce(f64v, Ty::F64, to)
            };
        }
        if to == Ty::Complex && (from.int_width().is_some() || from.is_float()) {
            let real = self.as_f64(v, from);
            let d = self.new_value(Ty::Complex);
            self.emit(Inst::Call {
                dest: Some(d),
                func: "zap_complex_from_f64".to_string(),
                args: vec![real],
            });
            return d;
        }
        // Concrete struct -> interface value: build the `{data, itable}` fat
        // pointer (D-0016, design/0006 §2.4). A reference struct is used directly
        // as `data` (shared heap identity). A `value struct` is **boxed** first
        // (I#75 Part 1, D-0051): copied to a fresh heap instance so `data` holds a
        // stable, GC-traced pointer to a copy that is independent of the source
        // (whose storage may be a stack slot under the register/sret ABI, or an
        // lvalue that must not alias). interface -> interface and `None` ->
        // interface are documented follow-ups.
        if let Ty::Struct(to_sid) = to {
            if let Some(iid) = self.ctx.iid_of_fat_sid(to_sid) {
                if let Ty::Struct(from_sid) = from {
                    if self.ctx.iid_of_fat_sid(from_sid).is_none() {
                        let data = if self.ctx.is_value_struct(from_sid) {
                            self.copy_value_struct(v, from_sid)
                        } else {
                            v
                        };
                        let dest = self.new_value(to);
                        self.emit(Inst::IfaceMake {
                            dest,
                            data,
                            iid,
                            concrete_sid: from_sid,
                        });
                        // Synthesize and record the concrete type's Display
                        // renderer (#707, D-0124) so codegen can place it in the
                        // renderer slot of this `(concrete, iid)` itable — letting
                        // `print`/`str`/f-string on the resulting interface value
                        // dynamically dispatch to the concrete type's renderer
                        // (`Circle(r=2.0)`, a tier-3 enum's structural form, …).
                        // Synthesis is memoized, so repeated coercions of the same
                        // concrete type are free. A renderer that cannot be built
                        // (an unrenderable field type — not reachable for the field
                        // types D-0124 supports) simply records nothing, leaving a
                        // null renderer slot rather than failing construction, so an
                        // interface used only for method dispatch is never regressed.
                        if !self.ctx.iface_renderers.borrow().contains_key(&from_sid) {
                            if let Ok(sym) = self.struct_repr_fn(from_sid) {
                                self.ctx.iface_renderers.borrow_mut().insert(from_sid, sym);
                            }
                        }
                        return dest;
                    }
                }
                // Not a concrete-struct source we can box — leave unchanged; sema
                // has already rejected a non-conforming or unsupported source.
                return v;
            }
        }
        if let (Some(fw), Some(tw)) = (from.int_width(), to.int_width()) {
            if fw == tw {
                return v;
            }
            let d = self.new_value(to);
            let kind = if tw > fw {
                CastKind::SExt
            } else {
                CastKind::Trunc
            };
            self.emit(Inst::Cast {
                dest: d,
                kind,
                val: v,
            });
            return d;
        }
        if from.int_width().is_some() && to.is_float() {
            let d = self.new_value(to);
            self.emit(Inst::Cast {
                dest: d,
                kind: CastKind::SIToFP,
                val: v,
            });
            return d;
        }
        if from.is_float() && to.is_float() {
            let d = self.new_value(to);
            let kind = if to == Ty::F64 {
                CastKind::FPExt
            } else {
                CastKind::FPTrunc
            };
            self.emit(Inst::Cast {
                dest: d,
                kind,
                val: v,
            });
            return d;
        }
        v
    }

    /// Widen a numeric value to `f64` (int -> sitofp, f32 -> fpext, f64 -> self).
    fn as_f64(&mut self, v: Value, t: Ty) -> Value {
        self.coerce(v, t, Ty::F64)
    }

    /// An explicit numeric conversion to `to` (the `int32(x)` / `float64(x)`
    /// form, D-0020). Unlike [`coerce`], which only widens, this also narrows
    /// and converts float -> int (a user wrote the cast, so data loss is their
    /// intent). Non-numeric operands pass through unchanged.
    fn convert(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        if from == to {
            return v;
        }
        if from == Ty::Bigint && to.int_width().is_some() {
            if to == Ty::I32 {
                let i32v = self.new_value(Ty::I32);
                self.throwing_call(Some(i32v), "zap_bigint_to_i32", vec![v]);
                return i32v;
            }
            let i64v = self.new_value(Ty::I64);
            self.throwing_call(Some(i64v), "zap_bigint_to_i64", vec![v]);
            return self.coerce(i64v, Ty::I64, to);
        }
        if from == Ty::Bigint && to.is_float() {
            return self.coerce(v, from, to);
        }
        let kind = match (from, to) {
            (f, t) if f.int_width().is_some() && t.int_width().is_some() => {
                if f.int_width() == t.int_width() {
                    return v;
                }
                if t.int_width().unwrap() > f.int_width().unwrap() {
                    CastKind::SExt
                } else {
                    CastKind::Trunc
                }
            }
            (f, t) if f.int_width().is_some() && t.is_float() => CastKind::SIToFP,
            (f, t) if f.is_float() && t.int_width().is_some() => CastKind::FPToSI,
            (f, t) if f.is_float() && t.is_float() => {
                if t == Ty::F64 {
                    CastKind::FPExt
                } else {
                    CastKind::FPTrunc
                }
            }
            _ => return v,
        };
        let d = self.new_value(to);
        self.emit(Inst::Cast {
            dest: d,
            kind,
            val: v,
        });
        d
    }

    fn truthy(&mut self, v: Value, t: Ty) -> Value {
        if t == Ty::I1 {
            return v;
        }
        if t == Ty::Complex || t == Ty::Bigint {
            let raw = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(raw),
                func: if t == Ty::Complex {
                    "zap_complex_truthy".to_string()
                } else {
                    "zap_bigint_truthy".to_string()
                },
                args: vec![v],
            });
            let zero = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: raw,
                rhs: zero,
            });
            return d;
        }
        // Truthiness overloading on user structs (D-0047): `if a:` / `not a` /
        // `a and b` call `a.__bool__()` when defined. A struct with no `__bool__`
        // is a non-null object, so it is truthy.
        if let Ty::Struct(sid) = t {
            let mangled = format!("{}__{}", self.ctx.names[sid as usize], "__bool__");
            if self.sigs.contains_key(&mangled) {
                let d = self.new_value(Ty::I1);
                self.throwing_call(Some(d), &mangled, vec![v]);
                return d;
            }
            return self.iconst(Ty::I1, 1);
        }
        if matches!(t, Ty::Match | Ty::Py) {
            // Reference types are truthy when non-null. Codegen's `ICmp` only
            // compares integers, so go through the runtime (`zap_ptr_nonnull`
            // returns 0/1) and compare that i64 against zero. For a match
            // object, null means "no match" (D-0010). A Python handle is a
            // non-null wrapper, so it is effectively always truthy here; real
            // Python truthiness (`PyObject_IsTrue`) is a documented follow-up.
            let nn = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(nn),
                func: "zap_ptr_nonnull".to_string(),
                args: vec![v],
            });
            let z = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: nn,
                rhs: z,
            });
            return d;
        }
        // Strings/bytes and built-in containers (list/dict/set/heap/array) follow
        // Python / D-0022 truthiness: empty is falsy, non-empty truthy. They are
        // heap pointers, so the integer `ICmp` fallthrough below would feed a
        // `PointerValue` where codegen expects an `IntValue` and crash (#382), and
        // the old `zap_ptr_nonnull` path made every non-null `str` truthy so `""`
        // was wrongly truthy (#385). Route through the length runtime and compare
        // the element/byte count to zero instead. `str` and `bytes` share the
        // `ZapStr {len, data}` layout, so both use `zap_str_len`.
        let len_fn = match t {
            Ty::Ptr | Ty::Bytes => Some("zap_str_len"),
            Ty::List(_) => Some("zap_list_len"),
            Ty::Dict(..) => Some("zap_dict_len"),
            Ty::Set(_) => Some("zap_set_len"),
            Ty::Heap(_) => Some("zap_heap_len"),
            Ty::Array(_) => Some("zap_array_len"),
            _ => None,
        };
        if let Some(len_fn) = len_fn {
            let len = self.new_value(Ty::I64);
            self.emit(Inst::Call {
                dest: Some(len),
                func: len_fn.to_string(),
                args: vec![v],
            });
            let z = self.iconst(Ty::I64, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: len,
                rhs: z,
            });
            return d;
        }
        if t.is_float() {
            let z = self.fconst(t, 0.0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::FCmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: v,
                rhs: z,
            });
            d
        } else {
            let z = self.iconst(t, 0);
            let d = self.new_value(Ty::I1);
            self.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Ne,
                lhs: v,
                rhs: z,
            });
            d
        }
    }
}

fn is_name(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Name(n, _) if n == name)
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

/// Process the common escape sequences in a (non-raw) string literal.
fn unescape(s: &str) -> Vec<u8> {
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
            Some(other) => {
                out.push(b'\\');
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
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
