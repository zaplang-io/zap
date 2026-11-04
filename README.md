# Zap

A statically typed, ahead-of-time compiled language with **Python-like
readability** and **native performance** — aimed at Python developers who need
speed. Python feel, Go-style toolchain and concurrency, with a compile-time
metaprogramming system for keeping business logic free of boilerplate.

> Status: **active pre-1.0 development.** The compiler and toolchain already
> cover a broad language surface, including native code generation and project
> tooling. The exact contract varies by feature: see the [stability policy](docs/STABILITY.md)
> for Stable, Experimental, and Internal tiers, and the [specification](SPEC.md)
> for language behavior.

## Documents

- [`docs/REPO_ARCHITECTURE.md`](docs/REPO_ARCHITECTURE.md) — "where to find what": the compiler pipeline, crate map, runtime/GC, stdlib, tooling, and tests.
- [`SPEC.md`](SPEC.md) — the language specification (the source of truth).
- [`DECISIONS.md`](DECISIONS.md) — the language and implementation decision log.
- [`docs/STABILITY.md`](docs/STABILITY.md) — versioning & stability policy (the 1.0 contract).
- [`CHANGELOG.md`](CHANGELOG.md) — notable changes, per [Keep a Changelog](https://keepachangelog.com/) (pre-1.0 / editorial 0.x).
- [`docs/RELEASING.md`](docs/RELEASING.md) — the release process: tag convention and how the version bumps.
- [`ROADMAP.md`](ROADMAP.md) — the original phased build plan; use the issue tracker and stability policy for current project status.
- [`OPEN-QUESTIONS.md`](OPEN-QUESTIONS.md) — the tracked backlog.
- [`docs/GRAMMAR.ebnf`](docs/GRAMMAR.ebnf) — the grammar.
- [`design/`](design/) — per-feature design docs.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) · [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) · [`SECURITY.md`](SECURITY.md) — contributing, conduct, and vulnerability reporting.

## Repository layout

```
Cargo.toml              Rust workspace
crates/
  syntax/               lexer, parser, AST, source spans
  sema/                 semantic analysis and built-in module contracts
  ir/                   typed intermediate representation and lowering
  codegen/              LLVM backend and native runtime
  driver/                `zap` CLI, project workflows, and test harnesses
  fmt/, doc/, pkg/, ...  formatter, API docs, package tooling, and support crates
docs/                    language tooling and platform documentation
design/                  feature and runtime designs
tests/
  golden/               language and runtime regression programs
  conformance/           executable checks for documented language claims
  diff/                  CPython differential tests for the shared subset
```

The compiler pipeline is:
lex → parse → semantic analysis → typed IR → lowering → LLVM codegen → native binary.

## Building

Requires a Rust toolchain (1.74+). If you don't have one:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then, from the repo root:

```bash
cargo build
cargo test
```

The default workspace build and test run cover the host-side compiler and
tooling crates. **Codegen needs LLVM** and is excluded from the default build
(it is gated behind the driver's `llvm` feature), so `cargo build` and the
default `cargo test` do not require LLVM.

## Compiling and running Zap programs (codegen)

Codegen lowers typed IR to native code via **LLVM** (D-0042). The backend
requires LLVM 18. The provided dev container (`.devcontainer/Dockerfile`) pins
that toolchain, and the `Makefile` wraps its common workflows:

```bash
make docker-build                         # build the LLVM 18 dev image (once)
make docker-test                          # full suite incl. LLVM codegen
make docker-run FILE=tests/golden/fib.zap # compile + run a program  -> 55
```

To **use** the toolchain (rather than hack on it) without building from source,
pull the prebuilt image `ghcr.io/zaplang-io/zap` — handy as a CI `container:`
for Zap package repos. See [`docs/DOCKER.md`](docs/DOCKER.md).

## Build performance and workflow: cached and memory-bounded (tickets #128, #123)

A full LLVM codegen build is heavy — the link step is the RAM peak, and running
several at once can swap a laptop to death. The build setup is tuned so a single
run has a predictable ceiling and concurrent runs from different worktrees are
safe. **The iteration workflow:**

```bash
make check          # host type-check of the LLVM-free crates — the quickest loop
make docker-check   # full-workspace type-check incl. codegen (no link) in the container
make docker-test    # FINAL GATE: full suite under both collectors — run before you push
```

Iterate with `cargo check` (`make check`, and `make docker-check` for
codegen-touching changes); it type-checks without codegen or linking, so it is
dramatically faster than a full build. Run the full `make docker-test` only as
the last step before pushing.

What makes it fast and bounded:

- **Resource cap (#128).** Each `docker-test` run is capped (`--memory`,
  `--cpus`) with container swap disabled, and parallel `rustc` is capped
  (`CARGO_BUILD_JOBS`), so one run's peak is predictable and an over-budget
  build fails fast instead of swapping the host. Tune per machine:

  ```bash
  ZAP_DOCKER_MEMORY=8g ZAP_DOCKER_CPUS=6 ZAP_BUILD_JOBS=6 make docker-test
  ```

  Two concurrent runs need the Docker VM to hold ~2× `ZAP_DOCKER_MEMORY` plus
  headroom — raise Docker Desktop's memory allotment, or lower the cap.
- **Isolated target per worktree (#123).** Each git worktree gets its own named
  cargo target volume (`zap-target-<worktree-hash>`), so concurrent runs from
  different worktrees can't reuse each other's stale artifacts (the
  shared-`zap-target` footgun that produced spurious failures in the stdlib
  wave). The same worktree reuses its volume across runs for a warm cache.
  `make clean-docker-target` drops just this worktree's volume.
- **Shared compiler cache (#128).** `sccache` is wired as `RUSTC_WRAPPER` over a
  shared `zap-sccache` volume, so a sibling worktree or a later run reuses
  compiled artifacts. Distinct from the target above: **the target is isolated,
  the cache is shared.** Check it is working with `make docker-sccache-stats`.
- **mold linker (#128).** Rust's own link step uses `mold` (via
  `-fuse-ld=mold`), a faster, lower-RAM linker than GNU ld. On this workspace
  (few deps, small binaries) it is mainly a link-*time* win — the compile peak
  is dominated by parallel `rustc`, so `CARGO_BUILD_JOBS` + the hard cap are the
  real memory levers. Mirrored in the `full` CI job.

Build times and test counts change with the workspace and host. The Makefile
keeps the target isolated per worktree, shares compiled artifacts through
`sccache`, and caps Docker resources. Use `make check` and `make docker-check`
while iterating, then run `make docker-test` for the full gate.

## The `zap` CLI (D-0053)

`zap` is the one user-facing tool; it drives the internal `zapc` compiler.

```
zap run [file]        compile + run a file, or the project (via zap.toml)
zap build [file]      build a native binary (default: target/<name>)
zap ir [file]         dump ZIR (diagnostic)
zap expand [file]     show decorator/@derive-generated code (D-0036)
zap fmt [paths]       rewrite source in the canonical style (D-0037)
zap doc <file>        render API docs — HTML site, Markdown, or JSON model
zap test [--doc]      run the @test suite, or docstring examples with --doc (#234)
zap new <name>        scaffold a new project (writes zap.toml)
zap init              scaffold zap.toml in the current directory
zap add/remove        manage manifest dependencies
zap zenv              manage the project environment
zap publish/login     publish a package or manage registry credentials (experimental)
zap search/info       discover packages in a registry (experimental)
zap mcp               serve language reference and code tools over stdio
zap lsp               start the language server over stdio
zap repl              start the interactive JIT session
zap playground        start the local web playground (#238)
zap inspect           inspect a running process and its runtime
zap explain <code>    explain a ZAP diagnostic code
zap version           print the toolchain version
zap help              print usage
```

`zap fmt --check` checks formatting without changing files. `zap run`,
`zap build`, `zap test`, `zap repl`, and `zap playground` require the LLVM
codegen feature; front-end and project commands can be built without LLVM.
Stability varies by surface: package publishing/discovery, the playground, and
several standard-library modules are Experimental. The hosted HTTPS publish API
is not enabled; local and `file://` indexes are supported. See
[`docs/STABILITY.md`](docs/STABILITY.md) for the current tiers.

`zap doc <file>` renders one module's public API (functions, types with their
fields and methods, enums, interfaces, and constants, with signatures and
docstrings). `--format html` (the default) writes a self-contained static site
to `--out` (default `doc/`); `--format md` emits Markdown; `--format json`
prints the serializable doc model (issue #232). `--private` includes
`_`-prefixed declarations (D-0028).

`zap test --doc [path]` runs **doctests** (issue #234, D-0074): a fenced
```` ```zap ```` block in any docstring is a runnable example, executed through
the REPL engine (identical semantics to compiled code) and checked against its
`#`-comment directives — `# => <line>` for expected stdout, `# raises: <substr>`
for a runtime exception, `# error: ZAPnnnn` for an expected compile error (no
directive = must run clean). Examples are self-contained (run in a fresh
session); doctests reuse the `zap test` report and exit non-zero on any failure.

`zap playground` (issue #238, D-0075) starts a local web UI — an editor and a
**Run** button — that compiles and runs a snippet server-side through the same
REPL `Session` engine and shows its stdout, auto-printed value, and `ZAPnnnn`
diagnostics. Because WASM is post-1.0, snippets cannot run in the browser, so the
server executes untrusted native code: **each request runs in its own short-lived
worker process** under resource limits (CPU, memory, file size, no core) with the
parent enforcing a per-snippet time budget, an output cap, and a concurrency cap.
A crashing snippet kills only its worker; the server stays up, and no two requests
share state. It binds `127.0.0.1` by default and is codegen-gated like `zap run`;
true filesystem/network denial (an OS sandbox) is deployment hardening required
before any public exposure. Share links are stateless (the snippet is encoded in
the URL fragment). See `design/0026-playground.md`.

`zap run`/`zap build` need codegen, so they run under the LLVM path:

```bash
make docker-run FILE=tests/golden/hello.zap   # uses the internal `zapc`
# inside the container: cargo run -p zapc --features llvm --bin zap -- run <file>
```

## Language and toolchain status

Zap has an end-to-end native compiler and an actively developed toolchain. The
implemented language includes typed declarations and inference, functions and
closures, control flow and pattern matching, structs and enums, exceptions,
generators, collections, modules, and structured concurrency. The repository
also includes an LLVM runtime, standard-library modules, package management,
tests and doctests, formatting, API documentation, an LSP server, an MCP server,
and a REPL. This list describes available implementation areas, not a promise
that every feature is complete on every target. The authoritative per-feature
status is in [`docs/STABILITY.md`](docs/STABILITY.md); documented behavior is in
[`SPEC.md`](SPEC.md).

The experimental `crypto.rsa_*` APIs accept ordinary RSA PEM keys and use
PKCS#1 v1.5 padding. Other key families, including RSA-PSS-only keys, are
rejected; see [D-0102](DECISIONS.md#d-0102--crypto-rsa-pkcs1-v15-signing-i628).

Verifier configuration failures emit fixed DEBUG messages through `logging`;
its default WARNING threshold suppresses them. Ordinary signature mismatches
remain quiet.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for how to build, test, and send a change.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
