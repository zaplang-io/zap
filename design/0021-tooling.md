# Proposal: Developer tooling & adoption surface (LSP, fmt, doc, registry, playground, diagnostics)

- **Author(s):** tooling-scoping session (I#157)
- **Last updated:** 2026-10-23
- **Status:** Draft
- **Discussion:** https://github.com/zaplang-io/zap/issues/157

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

I#157 is the EPIC that, together with the platform epic (I#158), gates the
public launch: the developer-experience layer a language needs to be
*adoptable*, not merely feature-complete.
This doc surveys the tooling-relevant surfaces in the tree today, breaks the
epic into sequenced, independently-shippable sub-tickets across its six areas
(`zap fmt`, LSP/editor support, `zap doc`, package registry, web playground,
diagnostics-quality), gives each a rough size and ordering, and recommends the
first concrete slice to implement.
This is a planning doc — it does not itself change the compiler or any tool.

Positioning note that drives every priority call below: Zap's thesis is to be
the first language optimized to be **(1) written by coding agents, (2) read by
humans, (3) run by machines**.
Tooling is not a side quest to that thesis — a canonical formatter, clear
diagnostics, and a machine-friendly analysis surface are *how* pillars 1 and 2
are delivered.
That is why the ordering here favours the items that tighten the
agent-authorship / human-readability loop over the ones that only add reach.

## Background — what the tooling can build on today

The front-end was split into small crates for incremental compilation (I#129):
`syntax` (lexer + parser + AST + spans), `sema` + `sema_types` (type checking),
`ir` + `ir_core` + `mono` (lowering + monomorphization), `codegen`, `driver`,
`pkg`.
The split was done precisely so that fmt / doc / LSP can reuse the analysis
engine without dragging in LLVM.
Everything below is grounded in what those crates expose right now.

### The analysis engine already exists — behind the MCP server (I#72)

`crates/driver/src/mcp/check.rs` exposes `check_source(src) -> CheckResult`:
it runs **lex → parse → type-check** (no LLVM, no lowering, no link) and returns
`Vec<Diagnostic>`, where

```rust
pub struct Diagnostic {
    pub severity: Severity,      // Error | Warning
    pub phase: &'static str,     // "lex" | "parse" | "type" | "warning"
    pub message: String,
    pub span: Span,              // raw byte offsets
    pub line: u32, pub col: u32, // resolved via span_to_line_col()
}
```

This is exactly the payload an LSP `publishDiagnostics` needs, and the MCP
server (`zap_check`) already serves it to agents.
**The diagnostics half of an LSP is therefore nearly free** — the work is the
transport and the *interactive* features (completion / hover / go-to-def).

`sema` exposes `check`, `analyze` (errors + warnings split), `check_str`,
`warnings_str`.
What it does **not** expose is a *queryable* result: the resolver's scope and
name-resolution machinery (`lookup`, scope stacks) is internal and discarded
after a pass returns.
There is no public "symbol at position", "type of expression at position", or
"definition site of this name" query.
Standing up those queries on top of the existing resolver is the real LSP work,
and is the main reason the LSP is an **L**, not an **M**.

### Spans, line/column, and the lexer discard comments

- `syntax::span::Span` is **byte offsets only** (`start`/`end: u32`).
  LSP positions are line + UTF-16 code-unit offsets, so the server needs a
  line-index + UTF-16 mapping layer.
  A byte-offset → `(line, col)` helper (`span_to_line_col`) already exists in
  the MCP module and can be promoted/shared.
- **The lexer discards comments and blank lines** (`lexer.rs`: comment-only and
  blank lines "produce no layout tokens"; inline comments are skipped as
  whitespace).
  The AST therefore carries **no comment trivia**.
  This is the single most load-bearing fact for this epic: a formatter that
  round-trips through the current AST would **delete every comment**, and a doc
  generator could not attach narrative comments to declarations.
  Preserving comment trivia is a shared prerequisite for `zap fmt` and `zap doc`
  (docstrings themselves are exempt — see below).

### Docstrings are first-class, so `zap doc` is partly independent of trivia

Per D-0041 / D-0054, a **docstring is a triple-quoted string that is the first
statement** of a module / function / type — it is a real AST node, not a
comment.
So `zap doc` can extract docstrings + signatures + inferred types from the AST
today, independently of the comment-trivia work.
`zap doc` generation was explicitly **deferred** at D-0053/D-0054 (the test
framework landed the doc *conventions*); this epic is where it is built.
Doctests (runnable docstring examples) are noted there as a follow-up and tie to
the test framework (I#73) + the JIT (I#159).

### The CLI already reserves the verbs

`crates/driver/src/cli.rs` parses `zap fmt [paths...]` and today prints
"the canonical formatter is not yet implemented (D-0037)".
`doc` and `repl` were deferred at D-0053 (I#71).
So fmt and doc slot into an existing, unit-tested arg surface rather than
needing new CLI plumbing.

### Package manager / zenv (I#69) and the org

D-0052 locked a **decentralized** distribution model: VCS URL + semver tag, MVS
resolution, a **checksum database**, optional caching proxy, **no central
registry** required to install.
`crates/pkg` + zenv implement this.
A "registry" in this epic is therefore **discovery + a checksum/notary index**,
not a mandatory code host — installs already work without it.
This must be coordinated with the zaplang-io org plan (hosting, auth, the
`zaplang.org` domain on Route 53) and is the lowest-urgency area because nothing
is blocked on it.

### REPL (I#159) and the benchmark (I#178) are already filed

- **I#159** (REPL via LLVM ORCv2 JIT, Swift-REPL model) is its own ticket and
  is explicitly the **engine for the web playground and runnable doc examples**.
  This epic *consumes* it; it does not re-scope it.
- **I#178** (zap-agent-eval, a separate repo) is the **measurement** of pillar 1
  — the chart the diagnostics-quality and fmt work move.
  It is the proof metric, not a sub-ticket here.

## Proposal — sub-tickets by area

Sizes: **S** ≈ a day, **M** ≈ a few days, **L** ≈ 1–2 weeks, **XL** ≈
multi-week.
Every tool that reuses the frontend must pass the standard gates (`cargo fmt`,
both clippy configs `--all-targets -D warnings`, `make docker-test` under both
collectors where codegen is touched) and verify both `pull_request` and `push`
CI triggers before "ready".
Tools that do not touch codegen/runtime (fmt, doc, LSP analysis) run on the
host CI job and need no collector matrix — a point in their favour for velocity.

### Area A — `zap fmt` (canonical formatter) — D-0037

Ordering within A: **A1 → A2 → A3**.

- **A1 (#225). Comment-trivia preservation in the frontend.**
  Attach lexer comments + significant blank lines to adjacent AST nodes (leading
  / trailing / dangling), or introduce a lossless concrete-syntax layer the
  formatter consumes.
  This is the gating prerequisite: without it the formatter deletes comments.
  Keep it additive so existing sema/codegen consumers ignore the new trivia.
  Shared with `zap doc` (C1) and improves LSP later.
  **Size M.** No dependencies. **Blocks A2, C1 (partly).**
- **A2 (#226). The formatter.**
  Define and implement the one canonical Python-faithful style (indentation,
  blank-line rules between defs/classes, import ordering, line-wrapping of long
  calls/collections/signatures, string-quote normalization, trailing commas) and
  render the (trivia-carrying) AST back to source.
  Hard requirement: **idempotence** (`fmt(fmt(x)) == fmt(x)`) and
  comment-preservation, enforced by a golden + round-trip property test corpus.
  Reuses the parser; no new parser.
  **Size M.** Depends on A1. **Blocks A3.**
- **A3 (#227). `zap fmt --check` + CLI wiring + community fmt-gate.**
  Replace the stub in `cli.rs`; implement in-place rewrite, stdin/stdout, and a
  non-zero `--check` mode for CI; document the style; add the fmt-gate to the
  project's own CI and the contributor docs.
  **Size S.** Depends on A2. Coordinates with I#158 community-infra.

### Area B — LSP server + first editor

Ordering within B: **B1 → B2 → B3 → B4** (B2 can start against stub analysis in
parallel with B1).

- **B1 (#228). Queryable analysis API on the frontend.**
  Promote the internal resolver state into a public, position-indexed result:
  symbol-at-position, type-of-expression-at-position, definition-site,
  document-symbol tree; re-analyze a single changed document cheaply (the I#129
  crate split is what makes this affordable).
  This is the substance of LSP support; reuses sema + `check_source`.
  **Size L.** Depends on nothing new (A1 trivia helps ranges but is not
  required). **Blocks B3.**
- **B2 (#229). LSP skeleton + diagnostics.**
  JSON-RPC/LSP transport (evaluate `tower-lsp` vs hand-rolled to keep the dep
  surface small), `textDocument` open/change/close lifecycle, UTF-16 line-index
  position mapping (promote `span_to_line_col`), and `publishDiagnostics` wired
  straight to `check_source`.
  Ships a usable "red squiggles + live errors" server on its own.
  **Size M.** Depends on nothing (diagnostics already exist). **Pairs with B1.**
- **B3 (#230). Interactive language features.**
  `completion`, `hover` (inferred types — a strong teaching feature for a typed
  language), `definition`, `documentSymbol`; `rename` + `references` as a
  fast-follow.
  **Size L.** Depends on B1 + B2.
- **B4 (#231). VS Code extension (first editor).**
  LSP client wiring + a TextMate grammar for syntax highlighting + packaging.
  VS Code first (largest agent/human overlap); Neovim (`nvim-lspconfig`) is a
  near-free follow-on; JetBrains later.
  **Size M.** Depends on B2 (works better with B3).

### Area C — `zap doc` (API docs) — deferred at D-0053, built here

- **C1 (#232). Doc model extraction.**
  Walk the AST, collect triple-quoted docstrings (D-0041) + signatures +
  inferred types (reuse sema) + visibility, into a serializable doc IR.
  Docstrings need no trivia work; attaching *narrative `#` comments* to
  declarations needs A1.
  **Size M.** Soft dependency on A1 for comment capture; docstring-only can ship
  without it.
- **C2 (#233). Renderers (HTML + Markdown).**
  Render the doc IR to a static site (HTML) and Markdown; the docs website
  consumes the output.
  **Size M.** Depends on C1.
- **C3 (#234). Doctests (stretch).**
  Execute runnable docstring examples via the test framework (I#73) + the JIT
  (I#159); keeps docs honest.
  **Size M.** Depends on C1 + I#159. **Defer** to after the REPL lands.

### Area D — Package registry (discovery + checksum index) — builds on I#69

- **D1 (#235). Registry design doc + hosting/auth model.**
  Decide what "registry" means on top of the decentralized D-0052 model: an
  **index/discovery service + checksum/transparency log** (and optional caching
  proxy), *not* a mandatory code host; publish/auth flow; namespacing under the
  zaplang-io org; relationship to `zaplang.org`.
  **Size M (policy/design).** Coordinates with the org move (held until sessions
  are quiet) and I#158 community-infra.
- **D2 (#236). Index + checksum/notary service.**
  Implement the hosted index + checksum database (the thing `zap.lock` verifies
  against) and a search/discovery endpoint.
  **Size L.** Depends on D1.
- **D3 (#237). Publish/discovery client.**
  `zap publish` + discovery/search in the CLI, integrated with `pkg`/zenv.
  **Size M.** Depends on D2.

  Area D is **sequenced last**: decentralized install already works, so nothing
  is blocked on it, and it is entangled with the not-yet-done org move.

### Area E — Web playground — gated on I#159

- **E1 (#238). Playground frontend + share-a-snippet.**
  A browser REPL on the docs site driven by the I#159 JIT engine — either
  server-side evaluation (sandboxed) or a WASM build (which ties to the I#158
  WASM target).
  This epic owns the **web frontend + snippet sharing + runnable doc examples**;
  the execution engine is I#159.
  **Size L.** **Hard-gated on I#159** (and, for the WASM route, on I#158 WASM).
  Sequence after the REPL.

### Area F — Diagnostics-quality pass — the pillar-1 multiplier

- **F1 (#239). Diagnostics-quality pass (ongoing).**
  Audit and improve error-message clarity across lex/parse/type: precise spans,
  "did you mean" suggestions, consistent phrasing, stable **error codes**, and
  secondary/labelled spans.
  This matters **doubly** for agent authorship — every improvement lowers
  iterations-to-green, the exact metric I#178 charts — and for human
  readability.
  Reuses the `Diagnostic` infra; error codes also give the LSP and MCP stable
  identifiers.
  **Size M, and genuinely parallel/continuous** — a first concrete pass can run
  alongside A, and it should set the error-code scheme early so fmt/LSP/MCP all
  adopt it.

## Rationale — recommended first slice: Area A (`zap fmt`)

**Recommendation: implement `zap fmt` first — A1 (trivia) → A2 (formatter) →
A3 (check mode) — with a first diagnostics-quality pass (F1), starting with the
error-code scheme, running in parallel.**

Why fmt leads:

1. **Highest leverage per unit effort on the core thesis.** A single canonical
   style means every coding agent emits byte-identical formatting: no style
   drift between agent runs, minimal diffs, zero style debate in review — that
   is pillars 1 (written by agents) and 2 (read by humans) directly, not
   reach-for-its-own-sake.
2. **Small, self-contained, low-risk.** It reuses the parser, touches no
   codegen/runtime (host CI only, no collector matrix), and slots into an
   already-stubbed CLI verb with a locked direction (D-0037) — no protocol, no
   new transport, no design debate about *what* the style is (Python-faithful,
   one true style).
3. **It builds the shared foundation.** A1's comment-trivia work is a
   prerequisite for `zap doc` (C1) and improves LSP range fidelity later, so the
   first slice de-risks two other areas.
4. **It unblocks the community fmt-gate** that the I#158 community-infra wants,
   and makes the project's own contributions consistent immediately.

Why not LSP first: the LSP is the single biggest adoption *lever*, but it is an
**L** (queryable analysis API + transport + interactive features + a client
extension) and should be the **second major slice**, started as soon as fmt is
in flight.
Its diagnostics half (B2) is cheap and could even ship early as a standalone
"errors-only" server if we want an adoption signal sooner.

Why pair F1 in from the start: diagnostics quality is the other direct
pillar-1 multiplier, it is parallelizable, and fixing the **error-code scheme**
early means fmt, LSP, and the MCP server all speak the same stable identifiers
instead of retrofitting them.

### Recommended global sequence

1. **A1 (#225) → A2 (#226) → A3 (#227)** (`zap fmt`), with **F1 (#239)** (diagnostics, error codes
   first) in parallel. ← first slice
2. **B2 (#229)** (LSP diagnostics skeleton) + **B1 (#228)** (analysis API) → **B3
   (#230)** → **B4 (#231)** (VS Code). ← biggest adoption lever, second
3. **C1 (#232) → C2 (#233)** (`zap doc`), reusing A1. (C3 #234 doctests waits on I#159.)
4. **E1 (#238)** (playground) — after I#159 lands.
5. **D1 (#235) → D2 (#236) → D3 (#237)** (registry) — last; waits on the org move, blocks nothing.

## Compatibility

Trivia preservation (A1) must be additive: existing sema/ir/codegen consumers
ignore the new comment nodes, so no behavioural change to compilation.
`zap fmt` defines a canonical style that will reformat existing `.zap` sources
once (a one-time diff), after which the fmt-gate keeps them stable.
Error codes (F1) are a new public surface and fall under the I#205 stability
policy — once published they are part of the 1.0 contract (STABILITY.md).
The LSP, `zap doc` output format, and any registry API are likewise new public
surfaces to be tiered under I#205.

## Implementation

No IR-schema or runtime-ABI boundaries are touched by A/B/C/F (they are
frontend- and driver-only); E and D are new services/clients outside the
compiler.
Parallelism: F1 runs alongside everything; A and the B-analysis API (B1) are
independent; `zap doc` (C) waits only on A1; the playground (E) waits on I#159;
the registry (D) waits on the org move.
Keep `docs/GRAMMAR.ebnf` current if any clarification of comment/docstring
placement is needed while doing A1 (per project convention).

## Open issues

- **A1 shape:** comment-attachment on the existing AST vs a separate lossless
  CST/green-tree layer (the latter is more work now but is the standard
  foundation for best-in-class fmt *and* LSP — worth deciding before A2).
- **A2 style specifics:** max line length, import grouping/ordering rules,
  string-quote canonical form, magic trailing comma (Black-style) or not —
  these should be pinned in a short style spec and reflected into D-0037.
- **B1 scope:** how much resolver state to expose, and whether to build it as an
  incremental/salsa-style query engine now or a recompute-per-keystroke model
  first (the crate split makes recompute viable for a while).
- **LSP transport dependency:** `tower-lsp` vs hand-rolled JSON-RPC, weighed
  against the project's dependency-surface conservatism.
- **F1 error-code scheme:** namespacing (`Zxxxx`?), stability tier, and whether
  codes are assigned retroactively to all existing diagnostics or forward-only.
- **D1 hosting:** where the index/checksum service runs under zaplang-io, the
  auth/publish trust model, and timing against the deferred org move.
- **E1 execution model:** sandboxed server-side JIT vs WASM — the WASM route
  couples the playground to the I#158 WASM target.
