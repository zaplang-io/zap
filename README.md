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

`cargo test` runs the `zapc_syntax` unit tests and the golden-harness
fixture checks. The end-to-end golden run is `#[ignore]`d until Phase 1 produces
a runnable binary; enable it with:

```bash
cargo test -- --ignored
```

## Phase 0 scope

This scaffold freezes the front-end data structures so later work can proceed in
parallel: source spans, the token set, and the AST (`crates/syntax`), plus the
CLI shape (`crates/driver`) and a golden-test harness. No lexing, parsing, type
checking or codegen is implemented yet — those are Phase 1 and beyond.
