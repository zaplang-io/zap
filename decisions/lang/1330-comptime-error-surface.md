---
title: "comptime.error + comptime-constant if resolve through the .zap surface (unblocks E3d)"
status: Accepted
issue: 1330
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# `comptime.error` + comptime-`if` through the surface

**Status:** Accepted. The comptime evaluator (E2, #1249/#1251/#1253) supported a
library-author abort (`comptime.error`), a `.kind` switch and container recursion,
and the S1/S2 surface (#1296) wired `comptime for` + `reflect.*`. But a generator
body that did anything *conditional* was rejected before it ran, so no failing or
branching derive was reachable from a real `.zap` program — which blocked **E3d**
(#1293, derive diagnostics) since its trigger did not exist. This wires the
missing surface.

## Context (what was broken)

- `comptime.error(f.span, msg)` in a generator body died in **sema** with
  `ZAP3001: unknown name 'comptime'` — `comptime` is not a value, so ordinary
  name resolution rejected it (the `reflect.*` builtins were already
  short-circuited; `comptime.error` was not).
- Even past sema, the unroll engine's `specialize_expr` evaluated each body
  expression and **swallowed** the result's `Err` (by design — most body exprs
  are not comptime-evaluable and stay as runtime code). A `comptime.error`'s
  abort is *always* an `Err`, so it was swallowed and the call fell through to
  runtime emission, where `comptime` is an unknown variable (`ZAP6001`).
- A comptime-constant `if` in a `comptime for` body was **not resolved**: both
  arms were specialized and emitted, so a `comptime.error` in *either* arm fired
  regardless of the condition — a guarded abort was impossible.
- The evaluator that unrolls a derive had **no `TypeRegistry`**, so `f.ty.kind`
  (which must resolve a field's declared type) could not evaluate — a kind-based
  guard silently failed.

## Decision

Wire the surface so a generator can inspect fields and conditionally abort,
reusing the existing engine primitives (no new ones):

1. **sema** (`check_call`): short-circuit `comptime.error(...)` as a comptime call
   of statically-unknown type, exactly like `reflect.*` — never name-resolve the
   `comptime` base. Its arguments (a `Span` such as `f.span`, a `str` message) are
   not inferred.
2. **unroll engine** (`specialize_expr`): a `comptime.error(...)` call *propagates*
   its `CtError` instead of being swallowed — that abort is the intended compile
   error.
3. **unroll engine** (`specialize_block`): a comptime-constant `if`/`elif`/`else`
   is resolved at specialization time — only the taken branch's statements are
   kept; the dead arms (and any `comptime.error` in them) are dropped. An `if`
   whose condition is not a comptime boolean is left as an ordinary runtime `if`.
4. **mono**: the `comptime for` evaluator is built with the module's
   `TypeRegistry` (cached on the pass), so `f.ty.kind` / container recursion
   resolve user types — enabling a kind guard such as
   `if f.ty.kind.name == "struct": comptime.error(f.span, …)`.

## Consequences

A library derive generator can now guard on a field's comptime kind (via the
`.kind.name` string) and abort with its own message, reachable end-to-end from a
`.zap` program. Verified host-side (`zap ir`, no LLVM): `Outer` with a nested
`Inner` field aborts with the author's message; `Flat` (primitives) compiles and
runs. Tests: engine if-pruning + `comptime.error` propagation
(`crates/comptime/tests/unroll.rs`), CLI abort/compile cases
(`crates/driver/tests/zap_cli.rs`), and the LLVM golden
`tests/golden/comptime_kind_guard.zap`. No change to a program that uses no
comptime surface.

This unblocks **E3d** (#1293): the `CtError` a `comptime.error` now produces is
the input E3d routes through #1256's `render_derive_error` (ZAP7003 at the user's
field, with a backtrace); today it still surfaces as a plain lowering error
(`ZAP6001`, no span) — the diagnostic **quality** is E3d's charter, not this
slice's.

## Deferred

- A `comptime <name> = <expr>` **binding** in a body is still rejected
  (`ZAP3001`); a generator inlines the expression into the guard instead. A
  separate surface piece.
- The **bare kind constants** the evaluator seeds (`Struct`, `List`, …, via
  `seed_kind_constants`) are *not* exposed as `.zap` names — they would clash with
  ordinary type names (`List`, `Dict`). A `.kind.name` **string** compare is used
  instead; a namespaced constant surface (e.g. `Kind.Struct`) needs its own
  naming decision.
