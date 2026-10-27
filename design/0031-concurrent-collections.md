# Proposal: CAS / lock-free concurrent collections (built on `Atomic[T]`)

- **Author(s):** Zap toolchain team
- **Last updated:** 2026-10-27
- **Status:** Draft (design-only; awaiting maintainer review — proposes D-0077,
  not accepted)
- **Discussion:** issue #184. Builds on D-0066 (`Atomic[T]`, I#176/PR#177) and
  D-0008/D-0009 (precise moving Immix GC, `design/0008-precise-gc.md`, I#120).
  Constrained by D-0076 (1.0 spec-freeze): anything shipped here is **additive /
  Experimental** (STABILITY §4.2).

> Formatting convention (from Go): wrap near the 80-column mark and start each
> sentence on a new line.

## Abstract

`Atomic[T]` (D-0066) gave Zap real compare-and-swap, which nominally unblocks
lock-free concurrent collections — a concurrent queue, a concurrent map,
counters.
This note argues that the data-structure algorithm is **not** the hard part and
is **not** where the risk lives.
The gating correctness question is the interaction between a slot that holds a
*heap pointer* and Zap's **moving** Immix collector (I#120): a collector that
relocates objects and a mutator that CAS-updates pointer words are, in general,
a recipe for stale pointers and use-after-collect.

The analysis (§4) resolves that interaction and reaches a clean split:

- **Non-pointer collections** (a counter, and a queue/stack of `int64` or opaque
  handle payloads) touch the GC not at all.
  They are correct on every platform today, including aarch64-Linux, and depend
  on nothing but `Atomic[T]`'s existing `seq_cst` guarantee.
- **Pointer-holding collections** *are* safe under the moving collector —
  **but only** because three runtime properties already hold (evacuation is
  stop-the-world, mutators reload pointers load-at-use, and reclamation is
  tracing), and **only if** the collection never defeats precise tracing by
  stuffing a pointer into an integer word.
  They additionally inherit the open aarch64 evacuation race (I#223) and are not
  trustworthy on that platform until it is fixed.

The recommendation (§6) is to **ship the minimum useful set first** — a
`ConcurrentCounter` and a lock-free **MPSC queue** — and **defer the map**, for
which a sharded `Mutex`+`dict` is almost always the right engineering answer
rather than a CAS map.
The proposed decision is **D-0077**, Proposed/pending ratification, with three
load-bearing calls surfaced in §7 for the maintainer to rule on.

**This is a design-only ticket.**
It adds this one file and changes no compiler, runtime, stdlib, parser, or
codegen code, and no `SPEC.md` / `GRAMMAR.ebnf` / `STABILITY.md`.

## Background

### What exists today

- **`Atomic[T]` (D-0066, shipped).**
  A built-in generic struct `struct Atomic[T]: T value`, one machine word, with
  intrinsic methods `load()`, `store(v)`, `add(n)`/`sub(n)` (integer cells),
  `swap(v)`, and `compare_and_swap(old, new) -> bool`.
  Every operation is **sequentially consistent** (`seq_cst`); relaxed/acq-rel
  were deliberately left as a later addition behind the same node shapes.
  A reference payload (`Atomic[Node]`) is accessed pointer-wide and — crucially —
  the pointer variant is an **ordinary struct whose single field is a reference**,
  so the collector's per-struct `ZapTypeDesc` marks that word as a traced pointer
  and scans it as a normal GC slot with *zero* new runtime code.
- **Sync toolbox (D-0032, shipped):** `Mutex`, `RWMutex`, `WaitGroup`, `Once`,
  plus `Atomic[T]`. Channels + `select` are the first-choice idiom ("share memory
  by communicating").
- **Plain collections (shipped):** built-in `list`/`dict`, and a `deque`
  (`crates/sema/src/modules/deque.rs`). None is thread-safe; the sanctioned way
  to share one today is to guard it with a `Mutex`.
- **The moving Immix collector (D-0008/D-0009, I#120, shipped, default).**
  Precise, tracing, **cooperative stop-the-world**, and as of I#120
  **opportunistically moving**: during a collection it evacuates live objects out
  of the sparsest blocks into dense ones and reclaims the emptied blocks.
  The pieces that matter here are spelled out in §4.
- **OS threads (I#175, closed):** a `threading` compat shim exists as a
  migration escape hatch; true parallelism is the M:N scheduler with no GIL, so
  concurrent collections must be correct under genuine multi-core parallelism,
  not just cooperative interleaving.

### The one-paragraph honesty check — what gap does this close vs. just locking?

The baseline is `Mutex` + a plain collection, and it is **already correct,
already shipped, and good enough for most workloads** — a mutex-guarded `deque`
is a perfectly fine concurrent queue.
A lock-free structure earns its complexity in exactly three situations, and
nowhere else:
(1) a **counter/flag**, where `Atomic[int64].add` is strictly simpler *and*
faster than a mutex and there is no reason to ever use a lock;
(2) a **producer/consumer queue under real cross-thread contention**, where a
mutex convoys — a coroutine (or OS thread) descheduled while holding the lock
stalls every other party, and lock-free enqueue/dequeue removes that cliff;
(3) **scalability** on many cores (I#175), where a single mutex is the
bottleneck.
For a **map**, by contrast, a sharded `Mutex`+`dict` wins almost always: it is
dramatically simpler, it supports the full `dict` surface (iteration, resize,
composite values) that a CAS map cannot cheaply offer, and the contention it
leaves on the table is rarely the program's bottleneck.
So this proposal is deliberately narrow: ship where lock-free is unambiguously
better (counter, queue), and *recommend the lock* where it is (map).

## Proposal

### Surface principle: concurrency lives in the type name, not the syntax

Per the ticket's design question 3, the read/iterate surface stays
Python-`list`/`dict`-faithful and introduces **no new syntax**.
Concurrency is expressed purely by choosing a concurrent *type*; the methods
look like the ordinary collection's where the semantics allow it.
All three types ship in a new **`concurrent`** stdlib module (import-gated, not
prelude), tiered **Experimental** per D-0076.

```zap
import concurrent

# 1. A counter — the trivial, unambiguous win.
let hits = concurrent.Counter()      # or Counter(start=0)
hits.add(1)                          # atomic; returns the new value
let n: int64 = hits.get()

# 2. An MPSC queue of a value payload (no GC interaction at all).
let q: concurrent.Queue[int64] = concurrent.Queue()
q.push(42)                           # any producer coroutine/thread
let v, ok = q.pop()                  # single consumer; comma-ok like channels

# 3. An MPSC queue of references (GC-interacting — see §4).
let jobs: concurrent.Queue[Job] = concurrent.Queue()
jobs.push(Job(id=7))
let job, ok = jobs.pop()
```

Notes on the surface:

- **`pop()` is comma-ok** (`value, ok`), matching Zap's channel-receive and
  comma-ok iteration protocol (D-0048) rather than raising on empty — a queue
  being empty is an ordinary, expected outcome, not an exception.
- **No iteration / no `len` contract on the queue.** A lock-free queue has no
  consistent snapshot to iterate, and a `len()` is racy-by-construction; exposing
  either would be a Python-faithful lie.
  `Counter.get()` and `Queue.pop()` are the honest surface.
- **Payload genericity is the whole GC story.** `Queue[int64]` and
  `Queue[Handle]` (an opaque integer newtype) are *non-pointer* instantiations;
  `Queue[Job]` is a *pointer* instantiation. They are the same generic type; the
  monomorphizer produces a pointer-free or pointer-bearing node descriptor
  exactly as it already does for `Atomic[int64]` vs. `Atomic[Node]` (D-0066).

### Queue choice: MPSC, not MPMC (for the first cut)

Recommend a **multi-producer / single-consumer** unbounded linked queue.
Justification:

- MPSC is the overwhelmingly common shape — fan-in to a worker, a log/event
  sink, a scheduler run-queue drain — and it is the one a mutex convoys worst
  (many producers contending on one lock).
- A single consumer removes the hardest part of a general MPMC lock-free queue
  (the dequeue-side CAS loop and its ABA exposure), so the first implementation
  is small and auditable.
- MPMC is a clean, additive follow-up (generalize the consumer side) once the
  GC-safety protocol below has been exercised in CI — it needs no surface change
  and no new decision.

A **Treiber-style lock-free stack** (`concurrent.Stack[T]`) is listed as an
*optional* extra in §6: it is the simplest possible CAS structure (one atomic
head, push/pop both single-CAS) and is useful as the proving ground for the
pointer protocol, but it is not load-bearing and can be dropped if we want the
absolute minimum.

## 4. GC-safety analysis (the load-bearing section)

This is the section the ticket exists for.
The claim is: **pointer-holding lock-free collections are safe under Zap's
moving collector, by leaning entirely on the GC for reclamation, provided the
collection stores pointers only in genuine reference slots.**
Here is why, property by property, tied to the collector *as built* (I#120).

### 4.1 The threat

A CAS on a word that holds a heap pointer races the collector in two ways:

1. **Reclamation race (use-after-free).** Classic lock-free structures can't free
   a node when it is unlinked, because another party may still hold a pointer
   into it — hence hazard pointers or epoch reclamation. If Zap freed nodes
   manually it would have exactly this problem.
2. **Relocation race (stale pointer).** With a *moving* collector, an object the
   CAS word points at can be relocated; a pointer captured before the move and
   CAS-installed after it would install a **stale** address.

### 4.2 What the collector actually does (I#120, as built)

From `design/0008-precise-gc.md` (§"Opportunistic evacuation", §"Stop-the-world",
and the "as built" notes), three properties are decisive:

- **Evacuation is stop-the-world.** The collector relocates objects **only**
  during a collection, and a collection runs **only** when every M has parked at
  a safepoint (the cooperative STW handshake; running Gs park because their M is
  parked at a poll, parked Gs are already at a scheduling-point safepoint).
  There is **no concurrent or parallel moving collector**: no mutator executes a
  single instruction — let alone a CAS — while an object is being moved.
- **The mutator observes a move load-at-use.** Under `gc-immix`, codegen reloads
  every GC-pointer value from its shadow-frame slot at each use, so after a
  safepoint that relocated an object, the mutator re-reads the collector-updated
  slot rather than a stale register.
  The collector's slot-based moving trace rewrites **every** reference slot it
  reaches — shadow chains, global roots, the `g_all` coroutine roots, precise
  struct fields, **and container interiors** — installing a forwarding pointer in
  the original and updating each slot as it is processed.
- **Reclamation is tracing.** An object is reclaimed only when it is unreachable
  from *all* roots, and at STW the collector has a globally consistent root set
  across every coroutine (`g_all[]` × shadow chains + globals).

### 4.3 Why leaning on the GC is correct (and hazard/epoch pointers are not needed)

- **Reclamation race — eliminated.** We never free a node manually; we drop the
  reference and let the collector reclaim it.
  If *any* party still holds the node — in the structure, or as a reloaded SSA
  value on a shadow frame — it is reachable at STW and is **not** collected.
  "Freed-while-referenced" is therefore impossible *by construction*.
  This is the ticket's preferred answer (design question 2: "lean on the GC"),
  and it is genuinely correct here, not just convenient.
- **Relocation race — eliminated by STW + load-at-use.** Because the move happens
  only while the mutator is parked, no CAS is ever in flight across a relocation.
  When the mutator resumes, its next use of any pointer reloads the
  collector-updated slot, and the pointer words *inside* the collection were
  themselves rewritten by the moving trace (they are either precise struct fields
  or precisely-tagged container interiors — the element-kind tags that already
  exist). So both the mutator's working copy and the structure's stored copy are
  consistent after the move.
- **ABA — downgraded from "must solve" to "mostly moot".** The classic ABA
  (address A is freed, reused, and reappears so a stale CAS succeeds wrongly) is
  driven by *memory reuse*. Under tracing GC a node's memory cannot be reused
  while any party still references it, which is exactly the window in which a
  stale CAS could observe it. The residual case — a node becomes fully
  unreachable, is collected, and a *fresh* node happens to be allocated at the
  same address before a stale CAS runs — cannot arise in the Treiber/MPSC
  patterns here, because a party that still holds the old pointer keeps the node
  reachable (so it is not collected, so its address is not reused). **This is why
  we do not need, and must not add, version-counter pointer-packing.**

### 4.4 The one thing the collection must guarantee: never defeat precise tracing

The safety above rests on the collector *seeing and being able to rewrite* every
pointer the collection holds. That gives one hard API/implementation rule:

> **Every pointer the collection stores lives in a genuine reference-typed slot**
> (an `Atomic[T]` with `T` a reference type, or a reference field of a node
> struct) **— never a pointer packed, tagged, or cast into an integer word.**

A pointer stuffed into an `Atomic[int64]` (the usual home for an ABA version
counter) is invisible to the collector: it is described as a scalar, so the
referent is **not traced** (use-after-collect) and, under the moving collector,
the integer is **not a slot the trace can forward** (stale pointer after a move).
This single rule is what keeps the "lean on the GC" scheme sound.
It is an easy rule to honor precisely because §4.3 removed the only reason a
lock-free algorithm reaches for packed pointers (ABA).

If a future structure genuinely needed packed/tagged pointers, it would require
**pinning** its nodes (the collector already supports pinning — anything marked
by a non-slot root, e.g. via `zap_gc_mark`, is never relocated) plus a custom
trace callback to keep the referents alive. That is explicitly **out of scope**;
the recommended structures do not need it.

### 4.5 Non-pointer payloads sidestep everything

A `Queue[int64]`, a `Queue[Handle]` (opaque integer newtype), a `Stack[int64]`,
and the `Counter` store **no pointers in their mutable slots** (the queue's
*nodes* are still heap objects, but a node of a non-pointer queue has a
pointer-free payload and only its `next` link is a reference — a link that is an
ordinary traced slot, updated by the moving trace like any other).
These instantiations therefore have **no novel GC interaction at all**, need no
new runtime guarantee, and are correct on every platform including
aarch64-Linux.
This is the ticket's design-question-1 escape hatch ("store only non-pointer
payloads"), and its cost is exactly the obvious one: the caller indirects through
an integer handle / index into a side table instead of holding the object
reference directly in the queue. For the counter and for id/handle fan-in that
cost is zero; for a queue of rich objects it is a real ergonomic tax, which is
why we also want the pointer variant (shown safe above).

### 4.6 Weak memory ordering, and the I#223 caveat

Two distinct ordering stories, which must not be conflated:

- **The collection's own ordering** is `Atomic[T]`'s, which is `seq_cst` for
  every op today (D-0066). Sequential consistency is the strongest model and is
  correct on both x86-TSO and the weaker aarch64 model with no further reasoning;
  the published Treiber/MPSC algorithms are proven under SC. A future relaxed/
  acq-rel `Atomic` surface would require re-auditing each algorithm's fences, so
  **the first cut pins itself to `seq_cst`** and does not opt into relaxed order.
- **The collector's own ordering** is a *separate* concern and is where the one
  real platform caveat lives. **I#223** is an open, intermittent **aarch64-Linux**
  failure in the moving collector's evacuation path (a suspected missing
  acquire/release around forwarding-pointer installs / to-space writes exposed by
  aarch64's weaker model; x86-TSO hides it). It is independent of `Atomic[T]` and
  independent of these collections — but any **pointer-holding** concurrent
  collection depends on the moving collector relocating pointer graphs correctly,
  so it **inherits** I#223 and is **not trustworthy on aarch64-Linux until I#223
  is fixed**.
  Non-pointer collections (§4.5) do not depend on evacuation of pointer graphs
  and are unaffected.
  This mirrors the D-0076 platform matrix, which already lists aarch64-Linux as
  *Experimental — known issue (I#223)*.

### 4.7 Summary of the resolution

| Variant | GC interaction | Reclamation | aarch64 today |
|---|---|---|---|
| `Counter` | none | n/a | fine |
| `Queue[int64]` / `Queue[Handle]`, `Stack[int64]` | node links only (ordinary traced slots) | GC | fine |
| `Queue[T]` / `Stack[T]` (reference payload) | payload slots traced + forwarded | GC (lean-on, §4.3) | **blocked on I#223** |

No pinning, no hazard pointers, no epochs, and no new write/read barrier are
required for any variant — the moving collector's existing STW + load-at-use +
precise-trace machinery is exactly the set of guarantees the "lean on the GC"
scheme needs.
The only new rule is §4.4 (no packed pointers).

## 5. Cost / complexity

Sizing per area (S / M / L / XL):

| Area | Size | Notes |
|---|---|---|
| **Surface / parser** | **S** | No new syntax. A new `concurrent` module in `crates/sema/src/modules/`; the types are generic structs riding the existing monomorphizer (the `Atomic[T]` route, D-0066). |
| **Runtime support** | **S–M** | The algorithms are pure Zap/ZIR over `Atomic[T]` intrinsics; **no new `zaprt.c`/`zapgc` code is required** for the recommended set, because §4 shows the existing GC machinery already provides every guarantee. A node is an ordinary heap struct. |
| **GC barrier / pinning work** | **none (S)** | Explicitly *zero* — the central finding of §4. No write barrier (FORK 3 is still "none in v1"), no pinning, no new descriptor kind. Contrast: a packed-pointer or manual-reclamation design would be **L–XL** (custom trace + pinning + epoch machinery) — which is the case *for* not doing it. |
| **Counter** | **S** | A thin wrapper over `Atomic[int64]`. Arguably could even be a documented idiom rather than a type; shipped as a type for discoverability. |
| **MPSC queue (non-pointer)** | **M** | The algorithm + comma-ok surface + the stress goldens below. |
| **MPSC queue (reference payload)** | **M** | Same algorithm; the only delta is the node descriptor (pointer vs. pointer-free), which monomorphization handles. Gated behind the I#223 caveat for aarch64. |
| **Testing** | **M–L** | See below — this is the real cost, not the algorithm. |
| **Concurrent map (if ever)** | **L–XL** | Deferred; see §6. |

### Testing lock-free correctness in CI

This is where the effort actually goes, and it has two independent layers:

1. **Deterministic-sum stress goldens**, exactly matching the pattern the repo
   already uses for the moving collector (`tests/golden/immix_evac_coro_stress`,
   `immix_spawn_contention_stress`, `scheduler_stress`, `fnval_gc_stress`): N
   producer coroutines each push a known multiset, one consumer drains, and the
   test asserts the **sum / count is the unique deterministic-correct value**
   (a dropped or duplicated element changes the sum).
   These run under both CI collector triggers and under the preemptive scheduler,
   so they exercise genuine interleavings.
   **They must obey the GC-goldens rule:** assert only the deterministic
   workload-level invariant (the sum/count), and **never an exact heap-byte
   count** — heap bytes are non-deterministic, which makes one CI trigger pass and
   the other fail. (This is the house rule the moving-collector goldens already
   follow, and it is doubly important here because evacuation timing is exactly
   what varies between triggers.)
2. **A loom-style exhaustive model check** of the enqueue/dequeue CAS protocol as
   a **Rust unit test** in the implementing crate, using the `loom` crate (or an
   equivalent bounded interleaving explorer) over a faithful Rust transcription of
   the algorithm. This is the only way to get real coverage of the rare
   interleavings a stress test hits only probabilistically; it is run on the
   Rust side (host, no LLVM) so it is cheap and deterministic, and it tests the
   *algorithm*, while the goldens test the *Zap lowering + GC integration*.
   An aarch64 stress golden (bounded, deterministic-sum, no exact-byte asserts) is
   the natural repro vehicle to tie to I#223.

## 6. Scope recommendation and phased plan

**Ship the minimum useful set first, defer the map.**

**Phase 1 (the minimum useful set).**
`concurrent.Counter` and `concurrent.Queue[T]` (MPSC, unbounded), with the
non-pointer instantiations (`int64`/handle) as the always-safe core and the
reference payload shipped behind the §4.6 aarch64/I#223 caveat.
Optionally `concurrent.Stack[T]` (Treiber) as the cheapest possible pointer
proving-ground.
All **Experimental** per D-0076.

**Phase 2 (generalize, once Phase 1 is exercised in CI).**
Generalize the queue consumer side to **MPMC** (additive; no surface change, no
new decision). This is the point at which the §4 protocol has been stress- and
loom-tested, so widening it is low-risk.

**Phase 3 (the map — only if demand is demonstrated, and probably not lock-free).**
Defer the concurrent map, and when it lands, recommend a **sharded `Mutex`+`dict`**
rather than a CAS map (design question 4).
Rationale: a CAS map cannot cheaply offer the Python-`dict` surface (ordered-ish
iteration, resize, arbitrary composite values) and its lock-free complexity buys
little over N independent mutex shards, which are simple, give the full `dict`
surface per shard, and scale with shard count.
A lock-free map would be an **L–XL** effort (incremental resize is the hard part,
and it re-opens every §4 question at a larger scale) for a payoff a sharded map
captures at a fraction of the risk.
If and when a concurrent map is scheduled, it gets its **own** design ticket.

Why not ship everything at once: the queue and counter validate the entire
GC-safety protocol (§4) and the CI testing story (§5) on the simplest possible
structures; proving the protocol once, in the small, is the prerequisite for
trusting it anywhere else.

## 7. Recommendation + proposed decision (D-0077, pending ratification)

**Recommendation (one line):** ship a lock-free `Counter` + MPSC `Queue` built on
`Atomic[T]`, leaning entirely on the tracing moving GC for reclamation (no
hazard/epoch pointers, no new GC barriers), with pointers stored only in genuine
reference slots; defer the map to a sharded-mutex design.

### Proposed decision

> **D-0077 (Proposed — pending maintainer ratification; NOT accepted).**
> Zap ships a small `concurrent` stdlib module (Experimental, per D-0076) of
> CAS-based collections built on `Atomic[T]` (D-0066): a `Counter` and an
> unbounded MPSC `Queue[T]`, optionally a Treiber `Stack[T]`.
> Memory reclamation is the tracing moving Immix collector (D-0008/I#120) — no
> hazard pointers, no epochs, no new write/read barrier, no pinning — which is
> sound because evacuation is stop-the-world, mutators observe moves load-at-use,
> and tracing reclaims only the unreachable (§4).
> The binding invariant is that **every stored pointer lives in a genuine
> reference slot** (no packed/tagged/integer-cast pointers).
> Non-pointer instantiations are correct on all platforms; reference-payload
> instantiations inherit the open aarch64 evacuation race **I#223** and are gated
> on its fix for that platform.
> No new language syntax; concurrency is expressed in the type name.

### The load-bearing calls I want the maintainer to rule on

1. **Reclamation strategy: lean-on-GC (recommended) vs. hazard/epoch pointers.**
   §4 argues lean-on-GC is correct under the moving collector and that
   hazard/epoch machinery is unnecessary (and would be L–XL). Confirm we commit
   to lean-on-GC and the §4.4 "no packed pointers" invariant, and do **not** build
   a reclamation subsystem.
2. **Payload policy: pointer-safe-under-moving-GC (recommended) vs.
   non-pointer-only.** §4 shows reference payloads are safe *given* the §4.4 rule,
   at the cost of inheriting I#223 on aarch64. The conservative alternative is to
   ship **non-pointer payloads only** (counter + integer/handle queue) for 1.0 and
   add reference payloads after I#223 lands. Which risk do you prefer to carry —
   a documented aarch64 caveat now, or an ergonomic tax now?
3. **Scope: queue-only-first + defer the map (recommended).** Confirm Phase 1 is
   `Counter` + MPSC `Queue` (+ optional `Stack`), MPMC is a later additive phase,
   and the concurrent **map is deferred** and will be sharded-mutex, not CAS, in
   its own ticket.

A secondary, non-blocking call: whether `Counter` is worth a type at all or
should be a documented `Atomic[int64]` idiom (recommended: ship the type for
discoverability).

## Compatibility

Additive only. A new import-gated `concurrent` module, no change to any existing
type, no new syntax, no prelude change. Tiered **Experimental** under D-0076, so
it carries no 1.0 stability promise and can evolve (e.g. MPSC → MPMC, add
relaxed-order fast paths) at MINOR versions.
Nothing here touches `SPEC.md`, `docs/GRAMMAR.ebnf`, or `docs/STABILITY.md` in
this ticket; the eventual implementation ticket would add the module to the
stdlib inventory and a one-line Experimental row to the stability tables.

## Implementation (sketch, after ack — NOT built here)

1. `crates/sema/src/modules/concurrent.rs`: register the module and the generic
   types `Counter`, `Queue[T]`, (`Stack[T]`), riding the I#154 monomorphizer.
2. The algorithms expressed in Zap/ZIR over `Atomic[T]` intrinsics (D-0066); node
   structs are ordinary heap structs with an `Atomic`-typed link field.
   No `zaprt.c` / `zapgc` change (the §4 finding).
3. Tests: deterministic-sum stress goldens (both collector triggers, preemptive
   scheduler; **no exact-heap-byte asserts**) + a `loom` model check of the CAS
   protocol as a Rust unit test; an aarch64 evac-stress golden tied to I#223.
4. Docs: add `concurrent` to the stdlib inventory as Experimental; record the
   ratified decision in `DECISIONS.md`.

Phase boundaries touched: **none mandatory** (no IR schema or runtime-ABI change
for the recommended set) — the central result of this design.

## Open issues

- The three §7 calls are the open decisions; everything else follows from them.
- MPMC generalization and the sharded-mutex map each get their own ticket when
  scheduled.
- A relaxed/acq-rel `Atomic` surface (a future extension of D-0066) would let
  these structures drop some `seq_cst` fences for throughput, at the cost of a
  per-algorithm fence audit; out of scope until that surface exists.
