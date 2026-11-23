---
title: set is ordered by subset/superset, deque lexicographically; dict/heap/oset are not orderable
status: Locked
issue: 1334
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Ordering comparisons on the core collections

**Status:** Locked

## Decision

`<` / `<=` / `>` / `>=` on the core collections now carry well-defined semantics
instead of falling through to a pointer-address compare. This is the ordering
sibling of the equality fix
([lang/1328-structural-collection-equality](1328-structural-collection-equality.md)),
which deliberately scoped ordering out.

- **`set[T]`** — Python's **subset/superset partial order**:
  - `a <= b` — `a` is a subset of `b`.
  - `a <  b` — `a` is a *proper* subset (`a ⊆ b` and `len(a) < len(b)`).
  - `a >= b` — `a` is a superset of `b`.
  - `a >  b` — `a` is a *proper* superset (`b ⊆ a` and `len(a) > len(b)`).

  This is a partial order: two sets can be incomparable (both `<` and `>` are
  `False`, e.g. `{1,2}` and `{2,3}`). Order-independent, like set equality.
- **`deque[T]`** — Python `collections.deque` **lexicographic** ordering: compare
  elements left to right; the first differing pair decides (by element `<`), and
  if one deque is a prefix of the other the shorter is smaller. Element ordering
  reuses scalar `<`/`>`, so numeric, `str`/`bytes`, and `char` elements order as
  expected; a non-orderable element type raises a clear error.
- **`dict[K,V]`, `heap[T]`, `oset[T]`** — **not orderable.** `< <= > >=` are a
  compile-time error (ZAP4012). `dict` is not orderable in Python; `heap`'s
  internal order is implementation-defined (consistent with its rejected `==`,
  #1328); `oset` has no direct Python analog and is ambiguous between set-like
  (subset) and sequence-like (lexicographic) ordering, so it is left
  unsupported rather than guessing. `is` / `is not` identity stay available on
  all three.

Equality (`==` / `!=`) semantics are unchanged from #1328. Ordering on `list`
is **not** addressed here (out of scope for this issue) and remains a separate
concern.

## Rationale

IR lowering's `binop` had no `Lt`/`Le`/`Gt`/`Ge` arm for these collection types.
They are reference values but not `Ty::Ptr`, so they fell through to the generic
numeric/integer path, which `coerce`s each collection pointer to `i64` and emits
an `ICmp` — a comparison of allocation addresses, producing a meaningless `bool`.
`arith_result` typed the `Lt | Le | Gt | Ge` arm as `bool` for any non-`complex`
operands, so sema did not catch it either.

The fix adds per-container arms, reusing the #1328 machinery:

- **`set_order_cmp`** — a `set_subset(x, y)` helper (`x ⊆ y`) built from
  `zap_set_items` + `zap_set_contains_*` (the same membership probe as `set ==`),
  combined with a length inequality for the strict operators. Not symmetric, so
  the operands are never swapped (the set must be the left operand).
- **`deque_order_cmp`** — a 3-way sign (`-1`/`0`/`1`) computed in a slot, like
  `zap_str_cmp` / `zap_bigint_cmp`: walk elements, the first differing pair (via
  `binop(Lt/Gt, ..)`) sets the sign, and equal prefixes fall back to a length
  compare. The operator maps to the sign against 0.
- **`dict`/`heap`/`oset`** — rejected in sema (`arith_result`, host-catchable)
  with a lowering backstop so the ir crate (exercised without sema) rejects them
  too.

No runtime entry point and no new ZIR `Inst` variant — the loops are built from
existing `Call`s, `ICmp`/`Bin`/`Cast`/`BConst` and branch terminators, so there
is no new codegen surface.

Alternatives rejected:

- **Diagnose set/deque too (no real ordering).** Python defines both orderings;
  rejecting them would contradict the Python-readability goal and the SPEC §5
  "collections define their own ordering" direction. The footgun (silent pointer
  compare) is removed either way, but the useful semantics are worth having.
- **Give `oset` an ordering.** Neither set-like nor sequence-like ordering is
  clearly correct for an ordered set, and it is niche; a diagnostic is honest and
  leaves the door open to a later decision.

## Consequences

- `set` subset/superset and `deque` lexicographic comparisons now work and match
  Python; element ordering recurses through scalar `<`/`>`.
- `dict < dict`, `heap < heap`, `oset < oset` (and the other three operators) now
  fail to compile (ZAP4012) where they previously silently pointer-compared.
- Runtime values are exercised by the `collection_ordering` golden on the LLVM CI
  matrix (boolean-only output, so order-independent); host-only ZIR tests
  (`crates/ir/tests/collection_order_lower.rs`) assert the structural lowering /
  rejection, and sema tests (`crates/sema/tests/collection_ordering.rs`) lock the
  accepted-vs-rejected matrix.
- `list` ordering is unchanged (still falls through to the generic path) and is a
  candidate for a follow-up, tracked separately.

See also [lang/1328-structural-collection-equality](1328-structural-collection-equality.md)
(the equality precedent and the membership machinery reused here).
