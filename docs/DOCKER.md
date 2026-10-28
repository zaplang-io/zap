# The prebuilt Zap toolchain image

**Status:** Tooling / platform (companion to I#207 release process and I#208
prebuilt artifacts). **Last updated:** 2026-10-28.

`ghcr.io/zaplang-io/zap` is a prebuilt image carrying the `zap` CLI and
everything it needs to **compile, link and run** Zap programs. It exists so that
non-core package repos (e.g. `zaplang-io/aws`) and contributors can get the
toolchain without building the LLVM-backed compiler from source — point CI at it
with `container:` and run `zap test`.

It is the containerized companion to the from-source release artifacts
([I#208](https://github.com/zaplang-io/zap/issues/208)) and tracks the same
version scheme as [`docs/RELEASING.md`](RELEASING.md) (I#207).

## Pull it

```bash
docker pull ghcr.io/zaplang-io/zap:latest
```

```bash
# Compile and run a program with the containerized toolchain.
docker run --rm -v "$PWD":/work -w /work ghcr.io/zaplang-io/zap:latest \
    zap run hello.zap
```

The image puts `zap` on `PATH`; its default entrypoint command is `zap --help`.

## Use it in CI as a container

Any GitHub Actions job can run *inside* the image — the toolchain is already
there, so there is no LLVM build step:

```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    container: ghcr.io/zaplang-io/zap:latest
    steps:
      - uses: actions/checkout@v4
      - run: zap test
```

Pin to a specific version (`:0.1.0`) or commit (`:sha-abc1234`) for reproducible
CI; `:latest` floats with `main`.

## Tags

| Tag            | Points at                              | Published on          |
| -------------- | -------------------------------------- | --------------------- |
| `latest`       | the newest `main` build                | every push to `main`  |
| `sha-<short>`  | one exact `main` commit                | every push to `main`  |
| `X.Y.Z`        | the `vX.Y.Z` release (e.g. `0.1.0`)    | release tag `v*`       |
| `X.Y`          | the latest patch of that minor         | release tag `v*`       |

Version tags follow the toolchain version in the root `Cargo.toml` and the
`vX.Y.Z` git-tag convention in [`docs/RELEASING.md`](RELEASING.md). Pre-1.0
(`0.x`) makes no compatibility promise (see [`docs/STABILITY.md`](STABILITY.md));
pin a tag you have tested against.

## What's inside

Two-stage build (see [`/Dockerfile`](../Dockerfile)):

- **Build stage** mirrors [`.devcontainer/Dockerfile`](../.devcontainer/Dockerfile)
  — Ubuntu 24.04 + LLVM 18 + the pinned Rust toolchain — and builds the `zap`
  release binary with the default **precise Immix GC** (`--features llvm`).
- **Runtime stage** is a slim Ubuntu 24.04 with just the `zap` binary plus the
  C toolchain and libraries the compiler's link step invokes: `clang-18`/`lld-18`
  (the `cc` driver, matching the LLVM 18 object `zap` emits), `libstdc++`,
  `libgc` (Boehm), PCRE2, zlib, OpenSSL and libpython. These are required
  because `zap build` compiles the C runtime (`zaprt.c`) and links it on the fly
  — the image must be able to *build* programs, not only parse them.

The default collector is the precise Immix GC (D-0009). The Boehm collector's
library (`libgc`) is present too, so a toolchain rebuilt with `--features
gc-boehm` also links.

## Portable builds for Amazon Linux 2023 / AWS Lambda

By default `zap build` emits a binary **dynamically linked** against the image's
glibc (2.39 on Ubuntu 24.04) plus libstdc++, PCRE2, zlib, OpenSSL and
libpython3.12. That binary **does not run on older-glibc distros** — notably
**Amazon Linux 2023** (glibc 2.34), the base of the AWS `provided.al2023` Lambda
runtime, where it exits 127 because glibc symbol versions are fixed at link time
(installing the libraries on AL2023 does not help). See issue #287.

Pass `--portable` (or set `ZAP_PORTABLE=1`) to build a **self-contained static
binary** that runs on AL2023 and essentially any Linux, with no toolchain or
shared libraries required at the destination (D-0082):

```bash
# Build in the toolchain image, deploy the binary anywhere — e.g. an AL2023 /
# provided.al2023 Lambda custom runtime, a scratch/distroless image, Alpine.
docker run --rm -v "$PWD":/w -w /w ghcr.io/zaplang-io/zap:latest \
    zap build --portable bootstrap.zap -o bootstrap
```

`--portable` links statically (`-static`, so there is **no glibc version floor
at all**) and **drops CPython interop**: libpython is not present on AL2023 and
static libpython is impractical, so a portable binary's `import python` raises a
catchable runtime error instead of linking the interpreter. Use the default
(non-portable) build when you need `import python` (D-0012, #27).

**Trade-offs** (accepted for a deploy bootstrap): static glibc means NSS-backed
`getaddrinfo` cannot load nsswitch plugins (ordinary DNS resolution is
unaffected), OpenSSL engine loading via `dlopen` is unavailable (the TLS client
does not use it), and the binary is larger. `ZAP_PORTABLE=1` lets an image or CI
job default every build to portable without the flag.

The smoke test below proves a `--portable` binary built in this (Ubuntu) image
runs unchanged inside stock `public.ecr.aws/amazonlinux/amazonlinux:2023`.

## Smoke test

[`scripts/docker-smoke.sh`](../scripts/docker-smoke.sh) verifies an image can
compile+run (`zap run`), build a native binary and execute it (`zap build`),
run `zap test`, and build a `--portable` binary and run it inside stock Amazon
Linux 2023 (the #287 portability proof) — the full codegen→link→execute path:

```bash
scripts/docker-smoke.sh ghcr.io/zaplang-io/zap:latest
```

CI runs it on every image build (see below).

## How it's published

[`.github/workflows/docker.yml`](../.github/workflows/docker.yml), separate from
`ci.yml` and **off** the required `CI gate`:

- **pull requests** that touch the image definition build + smoke-test the image
  **without pushing** (validation);
- **pushes to `main`** build, smoke-test, then push `latest` and `sha-<short>`;
- **release tags `v*`** push the `X.Y.Z` and `X.Y` version tags.

It authenticates to GHCR with the workflow's `GITHUB_TOKEN` (`packages: write`),
builds with `docker/build-push-action`, and uses GitHub Actions layer caching so
the LLVM build is reused across runs.

> **One-time maintainer setup.** GHCR packages are **private** on first push.
> After the first `main` build publishes `ghcr.io/zaplang-io/zap`, a maintainer
> must make the package **public** and link it to the repo in the org's package
> settings (GitHub → the package → *Package settings* → *Change visibility* →
> *Public*, and *Connect repository*). The image push already stamps the
> `org.opencontainers.image.source` label so it associates with this repo once
> public. This is a settings action, not something the workflow can do.
