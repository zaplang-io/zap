---
title: "E4 shadow-proves @derive(Ord) over the @derivable mechanism"
status: Accepted
issue: 1443
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Shadow-prove `@derive(Ord)` (#1443)

**Status:** Accepted — an E4 (#1429) dogfood slice, same shape and rationale as
the Eq shadow proof (see `decisions/lang/1431-derive-eq-shadow-prove.md`): a
library `@derivable` generator expresses the derive and a behavior-identical
golden shows it agrees with the built-in, **changing no wiring**.

## Built-in

`@derive(Ord)` → `gen_cmp_method` → `__cmp__(self, Self) -> int64`: per field in
declaration order a three-way compare (`gen_field_cmp`: `zap_str_cmp` for str,
greater/less selects for int/float), returning the first non-zero, else 0.
Scalars and str only.

## Shadow

A library `@derivable(StructOrd)` generator returns −1 / +1 on the first field
where `reflect.field(a, i) < / > reflect.field(b, i)`, else 0. The per-field
`<` / `>` lower to the same primitives the built-in uses, so the ordering agrees.

## Agreement is on sign, not raw value

`zap_str_cmp` is not normalized to ±1, so the built-in `__cmp__` can return any
negative/positive integer for a str-differing field, while the shadow returns
exactly −1 / +1. The ordering contract only uses the **sign** (`a < b` desugars to
`a.__cmp__(b) < 0`), so the golden asserts sign agreement between the built-in
(`<` / `==`) and the shadow (`.structcmp() < 0` / `== 0`) — never raw `__cmp__`
values.

## Guard

- Golden `tests/golden/derive_ord_shadow.zap`: int-primary and str-tiebreak
  orderings, both directions and equal (runtime output, CI LLVM matrix).
- Host test `a_library_generator_shadows_the_built_in_ord_without_replacing_it`:
  both `@P____cmp__` (built-in, untouched) and `@_derive_structord$P` emitted.

No wiring changes; the built-in `gen_cmp_method` stays.
