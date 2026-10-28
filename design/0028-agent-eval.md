# Proposal: Agent-authorship benchmark (agent-eval), the pillar-1 metric

- **Author(s):** Zap toolchain team
- **Last updated:** 2026-10-25
- **Status:** Draft
- **Discussion:** issue #178 (agent-authorship benchmark); sub-item of the
  tooling/adoption epic #157; sibling of zap-perf (pillar 3); consumes the
  toolchain surfaces shipped by #239 (ZAPnnnn diagnostics), #73/#234 (`zap
  test` / doctests), #159 (`Session` engine), and #225–#227 (`zap fmt`).

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line. This keeps review comments precise and diffs
> small.

## Abstract

Zap's headline positioning is that it is "the first language optimized to be
(1) written by coding agents, (2) read by humans, (3) executed by machines."
Pillar 3 has a metric (zap-perf).
Pillar 1 does not, so today "agent-optimized" is a slogan, not a number.

This proposal specifies **agent-eval**: a benchmark that measures how well
coding agents author *correct* Zap, and that gives the tooling built to move
that number — canonical `zap fmt`, stable `ZAPnnnn` diagnostics, the MCP
conventions server — something concrete to move.

The design is deliberately split into two cleanly separated modes.
A **deterministic CI mode** (required) scores a fixed, in-repo corpus of
pre-generated agent attempts against the real toolchain, with zero network and
zero external-LLM dependency, so CI reproduces the numbers on every run.
A **live-agent mode** (offline, optional) drives a real coding agent through a
provider-agnostic seam to *produce* attempts, which a maintainer curates into
the fixed corpus.
The metric is coupled only to the deterministic mode; no vendor, API, or
network is ever on the CI path.

This is a **design-only** ticket.
It specifies what the implementation ticket builds; nothing here is
implemented in this ticket.

## Background

### The positioning and the gap

The three pillars are the product thesis.
Pillar 3 is measured by zap-perf (a separate benchmark repo).
Pillar 2 ("read by humans") is served by canonical formatting and clear
diagnostics but is inherently a human-judgment property.
Pillar 1 ("written by agents") is the one pillar that is both *claimed loudly*
and *currently unmeasured*.
Issue #178 (LOCKED 2026-10-21) calls for exactly this benchmark and frames it
as "the hard evidence behind the positioning — without this, 'agent-optimized'
is a slogan; with it, it's a chart."

### The hard constraint: CI must not depend on an LLM

A benchmark that calls a live model to produce code is non-deterministic
(model drift, sampling, provider outages, rate limits, cost) and cannot run in
CI as a reproducible, gating artifact.
The issue's original sketch (a separate repo with multi-provider API deps) is
right for the *live sweep* but wrong for the *metric that CI reports*.
The load-bearing refinement in this design is therefore the **CI-vs-live
split**: the deterministic scorer operates on a *fixed corpus of already-written
attempts* and recomputes every score by running the real toolchain over that
source — no model is ever invoked in CI.

### The toolchain surfaces a benchmark can lean on (verified against the tree)

The benchmark reuses — never reimplements — surfaces that already exist in
`main` (12d65d7):

- **CLI exit-code convention** (`crates/driver/src/cli.rs`,
  `crates/driver/src/lib.rs`): `0` success, `1` failure (compile error,
  runtime fault, test failure), `2` usage/misuse.
  `zap run FILE` compiles and runs; a compile error exits `1`, and a run exits
  with the child process's status (an uncaught Zap exception routes through
  `zap_uncaught` → `exit(1)`).
  `zap build FILE` stops at the binary (`1` on compile error).
  `zap test [--json]` exits `0` iff every test passes, else `1`.
  `zap fmt --check` exits `0` if already canonical, `1` if reformatting is
  needed, `2` on I/O error.
- **Structured diagnostics** (`crates/driver/src/mcp/check.rs`):
  `pub fn check_source(src) -> CheckResult` runs the front end only (lex +
  parse + typecheck, **no LLVM**) and returns `Diagnostic { severity, phase,
  code: ZAPnnnn, message, start, end, line, col }` with a JSON encoding.
  This is a plain library call — the scorer can obtain machine-stable
  `ZAPnnnn` codes host-only, without codegen or the MCP server.
  Per D-0071/#239, the `ZAPnnnn` *code* is the compatibility surface; the
  message text is not.
- **`zap test` JSON** (`crates/driver/src/testing/report.rs::format_json`):
  `{ "tests": [{ file, name, status, message, output }], "summary": { total,
  passed, failed, skipped, ok } }`.
- **The `Session` engine** (`crates/driver/src/repl.rs`, #159, D-0073):
  `Session::eval(src) -> EvalOutcome { diagnostics: Vec<String>, runtime_error:
  Option<String> }` with `ok()` and `reset()`, codegen-gated, identical
  semantics to compiled code.
  Already the single executor behind the REPL, doctests (#234), and the
  playground (#238).
- **Canonical formatter** (`crates/fmt`, `zapc_fmt`, #226/D-0037): canonical
  style is *byte-identical* intended agent output; `zap fmt --check` is the
  host-only, no-LLVM stability check.

### CI shape (`.github/workflows/ci.yml`)

Three relevant jobs, each run under both the `pull_request` and `push`
triggers:
- **host (no LLVM)** — `cargo test` over default members, `cargo clippy
  --all-targets -D warnings`, and `zap fmt --check tests/golden`.
- **full (LLVM 18 + codegen)** — `cargo test --workspace` under *both*
  collectors (`--features zapc/gc-immix` and `--features zapc/gc-boehm`), plus
  clippy under both.
- **arm64** — the same workspace tests under both collectors.

`crates/codegen` is a workspace *member* but **not** a *default member*, so the
host job skips codegen and only the LLVM jobs build it.
The benchmark crate follows the same membership pattern (§Implementation).

## Proposal

### What it measures

Five metrics, each a deterministic property of recorded source that a specific
pillar-1 lever is meant to move.
All are recomputed by running the real toolchain over the corpus; none is read
from a recorded field (anti-gaming, §Anti-gaming).

A unit of measurement is an **attempt sequence**: an ordered list of source
snapshots `[a1, a2, …, an]` an agent produced for one task in one offline run
(one snapshot per edit/iteration).
A sequence "reaches green" at the first snapshot that both compiles and passes
the task's hidden tests.

1. **Compile-on-first-try rate.**
   Fraction of sequences whose *first* snapshot `a1` compiles (`zap build`
   exit `0`).
   Lever: syntax learnability + diagnostics that prevented the agent from
   emitting broken code in the first place; the cleanest single "is the
   language easy to emit correctly" signal.

2. **Tests-pass rate (final correctness).**
   Fraction of sequences whose *final* snapshot passes the task's hidden tests
   (`zap test` exit `0`).
   Lever: can an agent reach a *correct* program at all, canonical surface and
   stdlib included.

3. **Iterations-to-green.**
   For sequences that reach green, the 1-based index of the first green
   snapshot (reported as mean + median + a never-green count).
   Lever: the whole edit→compile→test→fix loop — above all whether a failing
   attempt's diagnostics let the *next* attempt recover quickly.

4. **Diagnostic actionability.**
   Measured in two facets, both from `check_source`:
   - *Coverage* (toolchain-quality, static): fraction of the corpus's
     compile-failing snapshots whose primary diagnostic carries a *specific*
     `ZAPnnnn` code — i.e. not the `ZAP0001` ICE / `ZAP0002` not-implemented
     escape hatches, and not a bare parse-desync.
     A high-quality diagnostic surface codes failures specifically.
   - *Recovery* (sequence proxy): for each adjacent failing→next pair in a
     sequence, the fraction where the next snapshot no longer emits the prior
     snapshot's primary code *and* reaches a strictly better state (compiles,
     or clears that code).
     This is an honest *proxy* for "the ZAPnnnn + message let the agent fix
     it" — it shows correlation, not causation, and §Anti-gaming states its
     limits.
   Lever: the stable-error-code scheme (#239) is explicitly justified as a
   pillar-1 multiplier; this is its scoreboard.

5. **Fmt-stability rate.**
   Fraction of *green* final snapshots that are already `zap fmt --check`-clean
   (exit `0`) without reformatting.
   Lever: canonical formatting (#227) claims agent output should already be the
   one true style; this measures how often that holds in practice.

Deliberately **excluded from CI mode**: tokens and wall-clock.
They are intrinsically non-deterministic and model-specific, so they are
recorded by live mode only and never enter a CI number (§Live-agent mode).

### The task corpus

A task is a self-contained, solvable programming problem.
Stored in-repo under `benchmarks/agent-eval/corpus/`, one directory per task:

```
benchmarks/agent-eval/corpus/<NN>-<slug>/
  meta.toml          # id, category, difficulty, tags, corpus_version
  task.md            # the prompt/spec shown to the agent (NO tests inside)
  solution.zap       # reference solution: canonical, fmt-clean, passes tests
  tests.zap          # hidden @test suite (the correctness oracle)
  attempts/<run-id>/
    meta.toml        # model label + conditions (mcp on/off, knowledge-md
                     #   on/off) + capture date — LABELS ONLY, never scores
    01.zap           # first snapshot
    02.zap           # second snapshot (fixes 01's error), …
```

**Categories** (from #178), sized for a meaningful first cut of **~24 tasks**,
~4 per category:
- syntax basics,
- types / generics,
- collections,
- errors / exceptions,
- concurrency / coroutines,
- stdlib use.

**Authoring.**
Tasks are hand-authored by maintainers: a spec precise enough to be solvable
without the tests, a reference `solution.zap`, and a `tests.zap` whose
assertions probe behavior and edge cases (not just the happy path echoed in the
spec).
Attempt sequences come from two sources: captured from live-agent mode
(offline) and curated in, and a small set of *hand-crafted flawed* sequences
that deliberately pin specific diagnostics (so facets of metric 4 are validated
from day one, before many live runs exist).

**Good and flawed samples.**
The point of the benchmark is to exercise the failure path, so the corpus
*must* contain deliberately flawed snapshots (first attempts that do not
compile, sequences that never reach green).
Reference solutions are canonical and fmt-clean; intermediate/flawed snapshots
are neither and must stay out of the fmt-gate (§Compatibility).

**Versioning.**
The corpus is **fixed and versioned**: a single `corpus_version` (a string in
each task's `meta.toml`, validated equal across the corpus) is bumped whenever
any `task.md`, `tests.zap`, or `solution.zap` changes.
Every report records the `corpus_version` and the toolchain commit, so numbers
are only ever compared within one `corpus_version` (§Baseline).

### The two execution modes

The modes share the corpus format and the metric definitions and *nothing
else*.
The scorer links no agent code; the live harness links no scoring-gate code.

#### (a) Deterministic CI mode — REQUIRED

Input: the fixed corpus (tasks + recorded attempt snapshots).
Process: for every task and every attempt sequence, the scorer replays the
snapshots through the real toolchain and recomputes each metric from first
principles:
- compile / run / tests via the built `zap` binary (`build`, `test`) — needs
  codegen,
- diagnostics via `check_source` — host-only,
- fmt via `zapc_fmt` / `zap fmt --check` — host-only.
Output: a raw per-sequence record, an aggregate report (human + JSON), and a
pass/fail on the *stability* assertions (not on the headline rates — see
§Scoring).

Properties, all structural:
- **Zero network, zero LLM.**
  The scorer crate depends on no HTTP/socket crate and invokes no model; the
  corpus is entirely local files.
  A CI test asserts determinism by scoring twice and requiring byte-identical
  JSON, and the crate's dependency set is the standing proof that no socket can
  be opened (§Implementation).
- **Reproducible.**
  Because every score is recomputed from committed source by a pinned
  toolchain, the same corpus + same commit yields the same numbers on every
  run, on every machine, under both collectors.

This mode is what gates and reports in CI.

#### (b) Live-agent mode — OFFLINE / OPTIONAL

A harness that actually drives a coding agent to attempt the corpus and records
the attempts.
It is **never** compiled into the scorer and **never** invoked from CI
(feature-gated `live-agent`, a separate binary).

The agent is reached through a provider-agnostic seam — no vendor is
hardcoded:

```rust
/// One attempt at one task, given optional feedback from the previous attempt.
trait AgentDriver {
    fn attempt(&mut self, task: &Task, feedback: Option<&Feedback>)
        -> anyhow::Result<AttemptSource>;
}
```

`Feedback` carries the toolchain's output from the previous snapshot (the
`ZAPnnnn` diagnostics from `check_source`, the `zap test` JSON) so the loop is
generate → compile/test → feed diagnostics back → regenerate, up to a
configured iteration cap.
The reference `AgentDriver` the crate ships is itself vendor-neutral: it shells
out to a user-supplied command over stdin/stdout, so even the bundled adapter
names no provider.
The sweep variables from #178 — MCP on/off, knowledge-`.md` on/off, model ×
provider — are *run configuration* that shape live mode only; the CI scorer
treats the recorded condition labels purely as metadata and never trusts a
recorded score.
Each run writes snapshots into the `attempts/<run-id>/` layout so a maintainer
can curate the result into the fixed corpus.

### Scoring and reporting

Pipeline: raw records → aggregate → report.

- **Raw** (one per attempt sequence): `{ task_id, category, run_id, labels,
  first_try_compile: bool, final_tests_pass: bool, iterations_to_green:
  Option<u32>, fmt_clean_final: bool, diag: [{ snapshot, primary_code,
  specific: bool, recovered_next: Option<bool> }] }`.
- **Aggregate**: the five headline rates overall and per category, with counts
  (never a rate without its denominator).
- **Report** — two artifacts:
  - *Human*: a Markdown table to stdout / a file (overall + per-category), easy
    to paste into a PR or the website.
  - *Machine*: a versioned JSON (`schema_version`, `corpus_version`,
    `toolchain_commit`, `generated_at`, the aggregate, and the raw records) —
    this is the artifact the docs/marketing site consumes and the one later
    runs diff against.

**Does it gate CI?**
The CI test hard-gates only on *stability/well-formedness*:
the scorer is deterministic (two runs byte-identical), the corpus is
well-formed (every task has the required files; `corpus_version` is uniform),
every `solution.zap` compiles, passes its own `tests.zap`, and is
`zap fmt --check`-clean, and the scorer opens no network.
It does **not** fail the build on a headline-rate regression: a benchmark
number is a *tracked* metric, not a merge blocker, and gating on it would both
be flaky and invite gaming.
The report is published as a CI artifact; regressions are reviewed, not
auto-blocked.

### Where it lives and what it is called

Recommendation: a **dedicated workspace crate `crates/agent-eval`** — *not* a
`zap` CLI verb and *not* a loose `scripts/` harness.
- A **library** (corpus loader + scorer + report), exercised by a codegen-gated
  `#[test]` that scores the committed corpus and asserts the stability
  properties above.
  It is a workspace *member* but **not** a *default member* (like
  `crates/codegen`), so the full scorer runs in the LLVM container jobs (full +
  arm64) under *both* collectors, exactly where the real compiler lives, and
  the host job stays fast.
- A thin **binary** (`cargo run -p agent-eval -- score`) that emits the human +
  JSON report, for local use and a report-publishing job.
- The **live harness** is a second binary in the same crate behind
  `--features live-agent`, never built in CI.

Why not `zap bench-agents`:
the benchmark measures *the toolchain*, it is not a command a user runs on
their own code, and exposing it under `zap` would wrongly imply a
stability-covered user surface (STABILITY.md) and bloat the CLI.
A crate + cargo-test gate + report binary keeps it as CI infrastructure and
mirrors the zap-perf sibling split.

### Anti-gaming / honesty

- **Hidden tests.**
  `task.md` states the spec only; `tests.zap` is the oracle and is never shown
  in the prompt.
  Tests probe behavior and edge cases, so terse-but-wrong (code that merely
  echoes the spec's happy path) fails.
- **Scores are recomputed, never recorded.**
  The scorer derives every pass/fail by running the toolchain over committed
  source; a sequence cannot claim a pass it does not earn.
- **Reference solution is proven.**
  Each `solution.zap` must compile, pass `tests.zap`, and be fmt-clean
  (asserted) — proof the task is solvable in canonical Zap, so a low score
  indicts the agent/toolchain, not an impossible task.
- **Fixed + versioned corpus.**
  Any task/test edit bumps `corpus_version`; you cannot quietly tune a task to
  flatter the number, and cross-version comparisons are refused.
- **Per-category reporting + denominators.**
  One gamed task cannot dominate; every rate ships with its count.
- **Stated proxy limits.**
  Diagnostic *recovery* (metric 4) is labeled a correlation proxy, not proof of
  causation; it is reported beside the static *coverage* facet precisely so the
  two are not conflated.
- **No token/wall-clock in CI.**
  The deterministic number rewards only correctness properties of source, not
  speed or verbosity, which belong to non-deterministic live mode.

## Rationale

### In-repo deterministic core vs the issue's separate repo

Issue #178 envisions a separate `zaplang-io/zap-agent-eval` repo with
multi-provider API deps, mirroring zap-perf.
This design *refines* that: the **metric definition + the deterministic scorer +
the fixed corpus land in-repo**, because they must version-lock with the
language and gate in the existing CI (a metric whose definition drifts from the
compiler is worthless).
The **heavy, multi-provider live sweep and the published results site** can
still live in the sibling repo, consuming this crate's corpus format and scorer
as the shared, authoritative definition.
In-repo = the honest, reproducible metric; separate repo = the live harness and
charts.
This is the recommended split and it adjusts the locked issue, so it needs user
sign-off before implementation.

### Baseline / comparison — recommendation

Recommend **tracking Zap's own numbers over time** (per `corpus_version`,
committed report snapshots), and *not* a cross-language comparison in CI.
Same-tasks-in-Python/Go is tempting for marketing but requires those toolchains
plus their own (non-deterministic) agent runs, and invites apples-to-oranges
disputes — it is heavy and un-CI-able.
Keep cross-language as an explicitly-flagged, directional *offline study* in
live mode only; never a CI gate.
The honest, low-overhead CI headline is Zap versus its own history.

### Why reuse, not reimplement

Every correctness judgment routes through the shipped toolchain: `check_source`
for diagnostics, the built `zap` binary for compile/run/test, `zapc_fmt` for
fmt, and (where a sequence is a single snapshot) the `Session` engine for the
same semantics doctests use.
A second, benchmark-local notion of "compiles" or "passes" would drift from the
compiler and quietly lie.

## Compatibility

Purely additive.
- New crate `crates/agent-eval` and a new tree `benchmarks/agent-eval/`; no
  existing crate, the compile path, or any golden changes.
- **Fmt-gate interaction (load-bearing).**
  The CI fmt-gate runs `zap fmt --check tests/golden` and walks only
  `tests/golden`.
  The corpus lives under `benchmarks/agent-eval/`, which the gate never walks,
  so deliberately-flawed snapshots do not break it.
  Canonical files are fmt-checked where they *should* be: the benchmark's own
  `#[test]` asserts every `solution.zap` is fmt-clean, while `attempts/**` and
  flawed snapshots are explicitly excluded from any fmt-check.
  This boundary is documented in the crate README and in DECISIONS.
- No new user-facing `zap` surface, so STABILITY.md is untouched (the benchmark
  is dev/CI infrastructure, not a covered surface).

## Implementation

Maps onto the #157 tooling epic (Area F / metric), host-CI-plus-container, no
IR-schema or runtime-ABI boundary touched.
STEP 2 only, after approval:

1. `crates/agent-eval`: `Cargo.toml` (member, not default member; `live-agent`
   feature off by default; dependency set contains no network crate), corpus
   loader + types, the scorer (calls `check_source`, `zapc_fmt`, shells the
   built `zap`), the report (human + JSON with `schema_version`).
2. The fixed first-cut corpus: ~24 tasks across the six categories, each with
   `task.md`, `solution.zap`, `tests.zap`, `meta.toml`, and at least one
   recorded attempt sequence (including a flawed one that pins a diagnostic).
3. The codegen-gated `#[test]` gate: score the corpus; assert determinism
   (byte-identical re-score), corpus well-formedness, every solution
   compiles/passes/fmt-clean, and zero network. Runs in the full + arm64 jobs
   under both collectors.
4. The `score` binary and a report-publishing step (artifact upload).
5. The `live-agent` seam: `AgentDriver` trait, `Feedback`, the vendor-neutral
   stdin/stdout reference adapter, the iteration loop that writes
   `attempts/<run-id>/`, all behind the feature and absent from CI.
6. Docs: `DECISIONS.md` **D-0076** (metrics, CI-vs-live split, in-repo core vs
   separate-repo live sweep, baseline recommendation), the crate README
   (corpus format, fmt-gate boundary, how to add a task, how to run live mode),
   and a short pointer from the tooling epic.
   Update `docs/GRAMMAR.ebnf`? No — no language surface changes.

Gates before pushing: `cargo fmt`; clippy both configs (`--all-targets -D
warnings`); full `make docker-test` under both collectors; in-container clippy
both configs; `zap fmt --check tests/golden`.
PR "Closes #178", confirming both CI triggers green at run level.

## Open issues

- **Does the user accept the in-repo-core / separate-repo-live-sweep split**,
  which refines the locked #178 (separate repo, multi-provider)?
  The deterministic core must land somewhere that version-locks with the
  compiler; in-repo is the recommendation.
- **First-cut corpus size** — ~24 tasks (4 × 6 categories) is proposed as
  "meaningful but buildable now"; confirm or resize.
- **Diagnostic-recovery proxy** — accept it as a flagged proxy beside the static
  coverage facet, or drop recovery from the headline and keep only coverage
  until a causal measure exists?
- **Report publishing destination** — CI artifact now; wiring into the
  docs/marketing site is deferred to the org move and the sibling repo.
