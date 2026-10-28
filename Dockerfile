# Prebuilt Zap toolchain image (I#208 companion; see docs/DOCKER.md).
#
# A two-stage build that produces a slim image carrying the `zap` CLI plus
# everything `zap build` / `zap run` / `zap test` need to COMPILE, LINK and RUN
# a Zap program — not just parse it. The compiler emits an object file with
# LLVM 18 and then shells out to a C compiler (`cc`) to compile the C runtime
# (`zaprt.c`) and link it against the system libraries the runtime wraps
# (libgc, PCRE2, zlib, OpenSSL, libpython, …). So the runtime stage must ship a
# working C toolchain and those libraries' headers, not merely their shared
# objects.
#
# Published to ghcr.io/zaplang-io/zap by .github/workflows/docker.yml.

# ---------------------------------------------------------------------------
# Stage 1: builder — the LLVM 18 build environment.
#
# This MIRRORS .devcontainer/Dockerfile (the environment CI's `full` job
# replicates), so the toolchain is built exactly as it is tested. Keep the apt
# list and the Rust pin in sync with that file; it is the source of truth.
# ---------------------------------------------------------------------------
FROM ubuntu:24.04 AS builder

ENV DEBIAN_FRONTEND=noninteractive

# LLVM 18 + the system libraries llvm-sys links against, a C toolchain, and the
# `-dev` packages the Zap C runtime (zaprt.c) is compiled against. `mold` is the
# faster/leaner linker used for Rust's own link step (matches CI, #128).
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        llvm-18 \
        llvm-18-dev \
        libpolly-18-dev \
        clang-18 \
        libclang-18-dev \
        libgc-dev \
        libpcre2-dev \
        python3-dev \
        pkg-config \
        zlib1g-dev \
        libssl-dev \
        libzstd-dev \
        libffi-dev \
        libedit-dev \
        libxml2-dev \
        libncurses-dev \
        mold \
    && rm -rf /var/lib/apt/lists/*

# Rust, pinned to match rust-toolchain.toml / .devcontainer/Dockerfile.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain 1.99.1 --profile minimal
ENV PATH="/root/.cargo/bin:${PATH}"

# Where llvm-sys / inkwell (llvm18-0) find LLVM 18.
ENV LLVM_SYS_180_PREFIX=/usr/lib/llvm-18
ENV LLVM_SYS_181_PREFIX=/usr/lib/llvm-18
# Link Rust's own binary with mold, matching CI's `full` job.
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-Clink-arg=-fuse-ld=mold"

WORKDIR /src
COPY . .

# Build the user-facing `zap` CLI in release mode with the default collector —
# the precise Immix GC (`--features llvm` ⇒ zapc/gc-immix, ticket #116/D-0009).
# Only the `zap` binary is needed in the image; `--bin zap` skips the internal
# `zapc` binary's extra link.
RUN cargo build --release -p zapc --features llvm --bin zap \
    && strip target/release/zap

# ---------------------------------------------------------------------------
# Stage 2: runtime — slim image that can compile, link and run Zap programs.
#
# No Rust, no LLVM *dev* libraries, no cargo cache: just the `zap` binary and
# the C-toolchain + libraries its on-the-fly link step (build_executable in
# crates/codegen) invokes. Same LLVM 18 major as the builder so the emitted
# object and the clang driver agree.
# ---------------------------------------------------------------------------
FROM ubuntu:24.04 AS runtime

ENV DEBIAN_FRONTEND=noninteractive

# The link toolchain + the libraries zaprt.c is compiled and linked against:
#   * clang-18 / lld-18  — the `cc`/linker driver (set as ZAP_CC below); matches
#                          the LLVM 18 object the compiler emits.
#   * libstdc++-14-dev   — `-lstdc++` at the link step (the C++ runtime bits).
#   * libgc-dev          — `-lgc` for the conservative Boehm collector + gc.h.
#   * libpcre2-dev       — `-lpcre2-8` + pcre2.h (regex runtime, #23).
#   * zlib1g-dev         — `-lz` + zlib.h (zlib/gzip modules, #106).
#   * libssl-dev         — `-lssl -lcrypto` + openssl headers (tls/https, #147).
#   * python3-dev        — libpython + <Python.h> for CPython embedding (#27).
#   * pkg-config         — how the compiler discovers the libpython flags.
# The remaining libs (libffi/libedit/libxml2/libtinfo/libzstd) come in as deps
# of the above and back the statically-linked LLVM inside `zap`.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        clang-18 \
        lld-18 \
        libstdc++-14-dev \
        libgc-dev \
        libpcre2-dev \
        zlib1g-dev \
        libssl-dev \
        python3-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/* \
    && ln -s /usr/bin/clang-18 /usr/local/bin/cc \
    && ln -s /usr/bin/clang-18 /usr/local/bin/clang

# The compiler shells out to `$ZAP_CC` (default `cc`) for the link step; pin it
# to the LLVM 18 clang so the C runtime link matches the emitted object.
ENV ZAP_CC=clang-18

COPY --from=builder /src/target/release/zap /usr/local/bin/zap

# Smoke check at build time: prove the binary is runnable and self-reports.
RUN zap version

WORKDIR /work
CMD ["zap", "--help"]
