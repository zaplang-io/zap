# I#223 — aarch64 multicore GC use-after-free: RESUME HERE

A fresh reader (human or session) should start from this file, not from zero. It
summarizes what is **established**, what is **refuted**, the **one tool needed
next**, and how to run the **regression harness** and every diagnostic knob.

> TL;DR: On aarch64 with ≥2 scheduler threads, a concurrent `gc.collect()` under
> coroutine load frees a **still-live** object → use-after-free → wrong sum or
> crash. It is **collector-agnostic** (both gc-immix AND gc-boehm fail), **not**
> evacuation, **not** the STW handshake, and **not** a data race ThreadSanitizer
> can see. It is a **logic race between correctly-synchronized operations**,
> exposed only by true multicore parallelism. The next step needs **interactive
> ptrace on real aarch64** (rr chaos-mode / gdb hardware-watchpoint) — which QEMU
> (no ptrace) and CI (non-interactive) cannot provide.

## Established facts (measured; real ARM CI + local QEMU-aarch64; x86 clean)
- **Use-after-free**: `ZAP_GC_NO_RECLAIM=1` (mark but never reuse freed memory) →
  0 failures. So a live object is freed and its memory reused.
- **A fully-FREE block is reused while holding a live object**: `ZAP_GC_NO_RECYCLE=1`
  (reuse only fully-free blocks, never hole-recycle) still fails → it is a missed
  *mark* of a live object, not a hole-reuse bug. Poison (`ZAP_GC_POISON_FREE=1`)
  shows the object was freed **and reallocated** (fault addrs are reused heap
  addresses, not the 0xAB poison) — classic UAF-via-realloc.
- **Needs true multicore**: MAXPROCS=1 → clean; MAXPROCS=2 → ~95% fail; MAXPROCS=4
  → ~97%. A collector on one core vs a mutator coroutine on another.
- **Both collectors**: gc-boehm amp 192/200 wrong on ARM (non-moving, conservative)
  — so the moving/evacuation collector (I#120) is exonerated.
- **Async preemption (I#172) AMPLIFIES ~10x**: `ZAP_NO_PREEMPT=1` drops the rate
  ~97% → ~8%, but does NOT close it — there is a base multicore race under plain
  `gc.collect()` without preemption.
- **aarch64 only**: x86-64 (native + container) = 0 failures over 120+ runs. QEMU
  reproduces (~97%) even though QEMU-user on an x86 host executes guest non-atomic
  ops with host TSO → **this is NOT a weak-memory ordering bug**; it is a
  thread-interleaving logic race tied to aarch64 execution/scheduling.
- Rate ≈ ~1% per collection; run-failure ≈ 1-(1-p)^(collections). The shipped
  `immix_evac_coro_stress` golden collects rarely (low rate → "mostly green" on
  main); `amp_evac` collects ~320×/run → near-certain.

## Refuted hypotheses (crash-safe detectors + TSan; see knobs below)
- NOT the STW handshake: no mutator runs/alloc/is-ACTIVE during mark
  (`ZAP_GC_MARK_RACE=1` → 0 hits), collector enumerates only valid shadow
  snapshots (STALE-ROOTS 0), no double-run (`ZAP_DBG_SCHED=1` → 0 hits). immix STW
  is a correct positive rendezvous.
- NOT a codegen rooting gap: boehm conservatively scans registers+stack, so an
  unrooted register/stack pointer would be covered — yet boehm fails.
- NOT a C-runtime data race: TSan (runtime built `-fsanitize=thread`, ~40 runs)
  flags ONLY documented-benign advisory races (monitor sampling of
  `schedtick`/`m->current`; the advisory `zap_gc_wanted`/`zap_collecting` flags;
  the `m->preempt` flag; exit join). NOTHING on ZapBlock/allocator/sweep/mark/
  evac/list — the per-P-bump-vs-sweep-occupancy hypothesis is refuted (those
  accesses run every collection; no race flagged → synchronized via `zap_stw_mu`).
  The bug never fires under TSan (timing window closed).

## The paradox (what a fresh investigator must resolve)
Roots valid + world provably stopped + marking single-threaded over a stable heap
⇒ deterministic correctness — yet corruption is multicore/timing-dependent and a
*reachable* object is freed. The relevant cross-M accesses are stw_mu-synchronized
(TSan confirms). So the wrong decision is produced by a specific multicore
*interleaving of correctly-locked operations*, not by an unsynchronized access.
Prime remaining suspects to probe with a watchpoint: the per-P allocation buffer
reset at STW (`zap_reset_alloc_bufs`) vs a migrated coroutine's in-flight
allocation; the block free/recycle list transitions across STW + migration; the
exact free→realloc of the victim list's data buffer.

## NEXT TOOL (the high-yield step this environment could not run)
Interactive ptrace on **real aarch64**:
1. `rr record --chaos ./amp_evac` on an aarch64 box, replay the failing run, set a
   hardware watchpoint on the victim list's `data`/header and walk back to the
   free that should not have happened.
2. Or run under gdb on real ARM; when it SIGSEGVs in `zap_list_push`, inspect the
   `ZapList` and the block's line_mark/live_lines history.
QEMU can't (gdb ptrace unimplemented under qemu-user; core dumps are the qemu
process, not loadable). CI is non-interactive. Needs an SSH-able ARM instance.

## Regression harness (one command; 0/N gate once fixed)
`tests/repro223/regress.sh` builds the amp repro with the just-built `zapc` and
loops it N× at MAXPROCS=2 and 4, reporting wrong/hang/crash counts. Run it for
whichever collector `zapc` was built with:
```
cargo build -p zapc --features zapc/llvm,zapc/gc-immix   # or zapc/gc-boehm
ZAPC=$(find target -name zapc -type f -path '*/debug/*' | head -1) \
  N=200 bash tests/repro223/regress.sh
```
Also wired as `make repro-223` (builds gc-immix in the container and loops).
Expected TODAY: aarch64 ~97% fail (preempt on) / ~8% (ZAP_NO_PREEMPT=1); x86 0%.
A fix must drive this to **0/N at MAXPROCS 2 AND 4 on BOTH collectors on real ARM**,
x86 unchanged (the I#167/#188 bar).

## QEMU-aarch64 local repro (billing-free, fast iteration)
```
docker build --platform linux/arm64 -t zap-dev-arm64 -f .devcontainer/Dockerfile .devcontainer
docker run --rm --platform linux/arm64 -m 8g --cpus 4 -v "$PWD":/work \
  -v zap-target-arm64:/work/target-arm64 -v zap-cargo-registry-arm64:/root/.cargo/registry \
  -w /work -e CARGO_TARGET_DIR=/work/target-arm64 zap-dev-arm64 \
  bash -c 'cargo build -p zapc --features zapc/llvm,zapc/gc-immix && \
    ZAPC=$(find /work/target-arm64 -name zapc -path "*/debug/*"|head -1) N=50 bash tests/repro223/regress.sh'
```
The zap runtime's own fault handler prints signal + faulting address + offending
coroutine + a symbolized backtrace to stderr (built with `-rdynamic`; add
`ZAP_CC_EXTRA="-g"` for line numbers). gdb cannot ptrace under qemu-user.

## ThreadSanitizer recipe (confirms no harmful C-runtime data race)
```
ZAP_CC_EXTRA="-fsanitize=thread -g -O1" $ZAPC build tests/repro223/amp_evac.zap -o amp_tsan
TSAN_OPTIONS="halt_on_error=0 history_size=7 suppressions=tsan.supp" MAXPROCS=4 ./amp_tsan
# tsan.supp: `race:zap_monitor_main` and `race:zap_safepoint` (both benign/advisory)
```
Runs on x86 (TSan is happens-before based, arch-independent). Bug does not fire
under TSan. Fiber annotations (`__tsan_switch_to_fiber`) would add rigor for
same-pthread coroutine switches but cannot catch this (cross-M + non-firing).

## Diagnostic knobs (all in crates/codegen/src/zapgc.inc.c + zaprt.c; env-gated)
- `ZAP_GC_NO_RECLAIM=1`  — mark but never reuse freed memory (UAF test; → clean).
- `ZAP_GC_NO_RECYCLE=1`  — reuse only fully-free blocks, no hole-recycle.
- `ZAP_GC_POISON_FREE=1` — 0xAB the free lines at sweep (reuse-of-live test).
- `ZAP_GC_EVAC_POISON=1` — clobber moved-from payload (evac stale-ref test).
- `ZAP_GC_MARK_RACE=1`   — log a mutator running/allocating OR any ACTIVE mutator
  during the collector's mark phase; also STALE-ROOTS (enumerating an invalid
  shadow snapshot). (Sets stderr unbuffered so detector output survives a crash.)
- `ZAP_GC_VERIFY_ROOTS=1`— conservative green-stack scan for unmarked live lists.
- `ZAP_DBG_SCHED=1`      — DOUBLE-RUN detector (same coro on two Ms).
- `ZAP_NO_PREEMPT=1`     — disable the async-preempt monitor (amplifier off → ~8%).
- `ZAP_GC_EVAC_STATS=1`  — per-cycle evacuation move counts.
Diagnostic-only G fields (`dbg_running`, `shadow_valid`) back the last two.

## Stopgap MITIGATION (prepared, NOT a fix, NOT PR'd)
`ZAP_NO_PREEMPT=1` at runtime, or build with `-DZAP_MITIGATE_223_PREEMPT_OFF`
(see the monitor-start gate in zaprt.c), disables async preemption. Honest caveats:
it only drops aarch64 failures ~97% → ~8% (**still ~1-in-12 UAF — NOT shippable as
a 1.0 answer**) and reintroduces the I#172 preemption-starvation risk on ARM
(a CPU-bound coroutine can monopolize its M). Use only to de-risk experiments.
