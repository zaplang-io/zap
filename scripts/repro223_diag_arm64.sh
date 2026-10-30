#!/usr/bin/env bash
# I#223 reproduce + diagnose under emulated aarch64 (cloud session: x86 + qemu).
#
# Builds the zapc that lives in the CHECKOUT THIS SCRIPT RUNS FROM (so run it from
# the investigation worktree, which carries the #223 diagnostic knobs: gc-immix),
# then runs the decisive experiments. The key datum the issue never reported:
# ZAP_GC_VERIFY_ROOTS' `in_shadow_chain=YES/NO` for the unmarked live list.
#
# Prereqs (set up once in the session): dockerd running, qemu-user-static +
# the aarch64 binfmt handler registered. Networking: the container reaches the
# agent proxy via --network=host (proxy at 127.0.0.1:43717) and trusts its CA
# bundle (/root/.ccr/ca-bundle.crt) — Docker build steps otherwise have no
# outbound HTTPS and rustup's installer fails silently.
set -uxo pipefail
SRC="$(cd "$(dirname "$0")/.." && pwd)"   # the checkout this script sits in
CTX="$(mktemp -d)"
PROXY="${AGENT_PROXY:-http://127.0.0.1:43717}"
CA="${AGENT_CA:-/root/.ccr/ca-bundle.crt}"

# --- build context: robust arm64 dev image ---------------------------------
if [ -f "$CA" ]; then cp "$CA" "$CTX/ca-bundle.crt"; else : > "$CTX/ca-bundle.crt"; fi
cat > "$CTX/Dockerfile" <<'DOCKER'
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
ARG PROXY=http://127.0.0.1:43717
# Only HTTPS goes through the proxy; apt (HTTP) resolves + fetches directly,
# which is known-good here — proxying it risks breaking the big apt step.
ENV https_proxy=${PROXY} HTTPS_PROXY=${PROXY}
ENV no_proxy=localhost,127.0.0.1 NO_PROXY=localhost,127.0.0.1
COPY ca-bundle.crt /usr/local/share/ca-certificates/agent-proxy-ca.crt
COPY ca-bundle.crt /etc/ssl/agent-ca-bundle.crt
ENV SSL_CERT_FILE=/etc/ssl/agent-ca-bundle.crt CURL_CA_BUNDLE=/etc/ssl/agent-ca-bundle.crt \
    CARGO_HTTP_CAINFO=/etc/ssl/agent-ca-bundle.crt GIT_SSL_CAINFO=/etc/ssl/agent-ca-bundle.crt \
    REQUESTS_CA_BUNDLE=/etc/ssl/agent-ca-bundle.crt
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && update-ca-certificates \
    && apt-get install -y --no-install-recommends \
        build-essential curl git \
        llvm-18 llvm-18-dev libpolly-18-dev clang-18 libclang-18-dev \
        libgc-dev libpcre2-dev python3-dev pkg-config \
        zlib1g-dev libssl-dev libzstd-dev libffi-dev libedit-dev \
        libxml2-dev libncurses-dev mold \
    && rm -rf /var/lib/apt/lists/*
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# The egress policy blocks sh.rustup.rs (403) but allows static.rust-lang.org,
# where the rustup-init binary and all toolchains live — install from there.
RUN curl --proto '=https' --tlsv1.2 -sSf \
        https://static.rust-lang.org/rustup/dist/aarch64-unknown-linux-gnu/rustup-init \
        -o /tmp/rustup-init \
    && chmod +x /tmp/rustup-init \
    && /tmp/rustup-init -y --default-toolchain 1.99.1 --profile minimal \
    && rm -f /tmp/rustup-init
ENV PATH="/root/.cargo/bin:${PATH}"
ENV RUSTUP_DIST_SERVER=https://static.rust-lang.org \
    RUSTUP_UPDATE_ROOT=https://static.rust-lang.org/rustup
RUN rustc --version && cargo --version
ENV LLVM_SYS_180_PREFIX=/usr/lib/llvm-18 LLVM_SYS_181_PREFIX=/usr/lib/llvm-18
WORKDIR /work
DOCKER

docker build --platform linux/arm64 --network=host --build-arg PROXY="$PROXY" \
  -t zap-dev-arm64 -f "$CTX/Dockerfile" "$CTX" 2>&1 | tail -15

# --- build zapc + run the diagnostic matrix inside the arm64 container ------
docker run --rm --platform linux/arm64 --network=host -m 8g \
  -v "$SRC":/work \
  -v zap-target-arm64:/work/target-arm64 \
  -v zap-cargo-registry-arm64:/root/.cargo/registry \
  -w /work -e CARGO_TARGET_DIR=/work/target-arm64 \
  zap-dev-arm64 \
  bash -c '
    set -eux
    echo "GUEST arch: $(uname -m)"
    cargo build -p zapc --features zapc/llvm,zapc/gc-immix 2>&1 | tail -25
    ZAPC=$(find /work/target-arm64 -name zapc -type f -path "*/debug/*" | head -1)
    echo "ZAPC=$ZAPC"
    BIN=/tmp/amp_evac
    "$ZAPC" build /work/tests/repro223/amp_evac.zap -o "$BIN" 2>&1 | tail -5
    EXP=159976000

    run_cell () { # label  env...  -> prints ok/wrong/crash over $N runs
      local label="$1"; shift
      local n="${N:-20}" p="${P:-4}" ok=0 wrong=0 crash=0 out rc
      for i in $(seq 1 "$n"); do
        out=$(env "$@" MAXPROCS="$p" timeout -s KILL 40 "$BIN" 2>/dev/null); rc=$?
        if [ "$rc" = 137 ] || [ "$rc" = 139 ] || [ "$rc" = 134 ] || [ "$rc" = 124 ]; then crash=$((crash+1));
        elif [ "$out" != "$EXP" ]; then wrong=$((wrong+1)); else ok=$((ok+1)); fi
      done
      echo "CELL $label (MAXPROCS=$p N=$n): ok=$ok wrong=$wrong crash=$crash"
    }

    echo "===== A. baseline repro (default, preempt on) ====="
    N=30 P=4 run_cell "default-mp4"
    N=20 P=2 run_cell "default-mp2"
    N=20 P=1 run_cell "default-mp1"
    echo "===== B. UAF control: NO_RECLAIM (expect 0 fail) ====="
    N=20 P=4 run_cell "no-reclaim" ZAP_GC_NO_RECLAIM=1
    echo "===== C. base race w/ preempt OFF ====="
    N=30 P=4 run_cell "no-preempt" ZAP_NO_PREEMPT=1

    echo "===== D. DECISIVE: VERIFY_ROOTS -> in_shadow_chain? ====="
    : > /tmp/verify.log
    for i in $(seq 1 60); do
      MAXPROCS=4 ZAP_GC_VERIFY_ROOTS=1 timeout -s KILL 40 "$BIN" >/dev/null 2>>/tmp/verify.log || true
      if grep -q "VERIFY: UNMARKED" /tmp/verify.log; then echo "caught VERIFY on run $i"; break; fi
    done
    echo "--- VERIFY / race lines (count by kind) ---"
    grep -oE "VERIFY: UNMARKED|MARK-RACE|MARK-ACTIVE|STALE-ROOTS" /tmp/verify.log | sort | uniq -c
    echo "--- in_shadow_chain tally (THE decisive datum) ---"
    grep -o "in_shadow_chain=[A-Z]*" /tmp/verify.log | sort | uniq -c
    echo "--- sample VERIFY lines ---"
    grep "VERIFY: UNMARKED" /tmp/verify.log | head -8
    cp /tmp/verify.log /work/target-arm64/verify.log 2>/dev/null || true
  '

echo "DIAG SCRIPT DONE rc=$?"
