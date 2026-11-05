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
| `racy_counter.zap`     | plain `int64` module global | **none** | **RACE reported** (names `counter`, with location; non-zero exit) |
| `racy_read_write.zap`  | plain `int64` module global | **none** (one coroutine writes, another reads) | **RACE reported** on `counter` (read/write race) |
| `racy_write_write.zap` | plain `int64` module global | **none** (two coroutines write) | **RACE reported** on `counter` (write/write race) |
| `clean_mutex.zap`      | plain `int64` module global | shared `Mutex` | clean (lint still warns; detector precise → silent; prints `100000`) |
| `clean_atomic.zap`     | `Atomic[int64]` cell        | atomic ops | clean (shareable type → not instrumented; prints `100000`) |
| `clean_single.zap`     | plain `int64` module global | only one coroutine writes it | clean (no concurrent access — guards against false positives; prints `50000`) |

`racy_counter.zap` + `clean_mutex.zap`/`clean_atomic.zap` together are the
issue's **"Done when (v1)"** pair: the racy program is reported with a location,
and a correctly `Mutex`/`Atomic`-guarded version is clean. The `racy_read_write`
and `racy_write_write` variants widen the walled garden to both conflict kinds
(a write concurrent with a read, and two concurrent writes), and
`clean_single.zap` pins the no-false-positive guarantee.

The verdict is **schedule-independent**: a race is reported because there is no
happens-before edge ordering the two accesses, not because a particular run
happened to interleave them — so the report is a stable golden regardless of
`MAXPROCS` or the collector.

## Status (delivery is sliced — see D-0114)

- **v1:** the `--race` flag surface, the design decision, and these fixtures
  landed. The flag is plumbed on `run`/`build`/`ir`/`test`.
- **slice 1 — #677 (landed):** access instrumentation in lowering. Behind
  `--race`, every ZAP5125 shared-global access emits a `__zap_race_read` /
  `__zap_race_write` hook, calling runtime **no-op stubs** (`zaprace.inc.c`). The
  flag is no longer inert at the compiler level (the accepted-but-inert notice is
  gone).
- **slice 2 — #678 (this slice):** the **happens-before / vector-clock engine**
  replaces the stubs (`zaprace.inc.c`). Each coroutine carries a vector clock,
  each `Mutex` a release/acquire clock, and each instrumented slot shadow state
  (last write / read clocks). Happens-before edges are wired at **spawn**
  (parent → child) and **`Mutex` lock/unlock** (acquire / release); `Atomic` is
  clean by exemption (a `@shareable` cell is not instrumented). On a conflicting
  access with no ordering edge the engine prints a report naming the global and
  both accesses' source spans and exits non-zero (66). All six fixtures are now
  CI-gated goldens (`race_detector_reports_races_and_clears_clean_programs` in
  `crates/driver/tests/golden.rs`), run under both collectors: the three `racy_*`
  fixtures assert a reported race on `counter`, the three `clean_*` a silent run
  with the exact total. A single global detector lock serializes all engine
  state (obvious correctness over speed; safe under `MAXPROCS>1`).
- **deferred — slice 3 (tracked under #665 / #679):** breadth — channel /
  `WaitGroup` / `Once` happens-before edges, tracking the main/native thread's
  own accesses, shared heap-object instrumentation, richer multi-frame reports,
  and `MAXPROCS>1` validation. (Because the main thread's accesses are not yet
  tracked, the `WaitGroup`-joined `main` read in the `clean_*` fixtures stays
  clean without a join edge.)

Runtime goldens run only in CI (local Docker/LLVM are unavailable in this
workspace), so the detector slices are validated there.
