# `tests/gc-stress/` — many-core GC/scheduler stress & deadlock-debug harness

Out-of-band stress tooling for the **nondeterministic multicore GC races** that
the hosted CI runners almost never trip but a big many-core box trips within
seconds:

- **#223** — aarch64 multicore GC hang / use-after-free (collector-agnostic:
  reproduces under **both** `gc-immix` and `gc-boehm`).
- **#693** — the `gc-boehm` in-signal `copreempt`-vs-stop-the-world hypothesis
  behind the #295 `threading_counter` flake (so far **not** separable from #223
  on aarch64; its x86-only `gc-boehm` signature is still unconfirmed).

This is a **debug harness, not a CI gate**. The per-PR arm64 gate stays the
hosted `ubuntu-24.04-arm` job; the permanent in-tree #223 regression lives in
[`../repro223/regress.sh`](../repro223/regress.sh) and the race-detector
fixtures in [`../race/`](../race/). This folder is what you reach for when you
have a real many-core aarch64 box and want to *provoke* and *root-cause* the
hang under `gdb`.

## Hardware

The race needs **genuine multicore parallelism on real aarch64 silicon** —
QEMU/emulation does not reproduce it (swapcontext-from-signal is not faithfully
emulated), and x86 does not reproduce #223 at all. In practice: a many-core
Graviton box (e.g. AWS `c7g.8xlarge` = 32 vCPU; `c7g.16xlarge` = 64 vCPU if 32
won't surface a given case). Provision it on-demand and tear it down after — the
provisioning is out of scope for this repo (kept in private infrastructure), and
nothing account-specific belongs here.

A debug build is needed for the backtraces (the race lives in the C runtime):

```bash
cargo build --release -p zapc --features zapc/gc-boehm      # or zapc/gc-immix
```

## 1. Stress (`stress.sh`)

Loops the concurrency stress goldens via `zap run` under the chosen collector
and flags any run that **hangs** (timeout), **crashes** (non-zero exit), or
prints a **wrong total** (compared against each fixture's `.expected`). Run it
under **both** collectors — that is how you attribute a failure:

- fails under **both** immix and boehm → **#223** (collector-agnostic);
- `threading_counter` fails under **boehm but not immix** → the **#693** signature.

```bash
# usage: stress.sh [immix|boehm] [iters] [maxprocs]
ZAPC=target/release/zapc tests/gc-stress/stress.sh boehm 2000 "$(nproc)"
ZAPC=target/release/zapc tests/gc-stress/stress.sh immix 2000 "$(nproc)"
```

Knobs: `MAXPROCS` (worker-M parallelism; the harness defaults it to `nproc`) and
`GC_FREE_SPACE_DIVISOR=40` (forces frequent Boehm stop-the-world; a no-op under
immix — there, widen the window with oversubscription, e.g. `MAXPROCS=$(( $(nproc) * 2 ))`).

Run it inside `tmux` so a dropped SSH doesn't kill the run:

```bash
tmux new -s gc
ZAPC=target/release/zapc tests/gc-stress/stress.sh boehm 2000 "$(nproc)" 2>&1 | tee boehm.log
```

## 2. Catch a hang/crash under gdb (`catch.sh`)

Loops one debug binary and, on a hang, attaches `gdb` for an all-thread
backtrace; on a crash, opens the core. Build the fixture first:

```bash
sudo sysctl -w kernel.yama.ptrace_scope=0     # allow gdb to attach (fresh box each time)
ulimit -c unlimited; sudo sysctl -w kernel.core_pattern=/tmp/core.%e.%p   # for crashes

ZAP_CC_EXTRA="-g -fno-omit-frame-pointer" target/release/zapc build tests/golden/immix_evac_coro_stress.zap
# NB: `zapc build` writes the binary to the CURRENT dir: ./immix_evac_coro_stress

tests/gc-stress/catch.sh ./immix_evac_coro_stress "" 60
# threading_counter has a known-good total, so you can assert it:
ZAP_CC_EXTRA="-g" target/release/zapc build tests/golden/threading_counter.zap
tests/gc-stress/catch.sh ./threading_counter 80000 60
```

`rr` replay is unreliable on Graviton — the gdb all-thread backtrace plus cores
are the dependable tools here.

### Reading a backtrace

- **timeout** = a **hang** (deadlock/livelock); **non-zero exit** = a **crash**.
- Known `gc-boehm` hang signature (aarch64, captured 2026-11-05, twice): every
  thread blocked on the **bdwgc global allocator mutex** or idle — worker
  coroutines in `GC_generic_malloc_many` (← `zap_obj`/`zap_list_new`), one worker
  M in `set_bottom`→`GC_call_with_alloc_lock` from `switch_to_g`
  (`crates/codegen/src/zaprt.c` ~4489→4322), other Ms idle in
  `sched_find_runnable`, GC-markers idle, `main` parked in `chan_recv`.
- Root cause and the fix direction are written up in
  [`results/2026-11-05-graviton-c7g8xlarge.md`](results/2026-11-05-graviton-c7g8xlarge.md).
  Under immix the blocked lock is a *different* mutex (immix's own heap/evac
  lock; `GC_call_with_alloc_lock` is a compile-time no-op there) reached via the
  **same** pattern — a lock held across a coroutine switch/park.

## 3. Results

Each stress/debug session drops a **timestamped** markdown file under
[`results/`](results/) (board: hardware, collector, `MAXPROCS`, per-fixture
outcome, and any captured backtrace signature). Keep raw `*.bt` captures next to
the write-up when you have them. These are evidence for the fixer, not a gate.
