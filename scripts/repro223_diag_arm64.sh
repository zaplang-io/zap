#!/usr/bin/env bash
# I#223 reproduce + diagnose under emulated aarch64 (cloud session, x86 + qemu).
# Builds the RESUME-branch zapc (gc-immix; carries the #223 diagnostic knobs) and
# runs the decisive experiments. The key datum the issue never reported:
# ZAP_GC_VERIFY_ROOTS' `in_shadow_chain=YES/NO` for the unmarked live list.
set -uxo pipefail
SP=/tmp/claude-0/-home-user-zap/51de7eec-4f39-5be1-adc4-9ecf20680e51/scratchpad
SRC=/home/user/zap-223        # resume-branch worktree (has diagnostics + harness)

# 1. arm64 dev image (resumes from cached layers).
docker build --platform linux/arm64 -t zap-dev-arm64 -f /home/user/zap/.devcontainer/Dockerfile /home/user/zap/.devcontainer 2>&1 | tail -8

# 2. Build zapc + run the diagnostic matrix inside the arm64 container.
docker run --rm --platform linux/arm64 -m 8g \
  -v "$SRC":/work \
  -v zap-target-arm64:/work/target-arm64 \
  -v zap-cargo-registry-arm64:/root/.cargo/registry \
  -w /work -e CARGO_TARGET_DIR=/work/target-arm64 \
  zap-dev-arm64 \
  bash -c '
    set -eux
    echo "GUEST arch: $(uname -m)"
    cargo build -p zapc --features zapc/llvm,zapc/gc-immix 2>&1 | tail -25
    ZAPC=$(find /work/target-arm64 -name zapc -type f -path "*/debug/*" | head -1)
    echo "ZAPC=$ZAPC"
    BIN=/tmp/amp_evac
    "$ZAPC" build /work/tests/repro223/amp_evac.zap -o "$BIN" 2>&1 | tail -5
    EXP=159976000

    run_cell () { # label  env...  -> prints ok/wrong/crash over $N runs
      local label="$1"; shift
      local n="${N:-20}" p="${P:-4}" ok=0 wrong=0 crash=0 out rc
      for i in $(seq 1 "$n"); do
        out=$(env "$@" MAXPROCS="$p" timeout -s KILL 40 "$BIN" 2>/dev/null); rc=$?
        if [ "$rc" = 137 ] || [ "$rc" = 139 ] || [ "$rc" = 134 ] || [ "$rc" = 124 ]; then crash=$((crash+1));
        elif [ "$out" != "$EXP" ]; then wrong=$((wrong+1)); else ok=$((ok+1)); fi
      done
      echo "CELL $label (MAXPROCS=$p N=$n): ok=$ok wrong=$wrong crash=$crash"
    }

    echo "===== A. baseline repro (default, preempt on) ====="
    N=30 P=4 run_cell "default-mp4"
    N=20 P=2 run_cell "default-mp2"
    N=20 P=1 run_cell "default-mp1"

    echo "===== B. UAF control: NO_RECLAIM (expect 0 fail) ====="
    N=20 P=4 run_cell "no-reclaim" ZAP_GC_NO_RECLAIM=1

    echo "===== C. base race w/ preempt OFF ====="
    N=30 P=4 run_cell "no-preempt" ZAP_NO_PREEMPT=1

    echo "===== D. DECISIVE: VERIFY_ROOTS -> in_shadow_chain? ====="
    # Capture stderr; loop until we catch VERIFY lines (or 40 runs).
    : > /tmp/verify.log
    for i in $(seq 1 40); do
      MAXPROCS=4 ZAP_GC_VERIFY_ROOTS=1 timeout -s KILL 40 "$BIN" >/dev/null 2>>/tmp/verify.log || true
      if grep -q "VERIFY: UNMARKED" /tmp/verify.log; then echo "caught VERIFY on run $i"; break; fi
    done
    echo "--- VERIFY lines (unique) ---"
    grep "VERIFY: UNMARKED\|MARK-RACE\|MARK-ACTIVE\|STALE-ROOTS" /tmp/verify.log | sort | uniq -c | sort -rn | head -40
    echo "--- in_shadow_chain tally ---"
    grep -o "in_shadow_chain=[A-Z]*" /tmp/verify.log | sort | uniq -c
    cp /tmp/verify.log /work/target-arm64/verify.log 2>/dev/null || true
  ' 2>&1 | tee "$SP/repro223_diag.log"

echo "DIAG SCRIPT DONE rc=$?"
