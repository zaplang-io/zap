---
title: The zap test harness keeps module-level const/mutable bindings
status: Accepted
issue: 1236
pr:
date: 2026-11-21
domain: tooling
supersedes:
superseded-by:
---

# The zap test harness keeps module-level const/mutable bindings

**Status:** Accepted

## Decision

The `@test` harness transform (`testing::harness::build_harness`) **carries
module-level single-name bindings (D-0112) into the harness module** — `const T
NAME = expr` (#658) and mutable `T NAME = expr` (#662) — in their original
source position, instead of dropping every top-level statement. Every other
top-level statement (a bare executable statement, a module docstring) is still
dropped. A `@test`, a function it calls, or a default argument may therefore
read a module constant, exactly as under `zap run` / `zap build`.

## Rationale

`build_harness` rebuilt the test module by keeping declarations and a synthesized
`main`, and discarding `Item::Stmt(_)` as "a script's main body". But a
module-level `const` / mutable binding is an `Item::Stmt(Stmt::Let { .. })`, not
a bare statement: it is persistent module state that lowers to a module global
initialized once before `main`. Dropping it meant the harness module no longer
declared the global, so lowering (AST→ZIR) — and then native codegen — reported
every reference to it as `ZAP6001: unknown variable`, even though sema had
accepted the original module. This blocked the native-test migration for
`zaplang-io/datetime-utils` (whose suites read top-level typed constants
`EASTER_JULIAN` / `EASTER_ORTHODOX` / `EASTER_WESTERN` and `UTC`), the same way
#1232 blocked imported-symbol resolution. #1232 fixed the sema/import side of
`run_file`; this is its codegen/preparation counterpart in the same path.

Keeping only `Stmt::Let` (not all statements) is the minimal, general fix: it
mirrors what `modlink`'s `check_toplevel_statements` already treats as a legal
module-scope declaration, so the harness and a normal build agree on what module
scope contains. Preserving the bindings' source order preserves their
initialization order (a later const may read an earlier one). No special-casing
of names or types: a scalar, a value struct with a reference field (the
timezone/Easter shape), a function reading a const, and a default argument
referencing one all work through the one change.

Alternatives rejected:

- **Keep dropping statements and re-synthesize globals from the const list.**
  Reconstructs what the module already carries and would need its own init-order
  logic — the bindings already encode both.
- **Lower the whole original module instead of the harness transform.** The
  harness must still drop the user `main`, strip test decorators, and add the
  driver `main`; the fix is one arm of that existing transform, not a new path.

## Consequences

- A `@test` reads module `const`/mutable bindings — scalar and struct/value,
  including a value struct with a reference field — and so do functions it calls
  and default arguments, with initialization order preserved.
- Native test discovery and reporting are unchanged (only non-`main` functions
  are counted/driven); ordinary `zap run` / `zap build` behavior is unchanged
  (`strip_test_items` still removes `@test` functions and leaves module bindings
  in place — a module const is not a test item).
- Host-verifiable: the failure surfaced at AST→ZIR lowering (`ZAP6001`), which
  runs without LLVM, so the carried-over binding is proven to lower host-side;
  the full native harness run is exercised in CI's LLVM matrix.

Builds on `decisions/tooling/1232-zap-test-module-resolution.md` (the import/
sema side of the same `run_file` path).
