#!/usr/bin/env bash
# I#223 regression harness (permanent; see tests/repro223/RESUME.md).
#
# Builds the amplified evacuation-stress repro with the given zapc and runs it
# N times at MAXPROCS=2 AND 4, reporting wrong/hang/crash counts. Run it once per
# collector (build zapc with zapc/gc-immix, then zapc/gc-boehm).
#
# TODAY (the open bug): on aarch64 this fails ~97% (preempt on) / ~8%
# (ZAP_NO_PREEMPT=1); on x86-64 it is 0/N. A FIX must make this 0/N at BOTH
# MAXPROCS on BOTH collectors on real aarch64, with x86 unchanged.
#
#   ZAPC=<path to zapc>  N=200  bash tests/repro223/regress.sh
#
# Env: ZAPC (required), N (runs per cell, default 200), TIMEOUT (per-run seconds,
# default 30; a hang counts as a failure), EXP (expected sum, default 159976000),
# plus any ZAP_* diagnostic knob (passed through, e.g. ZAP_NO_PREEMPT=1).
set -u
ZAPC="${ZAPC:?set ZAPC to the built zapc binary}"
N="${N:-200}"
TIMEOUT="${TIMEOUT:-30}"
EXP="${EXP:-159976000}"
HERE="$(cd "$(dirname "$0")" && pwd)"
PROG="$HERE/amp_evac.zap"
BIN="$(mktemp -u /tmp/amp_evac.XXXX)"

echo "== I#223 regression ($(uname -m)); zapc=$ZAPC N=$N timeout=${TIMEOUT}s =="
"$ZAPC" build "$PROG" -o "$BIN" 2>&1 | tail -2 || { echo "COMPILE FAILED"; exit 1; }

fail_total=0
for P in 2 4; do
    wrong=0; hang=0; ok=0
    for i in $(seq 1 "$N"); do
        out=$(MAXPROCS="$P" timeout -s KILL "$TIMEOUT" "$BIN" 2>/dev/null); rc=$?
        if [ "$rc" = 137 ] || [ "$rc" = 124 ] || [ "$rc" = 139 ] || [ "$rc" = 134 ]; then
            hang=$((hang + 1))
        elif [ "$out" != "$EXP" ]; then
            wrong=$((wrong + 1))
        else
            ok=$((ok + 1))
        fi
    done
    echo "RESULT MAXPROCS=$P: ok=$ok wrong=$wrong crash/hang=$hang / $N"
    fail_total=$((fail_total + wrong + hang))
done
rm -f "$BIN"
echo "== total failures: $fail_total (want 0) =="
[ "$fail_total" -eq 0 ]
