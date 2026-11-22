---
title: Bare payload `T` auto-wraps to `Some(T)` in every `Optional[T]` position sema accepts
status: Locked
issue: 1262
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Bare payload `T` auto-wraps to `Some(T)` in every `Optional[T]` position sema accepts

**Status:** Locked

## Decision

A bare value of type `T` supplied where an `Optional[T]` is expected is lowered
to a tagged/boxed `Some(T)` — never stored as the raw scalar/pointer. This is the
**symmetric partner** of the bare-`None` lowering (#1075/#1080) and lives in the
**same** chokepoint: `FnLower::expr_coerced` → `expr_collection_context`. When the
destination is an `Optional[T]` and the source is a bare payload (not already an
`Optional[T]`, not a bare `None`), the value is lowered against the payload type
and wrapped via the existing `gen_opt_some` — the identical
`StructNew`/`FieldSet` construction an explicit `Optional.Some(...)` emits.

Because it is in the one chokepoint, the wrap applies to **every type-contextual
position that sema currently accepts a bare payload in**: constructor fields
(positional, keyword, and an omitted field's evaluated default), field assignment
(`obj.field = value`), index/key assignment (`xs[i] = value`, `m[k] = value`),
and `.append(value)`. An already-`Optional[T]` source (an Optional-typed binding,
`Optional.Some(...)`, a ternary of Optionals) lowers to the Optional type itself
and is returned unwrapped, so it is never double-wrapped; a bare `None` keeps its
own `lower_none_expr` path; an incompatible source passes through for `coerce` /
sema to reject.

We chose **auto-wrap over sema-reject**: sema already types the constructor
keyword form `Delta(day=1)` as valid, it matches Python's `x: Optional[int] = 1`
parity, and it mirrors `= None` already being accepted in the same position. The
alternative — making sema reject these — would break that accepted form and the
`RelativeDelta(day=1)` ergonomics it enables.

## Rationale

#1262 was a **memory-safety** bug in a *checked* program: a bare scalar keyword
for an `Optional[T]` struct field was stored unboxed (`iconst` + `fieldset`, no
`Some` allocation or discriminant tag), so a later `.unwrap()` dereferenced the
integer as an Optional object and SIGSEGV'd at `0x1`. #1075 fixed the bare-`None`
half of this exact coercion (one shared `lower_none_expr`); the non-`None`
payload half was never wrapped. Reusing `gen_opt_some` rather than emitting a
fresh allocation keeps the result on the already-GC-rooted `StructNew` path (a
ZIR `Inst` that defines a pointer must be in codegen `inst_dest()` or the result
is a NULL-reload crash under precise Immix), and introduces no new raising
runtime call.

## Consequences

- No new ZIR `Inst`/`Elem`/`Expr`/`Ty` variant, so no new `inst_dest` rooting
  entry and no new exhaustive-match arms: the wrap only emits the pre-existing
  `StructNew`/`FieldSet` that `gen_opt_some` already produces for
  `Optional.Some(...)`.
- The emitted ZIR for `Delta(day=1)` now shows a `Some` allocation + tag before
  the field store, identical to `Delta(day=Optional.Some(1))` — verified by the
  host ZIR-structure tests in `crates/ir/tests/optional_scalar_wrap.rs` and the
  native-value `optional_scalar_wrap` golden + `optional_scalar_wrap_module`
  golden project (the scalar-vs-pointer storage only crashes once the native
  collector runs).

## Scope boundary (precise)

This PR fixes the memory-safety crash in **exactly the positions sema already
accepts a bare payload**. The remaining positions still reject a bare payload
with a clean sema type error, and are **unchanged** here:

| Position | Bare payload today |
|---|---|
| Constructor field (positional / keyword / omitted default) | accepted → now wrapped |
| Field assignment `obj.f = v` | accepted → now wrapped |
| Index / key assignment `xs[i] = v`, `m[k] = v` | accepted → now wrapped |
| `.append(v)` | accepted → now wrapped |
| `return v` (fn `-> Optional[T]`) | rejected `ZAP4003` (unchanged) |
| `Optional[T]` parameter (call arg) | rejected `ZAP4021` (unchanged) |
| Typed binding `Optional[T] x = v` | rejected `ZAP4001` (unchanged) |
| List-literal element `[v, …]` | rejected `ZAP4001` (unchanged) |

Whether a bare `T` should *also* implicitly widen to `Optional[T]` in the
rejected positions (full Python parity) is an implicit-widening **language-design
decision** that the maintainer owns; it must not ride into a memory-safety fix.
It is deferred to **#1265**. The IR chokepoint already wraps those positions too,
so if #1265 relaxes sema to admit `T → Optional[T]` there, no further lowering
change is needed.

<!-- The wrap and the symmetric bare-`None` path both live in
     crates/ir/src/lower.rs (`expr_coerced` / `expr_collection_context`,
     `gen_opt_some` / `lower_none_expr`). Mirrors #1075 / #1080. -->
