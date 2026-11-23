---
title: The native `@test` harness lowers with sema's inferred types
status: Locked
issue: 1308
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# The native `@test` harness lowers with sema's inferred types

**Status:** Locked

## Decision

`zap test` now threads the sema `TypeMap` returned by the front end into the
`@test` harness's `build_executable` call, instead of lowering the harness with
an empty `TypeMap::default()`. Monomorphization consumes that map to pin a
generic argument its own best-effort inferer cannot derive (#1167), so a test
body lowers exactly as the identical program does under `zap run` / `zap build`.

This closes two `zap test`-only `ZAP6001` failures that did not occur in ordinary
compilation:

- **#1308** — `Optional.Some(int64(0))` (an explicit integer conversion, standalone
  or through a `value=` keyword argument): "cannot infer type argument 'T' of
  generic 'Optional'".
- **#1310** — a generic reflected operator over a built-in / seam result
  (`len(xs) + Delta(1)`, `date + Delta(1)`): "cannot infer type argument 'T' of
  operator method '__radd__'".

## Rationale

The native runner builds the harness as an AST→AST transform and then lowered it
with `TypeMap::default()`, on the stated assumption that the transformed module's
spans "would not key into the original `TypeMap`". That assumption was wrong for
the part that matters: `build_harness` **clones** the user's test / hook / helper
function bodies verbatim (it only sheds their decorators and replaces `main`), so
every user expression keeps its original span and still keys into the map sema
recorded. Only the harness-synthesized `main` and the per-test wrapper statements
carry fresh spans — and those have no entries in the map, so they fall back to
the monomorphizer's own inference exactly as before. Threading the real map is
therefore a pure augmentation: the user-body expressions gain their sema types,
the synthesized scaffolding is unaffected.

The two bugs are the native-test face of the #1167 gap: ordinary compilation
already threads the map (that is what let `zap run` / `zap ir` succeed on the same
source), so the fix is to make the test runner do what every other front-end
consumer already does, not to teach the monomorphizer a new inference rule. The
fail-closed boundary #1167/#1190 established (a synthetic, unspellable iterator
type is left `Unknown` and the generic rejected cleanly, never bridged into a
`$`-sigil instance) is inherited unchanged, because the harness now uses the very
same `lower_module_opts_with_types` path.

## Consequences

- A `@test` body may use any form whose generic argument type sema infers but the
  monomorphizer alone cannot — explicit conversions, built-in / stdlib-seam call
  results, and generic reflected operators over them — and it lowers under
  `zap test` just as under `zap run`. The zero-timestamp `Optional[int64]` the
  otel downstream needs (zaplang-io/otel#15) and the Date/DateTime reflected
  operators datetime-utils#22 needs both pass.
- No change to test discovery, the synthesized harness, the stdout sentinel
  protocol, or reporting: only the `TypeMap` argument to the harness build
  changed. `front_end_test_module` now returns the module paired with its types.
- No new ZIR variant / GC-rooting / runtime change: the fix is confined to the
  driver's test runner wiring (`crates/driver/src/testing/mod.rs`).
- Out of scope / unchanged: the `$`-synthetic-iterator rejection boundary and the
  monomorphizer's own inferer, both untouched.

<!-- Related: the sema-TypeMap backfill this reuses is
     decisions/lang/1169-mono-consumes-sema-types.md; the harness transform is in
     crates/driver/src/testing/harness.rs (`build_harness`). The interface-return
     sibling bug is decisions/lang/1309-interface-signature-monomorphization.md. -->
