# Convenience targets. The `docker-*` targets build and run inside the LLVM 18
# dev container (see .devcontainer/Dockerfile) with the repo mounted at /work and
# a separate Linux target dir so they never clash with the host macOS target/.

IMAGE := zap-dev
MOUNTS := -v "$(CURDIR)":/work \
          -v zap-target:/work/target-docker \
          -v zap-cargo-registry:/root/.cargo/registry \
          -e CARGO_TARGET_DIR=/work/target-docker
DOCKER_RUN := docker run --rm $(MOUNTS) $(IMAGE)

.PHONY: docker-build docker-test docker-build-crate docker-shell docker-run

## Build the dev container image.
docker-build:
	docker build -t $(IMAGE) -f .devcontainer/Dockerfile .

## Run the full test suite (incl. LLVM codegen) in the container, under both
## collectors: the default precise Immix GC (ticket #116) and the Boehm
## fallback. Mirrors the `full` CI check.
docker-test:
	$(DOCKER_RUN) cargo test --workspace --features zapc/gc-immix
	$(DOCKER_RUN) cargo test --workspace --features zapc/gc-boehm

## Compile the whole workspace (incl. codegen, default Immix collector) in the
## container.
docker-build-crate:
	$(DOCKER_RUN) cargo build --workspace --features zapc/llvm

## Open an interactive shell in the container.
docker-shell:
	docker run --rm -it $(MOUNTS) $(IMAGE) bash

## Compile and run a .zap file in the container, e.g. make docker-run FILE=tests/golden/fib.zap
docker-run:
	$(DOCKER_RUN) cargo run -q -p zapc --features llvm -- run $(FILE)
