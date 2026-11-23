---
title: list ordering (< <= > >=) is lexicographic, not pointer identity
status: Locked
issue: 1334
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# List ordering is lexicographic

**Status:** Locked

## Decision

`<` / `<=` / `>` / `>=` on two `list` values compare them **lexicographically** —
element by element; the first differing pair decides (by element `<`), and if one
list is a prefix of the other the shorter is smaller — matching Python's sequence
ordering. `is` / `is not` keep reference identity, unchanged. Element ordering
reuses scalar `<`/`>`, so numeric joins and `str`/`bytes` by contents order as
expected; an element type that does not support scalar `<` (e.g. `bool`, whose
scalar ordering is a separate question) is not asserted here.

This is the `list` companion to the collection-ordering decision
([lang/1334-collection-ordering](1334-collection-ordering.md)) — `set`
subset/superset, `deque` lexicographic, `dict`/`heap`/`oset` rejected — and to
the structural-equality work ([lang/1328](1328-structural-collection-equality.md),
[lang/1239](1239-list-structural-equality.md)). List *equality* was already
structural (#1239); only *ordering* fell through.

## Rationale

IR lowering's `binop` had no `Lt`/`Le`/`Gt`/`Ge` arm for `list`, so two lists fell
through to the generic numeric/integer path, which `coerce`s each list pointer to
`i64` and emits an `ICmp` — a comparison of allocation addresses, producing a
meaningless `bool`. `arith_result` types `Lt | Le | Gt | Ge` as `bool` for any
non-`complex` operands, which is *correct* for lists (they are orderable in
Python), so no sema change is needed — the fix is purely a lowering arm.

`list_order_cmp` mirrors the `deque` lexicographic lowering: a 3-way sign
(`-1`/`0`/`1`) computed in a slot, like `zap_str_cmp` / `zap_bigint_cmp`. Walk
elements; the first pair where `binop(Lt, ..)` or `binop(Gt, ..)` is true sets the
sign; equal prefixes fall back to a length compare; the operator maps to the sign
against 0. Element reads recover the full ZIR type via `elem_ty` (#625), so a
nested element reads back as its real type. No runtime entry point and no new ZIR
`Inst` variant — the loop is built from existing `Call`s (`zap_list_len` /
`zap_list_get_*`), `ICmp`/`Bin`/`Cast`/`BConst`, and branches.

Alternatives rejected:

- **Diagnose list ordering.** Python lists are orderable; rejecting `<` would
  contradict Python readability and the SPEC §5 direction, and would be a
  regression against user expectation. The footgun (silent pointer compare) is
  removed by giving the real semantics.
- **A `zap_list_cmp_*` runtime helper.** Would duplicate the element dispatch
  (numeric/str/struct/nested) lowering already performs. More surface, no benefit.

## Consequences

- `list < list` (and the other three operators) now compare lexicographically and
  match Python; element ordering recurses through scalar `<`/`>`.
- `is` / `is not` remain O(1) identity.
- Runtime values are exercised by the `list_ordering` golden on the LLVM CI
  matrix (deterministic output); host-only ZIR tests
  (`crates/ir/tests/list_order_lower.rs`) assert the structural lowering.
- Ordering of the other collections is covered by the sibling decision; this
  change touches only `list`.

See also [lang/1334-collection-ordering](1334-collection-ordering.md) and
[lang/1239-list-structural-equality](1239-list-structural-equality.md).
