# Contributing to Zap

Thanks for helping build Zap — a statically typed, AOT-compiled (via LLVM)
language with Python-like readability. Zap's bet is that a language can be
optimized for three readers at once: **written by coding agents, read by
humans, executed by machines.** Contributions (human or agent-authored) are
judged against that thesis: is it unambiguous to generate, pleasant to read, and
fast to run?

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md). To
report a security problem, **do not open a public issue** — see
[SECURITY.md](SECURITY.md).

## Licensing of contributions

Zap is dual-licensed under [Apache-2.0](LICENSE-APACHE) **or** [MIT](LICENSE-MIT),
at the user's option. Unless you explicitly state otherwise, any contribution
you intentionally submit for inclusion in Zap, as defined in the Apache-2.0
license, is dual-licensed as above, without any additional terms or conditions.
There is no CLA and no separate sign-off step.

## Where things live

| Path | What |
|---|---|
| [`docs/REPO_ARCHITECTURE.md`](docs/REPO_ARCHITECTURE.md) | "Where to find what" in the code: pipeline, crate map, runtime/GC, stdlib seam, tooling, tests |
| [`SPEC.md`](SPEC.md) | The language specification — the source of truth |
| [`DECISIONS.md`](DECISIONS.md) | Decision log (ADR): `D-NNNN` entries, never deleted, only superseded |
| [`OPEN-QUESTIONS.md`](OPEN-QUESTIONS.md) | Tracked backlog of unresolved questions |
| [`ROADMAP.md`](ROADMAP.md) | Phased build plan |
| [`docs/GRAMMAR.ebnf`](docs/GRAMMAR.ebnf) | The grammar |
| [`design/`](design/) | Per-feature design docs (`NNNN-name.md`, from [`TEMPLATE.md`](design/TEMPLATE.md)); `IR.md` and `RUNTIME-ABI.md` are the phase-boundary contracts |
| `crates/` | The compiler workspace (`syntax`, `sema`, `ir`, `codegen`, `driver`, …) and the runtime |
| `tests/golden/` | Golden end-to-end programs: `name.zap` + `name.expected` (+ optional `.exit`/`.stderr`/`.env`) |

## Getting set up

You need the pinned Rust toolchain (`rust-toolchain.toml` selects it
automatically via `rustup`). The front-end crates build and test on any host:

```bash
cargo build
cargo test
```

**Codegen needs LLVM 18**, which is not conveniently available on every host
(notably Intel macOS), so the full workspace builds and tests inside the dev
container defined in `.devcontainer/Dockerfile`, wrapped by the `Makefile`:

```bash
make docker-build   # build the LLVM 18 dev image (once)
make check          # quickest loop: host type-check of the LLVM-free crates
make fmt-check      # host-job formatting gates: cargo fmt + the .zap golden gate
make lint           # fmt-check + the LLVM-free clippy run (warnings are errors)
make docker-check   # type-check the whole workspace incl. codegen (no link)
make docker-test    # the final gate: full suite under BOTH collectors
make docker-run FILE=tests/golden/fib.zap   # compile + run a program
```

The README's "Build performance" section explains the memory caps, per-worktree
target volumes, and shared compiler cache; tune with `ZAP_DOCKER_MEMORY`,
`ZAP_DOCKER_CPUS`, and `ZAP_BUILD_JOBS` if your machine is small.

Iterate with `make check` / `make docker-check`; run the heavy `make docker-test`
only as the last step before you push.

## The CI gates

CI (`.github/workflows/ci.yml`) runs on **both** `push` and `pull_request`, in two
jobs — `host` (no LLVM) and `full` (LLVM 18 + codegen). A PR is mergeable only
when all of it is green. Run these locally first:

```bash
# 1. Formatting + the LLVM-free clippy run (the host-job style gates, as one
#    target). `make docker-test` also runs `fmt-check` as a prerequisite, so an
#    unformatted golden or a rustfmt miss fails locally before you push.
make lint               # = cargo fmt --all -- --check
                        #   + cargo run -q --bin zap -- fmt --check tests/golden   (D-0037)
                        #   + cargo clippy --all-targets -- -D warnings
                        # fix formatting with: cargo fmt --all
                        #                      cargo run --bin zap -- fmt tests/golden

# 2. Clippy under BOTH collectors (these link LLVM, so run in the container)
cargo clippy --workspace --features zapc/gc-immix --all-targets -- -D warnings     # full: default collector
cargo clippy --workspace --features zapc/gc-boehm --all-targets -- -D warnings     # full: fallback collector

# 3. Tests — the full suite under BOTH garbage collectors (runs `fmt-check` first)
make docker-test        # = cargo test --workspace --features zapc/gc-immix
                        #   + cargo test --workspace --features zapc/gc-boehm
```

Notes that have bitten people:

- **Both collectors.** The precise Immix GC is the default and primary; the
  conservative Boehm GC is the kept-green fallback. A change that passes under
  one and fails under the other is not done.
- **Clippy needs LLVM too.** The two `--features zapc/gc-*` clippy configs link
  against LLVM, so on a host without it run them inside `make docker-shell`.
- **`cargo fmt` is a gate, not a nicety.** "Format check" is the most common
  reason a PR goes red. Run `cargo fmt --all` before every push.
- **`.zap` sources have a fmt gate too** (D-0037). The host job runs
  `zap fmt --check tests/golden`; a new or edited `*.zap` must be canonically
  formatted. `make fmt-check` (and therefore `make docker-test`) now runs this
  locally, so it fails before you push instead of reddening CI after. Fix with
  `cargo run --bin zap -- fmt tests/golden` (or `zap fmt <file>`). The style is
  specified in `docs/FORMAT.md`.
- **Docs-only PRs still run every gate.** CI doesn't skip on file type; verify
  the checks for *both* the `pull_request` and `push` triggers are green before
  asking for review. A `push` run can fail when the `pull_request` run passed
  (and vice versa), usually from a flaky or non-deterministic test.

## Tests

- **Golden programs** (`tests/golden/`) are the primary regression net. A new
  language or stdlib feature should land with one or more `*.zap` programs and
  their `*.expected` stdout (`*.exit` for a non-zero exit code, `*.stderr` for
  expected diagnostics).
- **Goldens must be deterministic.** In particular, GC and stress goldens must
  *not* assert exact heap-byte counts — assert reclamation (e.g. "under a bound",
  "stays bounded") instead. Exact byte counts differ between runs and collectors
  and turn one CI trigger green and the other red.
- Unit tests live next to the code in each crate; the `driver` crate also has
  integration suites (`golden.rs`, `gc.rs`, `signals.rs`, `profiling.rs`,
  `zap_cli.rs`).

## Making a design change: the doc workflow

Zap deliberately avoids re-litigating settled questions. Anything that changes
the language surface, the IR schema, or the runtime ABI goes through docs
*before or alongside* the code:

1. **Open an issue** describing the problem (see the issue templates).
2. **Small, local change** (a bug fix, an internal refactor): no doc needed.
3. **New feature or semantics change:** write a proposal in `design/` from
   [`design/TEMPLATE.md`](design/TEMPLATE.md) — Abstract, Background, Proposal
   (with concrete `.zap` examples), Rationale (alternatives rejected),
   Compatibility, Implementation, Open issues. Wrap text near 80 columns and
   start each sentence on a new line so review comments and diffs stay precise.
4. **Record the decision** in [`DECISIONS.md`](DECISIONS.md) as a new numbered
   `D-NNNN` entry with **Status**, the decision, and the **Rationale**. Never
   delete an entry; mark a superseded one *Superseded* and point at its
   replacement.
5. **Update the spec** ([`SPEC.md`](SPEC.md)) — it is the source of truth, so it
   must match what the compiler does.
6. **Update the grammar** ([`docs/GRAMMAR.ebnf`](docs/GRAMMAR.ebnf)) **whenever
   syntax changes.** Parser changes without an EBNF change will be sent back.
7. Park genuinely unresolved questions in
   [`OPEN-QUESTIONS.md`](OPEN-QUESTIONS.md) rather than guessing silently.

Changes to the phase-boundary contracts (`design/IR.md`, `design/RUNTIME-ABI.md`)
need extra care: they are what lets front-end, middle-end, and runtime work
proceed in parallel.

**Changelog.** Any change to a public surface (language, stdlib, CLI, package
format) also adds an entry under `[Unreleased]` in
[`CHANGELOG.md`](CHANGELOG.md), and the PR states its change class
(PATCH/MINOR/MAJOR) per [`docs/STABILITY.md`](docs/STABILITY.md). How a release
is cut and how the version bumps is [`docs/RELEASING.md`](docs/RELEASING.md).

## Conventions

- **Issue and PR references.** Zap's issues and PRs share one number space on
  GitHub, so write issues as **`I#n`** and pull requests as **`PR#n`** in docs,
  commit messages, and discussion (e.g. `I#209`, `PR#194`). Commit subjects
  follow `type(scope): I#n short summary`, e.g.
  `feat(with): I#185 context-manager protocol + general with statement`.
  Types in use: `feat`, `fix`, `refactor`, `docs`.
- **Terminology: "coroutine", never "goroutine".** Zap's concurrency primitive is
  a *coroutine* (`spawn`, scheduler, netpoller, structured concurrency).
  "Goroutine" appears **only** when explicitly referring to Go (e.g. "Go
  goroutine semantics"). This applies to code, comments, docs, diagnostics, and
  test names.
- **Declarations are type-first** (`int64 count = 0`, `auto name = expr`) — never
  name-first (`count: int64 = 0`). This is a locked decision (D-0003).
- **Python-readable surface, explicit semantics.** Prefer one obvious way to
  spell something; ambiguity costs both human readers and code-generating
  agents. Public function signatures are explicit; inference is local.
- **Match the surrounding code**: naming, comment density, and idiom. Comments
  explain *why* (a ticket, a decision, a non-obvious invariant), not *what*.
- **Runtime/C changes** (`zaprt.c`): keep the section banner style, and mind the
  GC contract — see `design/RUNTIME-ABI.md` and the precise-GC notes before
  holding raw pointers across allocation or park points.
- **Dependencies.** Zap keeps its dependency surface small (system libraries such
  as libgc, PCRE2, zlib, OpenSSL are linked deliberately and recorded in
  `DECISIONS.md`). Don't add a crate or system dependency without discussing it
  in an issue first.

## Workflow: branches and pull requests

1. Branch from `main`. Descriptive branch names with the issue number work well:
   `feature/with-statement-185`, `fix/gc-immix-spawn-188`. If you work with
   several checkouts, use `git worktree add` — the Makefile gives each worktree
   its own cargo target volume so concurrent builds don't contaminate each other.
2. Keep a PR to **one issue**. Put `Closes #n` in the PR description (GitHub's
   keyword syntax; elsewhere still write `I#n`).
3. Run the [CI gates](#the-ci-gates) locally, then push.
4. Open the PR; the template is a checklist of what reviewers look for. Confirm
   that **both** CI triggers (`pull_request` and `push`) are green.
5. Don't force-push over review history without saying so; do rebase or merge
   `main` if you've fallen behind.

Using a coding agent is welcome — Zap is built for it. Hold agent output to the
same bar: it must pass the same gates, follow these conventions, and you are
responsible for what you submit.

## Reporting bugs and proposing features

Use the issue templates. For a compiler bug, the most valuable thing is a
minimal `.zap` program, the command you ran (`zap run`, `zap build`, …), the
output you expected, and the output you got. Include `zap version` and your
platform.
