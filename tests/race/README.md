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
| `racy_chan.zap`        | plain `int64` module global | **none** between the writers (each sends to `main` on a shared channel, unrelated to the race) | **RACE reported** on `counter` — a soundness guard that the channel edge does not fabricate a writer↔writer ordering |
| `clean_mutex.zap`      | plain `int64` module global | shared `Mutex` | clean (lint still warns; detector precise → silent; prints `100000`) |
| `clean_atomic.zap`     | `Atomic[int64]` cell        | atomic ops | clean (shareable type → not instrumented; prints `100000`) |
| `clean_single.zap`     | plain `int64` module global | only one coroutine writes it | clean (no concurrent access — guards against false positives; prints `50000`) |
| `clean_chan_rendezvous.zap` | plain `int64` module global | unbuffered **channel** send→recv (rendezvous) | clean (send happens-before recv orders the write before the read; prints `43`) |
| `clean_chan_buffered.zap`   | plain `int64` module global | **channel** cap-8 buffered hand-off (ring reused ~125×) | clean (write precedes every send, read follows every recv; prints `1098`) |
| `clean_waitgroup.zap`  | plain `int64` module global | **`WaitGroup`** `done`→`wait` (joiner is a coroutine) | clean (worker's write ordered before collector's read; prints `77`) |
| `clean_once.zap`       | plain `int64` module global | **`Once`** `once_end`→`once_begin` | clean (single init write ordered before every reader; prints `246`) |

`racy_counter.zap` + `clean_mutex.zap`/`clean_atomic.zap` together are the
issue's **"Done when (v1)"** pair: the racy program is reported with a location,
and a correctly `Mutex`/`Atomic`-guarded version is clean. The `racy_read_write`
and `racy_write_write` variants widen the walled garden to both conflict kinds
(a write concurrent with a read, and two concurrent writes), and
`clean_single.zap` pins the no-false-positive guarantee. The slice-3 `clean_*`
fixtures each put **both** the synchronizing write and read in spawned
coroutines, so each new happens-before edge is genuinely exercised (not masked by
the still-untracked `main`); `racy_chan.zap` is the matching soundness guard that
those edges stay precise and never hide a real race.

The verdict is **schedule-independent**: a race is reported because there is no
happens-before edge ordering the two accesses, not because a particular run
happened to interleave them — so the report is a stable golden regardless of
`MAXPROCS` or the collector. `race_detector_correct_under_parallelism` re-runs
the whole suite under `ZAP_MAXPROCS=4` to pin that down.

## Status (delivery is sliced — see D-0114)

- **v1:** the `--race` flag surface, the design decision, and these fixtures
  landed. The flag is plumbed on `run`/`build`/`ir`/`test`.
- **slice 1 — #677 (landed):** access instrumentation in lowering. Behind
  `--race`, every ZAP5125 shared-global access emits a `__zap_race_read` /
  `__zap_race_write` hook, calling runtime **no-op stubs** (`zaprace.inc.c`). The
  flag is no longer inert at the compiler level (the accepted-but-inert notice is
  gone).
- **slice 2 — #678:** the **happens-before / vector-clock engine** replaces the
  stubs (`zaprace.inc.c`). Each coroutine carries a vector clock, each `Mutex` a
  release/acquire clock, and each instrumented slot shadow state (last write /
  read clocks). Happens-before edges wired at **spawn** (parent → child) and
  **`Mutex` lock/unlock** (acquire / release); `Atomic` is clean by exemption. On
  a conflicting access with no ordering edge the engine prints a report naming
  the global and both accesses' source spans and exits non-zero (66). A single
  global detector lock serializes all engine state (obvious correctness over
  speed).
- **slice 3 — #679 (this slice):** breadth + hardening. New happens-before edges,
  all wired in the runtime (so a non-`--race` build stays byte-identical):
  **channels** — a send → the matching receive, for both a rendezvous and a
  buffered hand-off (keyed by the ring-buffer slot; the back-pressure edge is
  omitted, the safe direction); **`WaitGroup`** — every `done()` → a `wait()`
  that returns; **`Once`** — the winner's `once_end` → every other caller's
  `once_begin` return. The single-lock engine is validated under
  **`ZAP_MAXPROCS>1`** (true parallelism) across the whole suite
  (`race_detector_correct_under_parallelism`), and the report now also names each
  racing access's coroutine. `race_flag_is_honored_by_zapc_binary` is a host-safe
  guard that `zapc ir --race` really emits a hook (a tripwire for the #678
  dropped-flag silent no-op). **Still open** (tracked follow-ups): the
  **main/native thread's** own accesses stay untracked — a genuine
  main-vs-coroutine race is missed, and this is why the pre-slice-3
  `WaitGroup`-joined `main` reads stay clean without a join edge; **shared
  heap-object** instrumentation is split out as a **slice-4** phase of #665 (#684)
  (shadow state on heap objects + field/element lowering is a materially larger,
  GC-coupled change); and full **multi-frame stack traces** await the I#475
  green-stack unwinder.

Runtime goldens run only in CI (local Docker/LLVM are unavailable in this
workspace), so the detector slices are validated there.
