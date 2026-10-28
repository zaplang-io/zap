# Proposal: `zap doc` renderers — HTML site + Markdown (C2)

- **Author(s):** zap-doc-render session (I#157/C2, issue #233)
- **Last updated:** 2026-10-24
- **Status:** Draft
- **Discussion:** https://github.com/zaplang-io/zap/issues/233

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

I#233 is the back half of `zap doc` (Area C of the tooling epic #157): render
the serializable **doc model** from #232 (`design/0023-doc-model.md`) as
human-facing documentation — a static **HTML** site and **Markdown**.
The renderers consume the in-memory `zapc_doc::DocModel` directly (the same
model whose JSON is the stable external contract), so there is no JSON
round-trip on the internal path.
Doctests (#234) are gated on the REPL (#159) and out of scope.

## Proposal

### 1. Placement — renderer modules inside `zapc_doc`

The renderers are new modules in `crates/doc` (`markdown.rs`, `html.rs`, and a
shared `render.rs`), not a new crate — mirroring `zapc_fmt`, which keeps the
whole formatter in one crate, and giving the docs website a single dependency.
Public API: `DocModel::to_markdown(&self) -> String` and
`DocModel::to_html(&self) -> String`.
The `driver` gains the user-facing CLI.

### 2. Markdown renderer

One byte-stable page per module: `# <module>` + the module docstring, a
**Contents** index (items grouped by kind, each linking to its anchor), then one
`##` section per kind (Functions, Types, Enums, Interfaces, Constants).
A type nests its fields (a table) and methods beneath it; an enum lists its
variants.
Each item carries an explicit `<a id="…">` anchor, so intra-page links resolve
the same in every viewer regardless of its own heading-slug algorithm.
Output is deterministic (declaration order, nothing host- or time-dependent), so
it is golden-testable and safe to commit to a repo.

### 3. HTML renderer

A single, **self-contained** `index.html`: plain semantic HTML with a small
**inline** stylesheet — no JavaScript, no external assets — matching the
minimal-deps house style.
A header, a `Contents` sidebar with `#anchor` links, and one `<section>` per
item with its signature and rendered docstring.
**Intra-doc links**: a type reference in a signature or a field/variant type
that names a struct/enum/interface in this module links to that type's anchor.
Everything is HTML-escaped, so a docstring containing HTML metacharacters cannot
inject markup.

### 4. Docstring Markdown → HTML — an in-house safe subset, no dependency

The toolchain ships no third-party runtime dependencies — `zap lsp` took no
transport dep, `zap fmt` is hand-rolled, and the #232 model refused `serde` and
hand-rolls its JSON.
Rather than make `pulldown-cmark` the first such dep for the few Markdown
features a docstring uses, the HTML target renders a **documented safe subset**
itself: paragraphs, fenced code blocks, bullet lists, inline code (` `` `
runs, CommonMark-style), `**bold**`, and `*italic*`.
It runs **after** HTML-escaping, so the subset only ever adds known-safe tags
and no raw HTML passes through.
The Markdown target passes the docstring prose through unchanged (light
normalization: a lone `Args:`/`Returns:`/… section header is emphasised).
A heavier parser is unjustified for this input; if a docstring one day needs
richer Markdown, swapping in a vetted crate is a localized change behind
`render::md_to_html`.

### 5. CLI

```
zap doc [--format html|md|json] [--out <dir>] [--private] <file>
```

- `--format html` (default): writes the self-contained site into `--out`
  (default `doc/`) as `doc/index.html`.
- `--format md`: writes `<out>/<module>.md`, or prints Markdown to stdout when
  no `--out`.
- `--format json`: prints the serializable model to stdout (the #232 behaviour;
  `--json` is kept as a back-compat alias).
- `--private`: include `_`-prefixed (private, D-0028) declarations, each marked.

The module's logical name is the file stem (D-0028: one file = one module).

## Rationale

Consuming the `DocModel` struct directly keeps the renderers simple and lossless
(the JSON remains the contract for *external* doc tools).
A self-contained single page per module is the smallest durable artifact: it has
no asset-path contract to break and opens from a file:// URL with no server.

## Compatibility

Purely additive: two new `pub` methods on `DocModel`, three private modules, and
a widened `zap doc` CLI (the #232 flags still work; `--json` is unchanged).
The rendered-docs surface is tiered **Experimental** under the stability policy
(I#205), like the doc model and the LSP.
No grammar, IR-schema, or runtime-ABI boundary is touched.

## Open issues

- **Cross-module doc sets** (a whole package, resolved imports, inherited
  interface methods) remain future work; today `zap doc` renders one module per
  invocation, and `index.html` is that module's page.
  When multi-module lands, `index.html` becomes a package index and each module
  gets its own page — a forward change, not a break of the current artifact.
- **Type/field/variant docstrings** are not representable in the grammar yet
  (module/def only, per #232); the renderers already have a slot for them and
  will show them with no further change when extraction fills `doc`.
- **Non-section middle prose** in a docstring is rendered verbatim (the renderer
  shows the whole cleaned docstring, not only the parsed summary + sections), so
  nothing is lost.
