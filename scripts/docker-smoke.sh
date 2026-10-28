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
@test
def test_arith():
    assert 2 + 2 == 4

@test
def test_str():
    assert "a" + "b" == "ab"
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

echo "ALL SMOKE CHECKS PASSED for $IMAGE"
