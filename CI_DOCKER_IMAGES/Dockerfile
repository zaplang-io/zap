# syntax=docker/dockerfile:1

# Zap CI toolchain base image (x86_64 Linux) — issue #854; folds in #296.
#
# This is the *build* environment Zap's Linux CI runs INSIDE, published to
# ghcr.io/zaplang-io/zap-ci. It bakes the full LLVM 18 + Rust + C-dep toolchain
# so the Linux `full` CI leg no longer installs LLVM and the C headers from
# third-party mirrors on every job — those per-job apt.llvm.org fetches are slow
# and are the dominant source of CI *hangs* (a run wedges mid-download until it
# times out). The only network step left for the leg is the image pull from
# GitHub's own registry/CDN: fast, same trust domain, resumable.
#
# Relationship to the other two images in this repo (keep all three in sync):
#   * .devcontainer/Dockerfile  — the dev container; THE source of truth for the
#                                 apt package list and the Rust pin. This image
#                                 mirrors it (plus lld + Node) so CI builds in
#                                 the exact environment developers build in.
#   * /Dockerfile (root)        — the SLIM *runtime* image (ghcr.io/zaplang-io/zap,
#                                 #277): just the `zap` CLI + a C link toolchain,
#                                 for downstream repos to run `zap test`. It
#                                 cannot build the compiler from source, which is
#                                 why CI needs this separate, fatter toolchain
#                                 image.
#
# NOT a multi-stage/slim image: it is a build environment, so there is no
# `COPY . .` and no compiler build here. CI checks the repo out into /work at
# run time (`container:` + actions/checkout) and builds there against a warm
# cargo cache.

# Pin the base layer by digest for reproducibility. ubuntu:24.04 (noble) is the
# platform all three images target. (This is the amd64/arm64 multi-arch index
# digest; buildx selects the linux/amd64 manifest for this x86_64 image.)
FROM ubuntu:24.04@sha256:2198e4c65076e9cb0b3d695219df667c0e3b0072da64ca3c2648aca17dcecaf5

ENV DEBIAN_FRONTEND=noninteractive

# LLVM 18 + the system libraries llvm-sys/inkwell link against, a C toolchain,
# and the `-dev` packages the Zap C runtime (zaprt.c) is compiled and linked
# against. This list is kept IDENTICAL to .devcontainer/Dockerfile and the
# `full` job's inline install in .github/workflows/ci.yml (the source of truth),
# plus `lld-18` (#854 asks for lld in the toolchain).
#
# Everything LLVM comes in ONE apt transaction, so clang-18, lld-18, llvm-link,
# opt and libLLVM-18 are all the SAME point release (18.1.3 on noble) — there is
# no skew against inkwell's `llvm18-0`, which the LTO link path (llvm-link +
# `opt -O2` + clang-18) relies on. The trailing assertion fails the build loudly
# if noble ever moves off the 18.1 series, so a silent LLVM bump can never ship.
#   * mold    — the faster/leaner linker used for Rust's own link step (#128),
#               driven via -fuse-ld=mold in CI's RUSTFLAGS.
#   * sccache — the shared compiler cache (RUSTC_WRAPPER) the dev container uses.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        gnupg \
        llvm-18 \
        llvm-18-dev \
        libpolly-18-dev \
        clang-18 \
        lld-18 \
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
        sccache \
    && rm -rf /var/lib/apt/lists/* \
    && test "$(/usr/lib/llvm-18/bin/llvm-config --version | cut -d. -f1-2)" = "18.1"

# Node 20 LTS — folds in #296 so non-core / tooling CI (e.g. the VS Code
# extension's npm build) can run inside the container. NodeSource is the
# upstream LTS channel; matches the Node 20 used by the editors/vscode leg.
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

# Rust, pinned to rust-toolchain.toml (1.99.1) with the components CI gates on
# (clippy, rustfmt). Baked here so the Linux legs are turnkey — no per-job
# rustup download. Keep this pin in lockstep with rust-toolchain.toml and
# .devcontainer/Dockerfile.
#
# Installed under /usr/local (NOT /root/.cargo like the dev container): a
# GitHub Actions `container:` job runs with HOME=/github/home, so a rustup under
# /root would be invisible and cargo would try to re-download the toolchain on
# every leg — defeating the point. Pinning RUSTUP_HOME/CARGO_HOME to absolute
# /usr/local paths (the official `rust` image's pattern) makes the toolchain
# resolve regardless of HOME.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:${PATH}
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain 1.99.1 --profile minimal \
            --no-modify-path \
    && rustup component add clippy rustfmt \
    && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# Where llvm-sys / inkwell (llvm18-0) find LLVM 18 — matches ci.yml and the root
# Dockerfile. Baked into the image env so the CI leg inherits it with no setup.
ENV LLVM_SYS_180_PREFIX=/usr/lib/llvm-18
ENV LLVM_SYS_181_PREFIX=/usr/lib/llvm-18

WORKDIR /work
CMD ["bash"]
