#!/usr/bin/env bash
# Ticket #223 repro/measurement harness (TEMPORARY — removed before the final PR).
#
# Compiles the evacuation-stress programs once with the just-built zapc, then runs
# each binary N times under several collector "arms" and reports a failure rate.
# A failure = stdout differs from the deterministic expected sum. This measures
# the intermittent aarch64 evacuation corruption (#223) and isolates the layer:
#   default      — gc-immix with opportunistic evacuation (the suspect)
#   no-evac      — gc-immix, evacuation OFF (ZAP_GC_NO_EVAC): if it still fails,
#                  the bug is NOT the moving collector
#   poison       — gc-immix, clobber moved-from payload (ZAP_GC_EVAC_POISON): a
#                  surviving stale reference to a relocated buffer reads sentinel
set -u

ZAPC="${ZAPC:?set ZAPC to the zapc binary}"
N="${N:-200}"
P="${MAXPROCS:-4}"
WORKDIR="${WORKDIR:-/tmp/repro223}"
mkdir -p "$WORKDIR"

declare -A EXP
EXP[amp_evac]=159976000
EXP[immix_evac_coro_stress]=15962400

compile() {
    local name=$1 src=$2
    echo "compiling $name ..."
    "$ZAPC" build "$src" -o "$WORKDIR/$name" || { echo "COMPILE FAILED: $name"; exit 1; }
}

run_arm() {
    local name=$1 label=$2; shift 2
    local exp=${EXP[$name]} bin="$WORKDIR/$name"
    local fail=0 i out
    for i in $(seq 1 "$N"); do
        out=$(env "$@" MAXPROCS="$P" "$bin" 2>/dev/null)
        if [ "$out" != "$exp" ]; then
            fail=$((fail + 1))
            [ "$fail" -le 10 ] && echo "  [$name/$label] run $i: got '$out' want '$exp'"
        fi
    done
    echo "FAILRATE $name/$label: $fail / $N   (MAXPROCS=$P)"
}

compile amp_evac tests/repro223/amp_evac.zap
compile immix_evac_coro_stress tests/golden/immix_evac_coro_stress.zap

echo "================ #223 measurement (N=$N, MAXPROCS=$P) ================"
for name in amp_evac immix_evac_coro_stress; do
    run_arm "$name" default
    run_arm "$name" no-evac   ZAP_GC_NO_EVAC=1
    run_arm "$name" poison    ZAP_GC_EVAC_POISON=1
done
echo "======================================================================"
