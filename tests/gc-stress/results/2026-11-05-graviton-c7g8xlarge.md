# 2026-11-05 — many-core aarch64 (Graviton c7g.8xlarge, 32 vCPU)

Collectors: `gc-boehm` and `gc-immix`. `MAXPROCS=32`, `GC_FREE_SPACE_DIVISOR=40`.
Each fixture looped via `zap run` (see `../stress.sh`); hangs/crashes caught with
`../catch.sh` (`gdb thread apply all bt`).

## Stress board

| fixture | gc-boehm | gc-immix |
|---|---|---|
| `immix_evac_coro_stress` | hang @iter 65 | hang @iter 36 |
| `concurrent_queue_mpsc_coro_stress` | hang @iter 2 | hang @iter 5 |
| `concurrent_queue_mpsc_thread_stress` | crash @iter 33 | crash @iter 3 |
| `threading_counter` | crash @iter 1 | crash @iter 1 |

**Verdict: collector-agnostic → #223.** Every fixture fails under *both*
collectors, so these aarch64 failures are the #223 multicore race, not the
boehm-only #693 mechanism. `threading_counter` fails at iter 1 under both — so
the #295 `threading_counter` flake on aarch64 is #223. (The #693 x86-only
`gc-boehm` signature remains unconfirmed: 64k x86 `gc-boehm` iters stayed green.)

Note on debug builds: under `catch.sh` with a `-g -fno-omit-frame-pointer`
binary, `immix_evac_coro_stress` ran 800+ iterations clean — a heisenbug. Symbols
+ frame pointers slow the allocator/switch path and shrink the race window; the
release build hung at iter 36. This is itself evidence the bug is a tight timing
race, consistent with the root cause below.

## Hang signature (gc-boehm, captured twice)

`thread apply all bt` on the hang:

- worker coroutines blocked in `GC_generic_malloc_many` (← `zap_obj` /
  `zap_list_new` ← `worker`), waiting on the **bdwgc global allocator mutex**;
- one worker M in `set_bottom` → `GC_call_with_alloc_lock`, called from
  `switch_to_g` (`crates/codegen/src/zaprt.c` ~4489 → ~4322);
- all other Ms idle in `sched_find_runnable`;
- GC-marker threads idle (no collection in flight);
- `main` parked in `chan_recv`.

## Root cause — scheduler ↔ GC-allocator lock-ordering inversion

A coroutine acquires the bdwgc alloc lock (inside the allocator, or in
`set_bottom`) and is then **descheduled while holding it** — preempted on its
green stack, because `no_preempt` does not cover the allocator critical section
under aarch64 weak memory ordering (cf. #213). To resume *any* coroutine the
scheduler runs `switch_to_g` → `set_bottom` → `GC_call_with_alloc_lock`, which
needs the very lock the parked coroutine holds. The only coroutine that can
release the lock can only be rescheduled through a path that first demands that
lock ⇒ unbreakable deadlock once a coroutine parks holding the GC allocator lock.

Under **immix** `GC_call_with_alloc_lock` is a compile-time no-op
(`#define ...((fn)(arg))`, `zaprt.c` ~line 105), yet immix also hangs — so it
deadlocks on a *different* lock (immix's own heap/evac mutex) via the **same**
pattern: a non-reentrant runtime lock held across a coroutine switch/park. The
fix must therefore address the general pattern, not just the boehm symbol.

## Fix direction (lands under #223)

Break the inversion:

1. `set_bottom` on the scheduler/switch path must update the stack bottom
   **without** taking the bdwgc alloc lock; and/or
2. make preemption/parking impossible while any allocator lock is held — fix
   `no_preempt` coverage/visibility around `GC_generic_malloc_many` and the
   immix allocator on aarch64 (memory ordering, cf. #213).

Code: `crates/codegen/src/zaprt.c` `set_bottom` (~4318/4322), `switch_to_g`
(~4469/4477/4489), and the allocator on every `zap_obj` / `zap_ptrbuf` /
`zap_list_new`. Keep x86 behavior unchanged and the fix collector-agnostic.

## Verification note

This reproduces **only** on real many-core aarch64 silicon. x86 and the hosted
`ubuntu-24.04-arm` CI runner do not trip it reliably, so local `cargo test` /
CI going green does **not** prove the fix — only that nothing regressed. Final
confirmation is a re-run of `stress.sh boehm` and `stress.sh immix` under load on
a box like this one, which must reach 0 failures at `MAXPROCS=32` under both
collectors (and the permanent `../repro223/regress.sh` must be 0/N on both).
