#!/usr/bin/env bash
# tests/gc-stress/stress.sh — many-core GC/scheduler stress loop (#223 / #693).
#
# Loops the concurrency stress goldens via `zap run` under the chosen collector
# and flags any run that hangs (timeout), crashes (non-zero exit), or prints a
# wrong total (vs the fixture's .expected). A correctly-synchronized program
# must print its deterministic total every run; anything else is the defect.
#
#   usage: stress.sh [immix|boehm] [iters] [maxprocs]
#   env:   ZAPC   path to a built zapc for the matching collector (required).
#                 Build it first, e.g.:
#                   cargo build --release -p zapc --features zapc/gc-boehm
#                   ZAPC=target/release/zapc tests/gc-stress/stress.sh boehm 2000 $(nproc)
#          TIMEOUT          per-run seconds (default 120; a hang counts as a fail)
#          GC_FREE_SPACE_DIVISOR  forces frequent Boehm STW (default 40; no-op under immix)
#          MAXPROCS         worker-M parallelism (default: the 3rd arg, else nproc)
#
# Attribution rule: fails under BOTH collectors => #223 (collector-agnostic);
# threading_counter fails under boehm but NOT immix => the #693 signature.
# See tests/gc-stress/README.md. This is a debug harness, NOT a CI gate.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
cd "$ROOT"

COLLECTOR="${1:-boehm}"
ITERS="${2:-2000}"
MP="${3:-$(command -v nproc >/dev/null && nproc || echo 4)}"
TIMEOUT="${TIMEOUT:-120}"
export GC_FREE_SPACE_DIVISOR="${GC_FREE_SPACE_DIVISOR:-40}"
export MAXPROCS="$MP"

ZAPC="${ZAPC:?set ZAPC to a zapc built with --features zapc/gc-$COLLECTOR}"
[ -x "$ZAPC" ] || { echo "ZAPC not executable: $ZAPC"; exit 2; }

echo "== gc-stress: collector=$COLLECTOR iters=$ITERS MAXPROCS=$MP timeout=${TIMEOUT}s divisor=$GC_FREE_SPACE_DIVISOR ($(uname -m)) =="

# The #223 quarantined stress goldens + the #693 threading_counter. Override by
# passing a newline/space list in FIXTURES.
FIXTURES="${FIXTURES:-
tests/golden/immix_evac_coro_stress.zap
tests/golden/concurrent_queue_mpsc_coro_stress.zap
tests/golden/concurrent_queue_mpsc_thread_stress.zap
tests/golden/threading_counter.zap
}"

fails=0
for f in $FIXTURES; do
  [ -f "$f" ] || { echo "skip (missing): $f"; continue; }
  want=""
  [ -f "${f%.zap}.expected" ] && want="$(tr -d '[:space:]' < "${f%.zap}.expected")"
  echo "=== ${f##*/} x $ITERS (collector=$COLLECTOR) ==="
  for i in $(seq 1 "$ITERS"); do
    got="$(timeout -s KILL "$TIMEOUT" "$ZAPC" run "$f" 2>/tmp/gc-stress.err)"
    rc=$?
    if [ "$rc" -ne 0 ]; then
      echo "FAIL ${f##*/} iter $i: exit=$rc ($([ "$rc" = 124 ] || [ "$rc" = 137 ] && echo HANG || echo CRASH))"
      sed -n '1,20p' /tmp/gc-stress.err
      fails=$((fails + 1)); break
    fi
    if [ -n "$want" ] && [ "$(printf '%s' "$got" | tr -d '[:space:]')" != "$want" ]; then
      echo "FAIL ${f##*/} iter $i: WRONG VALUE got [$got] want [$want]"
      fails=$((fails + 1)); break
    fi
    [ $((i % 50)) -eq 0 ] && echo "...$i ok"
  done
done

echo "== gc-stress done: $fails fixture(s) failed (want 0) =="
[ "$fails" -eq 0 ]
