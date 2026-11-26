---
title: "E5: library @derive(Json) encode for list / dict collection fields"
status: Accepted
issue: 1559
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Library `@derive(Json)` encode for `list` / `dict` fields (#1559)

**Status:** Accepted — the E5 (#1551) encode slice that extends the byte-identical
library encoder (#1554) to the collection surface the built-in gained in #1530
(`list[T]`) and #1531 (`dict[str, V]`). The built-in is left intact.

## The proof method differs by kind — deliberately

- **`list[T]` is ordered.** The library encode is **byte-identical** to the
  built-in: `l.to_json() == l.to_jsonbytes()`. The generator builds `[` +
  `",".join` of per-element leaves + `]`, elements in list order, matching the
  built-in's `zap_csv_join` + `[]` layout.
- **`dict[str, V]` iteration order is unspecified and seed-dependent (#1066).**
  The built-in's own `derive_json_dict_field` golden (#1531) deliberately avoids
  any byte / `==` assertion for exactly this reason and proves the codec by keyed
  inspection of the decoded value. This slice follows the same rule:
  - an **empty** and a **single-entry** dict are order-trivial, so those ARE
    asserted byte-identical (`d1.to_json() == d1.to_jsonbytes()`), pinning the
    object envelope, key escaping, and value leaf against the built-in;
  - a **multi-entry** dict is proven by **round-trip** — the library's
    `to_jsonbytes()` is parsed back with the built-in `from_json` and inspected
    by key (order-independent). Asserting a multi-entry dict's bytes would be
    asserting an unstable order, which the dict memory rule forbids.

This is the first E5 point where "byte-identical" is not the universal gate: for
an unordered container it **cannot** be, because the built-in's own output is not
byte-stable. The retire slice (#1558) inherits this — its dict regression wall is
the existing keyed-inspection golden, not a byte golden.

## Element-type recursion needs no new primitive

The generator dispatches each element/value leaf with the SAME per-kind logic as
a scalar field, keyed on the **comptime element type**: `f.ty.elem` for a list,
`f.ty.value` for a dict (the container-element accessors from E1/#1253). The
inner `if f.ty.elem.name == …` folds per unrolled field (dead-arm elision #1326),
so `x` has the concrete element type in the surviving branch. A nested derived
struct element/value recurses through its own `to_jsonbytes()`. Ordinary source
iteration (`for x in xs`, `for k, v in d.items()`) drives the runtime walk. No
comptime/reflection primitive was added.

## Proof + CI gate

`tests/golden/derive_json_encode_collections.zap` covers `list[int64]` /
`list[str]` (incl. an escape-bearing element) / `list[Item]` (nested), empty
lists, and `dict[str,int64]` / `dict[str,str]` / `dict[str,Item]` across empty,
single-entry (byte-identical) and multi-entry (round-trip keyed) cases. `zap ir`
lowers clean and `zap fmt --check` is clean locally; the byte-identical and
round-trip equalities are gated by the CI LLVM-18 matrix (Docker/LLVM down).
