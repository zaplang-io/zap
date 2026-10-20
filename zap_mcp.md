# Zap repo conventions (served to coding agents via `zap mcp`)

House rules for writing and building code in this repository. `zap mcp`
discovers this file at the repo root and serves it so agents work in the repo's
style. Keep it short — it is injected into agent context (32 KiB cap).

## Build & test workflow (tickets #128, #123) — follow this

Codegen needs LLVM, which runs in the dev container. Builds are heavy (the link
step is the RAM peak), so **iterate with type-checks and gate with the full
suite** — do not run a full `make docker-test` on every change:

1. `make check` — host type-check of the LLVM-free crates (front end + driver).
   The quickest loop; cargo's incremental cache keeps it fast.
2. `make docker-check` — full-workspace `cargo check` incl. codegen (no link) in
   the container. Use when your change touches codegen.
3. `make docker-test` — **final gate only, before you push.** Full suite under
   both collectors (precise Immix GC and Boehm). Mirrors the `full` CI check.

### Running concurrent builds (parallel waves)

- `make docker-test` is concurrency-safe across worktrees: each worktree gets
  its own isolated cargo target volume (#123), and a shared `sccache` cache
  speeds sibling builds. You do **not** need the old `CARGO_TARGET_DIR` hack.
- A full LLVM build is memory-heavy. **Do not launch several heavy builds at
  once on one machine** — that is the problem #128 fixed. Each run is capped
  (`ZAP_DOCKER_MEMORY`, default 5g); two concurrent runs need the Docker VM to
  hold ~2× that plus headroom. Raise the cap only if the VM has the RAM:
  `ZAP_DOCKER_MEMORY=8g make docker-test`.
- Verify the shared cache is hitting with `make docker-sccache-stats`.

## Code conventions

- **Format before committing:** run `cargo fmt --all` — CI's Format check fails
  otherwise. Clippy runs with `-D warnings`; keep it clean (`cargo clippy
  --all-targets`).
- **Keep CI green on both collectors.** The `full` CI check builds and tests the
  workspace under both the default precise Immix GC (`--features zapc/gc-immix`)
  and the Boehm fallback (`--features zapc/gc-boehm`). CI fires on both `push`
  and `pull_request`.
- **Don't commit `Cargo.lock`** (it is git-ignored; dependency pins live as
  exact `=x.y.z` versions in the `Cargo.toml` files).
- **Infra/build changes must not change language behavior** — the golden
  programs under `tests/golden/` and their `*.expected` output are the
  contract; a build/infra PR leaves them untouched.

## Repo map

- `crates/syntax` — lexer, parser, AST (`zapc_syntax`).
- `crates/sema` — type checker (`zapc_sema`).
- `crates/ir` — typed SSA IR + AST→ZIR lowering (`zapc_ir`); `src/lower/` holds
  the per-stdlib-module lowerers.
- `crates/codegen` — LLVM codegen + the C runtime `zaprt.c`/`zapgc.inc.c`
  (`zapc_codegen`); needs LLVM, gated behind the driver's `llvm` feature.
- `crates/pkg` — package manager & project env (`zapc_pkg`).
- `crates/driver` — the `zap` CLI + internal `zapc` compiler + `zap mcp` server.

Source of truth for the language: `SPEC.md`; decisions in `DECISIONS.md`;
grammar in `docs/GRAMMAR.ebnf` (keep it current after grammar decisions).
