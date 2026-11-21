---
title: zap test resolves imports through the same module pipeline as builds
status: Accepted
issue: 1232
pr:
date: 2026-11-21
domain: tooling
supersedes:
superseded-by:
---

# zap test resolves imports through the same module pipeline as builds

**Status:** Accepted

## Decision

`zap test` runs the **same whole-program front end** as `zap run` / `zap build`
before it builds a test harness: it links the import graph (the module linker,
I#283/D-0081), desugars decorators, and type-checks the *merged, qualified*
module — rather than parsing the single entry file and type-checking it in
isolation. An imported dependency installed in the project `zenv`
(`<zenv>/pkg/<name>/src/…`, resolved from `ZAP_ZENV`), a sibling source module,
and any qualified exception types used in a `@test` body are therefore in scope,
exactly as they are in a build. This realizes the long-standing contract in
SPEC §16 / D-0054 that **tests use the same module resolution as builds**.

Mechanically, the driver's build front end is split so the module-resolution +
type-check logic has one home: `front_end_reports` returns structured
diagnostics (merged module, sema type map, warnings) and is shared by the
printing build path (`front_end_program`) and the new `zap test` path
(`testing::front_end_test_module`). There is one resolver, not a test-only copy.

## Rationale

`testing::run_file` previously did `parse_str` + `zapc_sema::check` on the one
entry file. That path never ran the module linker, so a test file's
`import <dep>` resolved to nothing: sema then rejected the first use of the
import with `error[ZAP3001]: unknown name '<dep>'`, and qualified exception
types from the dependency (`except <dep>.SomeError`) were rejected the same way
— even though `zap run` compiled the identical program. The non-core
`zaplang-io/memcached` native-test convention surfaced this: its package checks
passed under `zap run` (after #1222) but its `zap test` run failed at name
resolution. Every official non-core package (dns, otel, oauth, …) uses
`zap test tests/` as its CI, so the whole native-test convention depended on
this working.

Routing through the existing linker — rather than bolting a second,
test-only import resolver onto the test harness — is the minimal, general fix:
it inherits local-module, dependency, `when target.*`, cycle, and
declaration-only rules already proven for builds, and it cannot drift from build
semantics because it *is* the build front end. The decorator-desugar and
behavioral-decorator passes run here too; they key off dotted
registering-decorator names and user `decorator` declarations respectively, so
the `@test` / `@before` / `@expect_raises` test decorators pass through
untouched for the harness to consume, and the `@test` functions survive the
front end (only a real `zap run` / `build` strips them, via `strip_test_items`).

Alternatives rejected:

- **A parallel resolver inside the test harness.** Duplicates I#283/D-0081 and
  guarantees eventual divergence from build resolution — the opposite of D-0054.
- **Resolve only dependencies, not sibling modules (or vice versa).** Both fail
  for the same reason (no linker); one code path fixes both plus `when target.*`
  and cycle detection, for free.
- **Rewrite decorator-argument expressions in the linker so
  `@expect_raises(<dep>.Err)` resolves too.** Out of scope and broader-risk: it
  would change decorator-argument handling program-wide for every build. The
  natural, fully-working way to assert a qualified exception in a test is a
  body-level `try` / `except <dep>.Err` / `raise <dep>.Err(...)`, which the
  linker already rewrites; `@expect_raises` with a *qualified* type remains a
  harness limitation tracked separately, not a #1232 regression.

## Consequences

- `zap test` now reports an unresolved module as the linker's
  `error[ZAP3010]` (module not found) — the same diagnostic `zap run` gives —
  instead of sema's downstream `ZAP3001`. A genuinely unknown *name* in a test
  body still reports `ZAP3001`. Both surface as a single failing file-level test
  result, so test discovery and reporting are unchanged.
- `when target.*` cfg blocks in a test file are now resolved against the host
  target (as in a plain build) before the harness is built; previously the
  single-file path skipped cfg resolution.
- Warnings are computed but not printed on the `zap test` path: the stdout
  sentinel protocol is the test report, and a warning is neither a test nor a
  failure. (On a hard error the build path now prints errors without the
  preceding warning batch; no test asserts the old warnings-then-error ordering.)
- The import-resolution fix is verified host-side (resolution and type-checking
  are front-end only, no LLVM); the full native harness run — a dependency
  installed from a registry included — is exercised in CI via the non-core
  packages' `zap test` suites. A registry-installed dependency resolves through
  the identical `<zenv>/pkg/<name>/src/…` layout as a `--path` one, so the
  local-path fixture covers the resolution path; the two differ only in how the
  package is provisioned into the `zenv`, which is not a front-end concern.

See `decisions/lang/1222-imported-generic-bound-name.md` (the #1222 fix whose
`zap run` package check this restores for `zap test`).
