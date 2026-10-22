# Zap

A statically typed, ahead-of-time compiled language with **Python-like
readability** and **native performance** — aimed at Python developers who need
speed. Python feel, Go-style toolchain and concurrency, with a compile-time
metaprogramming system for keeping business logic free of boilerplate.

> Status: **early design + Phase 0 scaffold.** The language is fully specified
> (see below); the compiler is just getting started.

## Documents

- [`SPEC.md`](SPEC.md) — the language specification (the source of truth).
- [`DECISIONS.md`](DECISIONS.md) — the decision log (ADR), D-0001…D-0041.
- [`ROADMAP.md`](ROADMAP.md) — phased build plan + parallelization map.
- [`OPEN-QUESTIONS.md`](OPEN-QUESTIONS.md) — the tracked backlog.
- [`docs/GRAMMAR.ebnf`](docs/GRAMMAR.ebnf) — the grammar.
- [`design/`](design/) — per-feature design docs.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) · [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) · [`SECURITY.md`](SECURITY.md) — contributing, conduct, and vulnerability reporting.

## Repository layout

```
Cargo.toml              workspace
crates/
  syntax/               zapc_syntax: source spans, tokens, AST (Phase 0)
  driver/               the `zap` CLI + the internal `zapc` compiler
docs/GRAMMAR.ebnf       the grammar
tests/golden/           golden programs (*.zap) + expected output (*.expected)
```

The compiler pipeline (mirrors Go's structure) will be:
lex → parse → type-check → typed IR → lower → LLVM codegen → native binary.
See `ROADMAP.md`.

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

This builds the front-end crates (`zapc_syntax`, `zapc_sema`) and the driver.
**Codegen needs LLVM** and is excluded from the default build (it's gated behind
the driver's `llvm` feature), so `cargo build`/`cargo test` work on any host.

## Compiling and running Zap programs (codegen)

Codegen lowers the type-checked AST to native code via **LLVM** (D-0042). Because
LLVM isn't available as a Homebrew binary on Intel macOS, the LLVM build runs in
a dev container (`.devcontainer/Dockerfile`: Ubuntu + LLVM 18 + Rust). The
`Makefile` wraps it:

```bash
make docker-build                         # build the LLVM 18 dev image (once)
make docker-test                          # full suite incl. LLVM codegen
make docker-run FILE=tests/golden/fib.zap # compile + run a program  -> 55
```

## Build performance: fast + memory-bounded (tickets #128, #123)

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

### Measured (this host: 16 GB macOS, Docker Desktop VM = 8.5 GB / 4 CPU)

Full workspace incl. LLVM codegen, `CARGO_BUILD_JOBS=4`. "cold" = empty target
+ empty cache; "warm cache" = empty target but shared `sccache` populated.

| metric | before (GNU ld, no cache) | after (mold + sccache) |
|---|---|---|
| cold full build (fresh target) | 30 s | 26 s |
| rebuild, fresh target + warm sccache | 30 s (no cache: always cold) | **9 s** (55 reqs → 35 hits, 0 miss) |
| warm `cargo check`, host, after editing `lower.rs` (7k lines) | ~1.4 s | ~1.4 s |
| full `cargo test` suite (Immix, 309 tests) | — | 76 s |
| peak RSS, cold full build | ~1.0 GB | ~1.2 GB (parallel-compile bound, not link) |
| per-run memory ceiling | unbounded (≤ VM, could swap host) | **hard `--memory` cap, swap off** |

Takeaways: the shared cache turns a sibling/repeat build from a 30 s cold
rebuild into a **9 s** cache-hit build; a warm `cargo check` (~1.4 s) is ~18×
faster than a cold build and ~50× faster than the full test gate; and the
`--memory` cap with swap disabled makes each run's ceiling a hard, predictable
bound instead of "whatever the host has" — so concurrent runs can't swap the
machine to death.

## The `zap` CLI (D-0053)

`zap` is the one user-facing tool; it drives the internal `zapc` compiler.

```
zap run [file]        compile + run a file, or the project (via zap.toml)
zap build [file]      build a native binary (default: target/<name>)
zap ir [file]         dump ZIR (diagnostic)
zap expand [file]     show decorator/@derive-generated code (D-0036)
zap new <name>        scaffold a new project (writes zap.toml)
zap init              scaffold zap.toml in the current directory
zap version | help
```

`zap run`/`zap build`/`zap expand`/`zap ir`, `zap new`/`init`, and
`zap version`/`help` are implemented. `zap fmt` (D-0037), `zap test` (issue #73),
`zap add`/`remove` and `zap zenv` (issue #69), and `zap mcp` (issue #72) are
wired as stubs that name their owning ticket, so the command surface is stable
while those systems land.

`zap run`/`zap build` need codegen, so they run under the LLVM path:

```bash
make docker-run FILE=tests/golden/hello.zap   # uses the internal `zapc`
# inside the container: cargo run -p zapc --features llvm --bin zap -- run <file>
```

## Status

The vertical slice runs end to end: `fib.zap` → `55` and `loop_sum.zap` → `45`.
Implemented so far — lexer, parser, type checker (scalars + functions with
inference), and LLVM codegen for the slice (int64/f64/bool, functions incl.
recursion, control flow, `for`-over-`range`, calls, `print`). See `ROADMAP.md`
for what's next.

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
