---
title: List == / != is a structural element-wise compare, not pointer identity
status: Locked
issue: 1239
pr:
date: 2026-11-21
domain: lang
supersedes:
superseded-by:
---

# List `==` / `!=` is a structural element-wise compare, not pointer identity

**Status:** Locked

## Decision

`a == b` and `a != b` on two `list` values compare them **structurally** —
element-wise, order-sensitive, and length-sensitive — matching Python's value
semantics for sequences. Two lists are equal when they have the same length and
every pair of elements at the same index is `==`. `is` / `is not` keep reference
identity (same allocation), unchanged. This holds for every element type
uniformly (`int64`, `float64`, `str`, `bool`, user structs via `__eq__`, and
nested `list`/`dict`/… elements, which recurse into the same element `==`); the
element type observed in the bug report (`list[float64]`, via otel#9) was not
special — the defect applied to every list element type.

Element equality reuses scalar `==` exactly: numeric joins, `str`/`bytes` by
contents, a struct's `__eq__`. Ordering comparisons (`< <= > >=`) on lists are
out of scope for this change and unchanged.

## Rationale

IR lowering's `binop` had no `list` arm for `Eq`/`Ne`, so two lists fell through
to the generic numeric/reference comparison path, which `ptrtoint`-compares the
operands and emits a single `ICmp`. For a list that is a **pointer to a heap
header**, this is a pointer-identity test: two separately allocated lists with
equal contents (a function result versus a literal) reported unequal, even
though per-element `==` and `len` matched and `is` correctly reported distinct
identity. That is the otel#9 failure.

The fix adds a `list_cmp` arm, modelled on the existing `tuple_cmp` (D-0049) and
the list-membership (`in`) scan (#399):

- **Lower in-IR, element-wise, reusing `binop(Eq, ..)` per element.** This gives
  every element type the correct equality for free — numeric, `str` by contents,
  struct `__eq__`, and nested collections that recurse into `list_cmp` — with no
  per-element-type special-casing. The element's full ZIR type is recovered via
  `elem_ty` (#625) so a nested-collection element reads back as its real type,
  not a bare `str` pointer.
- **No runtime entry point and no new ZIR `Inst` variant.** The loop is built
  from existing `Call` (`zap_list_len` / `zap_list_get_*`), `ICmp`, `Bin`, `Un`,
  `BConst`, and branch terminators, so there is no new codegen surface to teach
  (`inst_dest`, `throwing_call`, re-exports, exhaustive matches all unaffected).
  A struct runtime `zap_list_eq_*` would instead have to thread `__eq__` /
  `__hash__` function pointers for struct elements and re-implement the numeric
  and nested dispatch the lowering already owns.
- **Length first, then short-circuit.** Distinct lengths are unequal without
  reading any element; the element loop stops at the first mismatch. Indices are
  always in range, so element reads use a plain `Call` (no catchable
  out-of-range raise), matching the `in` scan.
- **Symmetric routing.** A list on either side routes with the list operand on
  the left; if the other operand is not itself a list, `list_cmp` raises a clear
  "a list compares for equality only with another list" error (the tuple path's
  convention) rather than silently falling back to a pointer compare.

Alternatives rejected:

- **A `zap_list_eq_<suffix>` runtime helper.** Would duplicate element dispatch
  (numeric/str/struct/nested) that lowering already performs, and needs
  function-pointer plumbing for struct elements. More surface, no benefit.
- **Leave it as identity and diagnose the operation.** Contradicts SPEC §5
  (collections define their own equality) and Python parity; the CPython
  comparison audit already lists structural collection equality as the intended
  behavior.

## Consequences

- `list == list` now evaluates every element in the worst case (equal lists);
  this is the same cost profile as `x in xs` and as tuple comparison, and is the
  semantics authors expect. `is` remains O(1) identity for when that is wanted.
- Nested lists compare recursively, so `list[list[int64]]` equality works with
  no extra code.
- Ordering (`< <= > >=`) on lists is unchanged by this decision and remains a
  separate concern; only `==` / `!=` are made structural here.
- The runtime values are exercised by the `list_structural_eq` golden on the
  LLVM CI matrix; host-only ZIR tests assert the structural lowering (reads
  lengths and elements rather than comparing pointers).

See also [lang/1180-reflected-operators](1180-reflected-operators.md) for the
element `__eq__` dispatch and the tuple-comparison precedent (D-0049).
