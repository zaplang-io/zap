---
title: bool widens to a wider integer by zero-extension, not sign-extension
status: Locked
issue: 1344
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# `bool` widens by zero-extension

**Status:** Locked

## Decision

Widening a `bool` to a wider integer (the `i1` → `i64` the comparison/arithmetic
lowering performs) uses **zero-extension**. A `bool` is an unsigned `0`/`1`, so
`True` widens to `+1`. This makes `bool` ordering comparisons correct:
`False < True` is `True`, `True < False` is `False`, matching Python (where
`False < True`). `is` / `is not` and equality (`==` / `!=`) are unchanged.

## Rationale

`crates/ir/src/lower/coerce.rs` chose the widening cast by the source's sign:
zero-extend an unsigned source, sign-extend otherwise. `bool` is `Ty::I1` and
`Ty::I1.is_unsigned()` is `false`, so it took the sign-extend branch —
**sign-extending the `i1` value `1` (`True`) yields all-ones = `-1`**, not `+1`.
Confirmed at the ZIR level: `bool a < bool b` lowered to `Cast SExt` (`i1` → `i64`)
then `ICmp`, so `False < True` became `0 < -1` = `False` — every `bool` ordering
comparison was inverted (surfaced by the `list[bool]` cases of the list-ordering
work, #1334 / PR #1340, which were removed pending this fix).

Equality was unaffected: both operands sign-extend the same way, so `True` (`-1`)
and `False` (`0`) stayed distinct — which is why the structural-equality goldens
(#1239 / #1328 / #1336) passed with `bool` elements.

The fix adds `|| from == Ty::I1` to the zero-extend condition at the implicit
integer-widening site in `coerce` — the one site a `bool` actually reaches.

### Why only one site

`coerce` and the explicit-cast `match` have sibling widening and int→float arms
keyed on the same `is_unsigned()` test. Those are **not** reached by a `bool`
today: sema rejects an explicit `int64(True)` / `float64(True)` with ZAP4024
(`bool` is not a number), and `bool` is not numeric, so it never implicitly flows
into an integer-cast or int→float conversion. Only the implicit `i1` → `i64`
widening that the comparison lowering emits is reachable, so only that site is
changed (keeping the fix minimal and fully exercised). If `bool` → number casts
are ever allowed, those arms must adopt the same zero-extension rule.

Rejected alternative: make `Ty::I1.is_unsigned()` return `true` globally. That
would fix all sites at once, but `is_unsigned()` also drives sema's signed/unsigned
mixing rule (ZAP4015) and operator signedness selection, so it would wrongly
reject `True < 5` (a signed/unsigned mix) and change `bool` arithmetic op
selection — far more surface than this bug warrants.

## Consequences

- `bool` ordering (`<` `<=` `>` `>=`), as a scalar and as `list`/`deque` elements,
  is now correct and matches Python.
- The `list[bool]` ordering cases removed from `tests/golden/list_ordering.zap`
  in PR #1340 are restored in this change.
- Verified by the `bool_widening` golden (scalar ordering) and the restored
  `list[bool]` golden cases on the LLVM CI matrix; host-only ZIR tests
  (`crates/ir/tests/bool_widen_lower.rs`) assert the widening is `ZExt`, not
  `SExt`.
- Explicit `bool` → int / float casts remain a separate, sema-level gap (ZAP4024)
  and are out of scope here.

See also [lang/1334-list-ordering](1334-list-ordering.md) (which surfaced this)
and #552 (the signed/unsigned widening rules this refines).
