# Proposal: Concurrency

- **Author(s):** Zap team
- **Last updated:** 2026-10-18
- **Status:** Surface locked (D-0044, D-0045); runtime in progress (#47)
- **Discussion:** github.com/parveshgarg/zap#14

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line. This keeps review comments precise and diffs
> small.

## Abstract

Zap's concurrency is CSP-style: lightweight coroutines launched with `spawn`
(D-0013), communicating over typed channels (D-0031), running on an M:N
scheduler (D-0008).
A standard sync toolbox — `Mutex`, `RWMutex`, `WaitGroup`, atomics, `Once`
(D-0032) — backs the "share memory by communicating" idiom.
Coroutine lifetime is managed by **structured concurrency** (D-0033): a `scope:`
block owns the coroutines spawned inside it, awaits them at the dedent, and
cancels them if the block exits early or a sibling fails, so no coroutine
leaks.
The memory model is **DRF-SC** (D-0034): data-race-free programs are
sequentially consistent, and channel/sync operations are the synchronization
edges that establish happens-before.
This document fixes the surface syntax and specifies the runtime: the scheduler,
green-thread stack management, and the two interactions that the Boehm GC
(D-0009) and the table-based exception unwinder (D-0007, RUNTIME-ABI #22) force
on the design.

## Background

The locked decisions this document specifies:

- **D-0008** — CSP coroutines + channels on an M:N scheduler (the Go model);
  Zap ships its own scheduler and green-thread runtime.
- **D-0013** — `spawn` launches a coroutine; the operand must be a call;
  fire-and-forget (a statement, no handle).
- **D-0031** — `channel[T]()` is unbuffered (rendezvous); `channel[T](cap)` is
  buffered; plus `close`, comma-ok receive, and `select`.
- **D-0032** — sync primitives `Mutex`, `RWMutex`, `WaitGroup`, atomics, `Once`;
  `WaitGroup` is the primary way to await fire-and-forget `spawn`s.
- **D-0033** — cancellation via structured concurrency (scoped children,
  Swift/Kotlin/trio-style), chosen over Go's `context`; scope syntax was TBD.
- **D-0034** — DRF-SC memory model; data races are bugs, ideally caught by a
  race detector later.

The runtime context this design must fit:

- **GC (D-0009):** conservative **Boehm** GC in v1; the collector scans the
  stack, registers, and globals conservatively, so v1 needs no stack maps or
  safepoints (RUNTIME-ABI).
  A precise Immix-style GC is a later drop-in.
- **Exceptions (D-0007, RUNTIME-ABI):** zero-cost table-based Itanium unwinding
  with a Zap personality function; `raise`/faults call `zap_panic`; `try`
  installs landing pads; `finally`/`with` run as cleanup landing pads; an
  uncaught exception unwinds to the top frame of its coroutine (SPEC §10).
- **Codegen (D-0042):** LLVM via `inkwell`; the runtime is `libzaprt`, linked
  into every program.
- **Roadmap:** the concurrency track is Phase 3, and it depends on the runtime
  ABI and Boehm GC being frozen in Phase 2 (ROADMAP).

The SPEC §12 sketch (`spawn worker(ch)`, `ch <- v`, `<-ch`, `select:`) is the
starting point; this document makes it implementation-level and settles the open
`scope:` syntax left by D-0033.

## Proposal

### Coroutines: `spawn`

`spawn` launches a coroutine running a function call and returns immediately
(D-0013).
The operand must be a call expression; its arguments are evaluated **eagerly in
the spawning coroutine**, before the child starts, so argument evaluation cannot
race with the parent continuing.

```python
spawn worker(ch)             # launch; parent continues at once
spawn server.handle(conn)    # a method call is a call
spawn (lambda: log(x))()     # a lambda, immediately applied
```

`spawn` is a statement and yields no handle (fire-and-forget).
Coordination is explicit: results flow back over channels, and completion is
awaited with a `WaitGroup` (unstructured) or a `scope:` block (structured).
Every coroutine is **owned** by exactly one scope (see Structured concurrency);
a *bare* `spawn` with no enclosing `scope:` is owned by the program's implicit
**root scope**, which the runtime joins at program exit, so an ordinary
coroutine is never leaked.

**Detached coroutines (D-0044).** `spawn detached <call>` opts a coroutine out
of **all** scope ownership — it is never awaited, and the program does **not**
block on it at exit; when `main` returns, a still-running detached coroutine is
terminated (Go goroutine semantics). This is the escape hatch for true daemons —
loggers, metrics flushers, background pollers — that must outlive the block that
started them. `detached` is a contextual modifier valid only immediately after
`spawn`; it reserves no hard keyword.

```python
spawn detached metrics_flusher(registry)   # runs until program exit; never joined
```

Use a plain `spawn` (structured or root-scoped) by default; reach for `detached`
only for background work whose lifetime is deliberately the whole program.

### Channels

A channel is a typed, first-class reference value (`channel[T]`), created with a
constructor (D-0031):

```python
auto ch  = channel[int64]()       # unbuffered — rendezvous
auto bch = channel[int64](100)    # buffered, capacity 100
```

- **Unbuffered (`channel[T]()`):** a send blocks until a receiver is ready, and
  a receive blocks until a sender is ready; the two rendezvous and the value is
  handed over directly.
  This is the default — use it unless you have a reason not to.
- **Buffered (`channel[T](cap)`):** a send blocks only when the buffer is full;
  a receive blocks only when it is empty.
  Use a buffer only to decouple producer and consumer throughput.

Operations:

```python
ch <- 42                 # send
auto v = <-ch            # receive (blocks for a value)
auto v, ok = <-ch        # comma-ok receive: ok is False iff drained & closed
close(ch)                # close: no more sends
```

`close(ch)` marks the channel closed.
After close: a send raises `ClosedChannelError`; a receive drains any buffered
values, then returns the zero/`None` value of `T` with `ok == False`.
Closing is optional — only needed to broadcast "no more values" to receivers;
closing twice, or sending on a closed channel, is a programming error that
raises.
Only an owner that knows no further sends will happen should close; by
convention the sole sender closes.

Iterating a channel receives until it is closed and drained:

```python
for job in jobs:         # ends when jobs is closed and empty
    process(job)
```

`for v in ch:` is sugar for a comma-ok receive loop that stops when `ok` is
`False`.

### `select`

`select` waits on several channel operations and proceeds with the first that
becomes ready; if several are ready it chooses one **uniformly at random**
(fairness, like Go):

```python
select:
    case v = <-ch:           # a receive case; binds v (comma-ok form allowed)
        handle(v)
    case bch <- 1:           # a send case
        sent()
    case v, ok = <-done:
        if not ok:
            return           # done was closed
    default:                 # optional — makes the select non-blocking
        idle()
```

Semantics:

- Each `case` is a single channel send or receive.
- With no `default`, `select` blocks until one case is ready.
- With a `default`, if no case is ready the `default` runs immediately (a
  non-blocking poll).
- A closed channel counts as *ready* for receive (yields the comma-ok
  `ok == False`), so `select` is how a coroutine watches a cancellation/`done`
  channel while doing other work.

A common timeout idiom composes `select` with a timer channel:

```python
select:
    case v = <-ch:
        use(v)
    case <-after(5.seconds):   # after() returns a channel that fires once
        raise TimeoutError()
```

### Sync primitives (D-0032)

Channels are the idiom; the classic toolbox covers shared-state and
coordination cases.
These are **standard-library types** (in a `sync` module), not keywords.

```python
import sync

auto mu = sync.Mutex()
with mu.lock():              # context manager (D-0027) — released on block exit,
    counter += 1            # even on exception

auto rw = sync.RWMutex()
with rw.rlock():  v = cache[k]     # shared read
with rw.wlock():  cache[k] = v     # exclusive write

auto once = sync.Once()
once.do(init)                # init runs exactly once across all coroutines
```

`WaitGroup` is called out by D-0032 as the **primary way to await
fire-and-forget `spawn`s** outside a `scope:` block:

```python
auto wg = sync.WaitGroup()
auto results = channel[Result](n)
for i in range(n):
    wg.add(1)
    spawn do_work(i, results, wg)   # worker calls wg.done() when finished
wg.wait()                           # block until all n have called done()
close(results)
```

Inside the worker, `with wg.tracked():` (a context manager wrapping
`add(1)`/`done()`) is the exception-safe form, so a raising worker still
decrements the group.
Results come back over `results`, per the D-0032 pattern.

**Atomics** provide lock-free single-word operations with explicit ordering:

```python
auto n = sync.Atomic[int64](0)
n.add(1)                     # seq-cst by default
auto cur = n.load(order=sync.Acquire)
n.store(cur + 1, order=sync.Release)
auto swapped = n.compare_exchange(expected, desired)
```

The default ordering is **sequentially consistent**; `Acquire`, `Release`,
`AcqRel`, and `Relaxed` are available for experts and map to the LLVM atomic
orderings.

Prefer `Mutex`/channels; reach for `Atomic` only for counters, flags, and
lock-free structures.
Mutexes are **not reentrant** (re-locking deadlocks — a bug), and they are not
tied to the OS thread: a coroutine that blocks on a lock yields its M to another
coroutine (see Scheduler).

### Structured concurrency: the `scope:` block (resolves D-0033)

A `scope:` block is a lexical region that **owns** every coroutine spawned
inside it.
Control does not leave the block until all owned coroutines have finished; this
is the join point.

```python
scope:
    spawn producer(ch)
    spawn consumer(ch)
    # ... parent may do work here too ...
# <-- dedent: block waits here until producer and consumer both return
```

Rules:

1. **Ownership by lexical nesting.**
   A `spawn` is owned by the nearest enclosing `scope:`.
   Coroutines may spawn their own children in their own nested `scope:` blocks,
   forming a tree.
2. **Join on normal exit.**
   Reaching the dedent blocks the parent until all owned coroutines complete.
3. **Cancel on failure.**
   If any owned coroutine raises an uncaught exception, the scope **cancels all
   its siblings** and, once they have unwound, re-raises at the dedent (so
   failures are not silently dropped — the opposite of a bare fire-and-forget
   `spawn`). If **several** owned coroutines fail concurrently, the scope raises
   an aggregate **`ExceptionGroup`** carrying all of them (D-0045), not just the
   first; a single failure propagates as that one exception.
4. **Cancel on early exit.**
   If the parent leaves the block by `return`, `break`, or an exception of its
   own, the scope cancels and awaits its children first (like a `with` cleanup).
5. **No leaks, automatic propagation.**
   Because the scope awaits its children, a coroutine cannot outlive the block
   that created it, and cancellation flows down the tree without a `context`
   object threaded through every call (D-0033, chosen over Go).

Options on the scope configure deadlines and failure policy:

```python
scope(timeout=30.seconds):           # cancels all children at the deadline
    spawn fetch(a)
    spawn fetch(b)

scope(on_error=OnError.Wait):        # default is OnError.Cancel: a failure waits
    spawn best_effort(x)             # for siblings instead of cancelling them
    spawn best_effort(y)
```

`on_error` takes an **`OnError` enum** (`Cancel` | `Wait`), not a string, so a
misspelled policy is a compile-time error, not a silent runtime surprise (D-0044,
consistent with the interface/decorator typo-catching ethos).

Returning values is still channel-based (D-0032); a thin `Future[T]` helper over
a one-shot channel is a convenience the stdlib may provide, but the scope itself
traffics in coroutines, not values.

**Cancellation is cooperative.**
Cancelling a coroutine arms it; the cancel takes effect at the next
**cancellation point**, where the runtime raises a `Cancelled` exception in that
coroutine:

- any channel send/receive and any `select`;
- `sleep`, timer, and I/O waits;
- entering or leaving a nested `scope:`;
- an explicit `checkpoint()` call in long CPU-bound loops.

`Cancelled` unwinds through the normal exception machinery, so `finally` and
`with` cleanups run (D-0027) and resources are released.
`Cancelled` derives from a `BaseException` root **outside** the ordinary
`Exception` hierarchy (like Python's `KeyboardInterrupt`), so a broad
`except Exception:` does not accidentally swallow a cancel; code that catches it
for cleanup must re-raise.

### Memory model: DRF-SC (D-0034)

A program whose conflicting accesses are all ordered by synchronization is
**data-race-free**, and every data-race-free Zap program behaves as some
**sequentially consistent** interleaving of its coroutines.
A *data race* — two accesses to the same non-atomic location, at least one a
write, not ordered by happens-before — is a **bug**; v1 gives it no defined
meaning, and a race-detector tool (open issue) is the intended way to find them.

Synchronization establishes **happens-before** edges:

- `spawn f(...)` (and argument evaluation) happens-before the first statement of
  `f`.
- A send on a channel happens-before the completion of the receive that takes
  that value.
- `close(ch)` happens-before a receive that observes the channel closed.
- A `Mutex`/`RWMutex` unlock happens-before the next lock that acquires it; an
  `RWMutex` write-unlock happens-before subsequent read-locks.
- `wg.done()` calls happen-before the `wg.wait()` that they release.
- The completion of the function passed to `Once.do` happens-before the return
  of every `do` call on that `Once`.
- Every owned coroutine's completion happens-before the statement **after** its
  `scope:` block (the join is a full barrier).
- Atomic operations synchronize per their ordering argument (default seq-cst).

Implementation: channel and lock operations compile to the corresponding LLVM
atomic operations/fences (cheap on x86-64, explicit barriers on arm64); the
seq-cst default keeps the model simple, and relaxed orderings are opt-in on
`Atomic` only.

## Rationale

- **`scope:` as a block, not `with scope() as s:`** — spawning binds to the
  nearest `scope:` lexically, so the common case needs no nursery object and no
  `s.spawn(...)`; it reads like Python block structure and keeps `spawn` the one
  launch keyword (D-0013).
  A context-manager form was considered (trio's `nursery`), but it forces an
  explicit handle and a second spelling of `spawn`; the block form is lighter
  and still desugars onto `with`-style cleanup underneath.
- **Structured by default, `WaitGroup` for the rest** — D-0033 picks scoped
  lifetimes over Go's `context` precisely to prevent leaks and make cancellation
  automatic; `WaitGroup` (D-0032) remains for the deliberately unstructured
  fan-out where the root scope's program-exit join is the only backstop.
- **Cancellation as an exception** — reusing the D-0007 unwinder means `finally`
  and `with` already do the right thing on cancel, with zero new cleanup
  machinery; putting `Cancelled` outside `Exception` avoids the classic
  "swallowed cancel" bug.
- **Eager argument evaluation for `spawn`** — evaluating arguments in the parent
  removes a whole class of races (the child reading parent state that changes
  after the `spawn`) and matches intuition.
- **Unbuffered default, seq-cst default** — both follow D-0031/D-0034: the safe,
  predictable choice is the default; buffering and relaxed atomics are explicit
  opt-ins.
- **Go-style `select` fairness** — uniform random choice avoids starvation and
  matches the mental model most programmers bring from Go.

## Compatibility

This is new surface area; nothing existing changes behavior.

- **Keywords.** `spawn`, `channel`, and `select` are already reserved (SPEC
  §16).
  This proposal adds **`scope`** as a reserved word (used only as a block
  introducer); the coordinating session should add it to the keyword list.
  `close`, `after`, `range`, and `checkpoint` are ordinary built-in functions,
  not keywords; `Mutex`/`RWMutex`/`WaitGroup`/`Once`/`Atomic`/`Future` live in a
  `sync` module and reserve no words.
- **Operators.** `<-` (send/receive) is already in the SPEC grammar (§8); no new
  operator tokens.
- **No breaking changes.** Programs that never use concurrency are unaffected,
  and `spawn` without a `scope:` keeps the fire-and-forget behavior D-0013
  describes (now with a program-level root-scope join at exit).
- **Runtime ABI.** The scheduler, channel, stack, and cancellation entry points
  are **additive** to `libzaprt` (RUNTIME-ABI "additive growth is fine"); no
  change to object layout or the calling convention.

## Implementation

### M:N scheduler

The runtime multiplexes many coroutines (**G**) over a small pool of OS threads
(**M**), each thread holding a processor context (**P**) with a local run queue
— the Go M:P:G structure.

- **Run queues & work stealing.** Each P has a LIFO/FIFO local run queue; idle
  Ps steal from others and from a global queue, giving load balance without a
  central lock on the hot path.
- **Count of M.** Defaults to the number of hardware threads, overridable by an
  env var / runtime call (the `GOMAXPROCS` analogue); exact name is an open
  issue.
- **Scheduling points.** A coroutine yields its M at channel ops, `select`,
  locks, `sleep`/timers, I/O waits, nested `scope:` boundaries, and
  `checkpoint()`.
  v1 is **cooperative** at these points; preemption of long CPU loops without a
  `checkpoint()` is deferred (open issue).
- **Blocking syscalls.** A coroutine entering a blocking syscall hands its P to
  another M so sibling coroutines keep running (P-handoff, still a follow-up);
  non-blocking I/O goes through the **netpoller** (ticket #80, built), which
  parks the coroutine and re-queues it on readiness — see "Netpoller" below.

### Green-thread stacks (and the Boehm interaction)

This is the load-bearing runtime decision, and the **conservative Boehm GC
(D-0009) constrains it**.

- **Stacks are GC-heap objects.**
  Each coroutine gets its own contiguous stack allocated with `GC_malloc` (a
  non-atomic, pointer-bearing allocation), together with a saved-context block
  (callee-saved registers + SP + IP) also GC-allocated.
  Because the stack region is an ordinary traced heap object, Boehm scans it
  conservatively and so **retains everything reachable from a *suspended*
  coroutine's stack**, with no new GC machinery.
  This is the key trick: a parked coroutine is invisible to Boehm *as a thread*,
  but its stack stays reachable *as heap*.
- **Running coroutines.**
  The M OS threads are registered with Boehm (via its pthread wrappers), so at a
  collection the *running* coroutine's live registers and SP are scanned as part
  of that thread's machine context; the stack it is running on is the same
  GC-malloc'd object, scanned either way.
- **No stack copying in v1.**
  Conservative GC cannot tell a pointer from an integer, so it **cannot relocate
  stacks** (it would have to rewrite interior pointers it cannot identify).
  Therefore v1 does **not** use movable/segmented growable stacks: each
  coroutine gets a fixed, reasonably large stack (e.g. 64–256 KiB) with a
  **guard page**; overflow traps and raises `StackOverflowError` rather than
  growing.
  Cheap, Go-like growable stacks are deferred to the precise-GC era (open
  issue), when stack maps let the runtime relocate safely.
- **Reuse.** Finished coroutines' stacks return to a size-classed free list for
  reuse, bounding allocation churn.

### Interaction with the GC — summary

Under the default **Boehm** build:

- No GC safepoints and no stack maps are needed for coroutines, because
  suspended stacks are traced as heap and running stacks are traced as thread
  context — exactly the property RUNTIME-ABI relies on.
- Stop-the-world: Boehm suspends the **M** OS threads (which it knows about);
  coroutines not currently on an M are quiescent data.

Under the **precise Immix** build (ticket #48, D-0009, design/0008-precise-gc.md),
this is reimplemented precisely — the Boehm-specific tricks above are replaced,
not relied on:

- **Parked-coroutine roots.** Each coroutine carries a shadow-stack head saved
  in its `G`; `switch_to_g` saves/restores it across the `swapcontext`, so a
  *suspended* coroutine's live pointers are found by walking its saved shadow
  chain — a linked-list walk, with no stack scanning and no dependence on
  retaining the green stack as traced heap. `g_all[]` still enumerates every
  live `G` (and keeps the `G`, its green stack and its owning scope alive).
- **Running-coroutine roots.** The running coroutine's live head is in the
  per-thread `zap_shadow_head`; it is flushed into the `G` when the M parks at a
  safepoint, so the collector reads every coroutine's chain uniformly.
- **Stop-the-world.** Cooperative, not pthread suspension: codegen emits a
  `zap_safepoint()` poll at the top of every block; an M running Zap parks there
  when a collection is due, and an M idle in the scheduler loop, or any thread in
  a blocking channel/join wait, marks itself GC-safe (roots already flushed).
  The collector proceeds once no mutator is still executing Zap.
- Movable/growable stacks remain a v2 follow-up (v1 Immix is non-moving); the
  entry points and the fixed-stack decision above are unchanged.

### Interaction with exception unwinding

- **Per-coroutine unwinding.**
  Each coroutine owns its stack, and DWARF CFI-based unwinding (RUNTIME-ABI #22)
  walks frames **within that stack only**.
  The base frame installed by the `spawn` trampoline is the top of the
  coroutine: unwinding stops there.
- **Uncaught exceptions.**
  At the trampoline the runtime catches whatever reached the top.
  Under an owning `scope:` it stores the exception, cancels siblings, and the
  scope re-raises at its dedent (Structured concurrency, rule 3).
  Under the root scope (bare `spawn`) it aborts the program with a stack trace
  (SPEC §10).
- **Cancellation reuses the unwinder.**
  A cancel injects a `Cancelled` exception at a cancellation point, which unwinds
  through the ordinary landing pads, running `finally`/`with` cleanups (D-0027).
- **Stack switches are safe.**
  A context switch saves/restores callee-saved regs, SP, and IP; because the
  unwinder is driven by CFI per frame and never crosses the trampoline base, a
  coroutine suspended mid-unwind resumes its unwind correctly when rescheduled.
- **GC during unwind.**
  The in-flight exception object is a GC heap object (RUNTIME-ABI); the unwinding
  stack is the live GC-malloc'd stack, so conservative scanning keeps both the
  exception and its roots alive.

### Netpoller (as built — ticket #80)

Non-blocking I/O readiness, so a coroutine that would block on a file descriptor
**parks its G and frees its M** instead of blocking an OS thread, and is
re-queued when the fd is ready.

- **Reuses the one park/wake primitive.** The scheduler has a single park
  primitive (`gopark` / `goready`): a goroutine caller parks its G and frees its
  M; a native (`main`) caller blocks on its waiter's condvar. Channels, scope
  joins, and the netpoller all block on it — no parallel mechanism. A parked G
  is kept alive exactly as a channel-parked G is (the statically-rooted `g_all[]`
  registry; under precise Immix, traced via its saved shadow chain).
- **A dedicated poller OS thread** waits on the readiness mechanism and, on an
  event, hands the outcome to the parked waiter via `goready` (enqueue the G, or
  signal the native waiter). It allocates no GC memory and touches only
  non-traced scheduler fields under their locks, so it is safe without being a
  registered mutator; under Boehm it is started with `GC_pthread_create` so the
  collector still knows the thread. A `PollDesc` is plain (non-GC) infra memory.
- **Backend = epoll** (Linux / the dev container), behind a thin internal
  `np_backend_*` seam so a **kqueue** backend (macOS/BSD) can slot in later — a
  deliberate portability follow-up, not built in v1.
- **Edge-triggered** (Go-style): an fd is registered once for read+write
  interest; a readiness edge that arrives before any G parks is latched per
  direction so the next wait returns immediately. Callers drain to `EAGAIN`.
- **Deadlines** are absolute `CLOCK_MONOTONIC` nanoseconds (0 = none); the poller
  wakes an expired waiter with a timeout result. v1 scans the (small) live-desc
  registry for the nearest deadline; a timer heap is a follow-up.
- **Internal C ABI** (`zap_netpoll_open` / `_wait` / `_wake` / `_close`, see
  RUNTIME-ABI). There is no language-level I/O surface yet: the signal self-pipe
  (#77) and a socket/http layer (#79) are the first callers, and `zap_netpoll_wake`
  is the hook they (and a future cancellation-of-a-parked-G path) build on.
  Cancelling a coroutine already parked on an fd is now implementable via that
  hook but is not wired to `scope:` cancellation in this increment.

### Signals (as built — ticket #77, D-0058)

First-class Unix signals, built on the scheduler + netpoller. The capability
ships in every binary and wires up lazily; the mechanism:

- **Dedicated signal thread + self-pipe + netpoller.** The handled async signals
  are masked on the main thread (so scheduler workers, created later, inherit the
  block) and UNBLOCKED only on a dedicated signal thread, so an async-signal-safe
  handler — which only `write()`s the signal number to a **self-pipe registered
  with the netpoller** — runs there rather than on a worker mid context-switch.
  The signal thread PARKS on the netpoller (`zap_netpoll_wait`, the native-waiter
  path) and, on readiness, drains the pipe and **dispatches in normal context**.
  (D-0058 describes the dispatcher as a separate goroutine; collapsing it into the
  signal thread avoids forcing the M:N worker pool onto every binary — default
  handlers are on by default — and fan-out is a non-blocking, non-allocating
  channel offer that is scheduler-safe from a native thread. `@on_signal` delivery
  still runs on a real goroutine. Flagged in the PR.)
- **Surface.** `signal.notify(Signal.X) -> channel[Signal]` (a small **cap-1
  buffered, coalescing** channel — signals are edges, not a queue: a second edge
  is dropped while one is pending); `@on_signal(Signal.X)` on a function (sugar: a
  detached receiver goroutine over `signal.notify` invokes it per delivery);
  `scope(cancel_on=[Signal.X, ...], grace=<ms>)` — a listed signal cancels the
  scope tree cooperatively (a child observes it at its next cancellation point),
  with an optional hard-exit deadline that force-exits if graceful shutdown
  overruns. `notify`/`@on_signal`/`cancel_on` **take over** a signal's disposition
  (the OS default and any built-in default handler are suppressed; there is **no
  auto-exit** — the program decides when to exit after cleanup).
- **`Signal` is a compiler-known tier-1 enum** whose variant discriminants ARE the
  Unix signal numbers (so the runtime ABI speaks raw numbers); `SIGKILL`/`SIGSTOP`
  are excluded (uncatchable).
- **Default diagnostic handlers** (ON by default, opt out at build with
  `-DZAP_NO_DEFAULT_SIGNALS`, user-overridable): `SIGQUIT` -> goroutine stacks +
  runtime/scheduler summary; `SIGUSR1` -> force a GC + print memory stats. Both
  call the uniform `zap_gc_collect`/`zap_gc_stats` and the scheduler's `g_all[]`,
  so they work under either collector with no per-backend `#ifdef`. (`SIGUSR2`
  profiling is a separate deferred ticket, #78 — not built.)
- **Fault signals** (`SIGSEGV`/`SIGFPE`/`SIGBUS`/`SIGILL`) are NOT delivered via
  `notify`: a crash handler on a `sigaltstack` (so a stack overflow is still
  reportable) prints the faulting signal/address, the offending goroutine and a
  native backtrace, then re-raises the default disposition so the process dies
  with the right status.
- **Programmatic surface.** A compiler-known `gc`/`runtime` module —
  `gc.collect()`, `gc.stats() -> MemStats`, `runtime.goroutine_dump()`,
  `runtime.num_goroutines()` — over the same runtime entry points; the signals are
  the zero-code operator path to these. **`zap inspect <pid>`** (CLI) signals a
  running process to dump these (`SIGQUIT`, or `SIGUSR1` with `--gc`).
- **Deferred / flagged for review:** collapsing the dispatcher into the signal
  thread (above); `grace` is an integer-milliseconds expression (no first-class
  `Duration`/`30.seconds` surface yet — consistent with the deferred `timeout`);
  cooperative cancel_on reaches a coroutine already parked on a channel only at
  its next cancellation point (the `zap_netpoll_wake` parked-G interrupt is a
  follow-up); full per-goroutine symbolicated stack traces in the dump (v1 reports
  per-G state/ownership + the scheduler/GC shape); and retroactively masking a
  signal first subscribed from a worker thread (handlers are async-safe wherever
  they run, so this is a purity gap, not a correctness one).

### Runtime entry points (additive to `libzaprt`)

Sketch of the C ABI surface the scheduler needs (names illustrative):

```
zap_sched_init(n_threads)
zap_spawn(fn, args_closure, owning_scope) -> void   # G alloc + enqueue
zap_scope_enter() -> *Scope ; zap_scope_exit(*Scope) # join/cancel
zap_chan_new(elem_size, capacity) -> *Chan
zap_chan_send(*Chan, *val) ; zap_chan_recv(*Chan, *out, *ok)
zap_chan_close(*Chan)
zap_select(cases[], n, has_default) -> chosen_index
zap_mutex_* / zap_rwmutex_* / zap_waitgroup_* / zap_once_* / zap_atomic_*
zap_cancel(*G) ; zap_checkpoint()
# Netpoller (#80, built): park a G on fd readiness, free its M.
zap_netpoll_open(fd) -> *PollDesc
zap_netpoll_wait(*PollDesc, mode, deadline_ns) -> status  # ready/timeout/closed
zap_netpoll_wake(*PollDesc, mode) ; zap_netpoll_close(*PollDesc)
```

Channels, locks, `WaitGroup`, and the netpoller are built on a small internal
futex-like park queue keyed on the scheduler, not the OS, so a blocked coroutine
parks its G and frees its M.

### Roadmap mapping

- **Phase 2 prerequisite:** runtime ABI + Boehm GC frozen (ROADMAP).
  Confirm the GC-malloc'd-stack approach against the chosen `libgc` build and add
  the guard-page/overflow path.
- **Phase 3 (concurrency track):** build in dependency order and parallelize
  where possible:
  1. scheduler core (M:P:G, run queues, context switch, stacks);
  2. channels (unbuffered → buffered → `close`/comma-ok) + `select`;
  3. `spawn` codegen (closure capture, trampoline, root scope);
  4. sync primitives (`Mutex`/`RWMutex`/`WaitGroup`/`Once`/`Atomic`);
  5. `scope:` syntax + cancellation (depends on the exception ABI, #22);
  6. memory-model lowering (atomic/fence emission per happens-before rules).
  Steps 2 and 4 can proceed in parallel once step 1 lands; step 5 gates on #22.
- Touches **IR/ABI boundaries** additively only; no frozen-layout changes.

## Open issues

- **`select` details.**
  Confirm uniform-random fairness, the `default` non-blocking form, and whether
  send cases may appear alongside receive cases in the first implementation.
- **Preemption.**
  v1 is cooperative; whether/when to add signal- or safepoint-based preemption
  of long CPU loops that never hit a `checkpoint()`.
- **Growable/segmented stacks.**
  Deferred until the precise GC (D-0009) provides stack maps; until then fixed
  guard-page stacks. Initial stack size and free-list size classes to be tuned.
- **`GOMAXPROCS` analogue.**
  The name and mechanism for setting the M count.
- **Coroutine-local storage.**
  Whether to provide a coroutine-local (not thread-local) storage facility, and
  how it interacts with `scope:` inheritance.
- **Formal memory-model spec** (D-0034) and the **race-detector** tool
  (both already tracked in OPEN-QUESTIONS).
- **Timers/deadlines API** — the concrete `after()`, `sleep()`, and
  `scope(timeout=...)` surface and its clock.
- **Blocking FFI.**
  How CPython interop (D-0012) and its GIL interact with coroutines that must not
  block an M (likely a dedicated M pool for FFI calls).
