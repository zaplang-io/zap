---
title: "E5: library @derive(Json) struct decode"
status: Accepted
issue: 1557
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Library `@derive(Json)` struct decode (#1557)

**Status:** Accepted — the E5 (#1551) decode slice. A library `@derivable`
generator decodes a struct from JSON over the DOM, agreeing with the built-in
`@derive(Json)` decode. Builds on the E4 shadow decode (#1470/#1490/#1522) and
extends it to scalar `list`/`dict` fields and the AWS hooks. The built-in is
untouched.

## Mechanism

Parse with `json.loads`, zero-construct with `reflect.construct(T)`, write each
field with `reflect.set_field(out, comptime f.index, …)`, reading each field's
`JsonValue` off the decoded object with ordinary source helpers (a `match` on the
DOM — no compiler-private `__json_*`).

## Covered surface

`int64` / `str` / `bool` / `float64`; a nested derived struct **field** (recurses
via the generator, `T` inferred from the `set_field` destination, #1490); an
`Optional[int64]` (absent key OR explicit `null` → `None`, else `Some`, D-0065);
`list[scalar]` and `dict[str, scalar]` (decode each element/value off the DOM
into the zero-constructed empty collection field); and the AWS hooks — a base64
JSON string → `bytes` (`base64.decode`), a JSON number/string → `DateTime` per
`@json(timestamp=)` (`datetime.from_timestamp` / `datetime.parse`), the timestamp
form read off `f.attrs` by nested `comptime for`.

## Arm order matters (two comptime-dispatch gotchas)

1. Container/kind arms must precede `f.ty.name` checks — evaluating `f.ty.name`
   on a `list`/`dict` field misbehaves; lead with `f.ty.kind.name == "list"/"dict"`.
2. The `optional` arm must precede the `list` arm — an `Optional` field otherwise
   falls into the `list` element dispatch (`f.ty.kind.name == "optional"` is
   checked first). `DateTime` (a struct kind) is matched by `f.ty.name` before the
   generic `struct` arm. These are recorded so later codec generators avoid them.

## NOT covered — blocked on core gap #1575 (reported, not worked around)

A nested **derived struct** as a `list` element or `dict` value (`list[Item]` /
`dict[str, Item]`) cannot be decoded: the element decoder must be invoked for a
comptime-known element type with **no `set_field` destination to anchor
inference**, and an explicit type-argument call (`generator[f.ty.elem](…)`) does
not resolve inside a specialized generator body (`ZAP6001 unknown variable`).
Encode avoids this — it recurses via a method call on a known value. Filed as
**#1575** under #1248. Enum decode and generic-struct decode remain blocked on
**#1561**. The built-in decode stays the oracle; the retire slice (#1558) defers
these cases until #1575 / #1561 land.

## Proof + CI gate

`tests/golden/derive_json_decode.zap` proves agreement with the built-in:
`T.from_json(v.to_json()) ==` the original for the Eq structs (scalars, nested
field, Optional present/absent), and keyed/indexed inspection for the collection
and AWS-hook structs (not Eq). `zap ir` lowers clean and `zap fmt --check` is
clean locally; agreement is gated by the CI LLVM-18 matrix (Docker/LLVM down).
