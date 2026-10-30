# I#223 — cloud-session findings: root cause localized

Debugged in a Claude Code **cloud session** (x86-64 host) under **emulated aarch64**
(`docker --platform linux/arm64` + qemu-user), i.e. the RESUME doc's "billing-free
QEMU-aarch64" path. This is the investigation that was PARKED pending "continuous
aarch64 infra / interactive ptrace"; the emulated repro + targeted runtime
instrumentation turned out to be enough to localize the bug without a watchpoint.

## 1. Reproduction matrix (emulated aarch64, gc-immix, `amp_evac`)

| cell | result |
|---|---|
| default MAXPROCS=4 | **30/30 fail** (wrong sum or SIGSEGV) |
| default MAXPROCS=2 | **20/20 fail** |
| default MAXPROCS=1 | 0/20 — all correct |
| `ZAP_GC_NO_RECLAIM=1` MP4 | 0/20 — all correct (→ confirms use-after-free) |
| `ZAP_GC_NO_EVAC=1` MP4 | still fails (→ evacuation exonerated, as before) |
| `ZAP_NO_PREEMPT=1` MP4 | ~1/30 (preemption amplifies ~30x, not the cause) |

Matches the issue's characterization exactly, at an even higher rate than CI.

## 2. The victim is `keep` (the long-lived accumulator), not the evac temporaries

`ZAP_GC_VERIFY_ROOTS` (scans each coroutine's green stack post-mark for unmarked
but live-looking lists) is noisy: on **correct MP1** runs it already flags
thousands of `len=3`/`len=8` lists — the per-iteration `garbage`/`g2` dead
temporaries. That noise is why `in_shadow_chain=NO` alone was inconclusive in the
original investigation.

The signal: on **buggy MP4** runs it *additionally* flags **large lists**
(`len=1710..2000`, `cap=2048`) — that is `keep` itself — as UNMARKED. MP1 never
flags a large list. So the real defect frees the live `keep`.

## 3. Mechanism: a live coroutine's shadow-stack head is zeroed → roots dropped

Dumping the victim coroutine's chain at mark time:

```
VERIFY: UNMARKED live list ... len=2000 cap=2048 in_shadow_chain=NO
VERIFY-BIG: coro=4 shadow_head=(nil) frames=0 roots=0 found_in_full_walk=0
```

`g->shadow_head == NULL` for a coroutine that is deep in execution (keep already
has ~2000 elements). The collector enumerates roots per coroutine by walking
`g->shadow_head`; a NULL head means **none** of that coroutine's roots are marked,
so `keep` (and everything it owns) is swept and its lines handed out again → UAF.

## 4. Origin: `zap_flush_shadow` zeroes a RUNNING, migrated coroutine's head

Targeted runtime instrumentation (patch: `223_diag.patch`, applied to the
investigation branch's `zaprt.c`/`zapgc.inc.c`), with a `dbg_ever_nonnull` flag so
we ignore the harmless "fresh coroutine yields before its first push" case:

- **`FLUSH-NULL-LIVE`** — `zap_flush_shadow()` writing `g->shadow_head = NULL` for a
  coroutine that previously had a real chain — fires **first and in bulk**
  (hundreds/run), always on **`state=1` (G_RUNNING)**. This is the seed event.
- `SAVE-NULL-LIVE` (switch_to_g save) and `COPREEMPT-NULLHEAD` and
  `RESTORE-NULL-LIVE` fire *after*, far fewer — they propagate the NULL once seeded.
- **`DOUBLE-RUN` = 0** — not a two-Ms-run-one-coroutine bug.
- Every `FLUSH-NULL-LIVE` victim has **run on ≥2 distinct Ms** (`distinct_Ms` =
  2/3/4, never 1). MP1 (single M, no migration) produces **zero** such events.

So: for the flush, the flushing thread has `m->current = <live, running G>` but
its per-thread `zap_shadow_head == NULL` — an inconsistent pair that only arises
once the coroutine has **migrated across Ms** (preempt-requeue to the global queue
+ work-stealing). The NULL head is then written into `g->shadow_head` and the
coroutine's entire root set is lost at the next collection.

## 5. Why it reproduces as "collector-agnostic" and "not weak-memory"

- The save/restore/flush of the coroutine root snapshot lives in the **shared M:N
  scheduler** (`switch_to_g` + `zap_flush_shadow`), which both collectors drive;
  for gc-immix it manifests as a NULL `shadow_head`, and the analogous cross-M
  root-visibility gap is why gc-boehm (which scans the green stack / saved context)
  also drops the object. The moving collector (I#120 evacuation) is not involved.
- It reproduces under qemu-user on an x86 host, so it is a **thread-interleaving
  logic race in the migration handoff**, not an aarch64 weak-memory-ordering bug.
  (It is aarch64-"only" in practice because the x86 scheduler/codegen timing never
  lands a cross-M flush in the vulnerable window; the logic defect is arch-neutral.)

This refutes the issue's earlier "NOT a codegen/root race; paradox unresolved"
framing: it **is** a precise-GC root-visibility failure, seeded by `zap_flush_shadow`
capturing a NULL `zap_shadow_head` for a live coroutine that migrated across Ms.

## 6. Next step toward a fix

Audit the invariant that `zap_shadow_head` (per-M `__thread`) stays consistent with
`m->current` across the preempt→requeue→steal→resume path, and that a RUNNING
coroutine's `g->shadow_head` is never captured as NULL while it holds live frames.
The fix is in the scheduler ↔ precise-GC handoff (`switch_to_g` save/restore and
`zap_flush_shadow`), NOT in the marker or the evacuator.

## How to reproduce (this session's harness)

```
bash scripts/repro223_diag_arm64.sh   # from a checkout with the #223 diagnostics
```
Builds a proxy-aware emulated-arm64 dev image, builds gc-immix `zapc`, and runs the
matrix + the `VERIFY_ROOTS` / `FLUSH-NULL-LIVE` captures. `223_diag.patch` (in the
session scratchpad) is the instrumentation used for §3–§4.
