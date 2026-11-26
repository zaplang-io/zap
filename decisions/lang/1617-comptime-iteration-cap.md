---
title: A fixed per-loop cap bounds comptime `for` unroll breadth, with cost telemetry
status: Locked
issue: 1617
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# A fixed per-loop cap bounds comptime `for` unroll breadth, with cost telemetry

**Status:** Locked

## Decision

The comptime engine gains one more **fixed hard limit** (epic #1249, HQ5):
`DEFAULT_ITER_CAP = 100_000`, the maximum number of elements a single `comptime
for` may unroll. Exceeding it is a clean `CtError::budget` (`ZAP7001`) naming the
breadth and the cap, pointing at the loop's iterable — never a panic or an
exploded specialized output. It joins the existing step cap
(`DEFAULT_STEP_CAP = 1_000_000`) and depth cap (`DEFAULT_DEPTH_CAP = 128`); the
three layer cleanly — the iteration cap bounds **each loop's breadth**, the step
cap bounds the **total work** across nested loops, the depth cap bounds nesting.

Like the other caps it is a fixed limit, not a user-facing tunable quota; the
`with_iter_cap` builder exists only so tests drive it at a small, observable
value. The default is deliberately generous: every shipped codec unrolls over a
struct's fields or an enum's variants (a handful), orders of magnitude under it,
so it is a safety net real code never meets.

The engine also exposes **cost telemetry** for the E7 cost controls: an
`Evaluator` reports `steps_used()` and `unrolled_iterations()` (cumulative across
nested loops), alongside the registry's existing `MemoStats` (`type_info(T)`
`computed`/`reused`, #1255). Together these are a deterministic, comparable
compile-time-cost metric.

## Rationale

Before this, the step cap caught a runaway comptime computation by *total work*,
but there was no dedicated signal for a single pathological `comptime for` over a
large comptime sequence, and no engine-level way to *observe* comptime cost
beyond `MemoStats`. The fragment itself cannot generate a large list (no range,
comprehension, or recursion — §6 of `docs/COMPTIME.md`), so in practice the
breadth is bounded by field/variant count; the cap is therefore defensive — a
crisp, named diagnostic for the pathological case rather than a generic
step-limit error — and the telemetry is the useful, always-on part, giving the
cost-regression harness (extending `crates/comptime/tests/memoization.rs` and the
new `cost.rs`) a stable metric.

The change is confined to `crates/comptime` (the engine and `unroll`); the
`crates/mono` driver and the renderers are untouched. A fresh `Evaluator` is
built per `comptime for` in mono, so the cap is naturally per-loop and the
telemetry is per-loop; a cumulative per-derive aggregate is a possible follow-up.

## Consequences

- **Experimental surface.** The comptime cost model is part of the Experimental
  reflection/comptime surface (`docs/STABILITY.md`); this is additive.
- **Host-tested; no LLVM needed.** `crates/comptime` is pure host Rust, so the
  cap (boundary, nested-loop per-loop enforcement, clean `Budget` diagnostic) and
  the telemetry are fully host-tested (`crates/comptime/tests/cost.rs`). Real
  derives (far under the cap) are unaffected — the `crates/mono` and `zap_cli`
  suites pass unchanged.
- **The compile-time wall-clock benchmark over the real shipped codecs**
  (Json/serde/msgpack) — which needs building/running `zapc` on an LLVM host — is
  a tracked follow-up under #1617 / epic #1553; the deterministic engine-level
  cost metric (steps / iterations / memo) lands here and is the CI-friendly
  regression signal.
