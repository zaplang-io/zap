---
title: "Widen reflect.construct's zero set to nested-struct / Optional / collection / tuple fields"
status: Accepted
issue: 1490
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Widen `reflect.construct`'s zero set (#1490)

**Status:** Accepted — an E4 (#1429) sub-issue that carves out the **deferred
field-kind scope** of `reflect.construct` (#1454). It widens the zero-value set
the reflection *write* surface can build, and consumes that widening in the Json
and Yaml codec shadows so a nested-struct / `Optional` record now round-trips
through the **library** decoder.

## Why `reflect.construct` was the gate

A `@derivable` codec decoder follows one shape: zero-construct a `T`, then
overwrite each field via `reflect.set_field`:

```zap
auto out = reflect.construct(T)
comptime for f in reflect.type_info(T).fields:
    reflect.set_field(out, comptime f.index, <decoded value>)
return out
```

`reflect.set_field` is already type-agnostic — it desugars to `out.<field> = v`,
which lowers for any field type. The sole restriction was the zero-constructor:
#1454's v1 built a zero only for scalar / `str` / `bytes` fields and errored
cleanly ("unsupported yet") for anything else, mirroring the derive codegen's
field-type restrictions. So a nested-struct / `Optional` / collection field was a
hard stop at *decode-construction* time — the decoder generators were fine; the
value they stood on could not be built.

This issue was a **planned widening of that documented v1 scoping, not a defect
fix**.

## Two layers

### Layer 1 — the enabler (`crates/mono`, `zero_expr`)

`zero_expr` now builds a zero for, in addition to the v1 scalar / `str` / `bytes`
set:

- **nested struct** → recurse into `NestedType(zero(f0), …)`;
- **`Optional[T]`** → the empty optional (`None`);
- **`list[T]` / `set[T]` / `dict[K, V]`** → the empty literal (`[]` / `{}`);
- **tuple `(A, B, …)`** → the zero of each element.

A field type that still has no zero value — a **function type** — remains a clean
"unsupported yet" error naming the field and type.

**Cycle guard.** A struct that transitively contains itself *by value* (directly,
`struct S: S inner`, or indirectly `A → B → A`) has no finite zero. The recursion
carries a `visiting` stack of the struct names on the current by-value path and
reports a clean compile error on re-entry — never an infinite recursion. A
self-reference reached through an `Optional` / `list` / `dict` / `set` is **not** a
cycle: that field's own empty zero breaks the chain, so a recursive data type
(`struct Node: Optional[Node] next`) zero-constructs fine.

The pre-existing scalar / `str` / `bytes` path is byte-for-byte unchanged; no
existing program changes behavior.

Layer 1 is proven end-to-end by the `construct_zero_set` golden: a generator
zero-constructs a struct with a nested-struct, `Optional`, `list`, `dict`, `set`
and tuple field, and reads each field back (zero-valued nested, empty
collections, empty optional). The host tests (`crates/driver/tests/zap_cli.rs`)
cover the same kinds plus the cycle and function-type error paths.

### Layer 2 — codec decode branches (library Zap)

With Layer 1 in place the Json (#1470) and Yaml (#1484) decode shadows gain a
**nested-struct** branch over `f.ty.kind.name`: decode it via the nested type's
own `from_*codec()` — the same `@derivable` generator specialized at the field
type, the explicit type argument inferred from the `set_field` destination. The
golden now proves the previously encode-only nested-struct case round-trips
through the **library** decoder (before #1490 it was cross-checked only through
the built-in decoder).

## Scope: what Layer 2 does *not* cover, and why

The codec shadows exist to *agree with the built-in codec by round-trip*, so a
field kind is only shadow-able when the built-in codec round-trips it. Only
nested structs qualify today; the rest are left to the built-in codecs'
collection/optional follow-up, **not** worked around (workspace golden rule):

- **`Optional` fields.** The built-in `@derive(Yaml)` cannot codegen an `Optional`
  field at all (it emits a call to a non-existent `Optional$T__to_yaml`), and the
  built-in `@derive(Json)` does not represent an `Optional` in a form this
  source-accessible `JsonValue` DOM decoder round-trips. So there is no built-in
  codec to agree with. `reflect.construct` *does* zero-construct an `Optional`
  field (the empty optional) — that is Layer 1, proven by `construct_zero_set`.
- **`list` / `dict` / `set` fields.** The built-in `@derive(Json)` / `@derive(Yaml)`
  do not support collection fields yet ("collections and enums are a follow-up"),
  and `@derive(Eq)` rejects a non-hashable `list` / `dict` field, so there is no
  built-in codec to agree with and no `==` to assert it. Again, Layer 1 *does*
  zero-construct them (empty), proven by `construct_zero_set`.
- **CSV shadow (#1480)** is a flat-record codec with no DOM; the built-in
  `@derive(Csv)` has no nested / collection / optional surface, so it is unchanged.

## Alternatives considered

- *Add an `Optional` decode branch to the codec shadows.* Attempted, then
  reverted: the built-in Yaml codec cannot codegen an `Optional` field, and the
  naive DOM decode of a built-in-encoded `Optional` dereferenced a missing key —
  i.e. there is no built-in `Optional` codec to shadow yet. Deferred to the
  built-in optional/collection follow-up.
- *A total zero-constructor (every field type).* Rejected: a function type has no
  meaningful zero, and a by-value cycle has none at all; a clean error is the
  correct answer for both.
