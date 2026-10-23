#!/usr/bin/env bash
# TEMPORARY (#223): one zapc rebuild, then amp_evac under diagnostic arms to
# isolate the layer. Removed before the fix PR.
set -u
export LLVM_SYS_180_PREFIX=/usr/lib/llvm-18 LLVM_SYS_181_PREFIX=/usr/lib/llvm-18
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/work/target-arm64}"
export ZAP_CC_EXTRA="${ZAP_CC_EXTRA:--g}"
N="${N:-30}"; P="${MAXPROCS:-4}"; exp=159976000
FEATURES="${FEATURES:-zapc/llvm,zapc/gc-immix}"

echo "=== uname $(uname -m) · $FEATURES · building zapc ==="
cargo build -p zapc --features "$FEATURES" 2>&1 | tail -2 || exit 1
ZAPC=$(find "$CARGO_TARGET_DIR" -name zapc -type f -path '*/debug/*' | head -1)
"$ZAPC" build tests/repro223/amp_evac.zap -o /tmp/prog 2>&1 | tail -1 || exit 1

run_arm() {
  local label="$1"; shift
  local wrong=0 crash=0 ok=0 i out rc shown=0
  for i in $(seq 1 "$N"); do
    out=$(env "$@" MAXPROCS="$P" timeout -s KILL 90 /tmp/prog 2>/tmp/e >/tmp/o); rc=$?
    out=$(cat /tmp/o)
    if [ "$rc" != 0 ]; then
      crash=$((crash+1))
      if [ "$shown" = 0 ]; then shown=1; echo "  [$label] first crash rc=$rc:"; grep -E 'zap fatal|faulting|coroutine|push|worker' /tmp/e | head -6; fi
    elif [ "$out" != "$exp" ]; then wrong=$((wrong+1)); [ "$shown" = 0 ] && { shown=1; echo "  [$label] first wrong out=$out"; }
    else ok=$((ok+1)); fi
  done
  echo "ARM $label: ok=$ok wrong=$wrong crash=$crash / $N"
}

echo "================ $FEATURES ================"
run_arm default
run_arm no-evac                 ZAP_GC_NO_EVAC=1
run_arm no-reclaim              ZAP_GC_NO_RECLAIM=1
run_arm no-reclaim+no-evac      ZAP_GC_NO_RECLAIM=1 ZAP_GC_NO_EVAC=1
