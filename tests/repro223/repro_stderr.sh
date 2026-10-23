#!/usr/bin/env bash
# TEMPORARY (#223): reproduce the crash and capture the RUNTIME fault report
# (zap's own SIGSEGV/SIGBUS handler prints signal + faulting address + offending
# coroutine + a -rdynamic symbolized backtrace to stderr). No gdb needed, so it
# works under qemu-user and on real ARM. Removed before the fix PR.
set -u
export LLVM_SYS_180_PREFIX=/usr/lib/llvm-18 LLVM_SYS_181_PREFIX=/usr/lib/llvm-18
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/work/target-arm64}"
export ZAP_CC_EXTRA="${ZAP_CC_EXTRA:--g}"      # line numbers in the backtrace
N="${N:-120}"
P="${MAXPROCS:-4}"
FEATURES="${FEATURES:-zapc/llvm,zapc/gc-immix}"
PROG="${PROG:-tests/repro223/amp_evac.zap}"
ARM_ENV="${ARM_ENV:-}"            # e.g. ZAP_GC_NO_EVAC=1
exp="${EXP:-159976000}"

echo "=== uname $(uname -m) · features $FEATURES · env '$ARM_ENV' ==="
cargo build -p zapc --features "$FEATURES" 2>&1 | tail -2 || exit 1
ZAPC="$(find "$CARGO_TARGET_DIR" -name zapc -type f -path '*/debug/*' | head -1)"
"$ZAPC" build "$PROG" -o /tmp/prog 2>&1 | tail -2 || exit 1

wrong=0; crash=0; ok=0; shown=0
for i in $(seq 1 "$N"); do
    err=$(env $ARM_ENV MAXPROCS="$P" timeout -s KILL 60 /tmp/prog 2>/tmp/stderr >/tmp/stdout; echo "rc=$?")
    rc=${err#rc=}; out=$(cat /tmp/stdout)
    if [ "$rc" != 0 ]; then
        crash=$((crash+1))
        if [ "$shown" -lt 3 ]; then
            shown=$((shown+1))
            echo "========== CRASH run $i rc=$rc (stdout='$out') =========="
            cat /tmp/stderr
            echo "========== end crash $i =========="
        fi
    elif [ "$out" != "$exp" ]; then
        wrong=$((wrong+1)); echo "run $i WRONG out='$out'"; head -5 /tmp/stderr
    else
        ok=$((ok+1))
    fi
done
echo "=== RESULT ($PROG, $FEATURES, '$ARM_ENV'): ok=$ok wrong=$wrong crash=$crash / $N ==="
