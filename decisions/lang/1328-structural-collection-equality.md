---
title: dict / set / deque / oset == are structural; heap == is rejected
status: Locked
issue: 1328
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Structural equality for the core collections (and no heap equality)

**Status:** Locked

## Decision

`==` / `!=` on the core collections compare by **value**, not by pointer
identity. `is` / `is not` keep reference identity (same allocation), unchanged.
This is the dict counterpart to the list fix
([lang/1239-list-structural-equality](1239-list-structural-equality.md)) and is
widened across the collections in one change:

- **`dict[K, V]`** — equal when the two dicts have the same length and every key
  present in one is present in the other **with an `==`-equal value**.
  **Order-independent** (Python mapping semantics): the key insertion order does
  not matter. Values are compared by `==`, so they recurse — a `dict` whose value
  type supplies its own equality (`str`/`bytes` by contents, a struct's
  `__eq__`, a nested `list`/`dict`/… value) compares correctly. This is the
  reported bug (#1328) and the otel#3 aggregation shape: two measurements built
  from equal attribute sets must compare equal and aggregate into one stream.
- **`set[T]`** — membership-based, **order-independent** (Python set semantics):
  equal when they have the same length and every member of one is a member of
  the other.
- **`deque[T]`** — **order-sensitive**, length-sensitive, element-wise
  (Python `collections.deque` semantics): equal when they have the same length
  and equal elements in the same order.
- **`oset[T]`** — **order-sensitive** sequence equality: equal when they have the
  same length and equal elements in the same order, after the oset's own
  deduplication.
- **`heap[T]`** — **no value equality.** `==` / `!=` on a heap is a compile-time
  error (ZAP4012); `is` / `is not` identity remain available.

Keys of two compared dicts, and members of two compared sets/osets/deques, must
have the same element kind; a value type may differ in representation (e.g.
`dict[str, int64]` vs `dict[str, float64]`), since value equality applies the
numeric join, matching Python's `{"x": 1} == {"x": 1.0}`.

Ordering comparisons (`< <= > >=`) on these collections are **out of scope** and
unchanged. Python dicts are not orderable, Python set `<`/`<=` subset operators
are a separate feature (not a bug), and hashing of dict/set (they are unhashable
in Python) is not added. A latent pre-existing defect — `<` / `>` on
dict/set/heap also falls through to the generic pointer compare — is tracked
separately, not fixed here.

## Rationale

IR lowering's `binop` had `Eq`/`Ne` arms for tuples (D-0049) and lists (#1239),
but not for dict/set/oset/deque/heap. These are reference values but not
`Ty::Ptr`, so they fell through to the generic numeric/reference path, which
`ptrtoint`-compares the two container headers and emits one `ICmp`: a
pointer-identity test. Two separately allocated but equal containers — e.g. two
dicts with the same entries inserted in a different order — reported unequal.

The fix adds per-container `Eq`/`Ne` arms modelled on `list_cmp` (#1239):

- **Lower in-IR, reusing `binop(Eq, ..)` per value/element.** Every value/element
  type gets the correct equality for free — numeric joins, `str`/`bytes` by
  contents, struct `__eq__`, and nested collections that recurse — with no
  per-type special-casing. Full ZIR types are recovered via `elem_ty` (#625) so a
  nested value reads back as its real type, not a bare `str` pointer.
- **No runtime entry point and no new ZIR `Inst` variant.** The loops are built
  from existing `Call`s (`zap_dict_len`/`zap_dict_keys_*`/`zap_dict_contains_*`/
  `zap_dict_get_*`, `zap_set_len`/`zap_set_items`/`zap_set_contains_*`,
  `zap_{deque,oset}_len`/`zap_{deque,oset}_get_*`), `ICmp`, `Bin`, `Un`,
  `BConst`, and branch terminators, so there is no new codegen surface
  (`inst_dest`, `throwing_call`, re-exports, exhaustive matches all unaffected).
- **Length first, then short-circuit.** Distinct lengths are unequal without
  reading entries; the loop stops at the first mismatch. For dicts/sets, equal
  length plus "every left entry/member is matched in the right" is sufficient
  (pigeonhole).
- **Symmetric routing.** A collection on either side routes with it on the left;
  if the other operand is not the same collection kind, a clear error is raised
  rather than a silent pointer compare.

### Why `oset` is order-sensitive

An `oset` preserves insertion order and exists precisely to give deterministic,
observable ordering (dict/set iteration order is unspecified and seed-dependent,
#1064/#1066). Making its order part of equality keeps it meaningfully distinct
from `set` and matches the widely used Python `ordered-set` package, whose
`OrderedSet == OrderedSet` compares as a sequence. Membership-only equality is
what `set` already provides.

### Why `heap` has no value equality

A heap is a priority queue whose backing array satisfies the heap property, but
the array order is **not unique** for a given multiset — push order changes the
internal layout. So an element-wise array compare would report two logically
equal heaps as unequal (a footgun, per the footgun-triage rule: every
compiles-but-breaks pattern gets an explicit error/warning/doc decision), while a
multiset compare is a surprising amount of machinery for a type Python has no
equality for (heapq operates on plain lists). The honest choice is to reject
`==` / `!=` with a message pointing to a snapshot comparison
(`sorted(h.items())`) and keep `is` for identity. Silent pointer identity (the
prior behaviour) is exactly the footgun being removed.

Alternatives rejected:

- **Per-container `zap_*_eq` runtime helpers.** Would duplicate the value/element
  dispatch (numeric/str/struct/nested) lowering already performs and need
  function-pointer plumbing for struct keys/elements. More surface, no benefit.
- **`oset` membership equality.** Collapses the distinction from `set`; order is
  the point of `oset`.
- **`heap` element-wise or multiset equality.** Element-wise exposes impl-defined
  layout; multiset is heavy and unmatched by Python. Diagnostic is clearest.

## Consequences

- `dict`/`set`/`deque`/`oset` `==` now traverses entries in the worst case
  (equal containers); the same cost profile as `in` and tuple/list comparison,
  and the semantics authors expect. `is` remains O(1) identity.
- Nested containers compare recursively, so `dict[str, list[int64]]`,
  `dict[str, dict[str, int64]]`, etc. work with no extra code.
- `heap == heap` now fails to compile (ZAP4012) where it previously silently
  pointer-compared; callers compare `sorted(h.items())` or use `is`.
- Runtime values are exercised by the `collection_structural_eq` golden on the
  LLVM CI matrix; host-only ZIR tests
  (`crates/ir/tests/collection_eq_lower.rs`) assert the structural lowering, and
  sema tests (`crates/sema/tests/collection_equality.rs`) lock the heap
  diagnostic.

See also [lang/1239-list-structural-equality](1239-list-structural-equality.md)
(the precedent) and the tuple-comparison precedent (D-0049).
