---
title: Callable module-level constants dispatch to __call__ through the method path
status: Locked
issue: 1228
pr:
date: 2026-11-21
domain: lang
supersedes:
superseded-by:
---

# Callable module-level constants dispatch to `__call__` through the method path

**Status:** Locked

## Decision

Calling a module-level global (a `const`, or any module global) whose type
defines `__call__` — `MO(args)`, or `mod.MO(args)` for an imported constant —
now lowers to `MO.__call__(args)` (D-0047, #52), exactly like the local-value
form `f(args)`. Both the local-value and the module-global callable sugars are
routed through the resolved-method lowering path (`call_resolved_method`), so an
omitted default argument and keyword / out-of-order arguments are filled the
same way the explicit `MO.__call__(args)` call fills them. The plain-value use
of the constant (`MO.day`) and ordinary function calls are unchanged.

## Rationale

IR lowering's bare-name call path recognized a callable struct held in a
**local** variable but not a **module global**: after the `lookup` checks failed
it fell straight through to `ZAP6001 "call to unknown function 'MO'"`, even
though reading the same global as a value (`MO.day`) already resolved through the
global registry. An imported `mod.MO(args)` is rewritten by modlink to the same
flat mangled global name, so it hit the identical dead end.

- **Resolve the global, then dispatch to `__call__`.** The value-read path
  already turns a module-global name into its slot via `ctx.global`; the call
  path now does the same when the global is a struct type and dispatches to its
  `__call__`. Because modlink flattens `mod.MO` to `<prefix>MO`, the imported
  case needs no separate handling — it resolves through this one path.
- **Route the callable sugar through the method path, not `call_dunder`.**
  `call_dunder` coerces arguments positionally and does **not** fill omitted
  defaults, so the dateutil-style `MO()` (relying on `n = 1`) passed too few
  arguments — a mismatch only the native backend caught. The resolved-method
  path (`call_resolved_method`) fills defaults and accepts keyword / out-of-order
  arguments, which is what a call expression must do. Both the local-value form
  and the new module-global form use it, so `MO()` and `mo()` behave identically
  and match the explicit `MO.__call__()` form.
- **Local shadowing and existing calls are preserved.** The global check runs
  after the local `lookup` checks, so a local of the same name still shadows the
  global, matching the `Expr::Name` value-read order; first-class function
  values and ordinary function calls are untouched.

Alternatives rejected:

- **Keep `call_dunder` for both forms and ignore defaults.** `MO()` is the
  canonical dateutil weekday usage; shipping callable constants that cannot rely
  on a default argument would be a half-fix, and would leave the local-value
  form (the documented baseline) silently broken on the same input.
- **Handle only the module-global form, leave the local form on `call_dunder`.**
  That would make `MO()` fill the default while the local `mo()` did not — an
  indefensible inconsistency between two spellings of one feature.

## Consequences

- The callable sugar (`f(args)` for a struct value or a struct-typed module
  global) now supports default, keyword, and out-of-order arguments; the
  existing `operator_index_call` golden passes explicit arguments, so its runtime
  output is unchanged.
- A module global of a non-callable type called as `P(5)` is rejected by sema
  with `ZAP4071` (the `Call` operator interface is unsatisfied), before IR
  lowering, so the diagnostic is a clean "operator '()'" error, not "unknown
  function". Wrong arity is a clean lowering error.
- The name span of the call callee is now threaded to the global dispatch so a
  `--race` instrumented access of a mutable callable global is attributed
  correctly (a `const` global emits no race hook).

See also #52 (operator overloading surface) and D-0047 (operator dunders);
module-level `const` is D-0112 / #658.
