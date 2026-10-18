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

## Repository layout

```
Cargo.toml              workspace
crates/
  syntax/               zapc_syntax: source spans, tokens, AST (Phase 0)
  driver/               zapc: the compiler CLI (stub in Phase 0)
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

`zapc run <file.zap>` runs the full pipeline (lex → parse → type-check → LLVM →
link → run); `zapc build <file.zap> [-o out]` stops after producing the
executable.

## Status

The vertical slice runs end to end: `fib.zap` → `55` and `loop_sum.zap` → `45`.
Implemented so far — lexer, parser, type checker (scalars + functions with
inference), and LLVM codegen for the slice (int64/f64/bool, functions incl.
recursion, control flow, `for`-over-`range`, calls, `print`). See `ROADMAP.md`
for what's next.
