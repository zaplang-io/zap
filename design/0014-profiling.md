# Proposal: Signal-triggered runtime profiler (SIGUSR2 + `zap inspect --profile`)

- **Author(s):** Claude (I#78)
- **Last updated:** 2026-10-22
- **Status:** Accepted (implemented)
- **Discussion:** https://github.com/parveshgarg/zap/issues/78 (D-0058)

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

A sampling CPU profiler for Zap, built on the existing signal and scheduler
runtime.
`SIGUSR2` toggles profiling on a live process (the disposition D-0058 locked);
a dedicated profiler monitor thread — modeled on the I#172 preemption monitor —
periodically samples the on-CPU coroutine of every worker `M` across all `P`s.
Each sample is a native stack unwind recorded into a per-`M` lock-free ring
buffer by an async-signal-safe handler that never allocates or locks.
v1 ships **CPU profiling only** and writes a dependency-free **folded-stack text
file**; symbolication and pprof-protobuf conversion are tooling-side, offline.
Allocation profiling and in-runtime pprof are explicit follow-ups.

## Background

The runtime already provides every building block this feature needs, so the
profiler is overwhelmingly a matter of composing existing, proven machinery.

**Signals (I#77 / D-0058).**
Handled async signals are masked on all workers and unblocked only on a
dedicated signal thread, whose async-signal-safe handler merely `write()`s the
signo to a self-pipe registered with the netpoller
(`crates/codegen/src/zaprt.c`, "Signals" section).
The signal thread parks on `zap_netpoll_wait` and dispatches in **normal
context** (`zap_sig_dispatch`): this is where `SIGUSR2` is already routed and
where the profiler's start/stop logic will live — no work happens in a signal
handler.
`SIGUSR2`'s disposition is reserved by D-0058 for "toggle profiling"; it is not
a member of the user-facing `Signal` prelude enum used by `signal.notify`.

**Preemption monitor (I#172).**
A dedicated monitor OS thread (`zap_monitor_main`) wakes every `ZAP_PREEMPT_SLICE_NS`
(~10 ms), reads each `M`'s `schedtick` and `current`, and `pthread_kill`s a
worker that has run the same coroutine for a slice.
Its `SIGURG` handler (`zap_preempt_handler`) runs **on the target worker
thread** and already contains the exact safety guard a profiler sample needs:
it checks that the interrupted stack pointer lies inside the running
coroutine's green-stack range and that `g->no_preempt == 0` (no runtime lock /
allocation held) before doing anything.
This monitor-thread + `pthread_kill` pattern — not `setitimer`/`SIGPROF`
process-directed delivery — is the correct substrate for sampling **every** `P`
in an M:N scheduler, because a process-directed timer signal is delivered to one
arbitrary thread, not to each running worker.

**Unwinding.**
Every compiled Zap function carries **asynchronous** unwind tables: codegen sets
`uwtable=2` (`.eh_frame`/CFI) on every function (`crates/codegen/src/lib.rs:1158`,
for exceptions I#22).
The fault handler already unwinds with glibc `backtrace()` /
`backtrace_symbols_fd` (async-signal-safe, `zap_fault_handler`).
So native CFI unwinding of compiled Zap frames is available and proven.

**Coroutine identity and liveness.**
`G.all_idx` is a stable per-coroutine id; the statically-rooted `g_all[]`
registry and `zap_runtime_num_goroutines()` enumerate live coroutines;
`M.current` is the on-CPU coroutine of a worker.

**GC (I#48 precise Immix, default; I#120 moving).**
A CPU sample reads only **return addresses** (code addresses), never heap object
pointers, so it is immune to I#120 evacuation moving objects.
Evacuation runs at stop-the-world while all mutators are parked at safepoints;
a parked worker is not on-CPU, so it is simply not a CPU sample.
The one hazard is sampling a worker mid-context-switch on its native scheduler
stack — handled by the same green-stack-range guard the preempt handler uses.

**What is missing.** The GC shadow stack (`ZapShadowFrame {prev, nroots,
roots[]}`) carries roots but **no PC and no function identity**, so it cannot
produce a call stack.
The profiler must unwind the native stack; it does not reuse the shadow chain.

## Proposal

### Trigger and lifecycle

- **`SIGUSR2` (process-directed)** toggles profiling, routed through the existing
  signal thread into `zap_sig_dispatch` → a new `zap_profile_toggle()` that runs
  in normal context (safe to allocate, open files, start threads):
  - first edge: open the output file, warm up the unwinder (one `backtrace()`
    call outside any handler so libgcc's lazy init is done), allocate per-`M`
    ring buffers, start the **profiler monitor thread**;
  - second edge: stop the monitor, drain and symbolicate buffers, write the
    folded file, free buffers.
- **`ZAP_PROFILE=cpu`** (env) auto-starts the profiler in `zap_signal_init`
  (program prologue) for zero-code, whole-run profiles, stopped and flushed in
  `zap_rt_join_all`.
  `ZAP_PROFILE_HZ` (default 100) sets the sample rate; `ZAP_PROFILE_OUT` sets the
  path (default `zap-cpuprofile-<pid>.folded`).
- **Runtime API** (the I#121 module seam, matching `runtime.*`/`gc.*`):
  `runtime.profile_start()` / `runtime.profile_stop()` — zero-arg thin wrappers
  over the same start/stop, so a program can bracket a region of interest; the
  output path and rate come from `ZAP_PROFILE_OUT`/`ZAP_PROFILE_HZ`, keeping the
  surface tiny and avoiding a string-argument dependency in v1 (a `path`
  overload, and a Python-`cProfile`-shaped `profile` module, are follow-ups — the
  latter waits on `with`-statement context managers, I#185).

### Sampling mechanism

A dedicated **profiler monitor thread** (clone of `zap_monitor_main`, blocking
the sampling signal on itself) sleeps one sample period, then for each `M` with
`m->current != NULL` and no collection in progress sends the **sampling signal**
via `pthread_kill(m->thread, …)`.

The sampling signal is **`SIGPROF`**, the conventional profiling signal, unused
by I#77/I#172 and reserved by the runtime (never a `signal.notify` target,
exactly as `SIGURG` is reserved for preemption).
Its handler runs on the worker thread and is strictly async-signal-safe:

```
zap_prof_handler(sig, info, uc):
    m = cur_m
    if !m || !m->current: record DROP(reason=runtime); return
    g = m->current
    if sp(uc) not in [g->stack, g->stack+stack_size)   # mid context-switch
       or g->no_preempt != 0                            # in runtime crit-section
       or gc_collecting:
        record DROP(reason=runtime); return
    n = backtrace(frames, MAX_DEPTH)          # async-signal-safe, warmed up
    ring_push(m->prof_ring, g->all_idx, frames[0..n])   # lock-free, preallocated
```

The handler allocates nothing, takes no lock, and symbolicates nothing.
The per-`M` ring is a fixed preallocated array of fixed-depth PC records written
with a single relaxed-atomic head bump (single producer = this worker; single
consumer = the stop path after the monitor has stopped signaling).
A full ring drops the oldest sample and counts the drop.

Samples where the worker was in the runtime (allocator, lock, scheduler switch)
are **not discarded data** — they are bucketed as a synthetic `[runtime]` frame,
so the profile still accounts for 100 % of on-CPU time rather than silently
under-reporting runtime-heavy workloads.

### Output format (v1)

Folded / collapsed stacks (Brendan Gregg's `frame;frame;frame count`), one line
per unique stack, written once at stop:

```
main;compute;inner 1423
main;compute;[runtime] 211
```

Rationale over in-runtime pprof protobuf: folded text needs no protobuf/varint/
gzip encoder in the async-safe C runtime, keeps the I#77/I#80 "dependency-free"
ethos, is directly consumable by `flamegraph.pl`, and both Go's `pprof` and
`go tool pprof` ingest it (or a tiny tooling-side converter emits
`profile.proto`).
A **pprof-protobuf writer is a fast follow, done offline in the Rust driver**,
not in the hot runtime path.

### Symbolication

The handler records **raw PCs** only.
Symbolication happens offline at stop (normal context), via glibc
`backtrace_symbols`, which maps each PC to `module(symbol+0xoff)`.
To make Zap function names resolve, the link adds **`-rdynamic`** (export program
symbols to `.dynsym`).
This is a deliberate, cheap trade: `-rdynamic` enlarges the dynamic symbol table
slightly but adds **no hot-path or codegen cost** — unlike forcing frame
pointers — so the "zero-cost when profiling is unused" property holds (nothing
runs until a toggle).
It is also a bonus for the I#77 fault handler, whose `backtrace_symbols_fd`
report now carries symbol names.
A tooling-side symbolizer against the on-disk `.symtab` (dropping even
`-rdynamic`) remains a possible follow-up.

### Diagnostic tool

`zap inspect <pid> --profile [--seconds N]` extends the existing `zap inspect`
(`crates/driver/src/cli.rs`): send `SIGUSR2`, wait `N` seconds (default 10),
send `SIGUSR2` again, and report the output path the target wrote.
Dependency-free (shells out to `kill`, like the existing `--gc`).

## Rationale

- **Monitor + `pthread_kill` vs `setitimer`/`SIGPROF` process-directed:** only
  per-worker signaling samples all `P`s in the M:N scheduler; a process timer
  hits one thread.
  Reusing I#172's monitor pattern reuses its proven correctness (benign
  schedtick race, self-blocked signal, `GC_pthread_create` accounting).
- **CPU-only v1 vs CPU+alloc:** allocation profiling must hook the Immix
  bump/line allocator and the Boehm path and attribute a stack at each alloc —
  a different, heavier mechanism with its own async-safety story — so it is a
  clean v2 that reuses v1's unwinder and output.
- **Folded text vs pprof protobuf:** keeps the async-safe runtime dependency-free
  and still interops with the entire flamegraph/pprof ecosystem via a trivial or
  tooling-side conversion.
- **Native unwind vs shadow chain:** the shadow chain has no PCs; native CFI
  tables already exist (`uwtable=2`), so native unwind is both necessary and
  cheap.
- **Signal choices** follow D-0058 (`SIGUSR2` = toggle) and the I#172 precedent
  (`SIGPROF` reserved by the runtime, like `SIGURG`).

## Compatibility

Zero cost when unused: the monitor thread and ring buffers exist only after the
first `SIGUSR2` / `ZAP_PROFILE`, preserving the scheduler-laziness invariant
(I#47).
No grammar change — the surface is signal + env + ordinary `runtime.*` method
calls (no EBNF edit; confirm at implementation per the "update EBNF after
decisions" rule only if a dedicated `profile` module/`with` form is chosen).
`SIGPROF` becomes reserved by the runtime (it was never a `Signal`-enum member).
Builds with `-DZAP_NO_DEFAULT_SIGNALS` get no auto-default but keep the explicit
API.

## Implementation

Runtime-only plus a thin module-seam surface, disjoint from current lanes.

1. `zaprt.c` profiler section: ring buffer type, `SIGPROF` handler, monitor
   thread, `zap_profile_start/stop/toggle`, env auto-start hook in
   `zap_signal_init`, stop/flush in `zap_rt_join_all`.
2. `SIGUSR2` default wired into `zap_sig_dispatch` (reserve it, like SIGQUIT/
   SIGUSR1 defaults).
3. Offline symbolication + folded writer (C at stop, or Rust in the driver).
4. Module seam (I#121): `runtime.profile_start/stop` in
   `crates/{sema,ir/lower,codegen}/src/modules/runtime.rs` + registry lines.
5. `zap inspect --profile` in `crates/driver/src/cli.rs`.
6. Goldens (both collectors): a CPU-bound fixture profiled via the test
   `__raise_signal` hook asserts the hot function appears in the folded output —
   **no sample-count or timing assertions** (non-deterministic; per the GC
   goldens rule).
7. Follow-ups: pprof-protobuf writer; allocation profiling (v2); macOS once the
   netpoller/signal subsystem is non-Linux-capable.

Phase boundaries touched: none (no IR schema change; runtime-ABI gains
`zap_profile_*` symbols only).

## Resolved (orchestrator rulings)

- **A — unwinder async-safety.** WARMUP-ONLY for v1: `backtrace()` /
  `backtrace_symbols` are warmed once at profiler start so libgcc's lazy init is
  off the handler path.
  The force-frame-pointer codegen flag is **not** added — it would tax all
  compiled code even when never profiling.
  Frame-pointer walking stays documented as the fallback if warmup proves flaky.
- **B — output format.** Folded/collapsed-stack text for v1; pprof-protobuf is an
  offline tooling follow-up in the Rust driver.
- **C — allocation profiling.** Deferred to v2, reusing v1's unwinder/output.
- **D — surface API.** `runtime.profile_start/stop` + `SIGUSR2` + `ZAP_PROFILE`
  env for v1; a `profile` module is a later ergonomic layer (needs I#185 `with`).
- **E — SIGPROF reservation.** `SIGPROF` is reserved by the runtime and is not a
  member of the `Signal` prelude enum, so `signal.notify` cannot claim it —
  exactly as `SIGURG` is reserved for preemption.
- **F — scope.** Linux/glibc, x86-64 only for v1; a no-op elsewhere, consistent
  with the netpoller-gated signal subsystem.

### Added correctness requirement (EINTR)

A SIGPROF sample interrupts whatever syscall the sampled coroutine is in.
SA_RESTART auto-restarts the restartable syscalls; the ones it does not
(`poll`/`epoll_wait`/`nanosleep`) are EINTR-looped at their call sites, and the
handler skips coroutines that are off-CPU (parked/blocked) or mid-switch.
On Linux the only blocking syscall a *sampled* (on-CPU, non-pinned) coroutine
can be caught in is `nanosleep` (channel/netpoller waits park and free the M, so
they are not sampled) — and the `nanosleep` loop already resumes the remaining
time across interrupts.
`profile_eintr.zap` profiles, at a high rate, CPU-bound coroutines (heavily
sampled) alongside `lock_os_thread` coroutines that block in `nanosleep` on their
dedicated OS threads, and asserts a deterministic total with no spurious failure.

A pre-existing gc-immix instability (unrelated to profiling: it reproduces with
the profiler off) means non-pinned `time.sleep_ns` in a tight loop, and
`sleep`+mutex-contention+mass-spawn, can stall/crash under the moving collector;
that is tracked separately, and the EINTR golden deliberately avoids it.
