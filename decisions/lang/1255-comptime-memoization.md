---
title: Comptime memoization + specialization dedup + compile-time metric (E2)
status: Accepted
issue: 1255
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Comptime memoization + specialization dedup + compile-time metric (E2)

**Status:** Accepted — records the *implemented shape* of issue #1255 (epic
#1249's HQ5 build-time-cost controls). Builds on #1250/#1251/#1253.

## Decision: keep comptime reflection from becoming a compile-time tax

### 1. Memoize `type_info(T)` (`zapc_comptime::TypeRegistry`)

The registry now caches each type's reflection record and serves it on every
later introspection, keyed by a **faithful canonical type key** (`type_key`)
so distinct parameterized types never collide (`Optional[int64]` ≠
`Optional[str]`, `list[int64]` ≠ `list[str]`). `resolve_type` / `resolve_ty`
compute a record **once** and reuse it thereafter; the container-recursion and
`field.ty` paths (#1253) all route through the memo. An in-process cache for v1;
on-disk / cross-build caching is deferred.

`MemoStats { computed, reused }` exposes the accounting: `computed` is the number
of distinct records built (misses), `reused` the number served from the memo
(hits). `memo_stats()` / `reset_stats()` read and reset it.

### 2. Specialization dedup (`zapc_comptime::SpecializationCache`)

Models mono's global-recv dedup (#671) for the comptime generator layer: a
`(type, backend)` specialization is emitted **once**, lazily, per **used** pair —
not per call-site. `record(type, backend)` returns `true` the first time (emit)
and `false` thereafter (reuse the symbol); distinct backends are distinct
instantiations. The actual symbol emission is wired when the generator lands
(E3); the dedup discipline is host-tested now.

### 3. Compile-time metric (the CI regression signal)

`MemoStats` on a representative reflection-heavy program is a **deterministic,
comparable** compile-time metric: `computed` equals the number of distinct types
regardless of how many times each is introspected, so a lost-memo regression
blows it up and fails the test. This is a stable signal that needs no LLVM and
does not flake on timing (unlike a wall-clock bench). A harness test asserts and
prints it (`comptime-reflection-metric computed=… reused=… total=…`), so it runs
in the existing host CI.

## Scope deferred (owned here)

- **On-disk / cross-build cache** of `type_info` is deferred (v1 is in-process).
- **A dedicated wall-time CI benchmark job** is follow-up repo-infra; the
  deterministic computation-count metric is the regression signal provided now
  (no new CI workflow, no LLVM, no timing flakiness).
- The `SpecializationCache` is the comptime-generator dedup *model*; its wiring
  into mono's emission lands with the generator (E3), alongside the surface.

## Consequences

No behavior change for existing programs: compiler-internal, host-tested
(`crates/comptime/tests/memoization.rs`). The memo makes repeated introspection
of the same type O(1) after the first, and the metric guards against silent
build-time blow-up.
