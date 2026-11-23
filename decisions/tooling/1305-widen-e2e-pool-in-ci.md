---
title: Widen the end-to-end test pool in CI via a two-pass sensitivity-aware worker pool
status: Accepted
issue: 1305
pr:
date: 2026-11-23
domain: tooling
supersedes:
superseded-by:
---

# Widen the end-to-end test pool in CI via a two-pass sensitivity-aware worker pool

**Status:** Accepted

## Context

The dominant cost of every heavy CI leg (`full` and `arm64` in
`.github/workflows/ci.yml`) is a single step — `cargo test --workspace --features
zapc/<gc>` — of which ~90% is *running* the end-to-end `.zap` corpora, not
compiling the workspace. Those corpora (golden: 664 fixtures; conformance: 86;
CPython-diff: 78 curated + a seeded random set; plus the `tests/race/*` corpus)
compile→LLVM→clang→link→execute one program per fixture. The work is
embarrassingly parallel, but it ran on a bounded worker pool whose default width
was **2** (`golden.rs::golden_jobs()`), and CI never raised it — so 600+
compile-link-run cycles ran 2-at-a-time on multi-core runners, leaving most of
the runner idle. `conformance.rs` and `diff_cpython.rs` had no width knob at all
and walked their corpora serially.

Widening is not free of risk:

- The width-2 default was deliberate: the corpora include concurrency / GC /
  scheduler **stress** fixtures with a history of being timing-sensitive under
  load (the `#223` GC-deadlock family, the boehm-flaky `#295`, quarantined
  aarch64 stress goldens).
- The golden harness already times compile+link under a *generous* ceiling and
  the compiled program's *run* under a tight 60s deadline (`compile_then_run`,
  `#1113`), precisely so a linker starved of CPU on a loaded runner is not
  misreported as a program hang. But `conformance.rs`, `diff_cpython.rs` (its
  zap side) and the `tests/race/*` runner still used a single `zap run` under one
  deadline, so widening *them* naively would re-introduce the `#1113` flake.

## Decision

Introduce a shared end-to-end harness (`crates/driver/tests/common/mod.rs`) used
by the golden, conformance and CPython-diff integration tests, and widen the pool
in CI only, with the sensitive subset protected.

1. **Two-pass pool.** `run_e2e_parallel` runs each corpus in two back-to-back
   passes: a **wide** pass at `ZAP_GOLDEN_JOBS` workers over ordinary fixtures,
   then a **conservative** pass at `ZAP_GOLDEN_SENSITIVE_JOBS` workers
   (default `min(2, wide)`, clamped to never exceed the wide width) over the
   sensitivity-classified subset. Because the passes run sequentially, peak
   concurrency never exceeds the wide width.

2. **Sensitivity classification** (`is_sensitive`): a fixture runs in the
   conservative lane when it is under `tests/race/` (that corpus still uses the
   single-deadline `zap run` and is concurrency-heavy), when its name contains
   `stress` or `preempt` (the timing-sensitive scheduler/GC programs), or when it
   carries an explicit `<fixture>.serial` sidecar (same convention as
   `.skip_arch`/`.env`). The rule is pure and host-unit-tested.

3. **Conservative local default.** `ZAP_GOLDEN_JOBS` still defaults to 2 and
   `ZAP_GOLDEN_SENSITIVE_JOBS` to `min(2, wide)`, so a contributor's local
   `cargo test` behaves exactly as before. The widening is driven purely from CI.

4. **Port the `#1113` build/run split** to `conformance.rs` and the zap side of
   `diff_cpython.rs` (via the shared `compile_then_run`) so they can be
   parallelized without re-introducing linker-starvation false timeouts.

5. **CI wiring.** The heavy legs — `full` and `arm64`, both of which already run
   only on `merge_group` and `push:main` — export `ZAP_GOLDEN_JOBS=$(nproc)` on
   their `Test` steps (and the `full` LTO smoke step), using a portable core
   count rather than a hardcoded number. The sensitive pass keeps its
   conservative default.

Deterministic, readable failure reporting is preserved: each pass reports in
sorted fixture order and the merged failures are sorted by fixture path.

## Alternatives considered

- **Blanket-widen everything.** Rejected: it exposes the known-flaky
  concurrency/GC/stress fixtures and the single-deadline corpora (`#1113`) to the
  higher load the width-2 default was chosen to avoid.
- **Re-add the full matrix to more events / shard across more jobs.** Rejected:
  that spends more Actions minutes (the opposite of the goal and of the
  fast/full split, Epic #1088), whereas raising the per-leg pool width uses cores
  already paid for.
- **Make the hardcoded default `available_parallelism()`.** Rejected as the
  primary mechanism: it would silently saturate contributors' machines on local
  `cargo test`. Driving the width from CI env keeps local behavior unchanged.
- **Per-file width sidecar for every sensitive fixture.** Rejected as the sole
  mechanism (too much to maintain and easy to forget on new fixtures); kept only
  as the `.serial` escape hatch on top of the name/path rule.

## Validation

Docker/LLVM-18 is unavailable on the maintainer's host and the full matrix runs
only on `merge_group`/`push`, so the timing win and any width-induced flake only
show in a real full-matrix run. The width/classification policy is pure and
host-unit-tested (`common::tests`), and the host (no-LLVM) clippy/test gate
covers the module wiring. The PR's own merge-queue run is the end-to-end proof:
before/after `Test`-step durations are recorded in the PR, and the sensitive
subset is watched for new flakiness at the higher width (dial back the sensitive
width or widen the `.serial` set if it appears).
