---
title: Sema type-checks struct constructor fields (a deliberate tightening)
status: Locked
issue: 1464
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Sema type-checks struct constructor fields (a deliberate tightening)

**Status:** Locked

## Decision

A struct field constructor `StructName(args)` type-checks each positional/named
argument that maps to a declared field: the argument must be `coercible` to that
field's declared type, exactly as the equivalent direct binding (`int64 x =
<value>`) already is. A mismatch is a hard compile error (`ZAP4064`, the code
enum-variant construction already uses), surfaced through the sema-only
`check` / MCP `check_source` / LSP path as well as `zap build`.

This is a **deliberate language tightening** — a footgun-triage decision (resolved
in #1464): the newly-rejected programs **compiled before**, so this changes
end-to-end accept/reject behaviour. It was chosen over a warning, and over the
narrower "reject only what lowering already rejects" option, because struct
construction should obey the same assignability rules as every other position in
the language.

Phase 2 of #1452: Phase 1 (#1457) un-erased the `Optional` payload in sema, which
makes the `Optional`-payload field case tractable here.

## What was wrong

Sema did not type-check struct constructor fields at all (the constructor arm just
inferred the arguments for side effects and returned the struct type). So every
field-type mismatch compiled:

- `struct S: int64 x` then `S("hi")`, `S(1.5)`, or a narrowing `S(int64Value)` into
  an `int32` field — a plain type error everywhere else (`int64 x = "hi"` is
  `ZAP4001`) — compiled and **silently mis-compiled**, storing the wrong-typed
  value (or a pointer) into the scalar slot.
- The `bigint`-into-fixed-width (#1379) and `Optional[U]`-into-`Optional[T]`
  (#1407) field cases are the memory-safety defects in this family; they were
  caught only in IR lowering (`check_numeric_payload` / `check_optional_payload`),
  which both ADRs noted was the only line of defense for constructor fields.

## Scope

- Only the **field type** is checked. **Arity and unknown-field-name are
  unchanged** — still reported by IR lowering (`ZAP6001`) — so this adds exactly
  the type check, nothing else.
- Implicit widening is preserved (it routes through `coercible`/`assignable`): an
  `int32` into an `int64` field, an untyped literal into any matching numeric
  field, a concrete conformer into an interface field (D-0016), bare `None` into a
  nullable field, and the #1265 bare-payload wrap into an `Optional[T]` field all
  still compile.
- A field typed by a **generic type parameter** resolves to `Ty::Unknown`
  (unconstrained), which `coercible` admits, so generic-struct fields stay
  permissive — concrete checking remains per-instantiation in mono, as before.
- Enum-variant construction was already type-checked (`check_variant_construction`,
  `ZAP4064`); this brings struct construction to parity with it and with direct
  bindings.

## Placement

In sema, at the struct-constructor arm of `check_call`
(`crates/sema/src/lib.rs`), via one `check_struct_field_types` helper that mirrors
`check_variant_construction`. The IR lowering checks (`check_numeric_payload`,
`check_optional_payload`) **stay as defense in depth** for any path that reaches
lowering without the sema check (and monomorphized IR); they are no longer the
only line of defense for struct fields.

## Consequences

- A struct-field type mismatch is now an `error[ZAP4064]` at type-check time,
  including the #1379 `bigint`-into-fixed-width and #1407 `Optional`-payload cases,
  surfaced through `check_source` / LSP.
- **Breaking:** a program that constructed a struct with a mismatched field type
  (and silently mis-compiled) now fails to compile. This is the intended footgun
  fix; the value must be built at the field's type (e.g. `S(int64("…"))`-style
  explicit conversions, or a correctly-typed value).
- No new ZIR `Inst`, runtime call, or monomorph name. Correctly-typed constructions
  — including every implicit widening — are unaffected.
- Resolves the "constructor fields are still not sema-type-checked" follow-up noted
  in `decisions/lang/1452-sema-optional-payload.md`, #1379, and #1407.

<!-- Core: `check_struct_field_types` in crates/sema/src/lib.rs, called from the
     `user_types` struct-constructor arm of check_call. Mirrors
     check_variant_construction (ZAP4064). Lowering's check_numeric_payload /
     check_optional_payload stay as defense in depth. Phase 2 of #1452
     (decisions/lang/1452-sema-optional-payload.md); refs #1379, #1407, #1265. -->
