# `tests/race/` — walled-garden fixtures for the runtime data-race detector

These fixtures are the behavioural specification for `zap run/test --race`, the
runtime data-race detector of the light concurrency model (**#665, D-0114**;
epic **#661**). They are the "intent documentation in code" for the detector:
each file states, in its header, exactly what the detector must do with it.

## The detector in one paragraph

The light concurrency model (#661) leaves shared mutable **module globals**
unsynchronized. Two diagnostics keep it honest, neither of which changes what
the language accepts:

- the **static** sharing lint `ZAP5125` (#664, D-0113) — type-based, warns at
  every access of a mutable non-`@shareable` module global; and
- the **dynamic** `--race` detector (#665, D-0114) — a **Zap-native
  happens-before / vector-clock** detector (not LLVM/Clang ThreadSanitizer; see
  D-0114 for why) that observes an actual run and reports a true data race —
  two accesses to the same shared location, at least one a write, with no
  happens-before edge between them — with a source location.

`--race` is a **test-time diagnostic**, off by default, and instruments exactly
the ZAP5125 set (a mutable, non-`@shareable` module global; scalar or
reference). An `Atomic`/`Mutex`/`@shareable`-guarded version is therefore clean.

## Fixtures

| File | Shared state | Synchronization | `--race` verdict |
|------|--------------|-----------------|------------------|
| `racy_counter.zap`  | plain `int64` module global | **none** | **RACE reported** (names `counter`, with location; non-zero exit) |
| `clean_mutex.zap`   | plain `int64` module global | shared `Mutex` | clean (lint still warns; detector precise → silent) |
| `clean_atomic.zap`  | `Atomic[int64]` cell        | atomic ops | clean (shareable type → not instrumented) |
| `clean_single.zap`  | plain `int64` module global | only one coroutine writes it | clean (no concurrent access — guards against false positives) |

`racy_counter.zap` + `clean_mutex.zap`/`clean_atomic.zap` together are the
issue's **"Done when (v1)"** pair: the racy program is reported with a location,
and a correctly `Mutex`/`Atomic`-guarded version is clean.

## Status (delivery is sliced — see D-0114)

- **v1:** the `--race` flag surface, the design decision, and these fixtures
  landed. The flag is plumbed on `run`/`build`/`ir`/`test`.
- **slice 1 — #677 (landed):** access instrumentation in lowering. Behind
  `--race`, every ZAP5125 shared-global access emits a `__zap_race_read` /
  `__zap_race_write` hook, calling runtime **no-op stubs** (`zaprace.inc.c`). The
  flag is no longer inert at the compiler level (the accepted-but-inert notice is
  gone); a `--race` build of these fixtures links and runs identically to a
  non-`--race` run, which a CI-gated golden now asserts
  (`race_builds_run_identically_to_non_race` in `crates/driver/tests/golden.rs`).
- **deferred slices (tracked under #665):** (2) the happens-before engine +
  report + non-zero exit, at which point `racy_counter.zap` becomes a CI-gated
  golden (its report asserted) and the three `clean_*` fixtures assert a clean
  run (#678); (3) breadth (channels / `WaitGroup` / `Once` edges, shared heap
  objects, `MAXPROCS>1`).

Runtime goldens run only in CI (local Docker/LLVM are unavailable in this
workspace), so the detector slices are validated there.
