#!/usr/bin/env bash
# TEMPORARY (#223): run inside the emulated aarch64 container to reproduce the
# crash locally and capture a backtrace. Removed before the fix PR.
set -u
export LLVM_SYS_180_PREFIX=/usr/lib/llvm-18 LLVM_SYS_181_PREFIX=/usr/lib/llvm-18
export CARGO_TARGET_DIR=/work/target-arm64
N="${N:-150}"
P="${MAXPROCS:-4}"
FEATURES="${FEATURES:-zapc/llvm,zapc/gc-immix}"
ARM_ENV="${ARM_ENV:-}"      # e.g. ZAP_GC_NO_EVAC=1
exp_amp=159976000

command -v gdb >/dev/null || { echo "installing gdb..."; apt-get update -qq && apt-get install -y -qq gdb >/dev/null; }

echo "=== uname: $(uname -m) ==="
echo "=== building zapc ($FEATURES) ... ==="
cargo build -p zapc --features "$FEATURES" 2>&1 | tail -2 || exit 1
ZAPC="$(find "$CARGO_TARGET_DIR" -name zapc -type f -path '*/debug/*' | head -1)"
echo "zapc = $ZAPC"
"$ZAPC" build tests/repro223/amp_evac.zap -o /tmp/amp_evac || exit 1

echo "=== loop N=$N MAXPROCS=$P env='$ARM_ENV' ==="
wrong=0; crash=0; ok=0
for i in $(seq 1 "$N"); do
    out=$(env $ARM_ENV MAXPROCS="$P" timeout -s KILL 60 /tmp/amp_evac 2>/dev/null); rc=$?
    if [ "$rc" != 0 ]; then
        crash=$((crash+1))
        echo "run $i: CRASH rc=$rc out='$out'"
        if [ "$crash" = 1 ]; then
            echo "=== capturing gdb backtrace on first crash ==="
            env $ARM_ENV MAXPROCS="$P" gdb -batch -q \
                -ex 'run' -ex 'thread apply all bt' -ex 'info registers' \
                /tmp/amp_evac 2>&1 | tail -120
        fi
    elif [ "$out" != "$exp_amp" ]; then
        wrong=$((wrong+1)); echo "run $i: WRONG out='$out'"
    else
        ok=$((ok+1))
    fi
done
echo "=== RESULT: ok=$ok wrong=$wrong crash=$crash / $N ==="
