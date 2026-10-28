#!/usr/bin/env bash
# Smoke-test a prebuilt Zap toolchain image: prove it can COMPILE, LINK, RUN and
# TEST a Zap program — not merely parse one. Exercises the full codegen → cc
# link → execute path inside the slim runtime image (the reason the image ships
# a C toolchain + the runtime libraries, see Dockerfile / docs/DOCKER.md).
#
# Usage: scripts/docker-smoke.sh <image>   e.g. scripts/docker-smoke.sh ghcr.io/zaplang-io/zap:latest
set -euo pipefail

IMAGE="${1:?usage: scripts/docker-smoke.sh <image>}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# A program that actually allocates + computes (recursive fib exercises the
# codegen'd call path; the string print exercises the GC'd str runtime).
cat >"$work/hello.zap" <<'ZAP'
def fib(int64 n) -> int64:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def main():
    print(fib(10))
    print("hello from the zap container")
ZAP

# A minimal package with a test file so `zap test` has something to discover
# (it looks for *_test.zap and tests/).
mkdir -p "$work/pkg"
cat >"$work/pkg/zap.toml" <<'TOML'
[package]
name = "smoke"
version = "0.0.0"
TOML
cat >"$work/pkg/smoke_test.zap" <<'ZAP'
def dbl(int64 n) -> int64:
    return n * 2

@test
def test_arith():
    assert 2 + 2 == 4

@test
def test_call():
    assert dbl(21) == 42
ZAP

run() { docker run --rm -v "$work":/w -w /w "$IMAGE" "$@"; }

echo "== zap version =="
run zap version

echo "== zap run hello.zap (JIT/compile+execute) =="
out="$(run zap run hello.zap)"
printf '%s\n' "$out"
[ "$out" = $'55\nhello from the zap container' ] || { echo "FAIL: unexpected \`zap run\` output"; exit 1; }

echo "== zap build hello.zap -> native binary, then execute it =="
out="$(docker run --rm -v "$work":/w -w /w "$IMAGE" bash -c 'zap build hello.zap -o hello && ./hello')"
printf '%s\n' "$out"
[ "$out" = $'55\nhello from the zap container' ] || { echo "FAIL: built binary produced unexpected output"; exit 1; }

echo "== zap test (package with smoke_test.zap) =="
docker run --rm -v "$work/pkg":/w -w /w "$IMAGE" zap test

# --------------------------------------------------------------------------
# Portability proof (I#287): `zap build --portable` produces a self-contained
# static binary that runs on Amazon Linux 2023 (glibc 2.34, the AWS
# `provided.al2023` Lambda base) — where the default dynamically-glibc-2.39-linked
# Ubuntu build fails to start (exit 127). Build in the (Ubuntu) toolchain image,
# then execute the SAME binary inside a stock AL2023 container with no toolchain.
# --------------------------------------------------------------------------
AL2023_IMAGE="${AL2023_IMAGE:-public.ecr.aws/amazonlinux/amazonlinux:2023}"

echo "== zap build --portable hello.zap -> static binary =="
# `ldd` ships with glibc (libc-bin); on a static binary it reports "not a dynamic
# executable" and exits non-zero, so the grep (not ldd's status) is the check.
if docker run --rm -v "$work":/w -w /w "$IMAGE" bash -c '
      set -e
      zap build --portable hello.zap -o hello-portable
      echo "-- ldd --"; ldd hello-portable 2>&1 || true
      ldd hello-portable 2>&1 | grep -q "not a dynamic executable"
   '; then
    echo "OK: portable binary is statically linked (no dynamic glibc floor)"
else
    echo "FAIL: --portable binary is not statically linked"; exit 1
fi

# The decisive proof: the SAME binary runs on stock Amazon Linux 2023 (glibc
# 2.34). A default dynamic build (glibc 2.39) would exit 127 here.
echo "== run the portable binary on Amazon Linux 2023 ($AL2023_IMAGE) =="
docker pull -q "$AL2023_IMAGE" >/dev/null
out="$(docker run --rm -v "$work":/w -w /w "$AL2023_IMAGE" ./hello-portable)"
printf '%s\n' "$out"
[ "$out" = $'55\nhello from the zap container' ] || { echo "FAIL: portable binary did not run correctly on AL2023"; exit 1; }
echo "OK: portable binary runs on Amazon Linux 2023"

echo "ALL SMOKE CHECKS PASSED for $IMAGE"
