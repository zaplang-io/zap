# Proposal: `zap doc` model extraction — the serializable doc model (C1)

- **Author(s):** zap-doc-extract session (I#157/C1, issue #232)
- **Last updated:** 2026-10-24
- **Status:** Draft
- **Discussion:** https://github.com/zaplang-io/zap/issues/232

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

I#232 is the front half of `zap doc` (Area C of the tooling epic #157,
`design/0021-tooling.md`): extract a structured, serializable **doc model** from
Zap source.
`zap doc` was deferred at D-0053/D-0054; this builds the extractor.
The renderers (HTML + Markdown, #233) and doctests (#234, gated on the REPL
#159) are **out of scope** — this doc specifies only the model and the JSON that
is the contract between #232 and #233.

The extractor is a new leaf crate `zapc_doc` that walks a parsed module and
collects its documentable declarations — the module itself, functions, structs
(fields + methods), enums (variants + methods), interfaces, and top-level
constants — each with a fully-qualified name, a rendered signature, the attached
docstring, source location, and visibility.
It **reuses the frontend**: signatures render through `zapc_analysis`'s
`render_fn_signature`/`render_type` (so a doc signature reads *identically* to an
LSP hover), and source positions + inferred constant types come from the same
`zapc_analysis::Analysis` the LSP runs.
Extraction itself is a pure AST walk, so it degrades gracefully on a file with
type errors, exactly like the document-symbol outline.

## Background — what the tree gives us (verified)

- **Docstrings are first-class, module/def-level (SPEC §2, D-0041).**
  `ast::docstring(&Function)` returns the first-statement bare non-f
  triple-quoted string of a function/method.
  A **module** docstring is the same shape as the module's first top-level
  statement.
  **Types have no docstring slot:** `StructDef`/`EnumDef`/`InterfaceDef` carry no
  body block, and `parse_struct_body` parses only fields + methods.
  Narrative `#` comments need the A1 trivia work (#225), not yet landed.
  So `doc` is populated for the module and functions/methods only; it is reserved
  (`null`) for types/fields/variants/constants and will fill, with **no schema
  change**, when type docstrings or #225 land.
- **Visibility already exists and is enforced (SPEC §13, D-0028).**
  "Everything is public **except** names starting with `_`, which are private and
  enforced at compile time."
  `zap doc` adopts this locked rule verbatim — it introduces no new language
  notion.
- **`zapc_analysis` (#228) already renders signatures and maps positions.**
  `render_fn_signature`/`render_type` produce the type-first surface syntax hover
  shows; `LineIndex` maps a byte offset to 1-based line/column; `Analysis::type_at`
  infers an expression's type.
  Constants are `Item::Stmt(Stmt::Let { is_const: true, .. })`.

## Proposal

### 1. Placement — a new leaf crate `zapc_doc` (`crates/doc`)

A host-only tooling crate above the frontend, matching `zapc_fmt`/`zapc_analysis`.
Dependencies: `zapc_syntax` (the AST walk), `zapc_analysis` (signature rendering,
`LineIndex`, inferred const types), `zapc_sema_types` (`Ty`).
No LLVM, no codegen — host CI only.
`driver` gains a thin `zap doc [--json] [--private] <file>` entry point; the
rendering half is #233.

### 2. The only change to `zapc_analysis` — two helpers made `pub`

`render_fn_signature(&Function)` and `render_type(&Type)` become `pub`.
Both are **pure AST functions** already living in the tooling crate; **`sema` is
not touched**, so the compile-path parity test (`analyze_queryable` vs `check`)
is unaffected.
This is additive (D-0072) and keeps doc signatures byte-identical to hover.

### 3. Visibility policy

A declaration's `visibility` is `"private"` if its name starts with `_`, else
`"public"` — the D-0028 rule.
The default model documents the **public surface only**; `--private` (or
`DocOptions::include_private`) includes private declarations, each tagged
`"private"`.
The rule is applied uniformly to top-level items, struct fields, methods, and
enum variants.

### 4. Docstring structure

A docstring is kept verbatim (`raw`) plus a light Google-style split (D-0054):

- `summary` — the first paragraph, `inspect.cleandoc`-dedented and collapsed to
  one line (what an index page shows).
- `sections` — the labelled blocks (`Args:`/`Returns:`/`Raises:`/…), in source
  order, each dedented.
  A section header is a line of the form `Label:` with nothing after the colon.

No new convention is invented; this is exactly the structure D-0054 already
describes.

### 5. The JSON contract (what #233 consumes)

`DocModel::to_json_string()` emits pretty-printed, deterministic,
insertion-ordered JSON.
Per the toolchain's no-external-deps rule (`driver/src/json.rs`; `zap mcp`/`zap
lsp`/`zap test --json` all hand-roll JSON), this uses a small self-contained
writer, **not `serde`**.

Top level:

```json
{
  "module": { "name": "shapes" | null,
              "doc": <DocString> | null },
  "items": [ <DocItem>, ... ]   // declaration order
}
```

A `DocItem` has a common header plus kind-specific fields, tagged by `kind`
(`function` | `method` | `struct` | `value_struct` | `enum` | `interface` |
`const`):

```json
{
  "kind": "function",
  "name": "area",
  "qualified_name": "shapes.area",     // "module.Type.member" when nested
  "visibility": "public" | "private",
  "line": 9, "col": 1,                  // 1-based, of the declaration start
  "span": { "start": 95, "end": 248 }, // byte offsets
  "doc": <DocString> | null,
  // kind-specific:
  "signature": "def area(float64 r) -> float64",  // hover-identical
  "generics":  [ { "name": "T", "bound": "Ordered" | null } ],
  "params":    [ { "name": "r", "type": "float64" | null,
                   "default": <src> | null, "variadic": false } ],
  "ret":       "float64" | null,
  "decorators": [ "@memoize", "@retry(times=3)" ]
}
```

- `struct` / `value_struct`: `signature` (`struct Name[G]`), `is_value`,
  `generics`, `decorators`, `fields[]`, `methods[]` (each a `method` `DocItem`).
  A field is `{ name, type, default|null, bits|null, visibility, doc:null }`.
- `enum`: `signature`, `generics`, `decorators`, `variants[]`, `methods[]`.
  A variant is `{ name, value|null, fields:[{name,type}], visibility, doc:null }`.
- `interface`: `signature`, `generics`, `methods[]` (method signatures, no body).
- `const`: `signature` (`const <type> NAME`), `type` (declared, else inferred via
  the analysis API, else `null`).

`DocString` is `{ "raw": <verbatim>, "summary": <one line>,
"sections": [ { "label": "Args", "body": "r: the radius" } ] }`.

Design choices that make this a durable contract for #233:

- **Declaration order** is preserved; nesting is structural (methods/fields/
  variants under their owner), so a renderer walks the tree directly.
- The `signature` string is the human-ready, hover-consistent line; the
  **structured** fields (generic *bounds*, decorators, param defaults/variadics,
  bitfield widths) carry the full detail so #233 renders richly **without
  re-parsing**.
- `null` vs omitted: a field that *could* carry data but does not is `null`
  (e.g. a type's `doc`), so the shape is uniform per kind.

### 6. CLI

`zap doc --json <file>` prints the model; `--private` includes private
declarations.
Bare `zap doc <file>` prints a note pointing at #233 for rendering, reserving the
default output for the renderers.
The module's logical name is the file stem (D-0028: one file = one module).

## Rationale — a new crate, reusing analysis

A leaf crate keeps extraction off the type checker and out of `driver`, mirrors
#129's "frontend crates reused by tooling" intent, and gives #233 a single
dependency with a stable model.
Reusing `zapc_analysis` for signatures guarantees `zap doc` and the LSP never
drift in how they present a declaration — a direct win for pillar 2 (read by
humans).

## Compatibility

Purely additive.
`zapc_analysis` gains two `pub` helpers; nothing else in the frontend changes,
the compile path is byte-identical, and no existing golden moves.
The doc model / JSON is a new public tooling surface, tiered **Experimental**
under the stability policy (I#205), like the LSP and the eventual rendered docs.
No IR-schema or runtime-ABI boundary is touched; no grammar change (visibility
and docstring placement are already in `SPEC.md` / `docs/GRAMMAR.ebnf`).

## Open issues

- **Type / field / variant docstrings** are not representable in the grammar
  today (module/def only).
  A future ticket either adds a docstring slot to type declarations or attaches
  narrative `#` comments via #225 trivia; the `doc` field is already in the schema
  for it.
- **Cross-module doc sets** (a whole package, resolved imports, inherited/
  interface-provided methods) are a #233-era concern; #232 is one module per
  invocation.
- **Richer docstring parsing** (param-type cross-checks against the signature,
  `Examples:` extraction for #234 doctests) builds on the `sections` split here.
