# Design: Precise (Immix-style) garbage collector

- **Author(s):** Zap team
- **Last updated:** 2026-10-20
- **Status:** Implemented (v1, behind the `gc-immix` build flag; Boehm remains
  the default). All 5 forks were approved as recommended, including 4a.
- **Discussion:** github.com/parveshgarg/zap#48
- **Supersedes the GC sections of:** `design/RUNTIME-ABI.md`, `design/0005-concurrency.md`

## Abstract

D-0009 locked the *direction* — replace the v1 conservative Boehm GC with a
precise, Immix-style collector — but not the design. This document specifies
that collector: the Immix heap, the object-layout metadata and root-finding
mechanism, the (absence of a) write barrier, how the M:N scheduler's parked
coroutines and running mutator threads become **precise** roots without Boehm's
conservative tricks, the stop-the-world protocol, and a flag-gated migration
that keeps Boehm selectable so the golden suite stays green throughout.

It calls out every genuine design fork (marked **FORK**) for a decision.

## Background — what Boehm gives us today, and must be replaced

The current runtime (`crates/codegen/src/zaprt.c`) is a **conservative** Boehm
collector. Three properties it provides for free must be rebuilt precisely:

1. **Root finding.** Boehm scans the C stack, registers, and globals
   conservatively — *no stack maps, no safepoints*. `zap_alloc(size)` is just
   `GC_MALLOC(size)`; there is **no type-descriptor header on objects today**
   (user structs are allocated headerless and field offsets start at 0 — the
   "one-word header" in RUNTIME-ABI.md is forward-looking, not yet emitted).
2. **Parked-coroutine roots (0005 §"Green-thread stacks").** Each goroutine `G`
   gets a `GC_malloc`'d green stack and an in-`G` saved `ucontext_t`; because
   both are ordinary traced heap, a *suspended* G's live pointers are retained
   as heap, and every live G is also held in a static-rooted `g_all[]`. A parked
   coroutine is invisible as a thread but reachable as heap — the "key trick."
3. **Running-coroutine roots + STW.** A running G executes on its green stack on
   some M OS thread; `GC_set_stackbottom` is updated across each `swapcontext`
   so Boehm scans the right range. Stop-the-world is Boehm's **pthread
   suspension** of the M threads.

Replacing Boehm means redoing all three *precisely*. The good news: the IR
already carries exactly the metadata we need — `zir::StructLayout { fields:
Vec<Ty> }` records every field's `Ty`, and reference types are distinct `Ptr`
variants (`Ptr`, `List`, `Dict`, `Set`, `Heap`, `Array`, `Chan`, `Match`, `Py`)
vs. unboxed scalars (`I64`, `F64`, `Bool`, …). The pointer map of any struct is
a pure function of `fields`. Runtime collection objects (`ZapStr`, `ZapList`,
`ZapDict`, …) have fixed, known layouts and get hand-written descriptors.

## Proposal — overview

- **v1 is NON-MOVING** (mark-only line marking over an Immix heap). Opportunistic
  evacuation/defragmentation is deferred to v2. (**FORK 1**, recommended.)
- **Roots are found via an explicit shadow stack**, not LLVM statepoints, in v1.
  (**FORK 2**, recommended.) This makes parked-coroutine roots *trivial* and
  avoids decoding machine stacks entirely.
- **No write barrier, non-generational** in v1 — a tracing full-heap mark
  collects cycles naturally and keeps `FieldSet` codegen untouched. (**FORK 3**.)
- **Cooperative stop-the-world** via a global "GC requested" flag polled at
  safepoints, reusing the scheduler's existing cooperative yield points.
  (**FORK 4** is about whether to also add loop back-edge polls in v1.)
- **Boehm stays selectable behind a build flag** (`gc-boehm` default / `gc-immix`
  opt-in) during bring-up; goldens stay byte-identical on the Boehm path, the
  Immix path is exercised by new GC tests and an added `full`-check variant run.
  (**FORK 5**, recommended, preserves the two-check CI contract.)

### Immix heap structure (v1)

Classic Immix geometry:

- **Blocks** of 32 KiB, obtained from the OS in coarse chunks (mmap arena).
- Each block is divided into **lines** of 128 bytes (256 lines/block).
- **Bump allocation** within a block's current free region; objects may span
  lines but not blocks (large objects go to the LOS, below).
- Block states after a collection: **free** (no live lines → returned to the
  arena free list), **recyclable** (some free line runs → reused by bumping
  through the gaps), **full** (no usable free runs).
- **Implicit-mark conservative line marking (Immix's trick):** marking an object
  marks its lines; because small objects can straddle a line boundary, a marked
  line additionally guards the *following* line (the standard Immix
  one-line conservative margin). This is an allocation-granularity approximation
  only — object marking itself is fully precise.
- **Large Object Space (LOS):** objects larger than a tunable threshold
  (recommend **8 KiB**, ≈ a quarter block) are allocated directly from the arena
  as single-object, mark-only regions (never bump/line managed). Green-thread
  stacks (128 KiB, 0005) are LOS objects.
- **Per-P allocation.** Each scheduler `P` owns its current allocation block so
  the fast path is a lock-free bump; empty/overflowing blocks are swapped under
  a global heap lock. (Mirrors the existing per-P deque ownership.)

Why Immix over plain mark-sweep: near-bump-pointer allocation speed and good
locality without a size-classed free-list, while v1 stays non-moving so we never
touch an interior pointer. Evacuation (v2) later reclaims fragmentation by moving
objects out of lightly-occupied recyclable blocks — the geometry is chosen so
that upgrade needs no heap-format change, only a read/forwarding story.

### Object-layout metadata (the TypeDesc header)

On the **Immix build only**, every heap object gains the one-word header that
RUNTIME-ABI.md already reserves:

```
offset 0:  const TypeDesc*      (8 bytes)   — low bits reused for mark/pin state
offset 8:  payload (fields / buffer) ...
```

- **`zap_alloc` grows a descriptor argument:** `void* zap_alloc(const TypeDesc*
  desc)` (size lives in the desc). `zap_alloc_atomic` keeps a size-only,
  pointer-free form for byte buffers (their descriptor is the shared
  "no pointers" one). Codegen's `StructNew` passes a per-struct `TypeDesc*`.
- **Field offsets shift by one word** on the Immix build: codegen inserts the
  header into the LLVM struct type and GEPs fields at `idx + 1`. On the Boehm
  build nothing changes (headerless, byte-identical goldens).
- **TypeDesc shape:**

  ```c
  typedef struct TypeDesc {
      int64_t size;          // total bytes incl. header
      uint8_t kind;          // SCALAR_BITMAP | VARLEN_TRACE | ATOMIC
      uint8_t _pad[7];
      uint64_t ptr_bitmap;   // bit i set => word i of payload is a pointer
      void (*trace)(void *obj, void (*mark)(void *child));  // VARLEN_TRACE only
  } TypeDesc;
  ```

  - **Fixed structs** (≤ 63 pointer-word fields): a `ptr_bitmap` computed at
    codegen time from `StructLayout.fields` — bit *i* set iff `fields[i]` is a
    reference `Ty`. Emitted as an LLVM global constant per struct, in a dedicated
    section so globals/roots registration can find them.
  - **Variable-length runtime objects** (`ZapList` of pointers, `ZapDict`
    entries, `ZapArray` of pointer dtype, `ZapStr`'s `data`, the `G` struct and
    its saved context, `ZapPy`): `kind = VARLEN_TRACE` with a hand-written
    `trace` callback that calls `mark` on each live child (e.g. list traces
    `data[0..len]` when the element rep is a pointer; `data` buffers that are
    pointer-free use `ATOMIC`). One descriptor per runtime type, defined in
    `zaprt.c`.

- **Pointer maps are derivable, not guessed.** `crates/ir` gains a tiny helper
  (`StructLayout::ptr_bitmap()`) so codegen and any future reflection share one
  source of truth; this is the one IR/codegen change the collector *requires*.

### Root enumeration — the shadow stack (FORK 2)

v1 finds on-stack roots with an **explicit, codegen-maintained shadow stack**
rather than LLVM statepoints/stackmaps. Each frame that holds live GC pointers
across a safepoint/call links a small record onto a per-execution-context chain:

```c
typedef struct ShadowFrame {
    struct ShadowFrame *prev;   // caller's frame
    int32_t  nroots;
    void    *roots[/*nroots*/]; // slots the mutator spills live pointers into
} ShadowFrame;
```

- Codegen emits, in the prologue of any function with live refs across a call, a
  `ShadowFrame` alloca linked to the context-local head, and spills/reloads live
  pointers around calls (LLVM's built-in **`gc "shadow-stack"`** strategy does
  exactly this; we use a *per-context* head instead of LLVM's single global
  `llvm_gc_root_chain`, because we are multi-threaded + green-threaded).
- **The head pointer lives in the `G`**, not in a thread-local. This is the
  crux: **a parked coroutine's roots are just its shadow-stack chain**, walked
  directly from `G->shadow_head` with zero machine-stack decoding and zero
  dependence on retaining the green stack as traced heap. The Boehm "stack as
  heap" trick is *gone*; `g_all[]` still enumerates every live `G`, and for each
  we walk its shadow chain. Running vs. parked is irrelevant — same mechanism.
- **Globals** are enumerated from the per-struct descriptor section plus an
  explicit root registry (`zap_gc_add_global_root`) the runtime uses for its own
  static tables (e.g. the reflection registry, interned descriptors).

Why shadow stack for v1 (the recommendation): it is natively supported by LLVM,
is portable (no `RewriteStatepointsForGC`, no stackmap-section parsing, no
reliance on the exact relocation of return addresses across our hand-rolled
`ucontext` switch), and it collapses the hardest problem in this ticket — precise
roots for *suspended* coroutines — into "walk a linked list." Its cost is the
spill/reload and frame-link overhead on ref-heavy functions. Because v1 is
non-moving, we never need the "relocate the value the stack map points at"
capability that justifies statepoints; that capability becomes necessary only
when we add evacuation (v2), at which point statepoints are the natural upgrade.
**This is a genuine fork** — the alternative is to adopt LLVM statepoints now and
eat the complexity up front to avoid the shadow-stack runtime overhead and a
later migration. Recommendation: **shadow stack for v1, statepoints with v2
evacuation.**

### Write barrier / generational story (FORK 3)

**None in v1.** A non-generational, non-concurrent, full-heap tracing mark has no
inter-generational or incremental invariant to maintain, so `FieldSet` and array
stores emit no barrier and codegen is untouched. Tracing collects cycles
naturally (the cycle-collection GC test is satisfied by construction).

When we later add a **generational or concurrent** collector, the barrier's home
is already identified: a Yuasa/Dijkstra-style barrier wraps `Inst::FieldSet`, the
array/list element stores, and dict/set inserts in codegen, feeding a per-P
remembered set (sequential store buffer) drained at collection. Flagged here so
the hook points are agreed now; **not built in v1.**

### Stop-the-world (FORK 4)

Boehm's pthread suspension is replaced by **cooperative safepoints**, which fit
the scheduler's already-cooperative model (Gs yield only at scheduling points):

- A global `gc_stop_requested` flag + a barrier/condvar. The collector thread (the
  requesting M) sets the flag and waits until every M has **parked at a
  safepoint**.
- **Safepoints** are: (a) the allocation slow path (block exhausted), (b) the
  existing scheduler scheduling points / channel park-and-yield, and (c) a cheap
  poll at function entry and loop back-edges (**FORK 4**).
- At a safepoint an M flushes its current register-held roots to its active G's
  shadow frame (already there, by construction), marks itself parked, and spins
  on the barrier. When all Ms are parked, *all* Gs are provably at safepoints
  (running Gs because their M is parked at one; parked Gs because they
  suspended at a scheduling-point safepoint). The collector then has a globally
  consistent root set: `g_all[]` × shadow chains + globals.
- After marking + sweeping, the flag clears and Ms resume.

**FORK 4 is specifically the loop back-edge poll.** v1 is *cooperative with no
preemption* (0005): a tight, allocation-free, yield-free compute loop never hits
a safepoint and would stall a pending GC indefinitely. Options:
- **(4a, recommended)** Emit a one-load-one-branch safepoint poll at loop
  back-edges and function entry. Bounds time-to-safepoint; tiny overhead;
  reuses the same machinery a future preemptive scheduler wants.
- **(4b)** Accept the stall in v1 (consistent with "cooperative, no preemption"),
  poll only at allocation + scheduling points, and document the pathology. Less
  code now; a known liveness gap.

Recommendation: **4a** — the poll is cheap, and without it a `for` loop building
a large structure (exactly the large-live-set test) can starve the collector.

### Migration & safety (FORK 5, recommended)

- **Boehm stays selectable behind a build flag.** A cargo feature pair on the
  codegen/driver crate — `gc-boehm` (**default**) and `gc-immix` — selects which
  runtime is compiled and linked (`-lgc` vs. the new `zapgc` objects) and which
  `zap_alloc` ABI + header behavior codegen emits. During bring-up the default
  stays Boehm, so the **entire existing golden suite is byte-identical and
  green**; Immix is opt-in.
- **A/B and fallback.** The two builds share every `zap_*` entry-point name and
  observable behavior, so any program can be built both ways and diffed. If Immix
  regresses, the fallback is a one-flag change.
- **CI — two-check contract preserved.** `host (no LLVM)` is unchanged. The
  `full (LLVM 18 + codegen)` check additionally builds+runs the golden suite a
  second time under `gc-immix` (and runs the new GC stress tests only there).
  Both still roll up into the single required `full` check — no third required
  check, contract intact. Containerized clippy runs
  `--features zapc/llvm --all-targets -D warnings` as today; the immix runtime is
  C compiled by the same path as `zaprt.c`.
- **Flip criteria.** Promote `gc-immix` to default only after it passes the full
  suite + GC stress tests; keep `gc-boehm` buildable for one release as the
  documented fallback, then revisit removal.

### New GC-specific tests (Phase 2)

- **Allocation stress** — millions of short-lived objects force many collections;
  assert correct output + bounded RSS.
- **Cycle collection** — build an unreachable object cycle (mutually referencing
  structs); assert it is reclaimed (tracing handles this; a refcount scheme would
  not).
- **Across a coroutine boundary** — a `scope:`/`spawn` where a parked coroutine
  holds the only reference to a live object, GC runs while it is parked, and the
  object survives and is correct on resume. This is the parked-coroutine-root
  test and the direct replacement for the Boehm "stack-as-heap" property.
- **Large live set** — a big persistent structure (e.g. a long list/tree of many
  objects) kept live across repeated GCs; assert nothing is dropped and the live
  set is walked precisely (exercises FORK 4a).

## Rationale

- **Non-moving + shadow stack** is the shortest correct path off Boehm: it needs
  no interior-pointer rewriting and no stackmap decoding, and it turns the
  scariest requirement (precise roots for suspended coroutines, which the whole
  0005 design bent around) into a linked-list walk. We ship correctness first and
  earn moving/evacuation later on proven metadata.
- **Immix geometry now** means the v2 evacuation upgrade is additive, not a
  reformat.
- **Flag-gated Boehm** de-risks the single most invasive runtime change in the
  project: the suite is always green, A/B is trivial, rollback is one flag.

## Compatibility

The Immix path is a **breaking ABI change on its own build** (header word, shifted
field offsets, `zap_alloc` descriptor argument, new root/safepoint entry points),
but it is **invisible to the default Boehm build**, so no existing artifact
changes until we flip the default. Flipping the default is the ABI break and is
gated on the suite passing both ways.

## Implementation (Phase 2 sketch, after ack)

1. `crates/ir`: `StructLayout::ptr_bitmap()` + descriptor metadata on the module.
2. New runtime `zapgc.c` (or `crates/runtime`): Immix arena/blocks/lines, LOS,
   `zap_alloc(desc)` / `zap_alloc_atomic`, mark worklist, sweep, descriptor-driven
   trace, shadow-stack + `g_all[]` + global-root enumeration, cooperative
   safepoint STW.
3. `crates/codegen`: `gc-immix`-gated header emission + `idx+1` field GEPs,
   per-struct `TypeDesc` globals, shadow-frame prologues + root spills, safepoint
   polls (FORK 4a), `zap_alloc` descriptor-passing.
4. Scheduler: replace the Boehm `GC_set_stackbottom`/`g_all`-as-heap reliance with
   shadow-chain enumeration; keep `g_all[]` as the G registry; park-point =
   safepoint.
5. Tests above; wire the `gc-immix` variant into the `full` CI check.
6. Update `design/RUNTIME-ABI.md` §GC and `design/0005-concurrency.md` §GC
   interaction to describe the precise collector.

## Open issues / forks for decision

- **FORK 1** — v1 non-moving (recommended) vs. opportunistic evacuation now.
- **FORK 2** — shadow stack (recommended) vs. LLVM statepoints for roots.
- **FORK 3** — no write barrier in v1 (recommended); confirm generational is a
  later ticket, hook points as noted.
- **FORK 4** — loop back-edge safepoint poll: **4a** emit it (recommended) vs.
  **4b** accept the cooperative stall and document it.
- **FORK 5** — keep Boehm behind `gc-boehm`/`gc-immix` flag with the Immix CI
  variant folded into `full` (recommended) vs. a hard cutover.
- **Secondary:** LOS threshold (recommend 8 KiB); block/line sizes (recommend
  32 KiB / 128 B); whether runtime collection buffers carry headers or stay
  descriptor-by-container (recommend container-traced: the `ZapList` object has a
  header, its `data` buffer is a raw `ATOMIC`/traced-by-parent allocation).

## Implementation notes (as built)

Deltas from the plan above, decided during implementation (none change the
approved forks):

- **Header is behind the pointer, not a prepended field.** Every object is
  `[ descriptor|mark (8) | payload_size (8) | payload… ]` and code holds the
  *payload* pointer. This keeps struct field offsets and the runtime object
  layouts byte-identical to the Boehm build (no `idx+1` GEP shift was needed).
  The collector maps `payload -> descriptor` via `payload[-2]`. The size word
  lets line-marking know an object's extent even for variable-length atomic
  buffers that share one descriptor.
- **Descriptors use a pointer-offset array, not a bitmap.** Struct fields can be
  sub-word (i8/i16/i32/f32), so codegen emits the pointer fields' byte offsets
  (from the LLVM `TargetData` layout) rather than a word bitmap.
- **Pointer validation instead of per-value classification.** `Ty::Ptr` is
  overloaded (heap pointers, stack allocas, function addresses, inline
  value-struct pointers), so codegen roots *every* pointer-typed value and the
  collector validates each candidate against a block registry (membership +
  16-byte alignment) before dereferencing its header. In the non-moving design
  no interior pointers arise, so this is precise, not conservative — it only
  filters out the non-heap pointers safely.
- **GC runs only at safepoints, never inside allocation.** The runtime's own C
  code has no shadow frames, so a collection must never fire while runtime C
  holds unrooted temporaries. Allocation only sets a "GC wanted" flag; the next
  `zap_safepoint()` in compiled code (where all live pointers are shadow-rooted)
  performs the collection.
- **Shadow-stack rooting is per-ZIR-value, stored at end of the defining block;**
  dead-but-slotted values are harmless over-retention for a non-moving heap. The
  head is restored at each landing pad so an unwound-through frame chain is
  repaired before handler/cleanup code runs.
- **Container interiors are traced PRECISELY, not conservatively.** A hybrid
  (precise everywhere statically known, conservative scan of homogeneous
  container element buffers) was considered and approved as a fallback: in a
  non-moving v1 nothing is evacuated, so conservative interior scanning is
  cost-free for correctness (it can only over-retain, never corrupt) and is
  already strictly more precise than today's fully-conservative Boehm. But
  implementation found the typed entry points (`_i64`/`_ptr`, `_str`/`_fn`)
  already reveal each container's element/key/value pointer-ness, so a one-byte
  per-instance flag (set lazily in those entry points) makes the element buffers
  **fully precise** at negligible cost — so v1 takes that. A useful consequence:
  the **element-kind tags are the v2 prerequisite for evacuating container
  interiors, and they already exist**, so moving container contents in v2 needs
  no new metadata (only a read/forwarding barrier).
- **Uniform GC control surface (for ticket #77 signals).** Both backends export
  the same symbols so a SIGUSR1 "force GC + stats" handler needs no per-backend
  `#ifdef`: `void zap_gc_collect(void)` (force a full collection) and
  `void zap_gc_stats(int64_t *heap_bytes, int64_t *live_bytes)` (mapped-heap and
  live/used bytes, best-effort per backend — bdwgc heap/free bytes for Boehm,
  mapped + last-collection live estimate for Immix).
- **Finalizers / cleanup (ticket #92).** Objects that need cleanup when collected
  (at minimum the `ZapPy` CPython-interop wrapper, which `Py_DECREF`s its owned
  reference) register a finalizer through one backend-uniform call,
  `zap_gc_register_finalizer(payload, fn, client_data)`. Under Boehm this
  forwards to `GC_register_finalizer`; under Immix the collector runs it via a
  post-mark finalizer queue. The single call site is `zap_py_wrap`, so no
  `#ifdef` leaks into the interop code.
  - **Mechanism.** A registry of `(payload, fn, client_data)` records live
    registered objects. After the normal mark from the true roots drains, the
    collector determines the full set of registered objects whose mark bit is
    clear (unreachable), queues them and drops them from the registry in one
    pass, *then* marks that whole set (and its transitive referents) so neither a
    finalizable object nor anything it points at is reclaimed before its
    finalizer runs. The mark worklist is drained again, then sweep proceeds. The
    queued finalizers run after the sweep and after the world resumes, in the
    collector thread before the triggering collection returns — ordinary mutator
    context, never mid-mark — mirroring how Boehm runs finalizers outside the GC.
    `Py_DECREF` (which takes the GIL) is safe there. The registry is never a
    root: it is scanned only after marking, so registering an object does not by
    itself keep it alive.
  - **Semantics (and where they match/diverge from Boehm).** *One-shot:* each
    registration fires at most once and is removed when it fires; a finalizer
    that resurrects its object keeps the object but will not run again unless
    re-registered (`zap_py_finalize` NULLs its ref, so a repeat would no-op
    anyway). *Latency:* a finalizable object survives the collection that queues
    it and is reclaimed on a later cycle, once unregistered. *Finalizable
    cycles:* because the unreachable-registered set is fixed before any of it is
    marked, a cycle of mutually-referencing finalizable objects is still
    finalized (each exactly once) — deliberately simpler than Boehm, which leaves
    such cycles uncollected. The `ZapPy` wrapper never forms a finalizable cycle
    (each owns one independent CPython ref), so the observable behavior matches
    Boehm for the path this ticket exists for. *Ordering:* none is guaranteed
    among finalizers run in one cycle (Boehm topologically orders acyclic
    chains); this matters only for finalizable objects that reference each other
    and whose finalizers are order-dependent, which `ZapPy` is not.
  - **Test.** `__gc_finalizer_selftest()` (an internal hook, gc-immix only, like
    `__signal_selftest`) registers an incrementing stand-in finalizer on 64
    unreachable objects, forces two collections, and returns how many ran; the
    gc-immix driver test asserts 64 (every finalizer ran, exactly once). A
    stand-in counter stands in for the real `Py_DECREF` because the v1 Python
    interop (modules cached, only int/float/str marshaled through module
    functions) gives a real decref no deterministic, observable effect; the
    machinery exercised is identical to the `ZapPy` path.
- **v1 simplifications (follow-ups):** allocation is under one global heap lock
  (not per-P bump); user-level cyclic *literals* can't yet be written (the front
  end lacks `None` / struct-field mutation), though the tracing collector
  reclaims cycles by construction, as the reclamation tests show.
