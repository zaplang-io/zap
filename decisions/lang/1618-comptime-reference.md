---
title: The compile-time reflection / comptime surface has a normative in-repo reference
status: Locked
issue: 1618
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# The compile-time reflection / comptime surface has a normative in-repo reference

**Status:** Locked

## Decision

The compile-time reflection / `comptime` / `@derive` surface — shipped across
epic #1248 (E1 reflection records → E2 the comptime evaluator → E3
`@derivable`/`@derive` → E4–E6 the Json and `serde` codecs built on it) — is now
**load-bearing** (the prelude `@derive(Json)` and the bundled `serde` module are
both implemented in it), so it gets a **normative reference that lives in the
repository**, not in Discussions:

- a dedicated chapter, [`docs/COMPTIME.md`](../../docs/COMPTIME.md), covering
  `reflect.type_info(T)` and the reflection records, the kind-switch, `comptime
  for` / `comptime <expr>` / the comptime variant-match arm, `reflect.field` /
  `set_field` / `construct` / `construct_variant`, `@derivable` / `@derive`, the
  pure/sandboxed subset and its `not available at comptime` backstop, the
  `ZAP7xxx` diagnostics and instantiation backtrace, and the compile-time cost
  model;
- [`docs/GRAMMAR.ebnf`](../../docs/GRAMMAR.ebnf) kept current for the surface —
  this ADR adds the previously-undocumented **comptime variant-match arm**
  (`case comptime <v> in <seq> [as <payload>]:`, #1374) to `closed_pattern` and
  cross-references the chapter from `decorators` and `comptime_for_stmt`;
- a [`docs/STABILITY.md`](../../docs/STABILITY.md) Language-table row recording
  the surface's tier.

This follows the repo's golden rule for documentation placement (normative,
tooling/reader-read reference → repo; snapshot analyses / proposals / RFCs →
Discussions): a language reference a library author reads to write their own
`@derivable` is normative, so it belongs in-tree alongside `FORMAT.md`,
`MEMORY_MODEL.md`, and `GRAMMAR.ebnf`.

## Rationale

The mechanism was built and dogfooded without a reader-facing reference: before
this, the only documentation was `SPEC.md`'s per-feature `@derive(Eq/Ord/Hash/
Repr/Json)` entries and the crate module doc-comments. A library author had no
single place describing the reflection records' accessors, the kind-switch
idiom, the `reflect.*` primitives, or — critically — the exact boundary of the
sandboxed subset. `docs/COMPTIME.md` is that place, and keying it to this ADR
(as `MEMORY_MODEL.md` keys to D-0141) keeps the reference and its decision record
together.

## Consequences

- **Stability tier: Experimental.** The surface is shipped but not promised; its
  `ZAP6xxx` lowering and `ZAP70xx` comptime diagnostics are back-end/experimental
  codes. The chapter and the STABILITY row both say so. Graduation to Stable is a
  later, additive step (STABILITY §4.2).
- **Docs-only.** This slice changes no compiler behavior; it is gated by host
  checks and `zap fmt --check` on any example, with runtime correctness of the
  examples covered by the existing golden suite under the CI LLVM matrix. It is
  not a public-surface change in the changelog sense (the surface it documents
  already shipped), so it adds **no `changelog.d/` fragment** — consistent with
  `changelog.d/README.md`, which carves out non-surface changes and recognizes
  only the Keep-a-Changelog kinds (`added`/`changed`/`deprecated`/`removed`/
  `fixed`/`security`).
- The chapter is the reference the two sibling E7 slices (instantiation-backtrace
  diagnostics, #1616; comptime cost caps + benchmark harness, #1617) document
  their work against; its §7 and §8 describe the mechanisms those slices extend.
