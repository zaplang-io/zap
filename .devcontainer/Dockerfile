# Zap development container: Rust + LLVM 18, for building the LLVM codegen
# backend (which needs libLLVM, unavailable as a Homebrew bottle on Intel macOS).
#
# The repo is mounted at /work at runtime; builds use a separate Linux target
# directory (CARGO_TARGET_DIR) so they never clash with the host's macOS
# target/. See the Makefile `docker-*` targets.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# LLVM 18 + the system libraries llvm-sys links against, a C toolchain, and curl.
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
        zlib1g-dev \
        libzstd-dev \
        libffi-dev \
        libedit-dev \
        libxml2-dev \
        libncurses-dev \
    && rm -rf /var/lib/apt/lists/*

# Rust, pinned to match rust-toolchain.toml.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain 1.99.1 --profile minimal
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup component add clippy rustfmt

# Where llvm-sys finds LLVM 18. inkwell's `llvm18-0` feature uses llvm-sys 180;
# we set both prefixes so either resolves against the installed LLVM 18.
ENV LLVM_SYS_180_PREFIX=/usr/lib/llvm-18
ENV LLVM_SYS_181_PREFIX=/usr/lib/llvm-18

WORKDIR /work
CMD ["bash"]
