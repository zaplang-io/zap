---
title: Sema un-erases the Optional[T] payload and is payload-aware
status: Locked
issue: 1452
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Sema un-erases the Optional[T] payload and is payload-aware

**Status:** Locked

## Decision

Sema tracks `Optional[T]` as a payload-carrying type `Ty::Optional(T)` (like
`Ty::List`/`Ty::Dict`), not the opaque, payload-erased `Ty::Named("Optional")`.
`resolve_type` captures the annotation's payload; the value side un-erases too —
`Optional.Some(e)` infers `Ty::Optional(default_type(type_of(e)))`, `Optional.None`
/ a bare `Optional` / `dict.get(k)` carry their payload (`Unknown` where there is
none). `assignable`/`coercible` are payload-aware:

- `Optional[U]` is assignable to `Optional[T]` **iff `U == T`** (an `Unknown`
  payload on either side is a wildcard). This is a reject-on-mismatch rule, **not
  a widening** — it mirrors, at the sema layer, the monomorphized-payload equality
  that IR lowering's `check_optional_payload` already enforces (#1407).
- The #1265 bare-payload wrap (`T → Optional[T]`, the value is not itself an
  `Optional`) stays **permissive**, because IR lowering wraps and coerces a bare
  `str` / `float` / narrowing-numeric payload into `Some` and accepts it
  end-to-end. The one exception is the locked #1379 reject, mirrored here: a bare
  `bigint` into a fixed-width-numeric payload (`int*`/`float*`/`uint*`/`usize`) is
  not implicitly convertible and is rejected.

The consequence is that an `Optional`-payload type mismatch is now a `ZAP4001` at
the **type-checking layer**, surfaced through the sema-only `check` / MCP
`check_source` / LSP path, not only at `zap build` / `zap ir`.

## Scope boundary — no behaviour change end-to-end

This is a **diagnostic-surface** improvement only. Every program accepted or
rejected end-to-end before #1452 is accepted or rejected the same way after it: it
moves the existing lowering rejections (#1379/#1407) earlier and makes them
visible to sema-only consumers. In particular it adds **no new implicit
conversion** — notably no `Optional[int32] → Optional[int64]` payload widening
(that remains a separate, explicit decision). The sema payload comparison is
exact-after-defaulting (an untyped literal takes its default, e.g.
`Optional.Some(5)` → `Optional[int64]`), matching how mono monomorphizes the
payload, so sema and lowering reject/accept the identical set.

Constructor fields are still not fully sema-type-checked; this issue does not
undertake that (Phase 2 follow-up), but the payload-aware `Optional` type makes it
tractable.

## Defense in depth retained

The IR lowering checks `check_numeric_payload` (#1379) and `check_optional_payload`
(#1407) **stay** as defense in depth. Sema is no longer the only line of defense,
but lowering still guards the paths sema does not type-check (constructor fields)
and the monomorphized IR, so a path that reaches lowering without the sema check
is still caught.

## Rationale / placement

The clean representation is a new `Ty::Optional(Box<Ty>)` variant. `Ty` is
consumed by six crates — `sema`, `mono`, `doc`, `comptime`, `analysis`,
`sema_types` — so the variant forces an arm wherever the type is matched or
displayed. The highest risk is **`mono`**: its `Optional$T` monomorph naming must
stay byte-identical. It does, for two reasons: (1) `Ty::Optional(T).to_ast_type()`
produces `Optional[T]`, which mangles to exactly the old `Named("Optional")`
-plus-type-argument form; and (2) mono is driven by `ast::Type` and its own
inference, consulting the sema `TypeMap` only as an `Unknown`-fallback — and its
own arms already produce the correct `Optional$T` for every `Optional`
construction, so the richer `TypeMap` entries are not consulted there. The
behaviour-parity bar is enforced by the existing
`crates/ir/tests/bigint_optional_payload.rs` and
`crates/ir/tests/optional_payload_variance.rs` suites, the `crates/mono` tests,
and the full golden/conformance matrix, all unchanged.

Doc/reflect rendering gets *simpler*: `reflect_type_info` and the comptime
reflector read the `Optional` element straight off `Ty::Optional(e)` instead of
recovering it from the declared `Type`'s first argument, and a nested optional
element now reflects with its real payload instead of `Unknown`.

## Consequences

- A payload-mismatch program (`Optional[int64] y = Optional.Some(<bigint>)`,
  `Optional.Some(g)` for a `bigint g` / `int32 n` / `str`, in any type-checked
  position) is now a `ZAP4001` in sema / `check_source`, not only at `zap build`.
- Diagnostics and LSP hovers now render `Optional[int64]` rather than the bare
  `Optional`; a nested optional reflects its real element type.
- No new ZIR `Inst`, runtime call, or monomorph name; accepted programs are
  byte-identical.
- The "sema erases, verify downstream" caveat that each new `Optional` defect had
  to re-discover is removed. The broader #1265 follow-up noted in the #1379 and
  #1407 ADRs is resolved by this ADR.

<!-- Core: new `Ty::Optional(Box<Ty>)` in crates/sema_types/src/lib.rs; the
     resolve_type / check_variant_construction / dict.get / is_optional_ty /
     coercible / check_renderable / pattern-scrutinee / reflect_type_info updates
     in crates/sema/src/lib.rs; the comptime reflect_ast arms. Lowering's
     check_numeric_payload / check_optional_payload stay as defense in depth.
     Supersedes the deferral notes in decisions/lang/1379-bigint-optional-payload.md
     and decisions/lang/1407-optional-payload-variance.md. Phase 2 (sema
     type-checking of constructor fields) is the documented follow-up. -->
