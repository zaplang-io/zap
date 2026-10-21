# Proposal: `threading` — OS-thread compatibility library (migration escape hatch)

- **Author(s):** I#175 session
- **Last updated:** 2026-10-21
- **Status:** Accepted (design approved by orchestrator 2026-10-21; implementation
  gated on I#120 merge)
- **Discussion:** https://github.com/parveshgarg/zap/issues/175

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

A `threading` stdlib module whose surface mirrors Python's `threading`
(`Thread(target=...).start()/join()`, `Lock`, `RLock`, `Event`, `Condition`,
`Semaphore`) and whose `Thread`s run on **dedicated OS threads outside the M:N
scheduler pool**, so they exceed `MAXPROCS` and are never multiplexed.
It exists for one reason: to let a future Python→Zap transpiler translate
existing `threading` code by *syntax*, preserving OS-thread semantics, so a
production Python app can lift-and-shift to Zap without re-architecting its
concurrency into coroutines.
It is explicitly **not recommended** for new code — idiomatic Zap concurrency is
`spawn` + coroutines + channels.

The key design result: a `threading.Thread` is implemented as **a coroutine that
pins itself to a dedicated M via the I#172 `LockOSThread` substrate and runs to
completion**. It is therefore *not* a raw `pthread` living outside the runtime's
view — it is a first-class coroutine that already registers as a GC mutator,
honors stop-the-world and safepoints, and roots its pointers through the
per-coroutine shadow stack.  This dissolves the hard GC-correctness and
moving-collector hazards the ticket raised: there is no bare OS-thread stack or
register set hidden from the collector.

## Background

Zap's primary concurrency model is an M:N scheduler (M = OS thread, P =
processor bounded by `MAXPROCS`, G = coroutine), modeled on Go.
Coroutines are the idiomatic primitive; raw OS threads are not exposed.

Three recently-landed pieces make this library almost entirely a thin surface
over existing substrate:

- **I#172 `runtime.lock_os_thread()` / `unlock_os_thread()`** (merged, PR #183).
  Pins the calling coroutine to a *dedicated M* — its own OS thread that runs
  only that coroutine, bypassing work-stealing, excluded from the `g_ms[]`
  worker pool and from fairness preemption, with `p = NULL` (consumes no P).
  The dedicated worker (`dedicated_worker_main`, zaprt.c) calls
  `zap_mutator_register()` on entry and `zap_mutator_unregister()` on exit, polls
  GC safepoints, uses `zap_enter_blocking`/`zap_leave_blocking` when idle, and is
  joined at program exit via the `g_ded_live` barrier in `zap_rt_join_all`.
  The I#172 note states outright that this is "the substrate the later I#175
  threading-compat library reuses."
- **I#163 first-class function values** (merged, PR #181).
  A `fn(T)->R` is a single-pointer closure record `{code_ptr, caps...}`, a normal
  precisely-traced heap object.  This gives us a `target` value to store and
  invoke.
- **I#103 `sync`** (PR #143).
  `Mutex` (lock/unlock/trylock), `WaitGroup`, `Once`, built on the shared
  `gopark`/`goready` park/wake primitive and a `Waiter` queue.  The lock
  primitives `threading` needs are the same machinery.

## Proposal

### Surface (Python-faithful)

```zap
import threading

def worker(n: int) -> None:
    print("hi from", n)

def main() -> None:
    t = threading.Thread(target=lambda: worker(7))
    t.start()
    t.join()

    lock = threading.Lock()
    with lock:            # (see Open issues: `with` on a context manager)
        shared = shared + 1

    ev = threading.Event()
    ev.set(); ev.wait()
```

Planned surface, mapped 1:1 from Python where feasible:

| Python                         | Zap                                        |
|--------------------------------|--------------------------------------------|
| `Thread(target=f).start()`     | `threading.Thread(target: fn()->None)` `.start()` |
| `t.join()`                     | `t.join()`                                 |
| `t.is_alive()`                 | `t.is_alive()`                             |
| `threading.Lock()`             | `threading.Lock()` → `lock()/unlock()/acquire(blocking)->bool` |
| `threading.RLock()`            | `threading.RLock()` (reentrant, owner-tracked) |
| `threading.Event()`            | `set()/clear()/wait(timeout?)->bool/is_set()` |
| `threading.Condition(lock?)`   | `wait()/notify(n)/notify_all()` + lock pass-through |
| `threading.Semaphore(n)`       | `acquire(blocking)->bool / release()`      |
| `threading.current_thread()`   | handle of the running thread               |

### Thread model (the core mechanism)

`Thread.start()` spawns a **detached coroutine** running an internal trampoline:

```
_thread_entry(target, done_chan):
    runtime.lock_os_thread()      # pin to a fresh dedicated M (its own OS thread)
    try:
        call target()             # CallClosure on the fn-value
    finally:
        done_chan.close()         # wakes any joiner
        runtime.unlock_os_thread()
```

Because `lock_os_thread()` creates a dedicated M with `p = NULL` that is not in
`g_ms[]`, each `Thread` is a real OS thread **beyond the `MAXPROCS` cap** and is
never multiplexed onto a P — exactly Python `threading` semantics.
`join()` receives on `done_chan` (parking the joiner coroutine the normal way).

**Rooting the `target` closure (required).**  The fn-value record and its
captures must be a **live, scanned GC root for the whole thread lifetime** — it
is placed in a shadow-stack slot on the dedicated M's running coroutine, never
held only as a raw C local across a safepoint.  This is what lets the captured
heap pointers survive collection and receive I#120's load-at-use reloads.  The
hand-off from `start()` to the new coroutine must land `target` in that slot
before the first safepoint on the new thread (same discipline as the `spawn` and
channel paths).

### Answers to the four hard questions

**1. GC safety (raw-thread-root hazard — dissolved).**
A `threading.Thread` is a coroutine pinned to a dedicated M, not a bare
`pthread`.  `dedicated_worker_main` already calls `zap_mutator_register()` (so
the thread is in `zap_mutators`, scanned at STW) and `zap_mutator_unregister()`
on exit (under `zap_enter_blocking`, so a collection in flight never waits on the
exiting thread).  Its Zap heap roots live on the coroutine's **green stack**,
enumerated through the per-coroutine **shadow head** — identical to every other
coroutine.  The dedicated M's *native* C stack (the trampoline frame,
`swapcontext` scratch) holds **no** Zap roots under precise Immix; under
gc-boehm it is scanned conservatively like any registered thread.  So there is
no OS-thread stack or register set invisible to the collector.  It polls
`zap_safepoint()` from compiled code and parks GC-safe when blocked — STW
completes.

*Explicit requirement (orchestrator note):* a dedicated-M thread blocked in a raw
C call (a `join()`, a blocking syscall, the trampoline waiting to be handed its
coroutine) MUST be at a GC-safe point — i.e. inside `zap_enter_blocking()` /
`zap_leave_blocking()`, marking itself not-executing-Zap — so an STW never stalls
on it and its shadow roots stay stable for the duration (same discipline the
netpoller and syscall paths use).  `dedicated_worker_main` already brackets its
idle wait in `zap_enter_blocking`/`zap_leave_blocking`; every new blocking entry
point `threading` adds (`join`, the lock/condition waits) must do likewise before
it blocks in C.

**2. Moving collector / I#120 evacuation (no extra pinning needed for Zap
objects).**
Because the thread's roots are ordinary shadow-stack slots, I#120's planned
*load-at-use* codegen (re-reading a GC pointer from its collector-updated slot
after every safepoint) applies to this thread's compiled code **verbatim**.
An object reachable only from a `threading.Thread` is reached through a scanned,
updatable slot, so the evacuator relocates it and the thread observes the new
address on its next use — no special-case pinning.
The only pinned objects remain the ones I#120 already pins for everyone (the `G`,
its green stack, `Scope`, `ZapChan`, their raw buffers, finalizer-registered
objects), because those are held by raw C pointers in the scheduler — this is
unchanged by `threading` and `threading` introduces no new raw-C-held Zap
pointers.  gc-boehm stays correct (conservative, non-moving).
*Caveat to verify in implementation:* the fn-value `target` and any argument
objects handed across the `spawn` boundary must land in a shadow slot on the new
coroutine's frame before the first safepoint, not be held only in a runtime C
temporary across a safepoint — this is the same discipline the channel/`spawn`
paths already follow.

**3. Scheduler relationship (deliberately disjoint from `MAXPROCS`).**
Dedicated Ms are not worker Ms: `p = NULL`, absent from `g_ms[]`, skipped by the
fairness monitor (SIGURG is blocked on the dedicated thread).  They do **not**
consume a P and are **not** counted against `MAXPROCS`; N `threading.Thread`s add
N OS threads on top of the `MAXPROCS` worker pool.  They are not cooperatively
preempted (the OS time-slices them, which is what Python threads get anyway), but
they *do* poll GC safepoints, so STW is unaffected.
Channels and `sync`/`threading` locks work transparently from a `Thread`: a
blocking op parks the pinned coroutine and `sched_enqueue` routes it back to its
own dedicated M on wake (`locked_m` path), so it always resumes on the same OS
thread.

**4. Surface — Python-shaped, marked not-recommended.**
Class-shaped `Thread`/`Lock`/`Event`/... with methods, so a transpiler maps
Python 1:1.  Module doc-comment and the design doc both state loudly:
migration-only, not idiomatic; new code uses `spawn` + channels.

### Sync primitives

`Lock` reuses `sync.Mutex`.  `RLock`, `Event`, `Condition`, `Semaphore` are new
but are straightforward over the same `gopark`/`goready` + `Waiter` queue that
`sync` and channels already use (handles are opaque heap objects with
`ZAP_TD_ATOMIC` descriptors — nothing to trace — mirroring `sync`).  `RLock`
tracks an owner thread id + recursion count; `Condition` releases/re-acquires its
lock around the park, as in Python.

## Rationale

- **Reuse the pinned-coroutine substrate rather than spawning bare pthreads.**
  A bare `pthread` that allocates Zap objects would be exactly the GC hazard the
  ticket warns about: unknown to the collector, holding roots in registers/stack
  the marker never scans, dangling under evacuation.  Routing every `Thread`
  through `lock_os_thread()` means the thread *is* a coroutine for GC and
  scheduling purposes while *being* a dedicated OS thread for semantics.  We get
  OS-thread behavior and precise-GC correctness simultaneously, with almost no
  new runtime code.
- **Closures carry the args.**  Python's `Thread(target=f, args=(a,b))` maps to
  `Thread(target: () => f(a, b))` — the transpiler emits a capturing lambda, so
  the library's `target` can be a simple `fn()->None` and we avoid variadic
  arg-tuple plumbing and tuple-splat in the ABI.
- **Not-recommended by construction.**  OS threads cost ~MB stacks and real
  kernel scheduling; the module's purpose is faithful migration, and the docs say
  so, steering greenfield users to coroutines.

Alternatives considered: (a) a bare-pthread `threading` with a bespoke
root-registration shim — rejected, re-implements I#172 and fights the moving
collector; (b) lowering `Thread` in the compiler like `spawn` — rejected, more
surface-area change than a stdlib module and worse transpiler mapping.

## Compatibility

Additive: a new stdlib module on the I#121 registration seam (one new file in
each of `sema/src/modules/`, `ir/src/lower/modules/`, `codegen/src/modules/`,
plus a `threading` banner section appended at the end of zaprt.c).
No grammar change (ordinary method calls), so no EBNF edit beyond documenting the
module.  No change to existing coroutine/`spawn` semantics.  gc-boehm and
gc-immix both supported.

## Implementation

Phased, runtime-track; sequence **after I#120 merges** (it edits zaprt.c /
zapgc.inc.c + codegen hot path) and rebase onto main then, to avoid clobbering
its sections.

1. Runtime (zaprt.c, new `threading` banner section, append-only): the
   `_thread_entry` trampoline that `lock_os_thread()`s, invokes the fn-value via
   the approved C trampoline, closes the join channel, `unlock_os_thread()`s;
   the `daemon=` opt-out of the `g_ded_live` barrier; plus `RLock`, `Event`,
   `Condition`, `Semaphore` over the shared `Waiter`/`gopark` machinery, with the
   I#80 deadline path wired for the v1 timeout variants.  The one genuinely new
   runtime capability is **invoking a Zap closure from runtime C** (load code-ptr
   @0, indirect-call with the record as arg 0) — see Resolved decisions #1.
2. Surface wiring on the I#121 seam (sema/ir/codegen `modules/threading.rs` + one
   registry slice entry each).
3. Goldens (deterministic output, green under BOTH collectors, **no heap-byte
   asserts**): a `Thread` fan-out summing an order-independent total over a
   channel; a `Lock`/`RLock` contention test; an `Event`/`Condition`
   producer-consumer; a GC-stress variant that churns heap inside thread bodies
   to exercise STW with dedicated Ms live (model
   `immix_per_p_alloc_stress.zap`).

Phase boundaries touched: runtime ABI (new `threading` entry points, and the
runtime→Zap closure-call convention).  No IR schema change expected.

## Resolved decisions (orchestrator, 2026-10-21)

1. **Calling a Zap closure from runtime C — APPROVED (option a).**  Add a tiny C
   trampoline that treats the fn-value record as `ret (*)(void *self)` and calls
   `code_ptr(record)`, matching the I#163 calling convention.  Two musts: (a) the
   target closure record + captures are a live, scanned shadow-stack root for the
   whole thread lifetime (see "Rooting the `target` closure"); (b) fix and
   document the C ABI for args/return in the trampoline once.
2. **`daemon=` flag — SHIP IN v1.**  Faithful migration requires it; real code
   uses daemon threads.  Implement as an opt-*out* of the `g_ded_live`
   join-at-exit barrier: non-daemon (default) keeps current blocking semantics
   (`zap_rt_join_all` waits for it), daemon excludes the thread from
   `zap_rt_join_all` — matching CPython exactly.
3. **Timeouts — SHIP IN v1, reuse the I#80 deadline path.**  `join(timeout)`,
   `Event.wait(timeout)`, `Lock.acquire(timeout)` are common in real code; return
   Python-shaped results (`acquire` → bool, `wait` → bool).
4. **`RLock`/`Event`/`Condition`/`Semaphore` — implement in the runtime ONCE,
   expose via BOTH `sync` and `threading`.**  DRY; these are the same primitives
   the `with lock:` context-manager work (I#185) will build on.  Names stay
   Python-shaped on the `threading` surface.

## Open issues

1. **`with lock:` ergonomics.**  Python idiom is `with lock:`; Zap `with` today
   only does `expect_raises` (general context managers deferred, D-0027, tracked
   by I#185).  Until then `threading.Lock` offers explicit `lock()/unlock()`; the
   transpiler can emit try/finally.  Flag for the transpiler epic (I#157).
2. **`spawn`/structured-concurrency composition.**  `Thread`s are explicitly
   joined by the user, so the trampoline coroutine is *detached* from the
   enclosing `scope` (D-0044 `spawn detached`) and joined through its own
   `done_chan`, not the scope — confirm during implementation.
3. **Thread-locals.**  `threading.local()` is common in migrated code; it needs a
   per-dedicated-M TLS slot.  Fast-follow, not v1.
