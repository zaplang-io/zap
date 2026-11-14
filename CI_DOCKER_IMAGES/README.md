# CI toolchain base image

**Image:** `ghcr.io/zaplang-io/zap-ci` · **Issue:** [#854](https://github.com/zaplang-io/zap/issues/854) (folds in [#296](https://github.com/zaplang-io/zap/issues/296)) · **Platform:** Linux x86_64

The build environment Zap's Linux CI runs **inside**. It bakes the full LLVM 18
+ Rust + C-dependency toolchain so the `full` CI leg no longer installs LLVM and
the C headers from third-party mirrors (apt.llvm.org and friends) on every job.

## Why

Those per-job installs are slow (minutes × the matrix fan-out) and are the
**dominant source of CI hangs**: a job stalls mid-download of LLVM and the whole
run wedges until it times out. Baking the toolchain into an image on the org
GHCR means the only network step the leg does is the image pull from GitHub's
own registry/CDN — fast, same trust domain, resumable — instead of a hangy
mirror fetch on the critical path.

## What's baked

| Component | Detail |
| --- | --- |
| **LLVM 18** | `llvm-18`, `llvm-18-dev`, `libpolly-18-dev`, `clang-18`, `lld-18`, `libclang-18-dev` — LLVM **18.1.3** (the 18.1 series Ubuntu 24.04 LTS ships). `LLVM_SYS_180_PREFIX` / `LLVM_SYS_181_PREFIX` are set to `/usr/lib/llvm-18` in the image env. |
| **Rust** | `1.99.1` (pinned to `rust-toolchain.toml`) with `clippy` + `rustfmt`. |
| **C deps (dev + runtime)** | OpenSSL (`libssl-dev`), PCRE2 (`libpcre2-dev`), zlib (`zlib1g-dev`), libstdc++ (via `build-essential`), plus `libgc-dev`, `python3-dev`, `libzstd-dev`, `libffi-dev`, `libedit-dev`, `libxml2-dev`, `libncurses-dev`, `pkg-config`. |
| **Numeric accelerator** | OpenBLAS + LAPACK/LAPACKE (`libopenblas-dev`, `liblapacke-dev`) — the array perf epic's cblas/lapacke provider on Linux (#912/#914; Apple Accelerate on macOS). Lets the accelerator path build and link in CI. |
| **Linkers / build perf** | `mold` (Rust's own link step, `-fuse-ld=mold`), `lld-18`, `sccache`. |
| **Tooling** | `git`, **Node 20 LTS** (folds in #296), `curl`, `ca-certificates`. |

The package list is kept **identical to `.devcontainer/Dockerfile`** (the source
of truth) and the `full` job's former inline install in `.github/workflows/ci.yml`,
plus `lld-18` and Node. CI builds in the same environment developers build in.

### No LLVM version skew

Everything LLVM installs in **one apt transaction**, so `clang-18`, `lld-18`,
`llvm-link`, `opt` and `libLLVM-18` are all the same point release — there is no
skew against inkwell's `llvm18-0`, which the LTO link path (`llvm-link` +
`opt -O2` + `clang-18`, I#841/D-0150) depends on. The Dockerfile ends the LLVM
layer with `test "$(llvm-config --version | cut -d. -f1-2)" = "18.1"`, so if
noble ever moves off the 18.1 series the image build fails loudly rather than
shipping a silent bump. The base layer is pinned by digest for reproducibility.

## How it's built and published

`.github/workflows/ci-image.yml` is **path-scoped**: it triggers **only when
`CI_DOCKER_IMAGES/**` (or the workflow itself) changes**, so it never burns
action minutes on unrelated PRs.

| Event | Behaviour |
| --- | --- |
| **PR** touching `CI_DOCKER_IMAGES/**` | Build the image and smoke-test it — **no push** (validate the Dockerfile only). |
| **Push to `main`** | Build, smoke-test, and push `:latest` **and** `:sha-<short>` to `ghcr.io/zaplang-io/zap-ci`. |

The smoke test runs the built image and checks the toolchain reports itself
(`rustc`, `cargo clippy`, `clang-18`, `llvm-config`, `node`, `git`, and that
`LLVM_SYS_180_PREFIX` points at a real LLVM 18).

> **Local Docker is intentionally not used** — this project's dev environment
> cannot build Docker images locally, so the Dockerfile is authored and
> validated entirely in CI. CI is the sole gate.

## How CI consumes it

The Linux `full` leg (LLVM 18 + codegen + goldens) in `.github/workflows/ci.yml`
runs as `container: ghcr.io/zaplang-io/zap-ci:latest` and **drops** its per-job
LLVM + C-dep install and its per-job `rustup` install. Zap CI always pulls
**`:latest`**; the `:sha-<short>` tag exists only if we later want to pin a leg
to an exact image for reproducibility.

> **Rollout (bootstrap order).** A `container:` job can only pull an image that
> already exists in the registry, but this image is published *on merge to
> `main`*. So the image and the `full`-leg flip cannot land in one green PR: the
> flip's CI would try to pull an image that does not exist yet. The image +
> this workflow land first (this folder); the `full`-leg flip in `ci.yml` is a
> fast-follow that goes green once `:latest` is live and readable by Actions
> (the consuming job authenticates to GHCR with the repo `GITHUB_TOKEN` +
> `packages: read`).

### Tags

- **`:latest`** — moves with `main`; what CI pulls.
- **`:sha-<short>`** — immutable provenance tag for each `main` build.

### Scope / not converted

- **`host` (no-LLVM leg)** stays on the bare runner: it exists to prove the
  default members build and test **without** LLVM, and it installs no LLVM / C
  deps, so it is not a hang source.
- **`arm64` (aarch64)** stays on the bare runner: this image is x86_64 only
  (#854 is Linux x86_64 scoped); a multi-arch builder image is a future
  follow-up.
- **macOS / Windows** legs keep their own toolchain setup — a Linux container
  cannot serve them (out of scope, tracked separately).

## Updating the image

1. Edit `CI_DOCKER_IMAGES/Dockerfile` (keep the apt list and Rust pin in sync
   with `.devcontainer/Dockerfile`).
2. Open a PR — the path-scoped workflow builds and smoke-tests it (no push).
3. On merge to `main`, `:latest` and a new `:sha-<short>` are published; the
   next CI run on `main` pulls the updated `:latest`.

See also [`docs/DOCKER.md`](../docs/DOCKER.md) for the separate slim **runtime**
image (`ghcr.io/zaplang-io/zap`).
