---
title: "Enum @derive breadth: variant-payload diagnostics and the covered surface"
status: Accepted
issue: 1375
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Enum `@derive` breadth + diagnostics (#1375)

**Status:** Accepted — the breadth/dogfood slice of the enum-`@derive` epic
(#1371), on top of the synthesis (#1373), the comptime match (#1374), and the
kind-dispatch enabler (#1394). Delivers the E3d **enum-variant diagnostic**
attribution and records the covered surface and the two breadth cases scoped out
pending core fixes.

## Variant-payload diagnostic attribution

A derive failure a generator raises inside an enum variant payload
(`comptime.error(f.span, …)` for a payload field) now carries a `required
because of field `<name>` of `<Enum>`` frame, the enum analog of the struct-field
attribution (#1293/#1346). Monomorphization previously passed an empty field list
when unrolling `comptime for v in …variants`, so the chain had no frame to anchor;
it now flattens the enum's **variant payload fields** and attributes the cause to
the matching one. The consumer `ZAP7003` points at the payload field with the
library's own message as the root cause, and no comptime-internal symbol leaks.

## Covered surface (goldens + tests)

Working and regression-guarded end-to-end:

- enum-only codecs — tier-1 constant, one/multi-field payloads, mixed
  (`enum_derive_sum`, `enum_derive_tag`, `enum_derive_ordinal`);
- a trait derived across **struct and enum** via top-level kind dispatch
  (`enum_derive_kindswitch`, #1394);
- recursion where an enum variant payload is a **derived struct**
  (`enum_derive_nested`, #1394);
- per-variant comptime data and deterministic **declaration-order** iteration
  (`enum_derive_ordinal`, `comptime v.index`);
- the variant-payload consumer diagnostic (`zap_cli`
  `an_inapplicable_enum_derive_points_at_the_variant_payload_field`).

## Scoped out (core gaps, filed)

Two breadth cases need core language fixes, filed rather than worked around
(language-first):

- **enum→enum recursion** — a method call on an enum-typed variant-pattern
  capture (`reflect.field(payload, i).method()` where the field is an enum) fails
  with `method call on a non-struct value`, though struct captures and
  hand-written match bindings work (#1401). **Update (2026-11-24): resolved as
  NOT a core defect — see `decisions/lang/1401-enum-derive-recursion-supported.md`.
  Enum→enum recursion is supported; this scope-out was based on an incomplete
  generator that called `.sum()` on a scalar leaf field.**
- **generic enum construction** — `Box[int64].Variant(…)` is unresolved
  (`ZAP3001`), so generic-enum deriving cannot be exercised end-to-end though its
  synthesis is unit-tested (#1402).

## Consequence

Enum `@derive` is usable for the common cases — constant and payload enums,
traits shared with structs, nested-struct recursion — with average-programmer-grade
diagnostics pointing at the offending variant payload. Enum→enum recursion is
also supported (#1401, see above); the only remaining enum-`@derive` breadth gap
is generic-enum construction (#1402). E4 (the dogfood port of the built-in
derives) is the next phase.
