---
title: One pid+counter temp-name helper per crate; test modules never read the clock
status: Locked
issue: 917
pr:
date: 2026-11-19
domain: tooling
supersedes:
superseded-by:
---

# One pid+counter temp-name helper per crate; test modules never read the clock

**Status:** Locked

## Decision

Temporary paths are named from the **process id plus a process-global
`AtomicU64` counter, and never from a clock reading**. The driver crate's test
modules route every fixture-temp allocation through a single crate-wide helper,
`crate::testutil` (`unique_temp_path` / `unique_temp_dir`); the MCP module's
former `unique_temp_dir` now delegates to it. Production code that names a
throwaway path (`zap test`'s per-file test binary; the codegen runtime-bitcode
and runtime-object caches' pre-publish temps) uses the same pid+counter shape
inline, mirroring the existing `driver::unique_run_dir`.

## Rationale

A `SystemTime::now()…as_nanos()` nonce is not unique. On a microsecond-quantised
clock (macOS) the nanosecond value always ends `…000`, so two callers reading
the clock in the same tick get the same name. The process id does not save
callers inside one process — every thread shares it — so same-tick collisions
are routine, producing either silent fixture-stomping flakes (the test sites,
I#180 / I#735) or, on production paths, two builds clobbering one throwaway
binary / racing one cache publish (I#917).

The counter removes the only colliding input: `fetch_add` hands every call a
distinct value with no clock involved, so uniqueness holds across threads and
repeated calls in one process, while the pid keeps it across concurrent
processes. `Relaxed` ordering is enough — distinctness, not inter-thread
ordering, is all that is required.

PR #916 fixed the MCP modules this way but left four sibling sites on the old
clock-based nonce. Those were fixed piecemeal once already (I#735 added pid +
counter but left a redundant clock reading in the name). Routing all of a
crate's test sites through **one** helper, rather than copying the idiom per
module, is what stops the clock-based form from creeping back into just one of
them — the recurrence I#917 exists to end.

Alternatives rejected:

- **Keep the clock but add pid + counter (belt and braces).** The clock reading
  is dead weight at best and the exact thing that regressed before; keeping it
  invites a future "simplification" back to clock-only. Drop it entirely.
- **A `tempfile` dev-dependency.** Two of the four sites ship in production, so
  a dev-only crate cannot cover them; a three-line `static AtomicU64` needs no
  dependency at all.
- **Per-module copies of the helper.** That is the status quo that let the bug
  survive in four places; a single helper is the point.

## Consequences

- Generated names keep their original prefixes (`zaptest-`, `zaprt-`,
  `zap-diag-`, `zap-modlink-`, `zap-mcp-…`), so existing greps and cleanup globs
  still match; only the trailing `<nanos>` component becomes `<seq>`.
- Each site carries a walled-garden guard: threads behind a `std::sync::Barrier`
  allocate many names at once and assert all distinct — deterministic pass for
  the counter, deterministic fail on any regression to clock-based naming.
- New driver test modules that need a temp path use `crate::testutil` rather
  than growing their own nonce.

Follow-up to I#180 / PR#916; see also `driver::unique_run_dir` (the pre-existing
production reference shape, I#634).
