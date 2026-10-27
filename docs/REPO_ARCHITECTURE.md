# Zap — Repo Architecture & "Where to Find What"

Orientation for a new contributor (human or agent): open this one file and know
which crate or directory owns each stage of the compiler, the runtime, every
stdlib module, the tooling, and the tests — plus where the authoritative specs
and design docs live.

This is a **map, not a spec**. It does not restate language semantics or
grammar; those live in [`SPEC.md`](../SPEC.md) and
[`docs/GRAMMAR.ebnf`](GRAMMAR.ebnf) (see [Specs & process](#specs--process)).
For build/CI/review mechanics see [`CONTRIBUTING.md`](../CONTRIBUTING.md); this
doc complements its process-oriented "Where things live" table with a
code-oriented one.

Zap is a statically typed, Python-readable, AOT-compiled (LLVM) language. The
compiler is a Rust workspace (`crates/`) over a C runtime that ships inside the
codegen crate. All paths are relative to the repo root.

---

## The compiler pipeline

Source flows front-to-back; the front-end crates have **no LLVM dependency** and
build/test on any host. Only `zapc_codegen` needs LLVM.

```
 .zap source
    │
    ▼
 [ lex ]      zapc_syntax::tokenize        crates/syntax/src/lexer.rs
    │                                      (+ trivia.rs for comments/whitespace)
    ▼
 [ parse ]    zapc_syntax::parse           crates/syntax/src/parser.rs → ast.rs
    │         cfg conditional compilation  crates/syntax/src/cfg.rs
    │         generator (yield) analysis   crates/syntax/src/genanalysis.rs
    ▼
 [ type-check ]  zapc_sema::check          crates/sema/src/lib.rs
    │            type model & operations   crates/sema_types/      (Ty, *Info)
    │            stdlib sema facet          crates/sema/src/modules/
    ▼
 [ monomorphize ] zapc_mono                crates/mono/src/lib.rs  (erases generics)
    │
    ▼
 [ lower AST→ZIR ] zapc_ir::lower_module   crates/ir/src/lower.rs
    │              ZIR data structures      crates/ir_core/        (typed SSA)
    │              stdlib lowering facet    crates/ir/src/lower/modules/
    ▼  ZIR (Module)  ── frozen contract: design/IR.md
 [ codegen (LLVM) ] zapc_codegen           crates/codegen/src/lib.rs (inkwell)
    │               runtime-symbol facet    crates/codegen/src/modules/
    ▼
 [ link + run ]  links against the C runtime (zaprt.c, …) ── contract: design/RUNTIME-ABI.md
    │
    ▼
 native executable
```

- Front-end entry points: `tokenize` ([lexer.rs:42](../crates/syntax/src/lexer.rs)),
  `parse` ([parser.rs:32](../crates/syntax/src/parser.rs)),
  `check` ([sema/lib.rs:92](../crates/sema/src/lib.rs)),
  `lower_module` ([ir/lower.rs:404](../crates/ir/src/lower.rs)); `mono` runs at
  the top of `lower_module`.
- Backend entry points: the `Backend` trait and
  `build_executable` ([codegen/lib.rs](../crates/codegen/src/lib.rs)); `jit.rs`
  provides an in-process JIT (used by the REPL/playground).
- Tooling re-runs the front-end only (lex→parse→type-check, no LLVM) through
  `zapc_analysis` — see [Tooling](#tooling).

---

## Crate map

Workspace members and build gating are in [`Cargo.toml`](../Cargo.toml).
`crates/codegen` is **excluded from `default-members`** because it needs LLVM 18;
`cargo build`/`cargo test` on a bare host skip it (see
[`CONTRIBUTING.md`](../CONTRIBUTING.md) for the dev-container / `make` flow).

| Crate (`name`) | Owns | Start here |
|---|---|---|
| `syntax` (`zapc_syntax`) | Lexer, parser, AST, source spans, comment/trivia model, `cfg` conditional compilation, generator analysis | `src/lexer.rs`, `src/parser.rs`, `src/ast.rs` |
| `sema_types` (`zapc_sema_types`) | Source-level type model: the `Ty` the checker infers, enum/struct/interface `*Info` records, pure type ops (`assignable`/`join`/operator dunders) | `src/lib.rs` |
| `sema` (`zapc_sema`) | The type checker: name resolution, local inference (`auto`), declared-type checks; the sema facet of the stdlib module seam | `src/lib.rs`, `src/modules/` |
| `analysis` (`zapc_analysis`) | Queryable front-end for tooling — runs lex→parse→type-check over one document and exposes it (foundation of the LSP) | `src/lib.rs` |
| `mono` (`zapc_mono`) | Monomorphization: erases user generics into concrete code before lowering | `src/lib.rs` |
| `ir_core` (`zapc_ir_core`) | ZIR data structures — typed SSA `Module`/`Func`/`Block`/`Value` (no LLVM) | `src/lib.rs` |
| `ir` (`zapc_ir`) | AST→ZIR lowering (`lower_module`); the lowering facet of the stdlib module seam | `src/lower.rs`, `src/lower/modules/` |
| `codegen` (`zapc_codegen`) | ZIR→native via LLVM/`inkwell`, the JIT, **and the C runtime** (see below); runtime-symbol facet of the module seam | `src/lib.rs`, `src/modules/`, `src/zaprt.c` |
| `diag` (`zapc_diag`) | Stable `ZAPnnnn` error-code scheme (the machine-parseable diagnostic identity) | `src/lib.rs` |
| `fmt` (`zapc_fmt`) | The canonical formatter — `zap fmt` (D-0037) | `src/lib.rs`, `src/render.rs` |
| `doc` (`zapc_doc`) | `zap doc` model extraction + HTML/JSON/Markdown renderers | `src/lib.rs`, `src/render.rs` |
| `json` (`zapc_json`) | Dependency-free JSON value/parser/serializer (the toolchain ships no `serde`) | `src/lib.rs` |
| `pkg` (`zapc_pkg`) | Package manager & project env: `zap.toml` manifest, lockfile, resolver, registry/index, checksums, `zenv` | `src/lib.rs`, `src/ops.rs` |
| `driver` (`zapc`) | The toolchain glue: CLI dispatch, REPL, LSP, MCP, playground, test harness; ships the `zap` and `zapc` binaries | `src/cli.rs`, `src/bin/` |

**`zap` vs `zapc`:** `zap` ([`src/bin/zap.rs`](../crates/driver/src/bin/zap.rs))
is the user-facing CLI (D-0053); `zapc`
([`src/bin/zapc.rs`](../crates/driver/src/bin/zapc.rs)) is the internal compiler
it drives. Both share the pipeline in the `driver` lib. CLI subcommands
(`run`, `build`, `fmt`, `doc`, `new`, `add`, `test`, `repl`, `lsp`,
`playground`, `version`) are parsed in
[`src/cli.rs`](../crates/driver/src/cli.rs).

---

## Runtime & GC

The C runtime lives **inside the codegen crate** at `crates/codegen/src/` and is
compiled and linked by the backend. Its contract with compiled code (value
representation, object layout, calling convention, entry points) is
[`design/RUNTIME-ABI.md`](../design/RUNTIME-ABI.md); the IR it consumes is
[`design/IR.md`](../design/IR.md).

| File | What |
|---|---|
| [`crates/codegen/src/zaprt.c`](../crates/codegen/src/zaprt.c) | The runtime (~12k lines): object/string/collection entry points, the **M:N scheduler & coroutines** (ucontext-based), the **netpoller** (epoll/kqueue drives), **signals/preemption/faults**, CSPRNG, GC thread hooks |
| [`crates/codegen/src/zapgc.inc.c`](../crates/codegen/src/zapgc.inc.c) | The **precise Immix GC** (default collector), incl. `zap_gc_stats` heap accounting |
| `crates/codegen/src/zap_np_iocp.c` / `.h` | Windows IOCP netpoller (A2) |
| `crates/codegen/src/zap_ctx_win_x64.c` | Windows x64 coroutine context switch (A1) |
| `crates/codegen/src/zap_win_a3.c` | Windows signals/faults via SEH (A3) |

**Two garbage collectors** (feature-gated in codegen): `gc-immix` is the default,
precise, primary collector; `gc-boehm` is the conservative kept-green fallback.
CI tests under **both** — a change must pass under each. The runtime-facing
symbols each stdlib module needs are declared in `crates/codegen/src/modules/`.

---

## Standard library

Built-in, compiler-known namespaced modules (`os`, `io`, `time`, `net`, …) are
implemented through a **three-facet module seam** (I#121, D-0056/D-0058). A
module is *parallel-addable*: adding one means creating a `<name>.rs` in each
facet and adding a row to that facet's dispatch table — no edits to a shared
surface.

| Facet | Directory | Role |
|---|---|---|
| Sema | `crates/sema/src/modules/` | Per-method type-checking; dispatch table `MODULES` in `mod.rs` |
| Lowering | `crates/ir/src/lower/modules/` | AST→ZIR lowering of each method (`support.rs` = shared helpers) |
| Codegen | `crates/codegen/src/modules/` | Declares the runtime symbols the lowered calls bind to |

Start at [`crates/sema/src/modules/mod.rs`](../crates/sema/src/modules/mod.rs) —
its header documents the seam and names the twin registries.

**Current modules** (file is `<name>.rs` in each facet above):
`argparse`, `bisect`, `concurrent`, `crypto`, `datetime`, `deque`, `fs`, `gc`,
`gzip`, `http`, `io`, `json`, `logging`, `math`, `net`, `os`, `oset`, `process`,
`random`, `runtime`, `signal`, `sync`, `sys`, `threading`, `time`, `tls`,
`yaml`, `zlib`.

Per-surface stability tiering (Stable / Experimental / Internal) is **not** here —
it lives in [`docs/STABILITY.md`](STABILITY.md) (D-0076).

---

## Tooling

All user-facing tools dispatch from the `driver` crate and (for the front-end
tools) run on top of `zapc_analysis` so they never need LLVM.

| Tool | Where | Notes |
|---|---|---|
| `zap fmt` | crate `fmt` (`crates/fmt/`) | Canonical formatter; style spec in [`docs/FORMAT.md`](FORMAT.md) (D-0037). Also a CI gate on `.zap` sources |
| `zap doc` | crate `doc` (`crates/doc/`) | Model extraction + HTML/JSON/Markdown renderers |
| REPL | `crates/driver/src/repl.rs` | Uses the JIT backend |
| LSP | `crates/driver/src/lsp/` | Backed by `zapc_analysis` |
| Playground | `crates/driver/src/playground/` | Sandboxed worker + server; assets in `playground/assets/` |
| MCP server | `crates/driver/src/mcp/` | Agent-facing tools (check/reference/graph/test); see [`zap_mcp.md`](../zap_mcp.md) |
| Package manager / `zenv` | crate `pkg` (`crates/pkg/`) | `zap.toml`, lockfile, resolver, registry/index |
| VS Code extension | `editors/vscode/` | TypeScript client + syntax grammar |

---

## Tests

| Kind | Where | Notes |
|---|---|---|
| Golden end-to-end programs | `tests/golden/` | ~176 programs: `name.zap` + `name.expected` (stdout), optional `name.exit` (non-zero code), `name.stderr` (diagnostics), `name.env` |
| Driver integration suites | `crates/driver/tests/` | `golden.rs`, `gc.rs`, `signals.rs`, `profiling.rs`, `zap_cli.rs`, `lsp.rs`, `repl.rs`, `playground.rs`, `doctest.rs`, `error_codes.rs` |
| Unit tests | next to the code in each crate | `#[cfg(test)]` modules |
| Runtime C tests | `crates/codegen/src/*_test.c` / `.cc` | Windows ctxswitch/IOCP/SEH unit tests |

Golden programs are the primary regression net; a new language or stdlib feature
should land with one or more. Two rules bite people:

- **Goldens must be deterministic.** GC and stress goldens must **not** assert
  exact heap-byte counts — assert reclamation / bounded growth instead. Exact
  byte counts vary by run and collector and turn one CI trigger green and the
  other red.
- **`.zap` sources are fmt-gated** (D-0037): the host CI job runs
  `zap fmt --check tests/golden`. The **doctest** runner (`zap test --doc`,
  D-0074) executes ` ```zap ` docstring blocks via the REPL session.

Build, collector, and CI-gate mechanics (dev container, `make` targets, the two
`push`/`pull_request` triggers, clippy/fmt gates) are in
[`CONTRIBUTING.md`](../CONTRIBUTING.md).

---

## Specs & process

Source-of-truth index. Keep these current **with** the code — grammar and spec
changes are gated in review.

| Doc | Role |
|---|---|
| [`SPEC.md`](../SPEC.md) | The language specification — source of truth |
| [`docs/GRAMMAR.ebnf`](GRAMMAR.ebnf) | The grammar; update **whenever syntax changes** |
| [`docs/STABILITY.md`](STABILITY.md) | Versioning & stability policy; the D-0076 Stable/Experimental/Internal tiering and the 1.0 contract (D-0070) |
| [`docs/RELEASING.md`](RELEASING.md) | Release process: `vX.Y.Z` tag convention, how the version bumps, how the changelog rolls (I#207) |
| [`CHANGELOG.md`](../CHANGELOG.md) | Notable changes, Keep a Changelog format (pre-1.0 / editorial 0.x) |
| [`design/IR.md`](../design/IR.md) | ZIR — the typed SSA IR (front-end ↔ backend contract) |
| [`design/RUNTIME-ABI.md`](../design/RUNTIME-ABI.md) | Compiled-code ↔ runtime contract |
| [`docs/FORMAT.md`](FORMAT.md) | The canonical `zap fmt` style |
| [`design/NNNN-*.md`](../design/) | Per-feature design docs, from [`design/TEMPLATE.md`](../design/TEMPLATE.md) |
| [`DECISIONS.md`](../DECISIONS.md) | Decision log (ADR): numbered `D-NNNN` entries, never deleted, only superseded |
| [`OPEN-QUESTIONS.md`](../OPEN-QUESTIONS.md) | Tracked backlog of unresolved questions |
| [`ROADMAP.md`](../ROADMAP.md) | Phased build plan |
| [`CONTRIBUTING.md`](../CONTRIBUTING.md) | Build, CI gates, tests, and the design-change doc workflow |

**Conventions.** Decisions are recorded as `D-NNNN` in `DECISIONS.md` (mark a
superseded entry *Superseded* and point at its replacement; never delete). A new
feature or semantics change goes through `design/` (from `TEMPLATE.md`) →
`DECISIONS.md` → `SPEC.md` → `GRAMMAR.ebnf`, per the workflow in
`CONTRIBUTING.md`.
