#!/usr/bin/env bash
# tests/gc-stress/catch.sh — loop one debug binary until it hangs or crashes,
# then capture an all-thread gdb backtrace (#223 / #693 deadlock debugging).
#
#   usage: catch.sh ./<bin> [want] [hang_secs]
#     <bin>       a zapc-built binary compiled with symbols:
#                   ZAP_CC_EXTRA="-g -fno-omit-frame-pointer" \
#                     target/release/zapc build tests/golden/immix_evac_coro_stress.zap
#                 (`zapc build` writes ./<stem> in the CURRENT dir)
#     want        optional expected stdout (whitespace-stripped); mismatch = fail
#     hang_secs   per-iteration hang threshold in seconds (default 60)
#
#   env: MAXPROCS (default nproc), GC_FREE_SPACE_DIVISOR (default 40, boehm only)
#
# Prereqs on the box (fresh each time):
#   sudo sysctl -w kernel.yama.ptrace_scope=0            # let gdb attach
#   ulimit -c unlimited; sudo sysctl -w kernel.core_pattern=/tmp/core.%e.%p  # cores
#
# On a HANG: dumps `thread apply all bt` to /tmp/hang.<i>.bt. The known gc-boehm
# signature is every thread blocked on the bdwgc alloc mutex with one M in
# set_bottom->GC_call_with_alloc_lock from switch_to_g — see README.md. The
# `print GC_allocate_ml.__data.__owner` line names the holding TID under boehm
# (it is a no-op/aborts harmlessly under immix, where the lock is a different mutex).
set -u

bin="${1:?usage: catch.sh ./<bin> [want] [hang_secs]}"
want="${2:-}"
secs="${3:-60}"
export MAXPROCS="${MAXPROCS:-$(command -v nproc >/dev/null && nproc || echo 4)}"
export GC_FREE_SPACE_DIVISOR="${GC_FREE_SPACE_DIVISOR:-40}"

echo "== catch: $bin want=[${want:-<any>}] hang=${secs}s MAXPROCS=$MAXPROCS =="
i=0
while :; do
  i=$((i + 1))
  "$bin" >/tmp/gc-catch.out 2>/tmp/gc-catch.err & pid=$!
  w=0
  while kill -0 "$pid" 2>/dev/null; do
    sleep 1; w=$((w + 1))
    if [ "$w" -ge "$secs" ]; then
      echo "=== HANG iter $i (pid $pid) ==="
      gdb -p "$pid" -batch \
        -ex 'set pagination off' \
        -ex 'print GC_allocate_ml' \
        -ex 'print GC_allocate_ml.__data.__owner' \
        -ex 'thread apply all bt' 2>&1 | tee "/tmp/hang.$i.bt"
      kill -9 "$pid"; exit 1
    fi
  done
  wait "$pid"; rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "=== CRASH iter $i rc=$rc ==="; sed -n '1,60p' /tmp/gc-catch.err
    c="$(ls -t /tmp/core.* 2>/dev/null | head -1)"
    [ -n "$c" ] && gdb "$bin" "$c" -batch \
      -ex 'set pagination off' \
      -ex 'thread apply all bt full' 2>&1 | tee "/tmp/crash.$i.bt"
    exit 1
  fi
  if [ -n "$want" ] && [ "$(tr -d '[:space:]' </tmp/gc-catch.out)" != "$want" ]; then
    echo "=== WRONG VALUE iter $i: [$(cat /tmp/gc-catch.out)] want [$want] ==="; exit 1
  fi
  [ $((i % 50)) -eq 0 ] && echo "...$i ok"
done
