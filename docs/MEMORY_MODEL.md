# The Zap Memory Model

This document is the normative description of how memory behaves when a Zap
program runs more than one coroutine. It is the law behind the `--race`
detector: the detector flags exactly the programs this document calls races.

The decision record is **D-0141** (with D-0034, D-0114, D-0140); this file is the
reader-facing version of it.

## What Zap guarantees, in one paragraph

A **data-race-free** program behaves **sequentially consistently**: it runs as if
all coroutines' steps were interleaved in a single global order consistent with
each coroutine's own program order (this is *DRF-SC*). A program that **does**
contain a data race is still **memory-safe** — unlike C or C++, a race in Zap can
never corrupt memory or produce an out-of-thin-air value. A race is a **logic
bug**, caught by `zap test --race`, not undefined behavior.

## Happens-before

Two memory accesses are **ordered** if one *happens-before* the other. If two
accesses touch the same location, at least one is a write, and neither
happens-before the other, that is a **data race**.

Happens-before is the transitive closure of **program order** (within one
coroutine) and these cross-coroutine **synchronizing edges**:

| Edge | Rule |
|------|------|
| **spawn** | `spawn f()` happens-before the first statement of `f` (parent → child). |
| **scope join** | A *joined* child happens-before the code after the `scope:` that owns it (child → parent). "Joined" = ran to completion, or was cancelled and **drained** (see below). |
| **channel** | A send happens-before the matching receive. |
| **Mutex / RWMutex** | An `unlock` happens-before the next `lock` that acquires it. |
| **WaitGroup** | A `done` happens-before the `wait` it releases. |
| **Once** | The initializer happens-before every call that observes it complete. |
| **Atomic** | Every `Atomic` op is in one global order; an `Atomic` write happens-before every `Atomic` read that observes it. |
| **module init** | Module-level `const`/global init happens-before `main`, in import-topological + source order. |

If your program shares mutable state between coroutines, make sure one of these
edges orders every conflicting pair of accesses. If none does, it is a race.

### The classic example

```python
bool ready = Atomic[bool](false)
int64 data = 0

# producer coroutine
data = 42                 # (1) plain write
ready.store(true)         # (2) Atomic write — publishes (1)

# consumer coroutine
while not ready.load():    # (3) Atomic read — acquires (2)
    pass
print(data)               # (4) guaranteed to print 42
```

(2) and (3) are `Atomic` ops, so they create a happens-before edge: once (3)
observes `true`, (4) is guaranteed to see the (1) write. Make `ready` a plain
`bool` and the edge disappears — (4) may print `0`, and it is a race.

## Atomics are sequentially consistent

`Atomic[T]` operations — `load`, `store`, `fetch_add`, `compare_exchange`, … —
are **sequentially consistent**. They take **no memory-ordering argument**: there
is no `Relaxed`/`Acquire`/`Release` surface. You reason about atomics with one
rule — they form a single global order every coroutine agrees on — the same model
as Go's `sync/atomic` and Java's `volatile`.

(Weaker orderings are deliberately omitted. If a future release ever adds them for
a measured performance need, they will live behind a separate, explicitly-expert
API, never as an argument on the everyday `Atomic[T]`.)

## Structured concurrency and happens-before

`spawn` and `scope:` are the common way coroutines start and join, so they are
where most happens-before edges come from.

```python
scope:
    handle_a = spawn compute(a)
    handle_b = spawn compute(b)
# <-- at the dedent, the scope joins both children
results = [handle_a.value, handle_b.value]   # safe: the join ordered the writes
```

Because the scope joins both children before the code after it runs, each child's
writes happen-before that code. **Collect results by returning them** (as above)
and reading after the join — that is the race-free pattern. Having two children
mutate a shared `list` instead (`results.append(...)` from both) is a data race
regardless of the join; D-0140 keeps it memory-safe and `--race` will catch it,
but it is still a bug.

### Cancellation is cooperative

Cancellation (a deadline, a sibling's failure, an external cancel) injects a
`Cancelled` exception at the child's next **cancellation point** — any suspend
point, or an explicit `checkpoint()`. The child then unwinds through its
`finally`/`with` cleanup. A cancelled child that drains this way is still
*joined*: all of its writes, cleanup included, happen-before the parent's
continuation. Its state may be **partial** (it stopped early); the model
guarantees that partial state is *visible and un-torn*, and your program decides
whether it is *meaningful*.

### The sever rule (an uncooperative child)

A child that never reaches a cancellation point — a tight compute loop with no
safepoint, or a blocking FFI call — cannot be drained. Zap does **not**
force-kill it (tearing down a coroutine mid-instruction is unsafe) and does
**not** kill the process. Instead the scope **severs** it: at the grace deadline
the stuck child is **detached** (reparented to the root scope, exactly like
`spawn detached` — it becomes an orphan that leaks until it finishes, surfaced to
observability), and the scope raises `ScopeTimeout` to the caller.

A severed child therefore has **no join edge** to the caller's continuation.
Reading its partial state after catching `ScopeTimeout` is a race — bounded, not
UB, and caught by `--race`, but a bug. Treat `ScopeTimeout` as "give up on these
results," not "read what they managed so far."

Process exit is reserved for **signal-driven shutdown**
(`scope(cancel_on=[SIG…], grace=…)`), where the whole program is terminating
anyway. A per-request `scope(timeout=…)` never takes the process down — one slow
endpoint cannot kill in-flight healthy work.

> The current runtime does not yet implement the sever rule for a plain
> `scope(timeout=…)` — it either force-exits (with `grace`) or hangs the join
> (without). That divergence is tracked in `zaplang-io/zap#825`; this document
> describes the intended, settled semantics.

### Detached coroutines

`spawn detached f()` opts out of the enclosing scope: it is owned by the implicit
**root scope** and is joined (and gains the join edge) only at program exit. A
detached coroutine's writes are **not** ordered relative to the code after any
inner scope — synchronize with it explicitly (a channel, a `WaitGroup`, an
`Atomic`) if you need to observe its results.

## Data races: what you get

- **Scalars and pointers** (`int64`, `float64`, a reference, …) are word-atomic
  on the supported targets: a raced read sees the old or the new value, **never
  garbage**.
- **Built-in collections** (`list`, `dict`, `set`) are **not** safe under
  concurrent mutation. A concurrent structural mutation (two coroutines growing
  the same `list`) fails fast with a `fatal:` guard rather than corrupting the
  heap. Share a collection across coroutines only behind a `Mutex`, or use a
  `concurrent.*` type.
- **No out-of-thin-air values.** A race never fabricates a value that no
  coroutine wrote.

So a race in Zap is always a *logic* bug with a bounded, debuggable outcome —
never memory corruption. Run `zap test --race` to find them.

## Rules of thumb

1. Share nothing mutable between coroutines unless an edge above orders every
   access — prefer **passing values** (channels, returned results) over sharing.
2. For shared mutable state, reach for `Atomic[T]`, `Mutex`, or a `concurrent.*`
   collection — the types that carry `@shareable`.
3. A mutable **module global** must be `@shareable` (or marked `@unshared` for a
   single-threaded program) — see D-0140.
4. Collect results from `spawn`ed children **by return**, read after the scope
   join; never by mutating a shared container from siblings.
5. Run `zap test --race` in CI. A clean `--race` run is only as good as the
   interleavings your tests exercise, so test concurrent paths deliberately.

## References

- **D-0141** — the memory-model decision (edge set, DRF-SC, SC-only atomics,
  scope-join + sever rule).
- **D-0034** — DRF-SC principle. **D-0140** — the bounded-race guarantee and
  `@shareable`/`@unshared` globals. **D-0114** — the `--race` detector.
- **D-0044 / D-0094** — structured concurrency, cancellation, `ScopeTimeout`.
- **zaplang-io/zap#825** — aligning the scope-timeout runtime with the sever rule.
