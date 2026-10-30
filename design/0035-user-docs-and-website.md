# Proposal: User documentation & the public website (zaplang.org)

- **Author(s):** Zap toolchain team
- **Last updated:** 2026-10-30
- **Status:** Draft
- **Discussion:** issue #422 (epic: public docs site & user docs); sub-item of
  the tooling/adoption epic #157; consumes #232/#233 (`zap doc`), #234
  (doctests), #178 (agent-eval), #235 (registry), #238 (playground).

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line. This keeps review comments precise and diffs
> small.

## Abstract

This proposal specifies the plan for Zap's *user-facing* documentation and the
public website at zaplang.org.
It is a design-and-planning document only: nothing here is built now.
It is captured so the decisions survive until the language is ready to publish,
at which point #422 is split into sub-issues and implemented.

The repository today has a strong spec/design/contributor surface (`SPEC.md`,
`DECISIONS.md`, `design/`, `docs/`) but no learner- or evaluator-facing layer.
This document defines the information architecture (routed by reader intent),
the split between hand-written and generated content, the three benchmark
surfaces and the factual-accuracy rules for publishing them, and a versioning
strategy whose infrastructure is put in place from day one even though a single
channel is published before 1.0.

## Background

### What exists today

The documentation surface is entirely spec- and contributor-facing:

- Root: `SPEC.md` (the source of truth), `DECISIONS.md` (ADR log, D-00xx),
  `ROADMAP.md`, `OPEN-QUESTIONS.md`, `CHANGELOG.md`, `README.md`, plus
  `CONTRIBUTING`/`CODE_OF_CONDUCT`/`SECURITY`.
- `docs/`: `GRAMMAR.ebnf` (load-bearing via the #396 conformance test),
  `REPO_ARCHITECTURE.md`, `FORMAT.md`, `STABILITY.md` (the 1.0 contract),
  `RELEASING.md`, `DOCKER.md`.
- `design/`: per-feature design docs (`0001`…`0034`), plus `IR.md`,
  `RUNTIME-ABI.md`, `TEMPLATE.md`.

Doc *tooling* already ships: `zap doc` extraction (#232) and HTML/Markdown
renderers (#233), and doctests via `zap test --doc` (#234).
A browser playground exists (#238), as do a package registry design (#235) and
the agent-authorship benchmark design (#178, `design/0028-agent-eval.md`).

### Why it is insufficient

There is no tutorial, no guide, no narrative API reference, and no evaluator
landing surface.
Everything assumes a reader who is implementing or specifying the language, not
one deciding whether to adopt it or learning to use it.
Before a public launch that gap has to be filled, and the filling should reuse
the generators and benchmark harnesses that already exist rather than
hand-maintaining prose that drifts from the compiler.

## Proposal

### 1. Information architecture — route by reader intent

The homepage routes by *intent*, and doc *type* is the structure beneath each
route:

- **Evaluate** — Installation (with "who it suits"), performance, the
  agent-codegen story, "Zap for Python developers".
- **Learn** — Quickstart, the full language course, the language guide.
- **Do** — stdlib/API reference, tooling, cookbook, non-core modules.
- **Track** — changelog / release notes, public roadmap, stability status.

The four classic doc types (tutorial / how-to / reference / explanation) map
onto this: the course and quickstart are tutorials, the cookbook is how-to, the
API reference is reference, and the concurrency/perf/metaprogramming pages are
explanation.

### 2. Content inventory

Hand-written (narrative) pages:

- **Quickstart** — "first 10 minutes": install → hello world → build a small
  real program → run the native binary.
  Distinct from, and shorter than, the course.
- **Full language course** — the sit-down tutorial.
- **Language guide / features** — reference-leaning prose for each feature.
- **"Zap for Python developers"** — the key differences and a porting guide
  (idiom translation and gotchas, not just a diff list).
- **Concurrency guide** — coroutines and structured concurrency as narrative
  (the primitive is a *coroutine*; "goroutine" only appears in Go references,
  per the terminology decision in #171).
- **Metaprogramming / decorators / compile-time** — a headline differentiator
  from the README ("compile-time metaprogramming to keep business logic free of
  boilerplate") that currently has no user-facing page.
- **Performance & memory model** — Immix GC behavior, value structs vs heap,
  finalizers, AOT and static linking; the "why", for the speed-seeking
  audience.
- **Interop** — Python interop (#27), C FFI/ABI, and the AWS SDK (a genuine
  draw; likely its own section).
- **Deployment / platform** — supported targets, cross-compilation, the Lambda
  runtime, shipping static binaries.
- **Cookbook / recipes** — task-oriented snippets (read a file, HTTP request,
  parse JSON).
- **Installation**, **changelog / curated release notes**, and reader-facing
  views of **roadmap** and **stability status** (derived from `ROADMAP.md` /
  `docs/STABILITY.md`, not the raw internal files).

Generated content:

- **Core + stdlib API reference** — produced by `zap doc` (#232/#233) and kept
  honest by doctests (#234).
  This is the most-visited page of most language sites and must be generated,
  never hand-written.
- **Non-core module docs** — see §4 (registry / docs.rs question).
- **Playground embeds** — "try it" buttons inline in the course and guide,
  backed by #238.

### 3. Benchmarks — three surfaces, site renders only

There are three distinct benchmark surfaces.
In every case the site is a passive renderer: each benchmark lives in its own
repository whose CI emits machine-readable results (e.g. JSON keyed by task,
language, version, hardware, timestamp, metric) that the site charts.
No figure is ever hand-entered into a page.

1. **zap-perf (pillar 3)** — native execution performance vs Python/Go.
2. **agent-eval (pillar 1, #178)** — how well coding agents author *correct*
   Zap.
   Its deterministic CI mode scores a fixed in-repo corpus against the real
   toolchain with no LLM on the path, yielding reproducible metrics (compile
   rate, tests-pass rate, iterations-to-green, …).
   The corpus labels attempts by MCP-server on/off, which gives a clean
   ablation.
3. **AI-codegen token usage** — sourced from agent-eval's *live-agent* sweep.
   Tokens and wall-clock are deliberately excluded from the CI metric (they are
   non-deterministic: model drift, sampling), so these figures are published as
   dated snapshots pinned to specific model and language versions, clearly
   separated from the gated correctness metric.

Factual-accuracy rules for all three (the repository and its issues are public,
so these are stated as commitments):

- Numbers come from open, reproducible repositories; none are hand-edited into
  pages.
- Every result discloses hardware, compiler/runtime versions, flags, and the
  harness commit.
- Each claim is scoped to what was measured: the ablation reports the MCP
  server's measured effect; cross-language comparisons hold task semantics
  constant and name the baseline.
  A comparison of the tooling (MCP on vs off) is never phrased as a property of
  the language itself.

### 4. Non-core module documentation

Two viable models for per-package docs:

- **Main-site** — the website hosts non-core module reference.
- **Registry-hosted (docs.rs model)** — the registry (#235) auto-builds and
  hosts versioned per-package docs from each package's own `zap doc` output.

The docs.rs model keeps the main site focused on the core language and scales to
the ecosystem for free; it is the recommended direction but is left open here
because it is really a registry decision (tracked with #235).

### 5. Versioning — infra from day one, single channel pre-1.0

Recommendation: **build the versioning capability into the site from the start,
but publish only one channel until 1.0.**

- Retrofitting versioning into a live docs site is costly (URL scheme, version
  switcher, per-version search indexing, content-duplication strategy), so the
  generator is chosen for native versioning and URLs are laid out as
  `/docs/<version>/…` from the first commit.
- Pre-1.0 (0.x moves fast) only a `dev` channel tracking `main` is published;
  the version switcher stays dormant until it has ≥2 entries and is enabled at
  1.0.
- Version *selectively*:
  - Versioned: language reference, stdlib API reference, tooling docs,
    "what's new".
  - Unversioned (single living copy): blog, roadmap, community/contributing,
    design philosophy, playground.
- Versioned docs are sourced from the **release tag** (hand-written reference +
  `zap doc`-generated API ref snapshotted at the tag), so a doc version equals a
  compiler version by construction — the same principle that makes
  `GRAMMAR.ebnf` load-bearing (#396).
- The generated stdlib API reference is the first thing to version, even if
  narrative docs stay single-channel longer: showing a newer API under "latest"
  to a reader pinned to an older release is a real footgun.
- A support-window policy (e.g. current + last two minors) is fixed up front and
  aligned with the cadence in `docs/STABILITY.md`.

## Rationale

Intent-first navigation is chosen over doc-type-first because the homepage's job
is to get four very different readers (evaluator, learner, user, watcher) to the
right place in one click; the Diátaxis doc types are retained as the structure
underneath each intent rather than as the top-level nav.

The "site renders, repos generate" split is chosen so that documentation and
benchmarks cannot drift from the compiler: the content most sensitive to version
(API reference and benchmark numbers) is produced by the toolchain and pinned to
tags, not transcribed by hand.
This is the same reasoning already applied to `GRAMMAR.ebnf` (#396) and to
agent-eval's deterministic scorer (#178).

Versioning infrastructure is front-loaded while version *content* is not,
because the expensive, hard-to-reverse part is the plumbing, whereas maintaining
multiple published versions pre-1.0 is pure overhead with no readers on old
versions.

## Compatibility

No effect on the language, compiler, or runtime — this is documentation and
site infrastructure.
It does create a standing coupling between release tooling and docs: the release
process (`docs/RELEASING.md`) gains a step to snapshot generated + hand-written
reference at each tag once versioned publishing begins, and the support-window
policy must stay aligned with `docs/STABILITY.md`.

## Implementation

Deferred until publish readiness.
When #422 is picked up, the checklist there is split into sub-issues first, then
built, roughly in this order:

1. Choose the static-site generator (native versioning) and stand up the shell
   with `/docs/dev/…` URLs and a dormant version switcher.
2. Wire the generated API reference pipeline (`zap doc` → site).
3. Author the evaluate/learn core content (homepage, quickstart, course,
   "Zap for Python developers").
4. Add the benchmark pages as renderers over the zap-perf and agent-eval result
   artifacts, following the factual-accuracy rules in §3.
5. Settle the non-core module docs model with the registry work (#235).
6. At 1.0, begin tag-sourced versioned publishing and enable the switcher.

No phase boundaries (IR schema, runtime ABI) are touched.

## Open issues

- Static-site generator choice (Docusaurus / Antora / VitePress+plugin / mdBook
  + tooling): deferred to implementation.
- Non-core module docs: main-site vs registry-hosted docs.rs model (with #235).
- Exact support-window policy for versioned docs (align with `docs/STABILITY.md`
  at 1.0).
- Where "codegen with MCP" sits in the navigation — part of the evaluate story
  on the homepage vs a standalone benchmark page (likely both: a summary on the
  homepage linking to the full, methodology-disclosed page).
