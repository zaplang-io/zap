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
#
# Deliberately NOT `set -e`: a bench or build that fails should be reported in
# the table and flip the exit code at the end, not abort the whole run silently
# (and `read`-ing a newline-less line returns non-zero, which `set -e` would
# mistake for a failure).
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ZAPC="${ZAPC:-./target/release/zapc}"
REPS="${REPS:-5}"
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

# benchmark -> expected stdout (so a miscompile can't flatter the timing).
BENCHES=(fib loop_sum dict_string dict_int fstring_only)
declare -A EXPECT=(
  [fib]=2178309                    # fib(32)
  [loop_sum]=449999985000000       # sum(0..30_000_000)
  [dict_string]=19999900000        # sum of 200k string-keyed lookups
  [dict_int]=19999900000           # same workload, INT keys (isolates the dict)
  [fstring_only]=2577780           # 400k f"k{i}" built, summed by len (isolates strings)
)
declare -A LABEL=(
  [fib]="fib(32) — recursive, call-heavy, GC-free"
  [loop_sum]="loop sum 30M — tight integer loop, GC-free"
  [dict_string]="dict+string 200k — build + lookup, GC-heavy"
  [dict_int]="dict+int 200k — same ops, INT keys (dict cost only)"
  [fstring_only]="f-string ×400k — build + len, no dict (string cost only)"
)

now_ns() { date +%s%N; }

# Run "$@" REPS times. Sets globals TIMEIT_MS (min ms) and TIMEIT_OUT (output of
# the last run); returns the last run's exit status. No process substitution /
# `read`, so no set-e / trailing-newline hazards.
TIMEIT_MS=0
TIMEIT_OUT=""
timeit() {
  local best="" out="" t0 t1 ms st=0
  local i
  for i in $(seq 1 "$REPS"); do
    t0=$(now_ns)
    out="$("$@")"
    st=$?
    t1=$(now_ns)
    ms=$(( (t1 - t0) / 1000000 ))
    if [ -z "$best" ] || [ "$ms" -lt "$best" ]; then best="$ms"; fi
  done
  TIMEIT_MS="$best"
  TIMEIT_OUT="$out"
  return "$st"
}

# "b / a" to 1 decimal: how many times faster `a` (Zap) is than `b`.
ratio() { awk -v a="$1" -v b="$2" 'BEGIN{ if (a+0 < 1) a=1; printf "%.1f", (b+0)/(a+0) }'; }

fail=0
rows=""
echo "Zap:    $("$ZAPC" --version 2>/dev/null || echo '(zapc)')"
echo "Go:     $(go version)"
echo "Python: $(python3 --version)"
echo "Best of $REPS runs; times in ms (lower is better)."
echo "(zapc build warnings — e.g. OpenSSL 3.0 deprecations in the runtime — are"
echo " captured to a log and are not failures.)"
echo

printf '%-14s %10s %10s %10s %14s %12s\n' bench zap_ms go_ms python_ms "zap_vs_python" "zap_vs_go"
for name in "${BENCHES[@]}"; do
  exp="${EXPECT[$name]}"

  if ! "$ZAPC" build -o "$OUT/$name.zap.bin" "$HERE/$name.zap" >"$OUT/$name.zapbuild.log" 2>&1; then
    echo "!! zapc build FAILED for $name:" >&2
    tail -n 25 "$OUT/$name.zapbuild.log" >&2
    fail=1
    continue
  fi
  if ! go build -o "$OUT/$name.go.bin" "$HERE/$name.go" >"$OUT/$name.gobuild.log" 2>&1; then
    echo "!! go build FAILED for $name:" >&2
    cat "$OUT/$name.gobuild.log" >&2
    fail=1
    continue
  fi

  timeit "$OUT/$name.zap.bin"; zst=$?; zms="$TIMEIT_MS"; zout="$TIMEIT_OUT"
  timeit "$OUT/$name.go.bin";  gst=$?; gms="$TIMEIT_MS"; gout="$TIMEIT_OUT"
  timeit python3 "$HERE/$name.py"; pst=$?; pms="$TIMEIT_MS"; pout="$TIMEIT_OUT"

  for entry in "zap:$zst:$zout" "go:$gst:$gout" "python:$pst:$pout"; do
    lang="${entry%%:*}"; rest="${entry#*:}"; st="${rest%%:*}"; got="${rest#*:}"
    if [ "$st" -ne 0 ]; then
      echo "!! $name/$lang exited with status $st" >&2
      fail=1
    elif [ "$got" != "$exp" ]; then
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
    if [ "$fail" -ne 0 ]; then
      echo
      echo "> ⚠️ one or more programs failed to build / run / match — see the job log."
    fi
  } >> "$GITHUB_STEP_SUMMARY"
fi

if [ "$fail" -ne 0 ]; then
  echo "one or more programs failed — timings above may be incomplete" >&2
  exit 1
fi
