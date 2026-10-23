# Proposal: Queryable frontend analysis API (symbol / type / def at position)

- **Author(s):** lsp-analysis-api session (I#157/B1, issue #228)
- **Last updated:** 2026-10-23
- **Status:** Draft
- **Discussion:** https://github.com/parveshgarg/zap/issues/228

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

I#228 is the foundation of the LSP slice (Area B of the tooling epic #157,
`design/0021-tooling.md`).
The diagnostics half of an LSP already exists: `check_source` (behind the MCP
server, #72) runs lex → parse → type-check with stable `ZAPnnnn` codes (#239)
and no LLVM.
What is missing is a *queryable* result — the resolver's name-resolution and
inferred-type state is internal to `sema::Checker` and discarded when a pass
returns, so there is no "symbol at this position", "type of the expression
here", or "definition site of this name" query.

This doc specifies that queryable surface: a new leaf crate `zapc_analysis` that
exposes a position-keyed analysis handle, and the *minimal, gated* retention
hook inside `sema` that makes the resolver's decisions observable without
changing what the type checker computes or costing the normal compile path.
It does **not** build the LSP transport/server (that is #229) — only the
in-process API #229 and #230 call.

## Background — what the tree gives us (verified)

- **`sema::Checker` is the resolver.**
  `infer(&Expr) -> Ty` already computes a type for *every* expression (each
  `Expr` carries a `Span`), and `Expr::Name(n, span)` resolution already runs
  through `lookup(n)` against a scope stack `scopes: Vec<HashMap<String, Ty>>`.
  Both results are thrown away after the pass.
  This is the whole asset: the hard work (scoping, inference, generic/method
  resolution) is done; we only need to *record* it.
- **Declarations carry spans, names do not (yet).**
  `Function`, `Param`, `StructDef`, `Field`, `EnumDef`/variant, `Let`, `For`
  targets, comprehension vars, match captures, except bindings — each has a
  `span`, but it is the span of the whole construct; there is no name-only
  sub-span on a declaration (`Expr::Name`, `For` targets, and `Pattern::Capture`
  *do* have a precise name span).
  Go-to-def targets are therefore construct-granular for `Let`/params/
  comprehension vars and name-precise elsewhere; this tightens for free when the
  A1 CST/trivia work (#225) or name-spans land, with no API change.
- **`Ty::display()`** renders a type for hover; **`span_to_line_col`** (MCP
  module) maps a byte offset to 1-based line/col and will be promoted by #229
  for the UTF-16 layer.
- **The crate split (#129)** gives clean frontend crates (`syntax`, `sema`,
  `sema_types`, `diag`) with no LLVM dependency, so a tooling crate above `sema`
  re-analyses a document in milliseconds.

## Proposal

### 1. The queryable result — `zapc_analysis`

A new leaf crate `crates/analysis` (crate `zapc_analysis`), depending only on
`zapc_syntax`, `zapc_sema`, `zapc_sema_types`, `zapc_diag`.
It owns the ergonomic, position-keyed surface; `driver` (and later `fmt`/`doc`)
depend on it.

```rust
/// A fully analysed single document. Construct once per source snapshot; every
/// query is a cheap lookup against the retained tables.
pub struct Analysis { /* module AST + retained facts + line index + defs */ }

impl Analysis {
    /// Lex, parse, type-check, and retain the analysis facts.
    pub fn analyze(source: &str) -> Analysis;

    /// Structured diagnostics (reuses `check_source`'s payload + ZAPnnnn codes).
    pub fn diagnostics(&self) -> &[Diagnostic];

    // --- position-keyed queries; `Pos` is a byte offset, with line/col helpers ---

    /// The symbol under the cursor: its name, kind, type, and definition span.
    pub fn symbol_at(&self, pos: Pos) -> Option<SymbolRef>;

    /// The inferred type of the innermost expression covering `pos`.
    pub fn type_at(&self, pos: Pos) -> Option<Ty>;

    /// The definition site of the name under the cursor (go-to-def target).
    pub fn definition_of(&self, pos: Pos) -> Option<Span>;

    /// All use sites of the symbol under the cursor, within this document.
    pub fn references(&self, pos: Pos) -> Vec<Span>;

    /// The document-symbol tree (functions, types, fields, methods, …).
    pub fn document_symbols(&self) -> &[DocumentSymbol];

    // --- position mapping (promoted span_to_line_col; UTF-16 is #229's layer) ---
    pub fn offset_at(&self, line: u32, col: u32) -> Option<Pos>;
    pub fn line_col(&self, pos: Pos) -> (u32, u32);
}

pub struct SymbolRef { pub name: String, pub kind: SymbolKind,
                       pub ty: Ty, pub def: Span }
pub enum SymbolKind { Function, Method, Param, Local, Const, Struct, Enum,
                      EnumVariant, Field, Interface, Import, TypeParam, Builtin }
pub struct DocumentSymbol { pub name: String, pub kind: SymbolKind,
                            pub span: Span, pub name_span: Span,
                            pub detail: Option<String>,
                            pub children: Vec<DocumentSymbol> }
```

`document_symbols` is a pure AST walk (no checker needed) → it is always
available even when the body has type errors, which is what an outline needs.
`references` is document-local (cross-file is a later ticket, below).

### 2. Retaining resolver state — the load-bearing bit (minimal, gated)

The type-checking algorithm is **not** restructured.
`sema` gains an *optional, analysis-only* sink that records the decisions the
checker already makes:

- **Add `analysis: Option<AnalysisFacts>` to `Checker`** (default `None`).
  A new entry point `sema::analyze_queryable(&Module) -> (errors, warnings,
  AnalysisFacts)` sets it `Some`; `check`/`analyze` (the compile path) leave it
  `None`, so codegen builds record nothing.
- **`AnalysisFacts`** (returned to `zapc_analysis`) holds three span-keyed
  tables, all built only when the sink is `Some`:
  - `expr_types: Vec<(Span, Ty)>` — pushed by a one-line wrapper that splits the
    existing `infer` into `infer` (records) + `infer_inner` (the current body,
    unchanged). This is the entire basis of `type_at`.
  - `resolutions: Vec<(Span /*use*/, Span /*def*/, SymbolKind)>` — pushed in the
    `Expr::Name` arm (and attribute/method/variant access where resolvable)
    linking a use to its definition. This is `definition_of` / `references`.
  - `locals: Vec<(Span /*def*/, Ty)>` — definition sites with their inferred
    type, for `symbol_at` on a binding.
- **Def spans for locals** come from a *parallel, gated* scope stack
  `def_scopes: Vec<HashMap<String, (Span, SymbolKind)>>` that mirrors `scopes`.
  `declare` gains a `def: Span` argument (threaded through its ~17 call sites —
  mechanical, every site has a span in hand); when `analysis` is `Some`, the pair
  is also recorded in `def_scopes`, and name resolution reads the def span from
  there.
  The **existing** `scopes: Vec<HashMap<String, Ty>>` and the 8 `lookup` sites
  that return `Ty` are **untouched**, so no type-logic changes and the type
  checker still returns byte-identical diagnostics.
- **Def spans for globals** (functions, structs, enums, fields, methods —
  resolved via `sigs`/`structs`/`enums`, which carry no spans) come from an
  AST-built `global_defs: HashMap<String, Span>` in `zapc_analysis`, consulted
  when a `Name` resolves to a signature/type rather than a local.

**Why this keeps the compile path unchanged and free:**
the only always-on change is `declare` taking a `Span` (Copy, 8 bytes) it drops
when `analysis` is `None`, and the `infer`/`infer_inner` rename (one `if let
Some` branch per expression in `None` mode — negligible, and the recording
`Vec`s are never allocated).
No diagnostic, no inferred type, and no scope decision changes.
Proven by the existing sema + codegen goldens staying byte-identical.

**Is this a significant sema restructure?** No — it is additive: one optional
field, one parallel gated scope stack, a mechanical `declare` signature change,
and an `infer` wrapper split.
The checking algorithm, the `Ty` model, and every existing public entry point
are unchanged.
So per the #228 plan this is the "clean additive side-table" path, and I proceed
after posting this shape — flagging the one non-trivial touch (the `declare`
signature + parallel scope stack) here so it can be halted if the shape is
wrong.

### 3. Incremental story

- **Recompute-per-document now.** Each `Analysis::analyze(source)` re-runs
  lex+parse+check for *one* document. Thanks to the #129 split this is
  LLVM-free and fast enough for per-keystroke use for a long while, which the
  epic's open question (#228: "salsa-style engine now, or recompute first?")
  explicitly says is viable. I recommend **recompute-first**; a query engine is
  a later ticket.
- **The API does not preclude incrementality.** The handle is *immutable per
  snapshot* and keyed by span/offset, which is exactly the boundary a future
  salsa/query layer memoizes behind — swapping the engine changes how `Analysis`
  is built, not its query surface, so #229/#230 are insulated.
- **Cross-file.** `global_defs` and `resolutions` are document-local in this
  ticket (the issue scopes it to a single changed document). `SymbolRef.def` /
  `definition_of` return a `Span` in the current document; a later multi-module
  ticket (building on the import graph, D-0052/#190) widens the def target to a
  `(file, span)` without changing the query names.

## Rationale — a new crate, not a `sema` module

Putting the handle in `sema` would grow the type checker a second, tooling-aimed
public API and invite tooling concerns (position mapping, document symbols,
UTF-16) into the resolver.
A leaf crate above `sema` keeps `sema`'s surface the two gated hooks only, gives
#229 a single in-process dependency with no LLVM, and matches #129's intent
(frontend crates reused by tooling) and the epic's "machine-friendly analysis
surface".
`sema` exposes just the raw `AnalysisFacts`; all presentation lives in
`zapc_analysis`.

## Compatibility

Purely additive (D-0072).
`check`, `analyze`, `check_str`, `check_source`, and the MCP surface are
unchanged; codegen records nothing.
The analysis API is a new public tooling surface and is tiered **Experimental**
under the stability policy (I#205), like the LSP/`zap doc` surfaces.
No IR-schema or runtime-ABI boundary is touched.

## Implementation

- `crates/analysis` (`zapc_analysis`): `Analysis`, the query methods, the AST
  definitions/document-symbol walk, `global_defs`, the `LineIndex`, and the
  `Diagnostic` re-export.
- `crates/sema`: `analyze_queryable`, the `analysis` field + `AnalysisFacts`,
  the `def_scopes` parallel stack, the `declare(.., def)` threading, and the
  `infer`/`infer_inner` split.
- Tests: position → symbol/type/def on representative sources (locals, params,
  functions, methods/`self`, structs + fields, enums + variants, generics,
  comprehension/for/match bindings, imports D-0052); document-symbol trees;
  and goldens proving the compile path is byte-identical.
- Host CI only (no codegen/runtime touched), but the full matrix must stay green
  via the unchanged goldens.

## Open issues

- **Query-engine timing** (#228 open question): recommend recompute-first;
  salsa-style memoization is a later ticket the immutable-snapshot API allows.
- **Name-granular def spans** for `Let`/params/comprehension vars wait on A1
  (#225) CST/name-spans; construct-granular until then (no API change).
- **Cross-file go-to-def** widens `def` to `(file, span)` in a later
  multi-module ticket (D-0052/#190 import graph).
