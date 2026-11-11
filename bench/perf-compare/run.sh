#!/usr/bin/env bash
#
# Throwaway cross-language perf comparison (Zap vs Go vs Python) for a handful of
# small algorithms. NOT a regression gate — it just builds each program, runs it
# best-of-N, checks the output is correct, and prints a comparison table (also to
# $GITHUB_STEP_SUMMARY in CI). Wall-clock is machine-dependent; read the RATIOS,
# not the absolute milliseconds. Companion to the in-repo relative perf harness
# (crates/driver/tests/perf.rs, #568) — this one is for a human eyeball, not CI
# gating.
#
# Usage: ZAPC=./target/release/zapc REPS=5 bench/perf-compare/run.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ZAPC="${ZAPC:-./target/release/zapc}"
REPS="${REPS:-5}"
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

# benchmark -> expected stdout (so a miscompile can't flatter the timing).
BENCHES=(fib loop_sum dict_string)
declare -A EXPECT=(
  [fib]=2178309                    # fib(32)
  [loop_sum]=449999985000000       # sum(0..30_000_000)
  [dict_string]=19999900000        # sum of 200k string-keyed lookups
)
declare -A LABEL=(
  [fib]="fib(32) — recursive, call-heavy, GC-free"
  [loop_sum]="loop sum 30M — tight integer loop, GC-free"
  [dict_string]="dict+string 200k — build + lookup, GC-heavy"
)

now_ns() { date +%s%N; }

# Run "$@" REPS times; print "min_ms<TAB>stdout-of-last-run".
timeit() {
  local best="" out="" t0 t1 ms
  for _ in $(seq 1 "$REPS"); do
    t0=$(now_ns)
    out="$("$@")"
    t1=$(now_ns)
    ms=$(( (t1 - t0) / 1000000 ))
    if [ -z "$best" ] || [ "$ms" -lt "$best" ]; then best="$ms"; fi
  done
  printf '%s\t%s' "$best" "$out"
}

# faster-by ratio "b / a" to 1 decimal (how many times faster `a` is than `b`).
ratio() { awk -v a="$1" -v b="$2" 'BEGIN{ if (a<1) a=1; printf "%.1f", b/a }'; }

fail=0
rows=""
echo "Zap:    $("$ZAPC" --version 2>/dev/null || echo '(zapc)')"
echo "Go:     $(go version)"
echo "Python: $(python3 --version)"
echo "Best of $REPS runs; times in ms (lower is better)."
echo

printf '%-14s %10s %10s %10s %14s %12s\n' bench zap_ms go_ms python_ms "zap_vs_python" "zap_vs_go"
for name in "${BENCHES[@]}"; do
  exp="${EXPECT[$name]}"

  "$ZAPC" build -o "$OUT/$name.zap.bin" "$HERE/$name.zap" >/dev/null
  go build -o "$OUT/$name.go.bin" "$HERE/$name.go"

  IFS=$'\t' read -r zms zout < <(timeit "$OUT/$name.zap.bin")
  IFS=$'\t' read -r gms gout < <(timeit "$OUT/$name.go.bin")
  IFS=$'\t' read -r pms pout < <(timeit python3 "$HERE/$name.py")

  for pair in "zap=$zout" "go=$gout" "python=$pout"; do
    lang="${pair%%=*}"; got="${pair#*=}"
    if [ "$got" != "$exp" ]; then
      echo "!! MISMATCH $name/$lang: got '$got', expected '$exp'" >&2
      fail=1
    fi
  done

  zvp="$(ratio "$zms" "$pms")"
  zvg="$(ratio "$zms" "$gms")"
  printf '%-14s %10s %10s %10s %13sx %11sx\n' "$name" "$zms" "$gms" "$pms" "$zvp" "$zvg"
  rows="${rows}| ${LABEL[$name]} | ${zms} | ${gms} | ${pms} | ${zvp}× | ${zvg}× |
"
done

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "## Zap vs Go vs Python — throwaway perf compare"
    echo
    echo "Best of ${REPS} runs, wall-clock in **ms** (lower is better). Ratios are"
    echo "how many times **faster Zap is** than that language (>1 = Zap faster)."
    echo "Absolute ms are machine-dependent — read the ratios."
    echo
    echo "\`$("$ZAPC" --version 2>/dev/null || echo zapc)\` · \`$(go version)\` · \`$(python3 --version)\`"
    echo
    echo "| benchmark | Zap (ms) | Go (ms) | Python (ms) | Zap vs Python | Zap vs Go |"
    echo "|---|---:|---:|---:|---:|---:|"
    printf '%s' "$rows"
  } >> "$GITHUB_STEP_SUMMARY"
fi

if [ "$fail" -ne 0 ]; then
  echo "one or more programs produced the wrong result — timings are not trustworthy" >&2
  exit 1
fi
