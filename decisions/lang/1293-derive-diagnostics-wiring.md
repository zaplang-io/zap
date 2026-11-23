---
title: "@derive failures render as consumer ZAP7003 diagnostics (E3d)"
status: Accepted
issue: 1293
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# `@derive` failure diagnostics (E3d)

**Status:** Accepted — the fourth slice of E3 (#1293, epic #1289), and the one
that makes a failing `@derive` an average-programmer-grade compile error. It
consumes #1256 (the `DeriveBacktrace` / `render_derive_error` / `render_ct_error`
model, host-tested there over constructed inputs) and #1330 (which made a library
`comptime.error` reachable from a `.zap` generator, so the failure now actually
occurs in the live descent).

## Context

Before this slice, a comptime failure during a derive generator's `comptime for`
descent was flattened to a plain `LowerError { code: ZAP6001, message }` — the
message was the author's, but the error had no span, no consumer framing, and no
backtrace. The #1256 renderers that produce the consumer `ZAP7003` form were
called only in their own crate's tests. The failure's span was, however, already
correct: a `comptime.error(f.span, …)` carries the user's field span (span
fidelity through #1251/#1252/#1291).

## Decision

Carry the comptime failure up to the driver — where the entry source is
available — and render it through the #1256 renderers.

- **mono** stops flattening the `CtError` to a bare message. At the three unroll
  flatten sites (`comptime_for.rs`) it builds a `LowerError` whose new optional
  `derive: Option<Box<DeriveErrorInfo>>` payload carries the cause (message,
  span, user-abort vs engine-fault) as plain data (no `zapc_comptime` types cross
  the boundary). The code is routed by cause kind (`ZAP7002` user abort /
  `ZAP7001` engine fault).
- mono recovers the **derive context**: it records each `@derivable` generator's
  trait (name → trait), and while specializing such a generator at a concrete
  type (`emit_func_instance`) it sets a `cur_derive = (trait, type)`. When a
  failure surfaces from that body's unroll, it upgrades the payload with the
  trait and type (→ `ZAP7003`) and adds a `required because of field \`f\` of
  \`T\`` backtrace frame, matching the offending field by the cause span.
- the **driver** reconstructs a `CtError`/`DeriveBacktrace` from the payload and
  calls `render_derive_error` (consumer `ZAP7003` at the user's field, backtrace,
  root cause) — or `render_ct_error` for a comptime failure outside a derive —
  with the entry source, so the diagnostic carries `file:line:col` and a caret
  snippet. An ordinary lowering error is unchanged (location-free header).

## Result

```
error[ZAP7003]: cannot derive `Ser` for `User`
 --> user.zap:16:5
     Widget w
     ^~~~~~~~
 = note: required because of field `w` of `User`
 = note: the generator reported: `Ser` is not implemented for this field's type
```

The header names the user's **type** and points at the user's **field**, never a
generator symbol or evaluator internal; the library's own `comptime.error`
message is the root cause; and the compile fails with a diagnostic, never a
panic. Verified host-side (`zap ir`, no LLVM): `crates/driver/tests/zap_cli.rs`
(the example above, asserting no `_derive_ser`/mangled symbol leaks) and a
`render_derive_failure` unit test (`src/lib.rs`, the consumer and plain forms). A
valid derive emits no diagnostic (`tests/golden/comptime_kind_guard.zap`).

## Scope / deferred

- The **single** inferred frame (the offending field) is emitted. A multi-level
  `required because of …` chain for a nested container (`list[Widget]` element)
  needs the kind-switch recursion to push a frame per level — a refinement; the
  renderer (#1256) already supports an N-frame backtrace.
- The **dual-span generator-fault** form (`render_generator_fault`, generator
  line + "instantiated here") is not yet selected: an engine fault under a derive
  currently renders through `render_derive_error`/`render_ct_error` at the cause
  span (no panic, correct code). Wiring the dual span needs the generator +
  instantiation spans threaded into the payload — a follow-up.
- The rich render is on the front-end path (`zap ir`, analysis/LSP, `zap check`).
  The LLVM `build`/`run` path surfaces the author's message via the lowering
  error's text (no panic); threading the structured payload through
  `zapc_codegen::build_executable` is a follow-up.
