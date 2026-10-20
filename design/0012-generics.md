# Proposal: User-defined generics (D-0017) + `Option[T]` flagship

- **Author(s):** Claude (scoping/design session for I#154)
- **Last updated:** 2026-10-20
- **Status:** Proposed (design) — forks resolved with a recommendation each;
  implementation **staged** as vertical slices below. Decisions to be ratified as
  a D-number (next free: **D-0064**) by the orchestrator.
- **Discussion:** I#154 (epic). Parent decision D-0017 (generics: yes,
  monomorphized, staged).
- **Builds on:** `design/0006-types-and-generics.md` §5 (the surface-level
  generics design), `design/0008-precise-gc.md` (the precise Immix collector and
  its per-type descriptors), `design/RUNTIME-ABI.md` (heap layout, interface fat
  pointer, name mangling), `design/0010-json.md` (the `@default` optionality
  workaround this removes), `design/0011-aws-sdk.md` (which flagged `Option[T]`
  as its load-bearing blocker, fork O).

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

`design/0006` §5 already pinned the *surface* of Zap generics — subscript syntax
(`Option[T]`, `def map[T, U](...)`), monomorphization (D-0017), and interface
bounds — but implementation was **staged**, and three downstream features have
since bent around its absence: `json` used `@default` for optionality (which
conflates *absent* with *present-but-zero*, `design/0010` §3), the AWS SDK named
`Option[T]` its single load-bearing blocker (`design/0011` fork O), and the
tier-3 enum work shipped non-generic only (`Option[T]`/`Result[T, E]` explicitly
"ride on user generics", `design/0006` §3.3).

This document turns the staged surface into an **implementation design**: it
settles the hard engineering forks (how monomorphization is wired into the
AST→ZIR→LLVM pipeline, how per-instantiation **GC scan metadata** is generated so
the precise collector stays correct, how much call-site inference v1 does,
whether bounds ship in v1, how generics mesh with value structs / interface fat
pointers / enums / the special-cased built-in collections), and sequences the
work as vertical slices with a thin **first end-to-end vertical** that gets
`Option[T]` compiling and running.

The central engineering finding: **monomorphization dissolves the GC-metadata
problem rather than creating one.** A precise collector normally needs per-type
scan info for every generic type; but after monomorphization there *are* no
generic types at runtime — each instantiation is an ordinary concrete struct, and
the collector's existing codegen-time descriptor emission (`design/0008`: pointer
field byte-offsets from the concrete LLVM layout) runs per instance with **zero
new mechanism**. This is the same "rides the existing struct path, zero codegen
change" win that tier-3 enums already banked (enums-match #51).

## Background — what is already true

What the language provides today (all landed), that generics must mesh with:

- **The grammar already reserves generic syntax.** `docs/GRAMMAR.ebnf`:
  `generics ::= "[" IDENT { "," IDENT } "]"` is already attached to `function`,
  `struct_def`, `interface_def`, and `enum_def` — bare type-parameter lists parse
  today (they are accepted and then rejected downstream). Two things are *not* yet
  in the grammar: **bounds** (`[T: Ordered]`) and **call-site type arguments**
  (`Box[str](...)`, `list[T]()`).
- **Tier-3 data-carrying enums** (D-0046, PR #62, enums-match #51): non-generic
  only. Lowered as a **GC-allocated tagged struct `{ tag: i32, <flattened
  payload> }`** reusing `StructNew`/`FieldGet`/`FieldSet` — *zero* codegen /
  `zaprt.c` change. A recursive variant field is already a `Struct(sid)` pointer.
  `match` exhaustiveness is enforced. This is exactly the machinery `Option[T]`
  needs, minus the type parameter.
- **Interfaces as fat pointers** (D-0016, I#84, PR #113) + **value-struct boxing**
  (I#75, PR #114): an interface *value* is a `{data, itable}` fat pointer with
  **dynamic** dispatch; a `value struct` is boxed to the heap at the
  concrete→interface conversion. The structural **conformance checker** in sema
  (`coercible` = `assignable || iface_conforms`) already exists — this is what a
  generic *bound* will reuse.
- **Precise Immix GC** (D-0009, I#48/#116, now the default): every heap object
  carries a one-word descriptor *behind* the payload pointer
  (`[ descriptor|mark | payload_size | payload… ]`); codegen emits **one precise
  `ZapTypeDesc` per user struct** (the pointer fields' byte offsets, from the LLVM
  `TargetData` layout) plus one per spawn closure; the runtime hand-writes
  descriptors for its own objects. Roots are a per-coroutine shadow stack. **No
  conservative scanning.**
- **Built-in generic collections** `list[T]`/`dict[K,V]`/`set[T]`/`array[T]`/
  `heap[T]` (D-0043/#21/#24/#49/#76): these are **special-cased**, *not* user
  generics. They carry the element type at the surface, but at runtime the element
  type is encoded as a **tag + typed entry points**: `list` stores unboxed scalar
  words or pointer words and selects `_i64`/`_ptr`/`_str`/`_fn` entry points;
  `array` carries a `dtype` byte (`0=f64 1=i64 2=u8 3=bool`); `heap` a `kind`
  byte. The precise collector traces collection interiors via a **per-instance
  element-kind flag** set lazily in those `_ptr`/`_str`/`_fn` entry points. There
  is one `ZapList`/`ZapDict`/… runtime type, not one per element type.
- **Operator overloading** (D-0047, #52) + **`@derive`** (D-0036): the dunder
  interfaces (`Add`, `Ord`, …) are structural interfaces, and `design/0006`
  already says "operator interfaces double as generic bounds" (`def sum[T: Add]`).
- **Whole-program AOT** (D-0021) + **LLVM** (D-0042): every instantiation is
  visible at compile time — the precondition monomorphization needs.
- **Name mangling** (RUNTIME-ABI §"Name mangling"): already reserved to "encode
  module path and, for monomorphized generics, type arguments"; the scheme is an
  open issue this doc finalizes with concrete direction.

What generics **cannot** lean on (not yet landed): typed exception classes (I#12,
`raise` carries a string) — so `Option` error ergonomics must use the existing
string-message `raise`; and there is no compile-time user-macro API (D-0036
staged) — not needed, generics is a core-compiler feature, not a derive.

## Proposal — forks, each with a recommendation

### Fork M — Monomorphization vs type erasure (and the GC-metadata story)

**Recommendation: monomorphization (confirming D-0017), implemented as a ZIR
pass between sema and codegen. No new GC metadata mechanism is required.**

Type erasure (one compiled body, a runtime type descriptor / dictionary passed
per call, scalars boxed) is rejected for the reasons D-0017 already locked: it
boxes scalars (fighting the value-struct/unboxed-scalar ABI), adds a per-call
descriptor argument, and defeats the "generics are free at runtime" perf goal
that justified LLVM. Zap's whole-program AOT makes the monomorphization
precondition — *see every instantiation* — free.

**Where it lives in the pipeline.**

```
  AST ──► sema ──────────────► ZIR (generic schemas + instantiation requests)
          (type-check the        │
           generic body ONCE)    ▼
                            monomorphization pass  ──►  ZIR (concrete only)  ──► codegen ──► LLVM
                            (collect · substitute · dedup)     (unchanged)
```

1. **Sema keeps generics as schemas.** A generic `struct`/`enum`/`fn` is checked
   **once, at its definition**, against its declared bounds (Fork B) — the body
   may use only operations the bounds guarantee. This upholds the
   compile-time-safety ethos (D-0016): a generic that type-checks is correct for
   *every* valid instantiation, and errors land at the generic, not at a far-away
   use site. (This is the Rust model; the C++-template "check per instantiation"
   model is rejected — see the locked sub-decision below.) Sema records each
   concrete *instantiation request* `(generic_id, [concrete type args])` it meets
   at a use site, with type args either written explicitly or inferred (Fork I).

2. **A ZIR monomorphization pass** walks the whole-program instantiation set to a
   fixpoint (instantiating `Box[T]` with `T=Pair[str,int]` discovers
   `Pair[str,int]`), **substitutes** type parameters with concrete `Ty` in a
   cloned copy of the generic's ZIR, and assigns each a **mangled symbol**. After
   this pass the ZIR contains only concrete types — `Box[int64]` and `Box[str]`
   are two distinct `StructLayout`s with distinct field `Ty`s and distinct sizes.

3. **Dedup** by mangled symbol: identical `(generic, type-args)` tuples reached
   from anywhere in the program emit once.

4. **Codegen is untouched.** It only ever sees concrete `StructLayout`s and
   concrete functions — exactly what it compiles today.

**The GC-metadata story (the crux the epic calls out).** A precise collector
needs, per heap type, which words are pointers. For a *generic* type that is
unknowable (`Box[T]` is a pointer field iff `T` is a reference type). After
monomorphization the question never arises at runtime:

- Each monomorphized type is a concrete `StructLayout` with concrete field `Ty`s.
  Codegen's **existing** per-struct descriptor emission (`design/0008`: "the
  pointer fields' byte offsets, computed from the LLVM layout") runs on it
  unchanged. `Box[int64]` emits a descriptor with no pointer offsets; `Box[str]`
  emits one with the `str` field's offset. **No per-instantiation GC code is
  written — the descriptor falls out of the concrete layout the pass already
  produced.**
- **Generic enums ride the tier-3 path.** `Option[str]` monomorphizes to the same
  `{ tag: i32, payload… }` GC tagged struct that non-generic tier-3 enums already
  lower to (enums-match #51) — so it inherits tier-3's "zero codegen / `zaprt.c`
  change" property. The `Some` payload field is `str` for `Option[str]` (a
  pointer in the descriptor), `i64` for `Option[int64]` (not) — again purely from
  the concrete layout.
- **Collections of generic element types** also stay precise: a monomorphized
  `list[Box[str]]` is a `list` whose element kind is "pointer", selecting the
  `_ptr` entry point and setting the per-instance element-kind flag exactly as any
  pointer-element list does today. The runtime `ZapList` type is unchanged.

So the precise collector's correctness for generics is a **theorem, not new
code**: monomorphization reduces every generic type to a concrete one, and the
collector is already correct for concrete types.

**Code-bloat mitigation (the accepted cost of mono):**
- **Per-tuple dedup** (above) is the baseline and handles the common "same
  instantiation reached many times."
- **Built-ins absorb most instantiation pressure** — `list`/`dict`/`set` are
  *not* monomorphized (they stay special-cased, Fork X), so the dominant generic
  shapes in real programs cost zero instantiations.
- **v1 does no layout-sharing.** A known optimization (collapse instantiations
  with identical size + pointer-map into one body, à la a limited form of
  "polymorphization") is **deferred**; monomorphization-only is the v1 rule
  (D-0017), and the measured need is unproven. The hook is noted for later.
- A boxed/erased **fallback** for pathological instantiation counts is an explicit
  non-goal for v1 (open issue in `design/0006` §Open issues; stays open).

> **Locked sub-decision (M1): generic bodies are checked at *definition*
> (Rust model), not at *instantiation* (C++ template model).** Unconstrained type
> parameters admit only universal operations (bind, assign, pass, return, store,
> and equality via the type's own `==` **only** when a bound guarantees it);
> calling any method requires a bound (Fork B). Errors are reported once, at the
> generic. This is a genuine fork vs. C++-style duck-typed templates (less sema
> machinery, errors at instantiation, worse messages) — flagged as a product call
> in the report, recommendation **definition-time**.

### Fork S — Syntax (declaration + call-site type arguments)

**Recommendation: keep `design/0006`'s subscript syntax; extend the grammar in
two small, low-controversy ways — optional bounds, and call-site type args.**

Declarations (already the `design/0006` §5.1 surface):

```python
struct Box[T]:
    T value
    def get(self) -> T: return self.value

enum Option[T]:            # generic enum (the flagship)
    Some(T value)
    None

def map[T, U](list[T] xs, fn(T) -> U f) -> list[U]:
    ...

interface Container[T]:    # generic interface (reserved; realized in a later slice)
    def get(self, int64 i) -> T
```

Type parameters are used in every type position: field types, parameter types,
return types, local declarations, and nested instantiations (`list[T]`,
`Option[T]`).

**Grammar extensions (this doc updates `docs/GRAMMAR.ebnf`):**

1. **Bounds** — a type parameter may carry an interface bound:
   ```ebnf
   generics    ::= "[" type_param { "," type_param } "]" ;
   type_param  ::= IDENT [ ":" type ] ;          (* ":" bound is Fork B *)
   ```
   (`type` here is a `named_type`, i.e. an interface name, possibly itself
   generic.) This supersedes the bare-`IDENT` form.

2. **Call-site / construction type arguments** — explicit type args where
   inference cannot or should not carry them:
   ```ebnf
   type_args   ::= "[" type { "," type } "]" ;
   ```
   applied to a *type name* (`Box[str](...)`, `list[T]()`, `Option[int64].None`)
   or a *generic callee* (`map[int64, str](xs, f)`).

**Disambiguation from subscript indexing `a[i]` (D-0047 `__getitem__`).** `name[…]`
is **type arguments** iff `name` resolves to a *type* or a *generic function*;
otherwise it is a value subscript (`__getitem__`). This is a name-resolution
decision made in sema, not a grammar ambiguity: `list[str]` (type) vs `xs[0]`
(value) never collide because `list`/`str` are types and `xs` is a value. The
parser produces a uniform "postfix `[...]`" node; sema classifies it. (Python
resolves the identical `List[int]` vs `xs[0]` overload the same way.)

### Fork I — Type-inference scope for v1

**Recommendation: argument-driven unification, plus expected-type *sink*
propagation at `let`/`return`/argument boundaries. No full bidirectional / flow
inference in v1.**

- **Argument-driven unification** — infer type params from the static types of
  value arguments at a call/construction, left to right, unifying against the
  declared parameter/field types:
  ```python
  auto ys = map([1, 2, 3], double)   # T=int64 from list[int64]; U=int64 from double
  auto b  = Box(value=42)            # T=int64 from the field initializer
  ```
- **Expected-type sink** — when a generic constructor has **no argument** to infer
  from (the `Option[T].None` problem), v1 propagates the type from the immediate
  *expected type* at exactly three sinks: a typed `let`, a `return` in a function
  with a declared return type, and a typed argument position:
  ```python
  Option[int64] x = None             # T=int64 from the binding's type
  def find(...) -> Option[str]:
      return None                    # T=str from the return type
  take_opt(None)                     # T from the parameter's declared type
  ```
  This is the *minimum* bidirectionality that makes `None`/`Option` ergonomic; it
  is a one-level sink, not a constraint solver.
- **Explicit type args** are always available as the fallback and are *required*
  when neither rule determines the args (`list[str]()`, `Box[str]()`,
  `Option[int64].None` as a bare expression).
- **Deferred:** inference through multiple statements, return-type-driven
  inference of *function* type params beyond the sink, GADT-style or
  higher-ranked inference, and defaulting of unconstrained params. v1 errors with
  "cannot infer type argument `T`; write it explicitly" rather than guessing.

### Fork B — Constraints / bounds in v1

**Recommendation: ship *unconstrained* type parameters in the first vertical
(`Option[T]` needs no bounds); land *interface bounds* as the immediately
following slice, reusing the existing I#84 conformance checker. Designed in from
the start (grammar + sema schema carry the optional bound now).**

Rationale: the flagship — `Option[T]`, `Box[T]`, `Pair[K,V]` and their methods —
needs **no** bounds; forcing bounds into the first vertical delays `Option[T]`
(which unblocks json/AWS) for machinery it does not use. And bounds are *cheap*
when they land, because the hard part already exists:

```python
interface Ord:
    def __cmp__(self, other) -> int64

def max_of[T: Ord](list[T] xs) -> T:    # body may call __cmp__; nothing else
    ...
```

- `T: Ord` means "any type that **structurally** satisfies `Ord`" — checked by the
  **same** `iface_conforms` sema routine I#84 already uses, run at each
  instantiation.
- **Bounds mean static dispatch, not a fat pointer.** Because the body is
  monomorphized, a call to a bound method substitutes the concrete type and emits
  a **direct** call — there is no `{data, itable}` fat pointer and no dynamic
  dispatch for bounded generics. (Interface *values* — `Shape s = …` — remain the
  separate, dynamic, fat-pointer feature; see Fork X.) This is the Rust
  `impl Trait` vs `dyn Trait` split, and it keeps bounded generics free at
  runtime.
- Operator-overload dunder interfaces (`Add`, `Ord`, …, D-0047) are valid bounds
  for free: `def sum[T: Add](xs: list[T]) -> T` just works once bounds land.
- **Deferred:** multiple bounds per param (`[T: Add + Ord]`), where-clauses,
  associated types, and bounds referencing other type params. Single interface
  bound per param is the v1-of-bounds surface.

### Fork X — Interaction with existing machinery

- **Value structs (I#75).** A type argument may be a `value struct`; the
  monomorphized type inlines it (value layout, copy semantics) exactly as a
  non-generic field of that type would. A **generic** value struct
  (`value struct Vec2[T]`) is allowed — each instantiation is a concrete value
  struct. Boxing-behind-an-interface (I#75 Part 1) is unchanged: it triggers on
  the *concrete* instantiation at a conversion site.
- **Interface fat pointers (I#84).** Two orthogonal dispatch stories, both intact:
  *bounds* → static dispatch (monomorphized, direct call, no fat pointer);
  *interface-typed values* → dynamic dispatch (fat pointer). A generic can hold an
  interface-typed value as a normal value; the open follow-ups from I#84
  (interface value in a struct **field** / collection **element**) are the same
  follow-ups whether or not the enclosing type is generic — generics add nothing
  new to them.
- **Enums (Option *is* a generic enum).** Generic enums monomorphize to the
  tier-3 tagged struct (Fork M); `match` exhaustiveness, qualified/positional/
  named patterns, and recursive variant fields all carry over from enums-match
  #51 with the type parameter substituted. The tier-3 deferrals (`.name`,
  `for x in Enum`, enum methods) are being lifted on their own tickets; generic
  enums need **enum methods** (for `Option.unwrap` etc.), so that deferral is
  pulled into this epic's Option slice.
- **Built-in collections — stay special-cased in v1; documented migration path.**
  `list`/`dict`/`set`/`array`/`heap` are **not** rewritten as user generics for
  v1. Reasons: they are tuned runtime types (typed entry points, SIMD array
  kernels, open-addressing dict, element-kind GC flags) that a naive
  monomorphized `.zap` reimplementation would regress; and they already deliver
  the generic *surface* (`list[T]`) that users want.
  - **What generics reuse from them:** the *conceptual* instantiation/substitution
    machinery and the element-kind precision tags — **not** the runtime impl.
  - **Migration path (future, product call):** once user generics + bounds are
    mature, the built-ins can be **re-skinned** as generic *surface* types whose
    methods lower to the same runtime entry points (surface-compatible, no runtime
    rewrite) — e.g. a `list[T]` defined in a prelude whose `append`/`__getitem__`
    are intrinsics. This is a source-compatibility nicety, not a correctness need,
    and is explicitly out of scope for this epic.
- **`@derive` / decorators (D-0036).** Derives run per monomorphized
  instantiation (each concrete `Box[int64]`/`Box[str]` gets its own derived
  `__eq__` etc., field-wise over concrete types) — no new derive machinery.
  `@derive(Json)` (D-0059) over a generic struct likewise specializes per
  instance.

### Fork O — `Option[T]`, the flagship

**Recommendation: `Option[T]` is a stdlib generic enum, lowered via the tier-3
tagged representation (no niche optimization in v1). Ergonomics by methods +
`match`, not a new `?` operator. This is what lets json drop `@default`-as-
optionality and the AWS SDK model optional fields properly (fork O-a).**

```python
enum Option[T]:
    Some(T value)
    None

    def is_some(self) -> bool:          # via match on self
    def is_none(self) -> bool:
    def unwrap(self) -> T:              # Some(v) -> v ; None -> raise
    def unwrap_or(self, T default) -> T
    def map[U](self, fn(T) -> U f) -> Option[U]   # needs generic method (slice 4)
```

- **Representation (v1): tagged, reusing tier-3.** `Option[T]` monomorphizes to
  `{ tag: i32, value: T }` — the same GC tagged struct any tier-3 enum lowers to,
  so it inherits precise GC and zero codegen change. `Option[int64]` carries an
  unboxed `i64` payload (and so can represent "absent int", which bare `None`
  cannot — D-0014 makes only reference types nullable); `Option[str]` carries a
  pointer payload traced via the descriptor.
- **Niche / nullable-pointer optimization — deferred.** When `T` is a reference
  type, `None` *could* be the null pointer and `Option[T]` collapse to one word
  (Rust's `Option<&T>` niche). This is **semantically invisible** and a pure
  layout optimization, so v1 ships the tagged form (correctness + reuse) and
  leaves the niche as a documented follow-up — same philosophy as tier-3's
  "flattened record now, overlap later."
- **Pattern matching** is tier-3 `match` unchanged:
  ```python
  match find_user(id):
      case Option.Some(u): print(u.name)
      case Option.None:    print("absent")
  ```
- **Ergonomics — methods, not `?`.** Zap's error model is **exceptions**
  (D-0007), not `Result`-return, so a Rust-style `?` early-return operator does
  **not** fit (there is nothing to early-return in an exception-based flow). v1
  ships `unwrap()` (raises a string-message error on `None` until typed exceptions
  I#12 land, then a typed `UnwrapError`), `unwrap_or(default)`, `is_some`/
  `is_none`, and (once generic methods land, slice 4) `map`/`and_then`. A future
  **`or-else` sugar** (`x or default` when `x: Option[T]`) can desugar to
  `unwrap_or` via `__bool__`/a dedicated lowering — flagged, not v1.
- **Null-safety story.** Two notions of "nothing" coexist and are distinct:
  `None`-as-null-pointer for reference types (D-0014, member access traps), and
  `Option[T]` as the **typed, explicit** optional that works for *any* `T`
  including scalars and that the type system forces you to unwrap. `Option[T]` is
  the recommended way to express "may be absent" in new APIs; the bare-`None`
  reference-nullability stays for interop/low-level code. (A future lint could
  push reference-type optionality toward `Option[T]` too — out of scope.)

**(a) Letting `json` drop `@default`.** `design/0010` §3 uses `@default` (the
`=` form) to mark a field optional, so a missing key silently becomes the zero
value — conflating *absent* with *present-but-zero*. With `Option[T]`:
- The typed codec recognizes an `Option[T]` field: **absent key or JSON `null`
  → `None`**; present → `Some(decode(T))`. Encoding **omits** `None` fields (or
  emits `null` under an opt-in) and unwraps `Some`.
- `@default` reverts to its honest meaning — "use *this* value when the key is
  absent" — and is no longer overloaded to mean "optional". The absent-vs-zero
  conflation is gone.
- This is additive and API-compatible with `design/0010`'s stated plan ("a future
  `Option[T]` field … decoding null/absent to `None`"); it closes the json half
  of #100/#133.

**(b) Giving the AWS SDK proper optional modeling.** `design/0011` fork O's
**preferred O-a** path ("land `Option[T]`, model optional as `Option[T]`") becomes
available: AWS's overwhelmingly-optional shapes generate `Option[T]` fields; the
serializer omits unset (`None`) members, and output members decode absent →
`None` without the O-b "restrict the first wave to read-oriented ops where
absent≈zero" compromise or the O-c hazzer-struct bloat. This retires AWS's
load-bearing blocker (#89).

## Rationale

- **Monomorphization dissolves rather than creates the GC problem.** The epic
  framed per-instantiation GC metadata as a hard requirement; the finding is that
  it is *free* — concrete instantiations hit the collector's existing
  concrete-struct descriptor path. The riskiest-sounding fork is the cheapest.
- **Sema-schema + ZIR-pass + untouched codegen** mirrors the project's proven
  shape: tier-3 enums, value-struct boxing, and interface values all landed by
  *reusing* the concrete struct path and adding a thin lowering step, never by
  teaching codegen a new runtime concept. Generics follow the same grain.
- **Check-at-definition** upholds the compile-time-safety ethos that already
  justifies structural interfaces (D-0016) and `match` exhaustiveness (D-0040):
  the generic is correct for all instantiations or it does not compile.
- **Unconstrained-first, bounds-next** gets the high-value `Option[T]` landing
  (json + AWS unblock) off the critical path of the (cheap, but unneeded-for-the-
  flagship) bounds machinery.
- **Minimal inference (argument + sink)** is enough for the real surface
  (`map([...], f)`, `Box(value=…)`, `None` against a known type) without a
  constraint solver; explicit type args cover the rest honestly.
- **Tagged `Option` + methods, no `?`** fits an exception-based language and
  reuses tier-3 wholesale; the niche optimization is invisible and can wait.
- **Built-ins stay special-cased** because they are the performance of the
  language; re-skinning them is a source nicety, not a correctness requirement,
  and doing it under duress would regress tuned runtime types.

Alternatives rejected for v1: type erasure / dictionary passing (boxes scalars,
runtime cost, fights the ABI — D-0017); C++-style check-at-instantiation (worse
errors, breaks "check once"); bounds in the first vertical (delays `Option[T]`
for unused machinery); niche-optimized `Option` (invisible gain, new codegen);
rewriting the built-in collections as user generics (perf regression risk, no
correctness gain).

## Compatibility

- **Additive.** The declaration syntax parses today (grammar reserved generics
  from the start, D-0017); this doc adds only **bounds** and **call-site type
  args** to the grammar, both additive. No existing `.zap` program breaks.
- **No runtime-ABI change.** Monomorphized types use the existing heap/struct
  layout, the existing per-struct `ZapTypeDesc` emission, and the existing tier-3
  tagged-enum path. The one ABI item this *fills in* (not changes) is the
  **name-mangling scheme for monomorphized generics**, already reserved as an open
  issue in RUNTIME-ABI §"Name mangling" — finalized here as: mangled symbol =
  module path · generic name · arity · the ordered concrete type args (each type
  args mangled recursively), chosen so identical instantiations collide for dedup
  and distinct ones do not.
- **`design/0006` §5** is the surface spec; this doc is its implementation design
  and does not contradict it (it resolves its §Open-issues items: inference scope,
  bounds staging, code-size controls, generic-method/interface genericity
  staging).
- **`design/0010` (json)** line on `@default` optionality and **`design/0011`
  (AWS)** fork O become the migration consumers (slice 6); both were written
  anticipating this.

## Implementation — sequenced vertical slices

Each slice is a shippable vertical (front-end → sema → ir → codegen → golden),
following the project's one-ticket-per-vertical convention. The **first
end-to-end vertical** is slices 1–3 proving `Option[T]` — the thinnest path that
compiles and runs a generic.

**Slice 0 — front-end: parse type params (+ bounds) and call-site type args.**
Extend `docs/GRAMMAR.ebnf` (bounds, `type_args`); parse `[T, U]` with optional
`: Bound` on struct/enum/interface/fn; parse postfix `[...]` type-arg nodes at
call/construction/type positions (uniform node, classified in sema). Sema/lower
emit a clear "generics not yet implemented" until the next slices. *(Grammar
largely reserved; this makes it real and adds the two extensions.)*

**Slice 1 — sema: generic schemas, substitution, inference.** Register generic
type/fn **schemas**; type-check the generic body **once** at definition
(unconstrained: universal ops only — Fork B/M1); implement type-parameter
substitution; **argument-driven unification + expected-type sink** inference
(Fork I); classify postfix `[...]` as type-args vs `__getitem__` by name
resolution (Fork S); record `(generic, concrete-args)` instantiation requests.

**Slice 2 — ir: monomorphization pass + GC metadata (the engine).** Collect the
whole-program instantiation set to a fixpoint; substitute type params to concrete
`Ty` in cloned ZIR; **dedup** by mangled symbol (finalize the mangling scheme);
hand concrete `StructLayout`s to codegen so its **existing** per-struct
`ZapTypeDesc` emission produces correct pointer maps per instance (Fork M — *the
GC-correctness slice; it is mostly "let the existing path run", plus the
collection element-kind tag for generic element types*). Codegen unchanged.

**Slice 3 — `Option[T]` stdlib enum + sugar → FIRST END-TO-END VERTICAL.**
Define `Option[T]` as a generic enum riding tier-3 lowering; implement **enum
methods** on a generic enum (`unwrap`/`unwrap_or`/`is_some`/`is_none`), `None`
via the expected-type sink, and `match`. This is the first program that compiles
and runs a user-visible generic. Goldens: `Option[int64]`/`Option[str]` construct
/ match / unwrap / unwrap_or; a generic `Box[T]`/`Pair[K,V]` smoke test; both GC
collectors (the GC-correctness proof — a generic holding a pointer payload across
a collection).

**Slice 4 — generic functions + generic methods.** Free `def map[T,U](...)`,
generic methods (`Option.map[U]`), full argument-driven inference across function
type params. (`Box`/`Option` from slice 3 are generic *types* with concrete
methods; this adds type params on *functions*.)

> **Status (I#161, implemented).** Generic free functions and generic methods
> (with their own type parameters, on generic or non-generic owners) are done,
> extending the `mono.rs` erasure pass: a generic function is emitted on demand
> as a mangled concrete function (deduped by mangled name); a generic method is
> specialized per call and attached to its owner's concrete declaration, keeping
> the GC-metadata-free theorem (every instance is concrete). Call-site inference
> is argument-driven unification (over named/list/tuple/function type structure)
> plus the expected-type return sink, with explicit `f[T](...)` / `recv.m[U](...)`
> type arguments as the fallback. `Option[T]` gains a callback-free `and_opt[U]`
> (Rust's `Option::and`). **Deferred:** the *higher-order* flavor shown above
> (`map[T,U]` / `Option.map[U]` taking an `fn(T)->U`) needs first-class function
> *values* — the `fn(...)->...` type does not yet parse and codegen rejects
> `Type::Func`, so passing/calling a function value is a separate feature (its own
> follow-up ticket). Everything in slice 4 that does not require a function value
> is implemented and tested under both collectors (`tests/golden/generics_fn_*`,
> `tests/golden/generics_method.zap`). Interface bounds remain slice 5.

**Slice 5 — interface bounds (`[T: Iface]`).** Reuse the I#84 `iface_conforms`
checker at instantiation; allow bound methods in the body (static dispatch); wire
operator-dunder interfaces as bounds. (Grammar + schema already carry the bound
from slice 0/1.)

**Slice 6 — migration consumers.** (a) `json` typed codec recognizes `Option[T]`
fields (absent/null → `None`, omit on encode); **drop `@default`-as-optionality**
(#100/#133). (b) AWS SDK models optional shapes as `Option[T]` (fork O-a),
retiring #89's blocker. These are separate tickets owned by json/AWS, gated on
slices 1–3.

**Slice 7 — optional follow-ups (not scheduled here).** `Option` niche/nullable-
pointer layout optimization; `Result[T, E]`; `or-else` sugar; multiple/where-
clause bounds; generic *interfaces* (`Container[T]`) realization; built-in
collection re-skin as generic surface; layout-sharing for code-size.

**Parallel-safety.** Slices 1→2→3 are a hard chain (the first vertical). Slice 0
can precede/overlap. Slices 4 and 5 are independent of each other once 1–3 land;
slice 6 is independent per consumer and only needs slice 3. Phase boundaries
touched: **sema** (schemas/inference), **ir** (the monomorphization pass + mangle
finalize), **front-end** (two grammar extensions); **no codegen or `zaprt.c`
change** (the recurring win). Keep gates per the project norm: `cargo fmt` before
push (CI Format check), host `cargo test`, `make docker-test` under **both**
collectors, containerized `clippy -D warnings` under `zapc/llvm` and
`zapc/gc-immix`.

## Decisions (proposed — ratify as D-0064)

**Recommended as locked:**
- **M — monomorphization** (confirm D-0017), as a ZIR pass between sema and
  codegen; **M1 — generic bodies checked at definition** (Rust model), not at
  instantiation.
- **GC metadata — no new mechanism**: monomorphized instances hit codegen's
  existing per-struct `ZapTypeDesc` path; generic enums ride tier-3; generic
  collection elements ride the element-kind tag. Correctness is a theorem.
- **S — subscript syntax**, plus two additive grammar extensions (bounds,
  call-site `type_args`); postfix `[...]` classified type-args-vs-subscript by
  name resolution.
- **I — inference** = argument-driven unification + expected-type sink at
  `let`/`return`/arg; explicit type args required otherwise; no v1 flow inference.
- **O — `Option[T]`** = stdlib generic enum, tagged tier-3 representation (niche
  deferred), ergonomics by methods + `match` (no `?` operator); it is the first
  vertical and unblocks json (drop `@default`-as-optional) and AWS (fork O-a).
- **X — built-in collections stay special-cased** in v1; re-skin is a future
  source-compat nicety, not scheduled.
- **First end-to-end vertical = slices 1–3** (generic *types* + `Option[T]`);
  generic *functions* and *bounds* follow.

**Open product calls for the user (see report):**
1. **Bounds in the first vertical, or deferred to slice 5?** Recommendation:
   deferred — the flagship needs none.
2. **Check-at-definition (M1) vs C++-style check-at-instantiation?**
   Recommendation: definition-time.
3. **Collections migration — ever re-skin the built-ins as user generics, or
   leave special-cased permanently?** Recommendation: leave special-cased; revisit
   only if source-compat demand appears.
4. **`Option` niche optimization — accept the tagged form indefinitely, or
   schedule the one-word niche for reference `T`?** Recommendation: tagged now,
   niche only if profiling shows it matters.

**Deferred (fast-follow, not v1):** generic functions beyond the flagship
(slice 4 is still v1-ish but after the vertical); multi/where-clause bounds;
generic interfaces; `Result[T, E]`; `?`/`or-else` sugar; niche `Option` layout;
built-in re-skin; code-size layout-sharing; typed `UnwrapError` (rides I#12).
