# Convenience targets. The `docker-*` targets build and run inside the LLVM 18
# dev container (see .devcontainer/Dockerfile) with the repo mounted at /work.
#
# Builds are faster AND memory-bounded (tickets #128, #123):
#   * resource caps  — a single run has a predictable memory ceiling and two can
#     run concurrently without swapping the host to death (#128);
#   * isolated target — a per-worktree cargo target volume so concurrent runs
#     from different worktrees can't contaminate each other (#123);
#   * shared sccache — a compiler cache shared across worktrees/runs so sibling
#     and successive builds reuse compiled artifacts (#128);
#   * mold linker    — the link step's RAM peak is a fraction of GNU ld's (#128).
# See the "Build performance" section of README.md for the full workflow and
# how to tune the knobs below.

IMAGE := zap-dev

# --- #123: isolated cargo target, per worktree -------------------------------
# A named volume keyed on this worktree's absolute path. Different worktrees get
# different volumes (no cross-contamination); the SAME worktree reuses its
# volume across runs (warm build cache). Override ZAP_TARGET_VOLUME to pin one.
WORKTREE_ID := $(shell printf '%s' "$(CURDIR)" | shasum -a 256 | cut -c1-12)
ZAP_TARGET_VOLUME ?= zap-target-$(WORKTREE_ID)

# --- #128: shared, read-mostly caches (safe across worktrees and runs) -------
#   * the crate-source registry (download cache);
#   * the sccache compiler cache: hashed (toolchain, flags, source) -> object,
#     so a sibling worktree or a later run reuses compiled artifacts instead of
#     recompiling from scratch. Distinct from the per-worktree target above:
#     the target is isolated, the cache is shared.
ZAP_REGISTRY_VOLUME ?= zap-cargo-registry
ZAP_SCCACHE_VOLUME  ?= zap-sccache

# --- #128: resource ceiling for ONE run --------------------------------------
# Bound a single run so its peak stays predictable. `--memory-swap` == `--memory`
# disables container swap, so an over-budget build fails fast instead of
# swapping the host to death. Two concurrent runs need the Docker VM to hold
# ~2 x ZAP_DOCKER_MEMORY plus headroom (raise Docker Desktop's memory, or lower
# this). Tune via the environment, e.g. `ZAP_DOCKER_MEMORY=8g make docker-test`.
ZAP_DOCKER_MEMORY ?= 5g
ZAP_DOCKER_CPUS   ?= 4
# Cap parallel rustc processes — the dominant lever on peak RAM. Keep it <= CPUS.
ZAP_BUILD_JOBS    ?= 4

MOUNTS := -v "$(CURDIR)":/work \
          -v $(ZAP_TARGET_VOLUME):/work/target-docker \
          -v $(ZAP_REGISTRY_VOLUME):/root/.cargo/registry \
          -v $(ZAP_SCCACHE_VOLUME):/sccache

# sccache is incompatible with incremental compilation, so force it off here
# (the final-gate build doesn't need it); host-side `make check` keeps cargo's
# incremental default for fast iteration. The mold linker slashes the link-step
# RAM peak; scope it to the Linux target so it never touches a host build.
ENVS := -e CARGO_TARGET_DIR=/work/target-docker \
        -e CARGO_BUILD_JOBS=$(ZAP_BUILD_JOBS) \
        -e CARGO_INCREMENTAL=0 \
        -e RUSTC_WRAPPER=sccache \
        -e SCCACHE_DIR=/sccache \
        -e CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS=-Clink-arg=-fuse-ld=mold

LIMITS := --memory=$(ZAP_DOCKER_MEMORY) --memory-swap=$(ZAP_DOCKER_MEMORY) \
          --cpus=$(ZAP_DOCKER_CPUS)

DOCKER_RUN := docker run --rm --init $(LIMITS) $(MOUNTS) $(ENVS) $(IMAGE)

.PHONY: docker-build docker-test docker-check docker-build-crate docker-shell \
        docker-run docker-sccache-stats check clean-docker-target

## Build the dev container image.
docker-build:
	docker build -t $(IMAGE) -f .devcontainer/Dockerfile .

## Run the full test suite (incl. LLVM codegen) in the container, under both
## collectors: the default precise Immix GC (ticket #116) and the Boehm
## fallback. Mirrors the `full` CI check. This is the FINAL GATE — iterate with
## `make check` / `make docker-check` and run this before you push.
docker-test:
	$(DOCKER_RUN) cargo test --workspace --features zapc/gc-immix
	$(DOCKER_RUN) cargo test --workspace --features zapc/gc-boehm

## Fast full-workspace type-check in the container (codegen included, NO
## codegen/link). Use this to iterate on codegen-touching changes; far cheaper
## than docker-test.
docker-check:
	$(DOCKER_RUN) cargo check --workspace --features zapc/gc-immix

## Compile the whole workspace (incl. codegen, default Immix collector) in the
## container.
docker-build-crate:
	$(DOCKER_RUN) cargo build --workspace --features zapc/llvm

## Show sccache hit/miss stats (verify the shared cache is working).
docker-sccache-stats:
	docker run --rm $(MOUNTS) $(ENVS) $(IMAGE) sccache --show-stats

## Open an interactive shell in the container.
docker-shell:
	docker run --rm -it $(LIMITS) $(MOUNTS) $(ENVS) $(IMAGE) bash

## Compile and run a .zap file in the container, e.g. make docker-run FILE=tests/golden/fib.zap
docker-run:
	$(DOCKER_RUN) cargo run -q -p zapc --features llvm -- run $(FILE)

## Fast host-side type-check of the LLVM-free crates (front end + driver). No
## container, no codegen, no link — the quickest iteration loop. Keeps cargo's
## incremental default on. Codegen type-errors still need `make docker-check`.
check:
	cargo check --all-targets

## Remove this worktree's isolated target volume (frees disk; forces a cold
## rebuild next run). The shared registry/sccache volumes are left intact.
clean-docker-target:
	-docker volume rm $(ZAP_TARGET_VOLUME)
