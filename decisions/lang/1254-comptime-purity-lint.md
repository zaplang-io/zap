---
title: Comptime purity — leaf-table reject-lint + interpreter backstop + cap (E2)
status: Accepted
issue: 1254
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Comptime purity — leaf-table reject-lint + interpreter backstop + cap (E2)

**Status:** Accepted — records the *implemented shape* of issue #1254 (epic
#1249's locked enforcement design). Builds on the #1250 interpreter.

## Decision: two layers over one shared table

### The leaf table — one closed set (`zapc_comptime::leaf`)

A single `LeafKind` set — `Io`, `Ffi`, `Spawn`, `Channel`, `Time`, `Rng`,
`GlobalMutation`, `Nondet` — enumerates exactly the runtime-observable effects
the comptime fragment refuses (epic #1249, locked decision 3). `Nondet` is
`dict`/`set` iteration (unspecified seed-dependent order, #1066). **Allocation is
deliberately not tagged** — comptime legitimately builds strings/lists.
`classify_call(callee)` maps a bare impure builtin (`print`/`input`) or an impure
stdlib module function (`time.now`, `random.rand`, …) to its leaf kind; a pure or
unknown callee returns `None`. The v1 membership is a defensible closed set,
intentionally tunable (#1249).

Both enforcement layers consult this one table, so they can never disagree about
membership (the shared-table invariant).

### (a) Static reject-lint (`zapc_comptime::lint`)

`lint_comptime_callable(func, others)` scans a function used in a comptime
context for any **mention** of an impure leaf and rejects it at the definition
site. Locked properties:
- **Fail-closed / stricter:** the scan is a **join over every branch** —
  including a provably-dead `if False:` branch — so a function may not even
  *mention* I/O. Conditions are not evaluated.
- **Defer on the unresolved:** an unknown / dynamic callee (`classify_call` →
  `None`) is **not** rejected; the interpreter catches it. The lint only ever
  rejects, never accepts.
- **Best-effort transitive propagation (leaf-only first cut):** a call to a known
  comptime-callable function is followed into its body (cycle-guarded).
- `spawn`/channel/`select` and `dict`/`set`-literal iteration are detected
  structurally. `Ffi` and `GlobalMutation` are left to the interpreter in v1 (the
  lint cannot resolve them without types/scope) — consistent with "leaf-only
  acceptable first cut".

### (b) The interpreter is the soundness authority (#1250)

The bounded interpreter physically has no impure primitive, so reaching one
errors `… is not available at comptime`. It now consults the **same** leaf table
for calls, so `print(...)` / `time.now()` are refused with their leaf kind rather
than a generic "unknown function" — the two layers share membership. Everything
the lint defers is caught here.

### Cost cap (HQ5)

A **fixed** hard limit on comptime steps (`DEFAULT_STEP_CAP`) and call/recursion
depth (`DEFAULT_DEPTH_CAP`) with a clear `Budget` diagnostic on exceed — not a
user-tunable quota. (Overridable only in-process via `with_step_cap` /
`with_depth_cap` for testing; the shipped default is fixed.) The interpreter's
recursion ban and this depth cap together bound both recursive and deep-but-
finite comptime work; a runaway fails fast instead of hanging the compiler.

## Scope deferred (owned here)

- **No parser/sema surface** — the lint is a host-tested standalone pass; wiring
  it to run at a function's definition site when the function is used in a
  comptime context lands with the `comptime`/`reflect` surface follow-up, which
  is also when "used in a comptime context" becomes determinable.
- `Ffi` / `GlobalMutation` static detection, and type-aware `Nondet` detection
  (dict/set iteration over a typed variable, not just a literal), are left to the
  interpreter in v1 and can be deepened later.

## Consequences

No behavior change for existing programs: compiler-internal, host-tested
(`crates/comptime/tests/purity_lint.rs`). The lint moves common comptime-purity
errors to the definition site with a clean message; the interpreter remains the
sole soundness authority.
