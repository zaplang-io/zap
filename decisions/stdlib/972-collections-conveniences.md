---
title: stdlib collections — Counter / defaultdict / OrderedDict as prelude generics
status: Locked
issue: 972
pr:
date: 2026-11-19
domain: stdlib
supersedes:
superseded-by:
---

# stdlib `collections` — `Counter` / `defaultdict` / `OrderedDict`

**Status:** Locked

## Decision

Add a `collections` standard-library module exposing the three Python
`collections` conveniences, each a **generic prelude struct over the built-in
capability-typed `dict`/`list`** (#812) — **pure prelude Zap, no runtime C**,
the same shape as [`decimal`](981-decimal-exact-base10.md) /
[`fractions`](982-fractions-exact-rational.md) (prelude struct + module
constructor) and the lock-free `concurrent` collections (D-0078).

The Python-faithful surface is reached through the **seam-generic construction
sugar** (D-0202): `collections.Counter[K](...)` /
`collections.defaultdict[K, V](...)` / `collections.OrderedDict[K, V](...)` are
front-end sugar (`zapc_syntax::seam`) rewritten to the bare prelude free
constructors `new_counter` / `new_defaultdict` / `new_ordered_dict` that the
monomorphizer keys on — exactly as `concurrent.Queue[T]()` desugars to
`new_queue[T]()`. The behaviour lives on the structs as ordinary generic-struct
methods, including the index / membership / size dunders (D-0047), so the surface
is Pythonic: `c[k]`, `c[k] = n`, `k in c`, `len(c)`, `c1 + c2`, `d[k].append(x)`.

- **`Counter[K]`** — a multiset / tally over `dict[K, int64]`. Missing keys read
  `0` (`c[k]` / `get(k)`). Surface: `add(key, n=1)`, `subtract(key, n=1)`
  (non-positive counts kept, as in Python's `Counter.subtract`), `update(items)`,
  `total()`, `keys()`, `elements()`, `most_common(n=-1)` (all when `n < 0`), and
  the operators `+` / `-` (which **drop** non-positive results, as in Python).
  Construct from an iterable (`collections.Counter(items)`) or empty
  (`collections.Counter()` — the element type comes from the annotation).
- **`defaultdict[K, V]`** — a dict that autovivifies a missing key from a
  zero-argument `factory` (`fn() -> V`) on `c[k]` access, so the grouping idiom
  `d[key].append(x)` works with no prior membership check. `k in d` and
  `d.get(k, fallback)` are **read-only** and never create the key (Python).
- **`OrderedDict[K, V]`** — an **insertion-ordered** map: `dict[K, V]` for O(1)
  lookup plus a `list[K]` recording insertion order (the compose-from-existing
  trick that shipped `oset`). `keys()` / `values()` / `items()` iterate in
  insertion order; re-assigning an existing key keeps its position. Plus
  `move_to_end(key)` and `delete(key)`.

The backing prelude structs are deliberately renamed — `CounterMap[K]`,
`DefaultDict[K, V]`, `OrderedMap[K, V]` — and the `SEAM_GENERIC_CTORS` table
gained a fourth "bare prelude type" column so the surface name can differ from
the backing struct. A user never spells the backing name.

### The four settled design calls

1. **`namedtuple` — closed as redundant (won't-do).** Zap's `value struct` is a
   strict superset of Python's `namedtuple`: named, immutable, stack-resident,
   with `@derive(Eq, Ord, Repr)` and structural hashing (so it is a dict/set
   key). The namedtuple equivalent is simply
   `value struct Point: int64 x; int64 y`, with real field names and derives and
   **less** ceremony than `namedtuple('Point', 'x y')`. The anonymous positional
   case is already covered by tuples (`(a, b)` with `t[0]` access, D-0049). No
   helper is added.

2. **`OrderedDict` — a real gap, implemented (NOT redundant).** The issue guessed
   it "unnecessary if `dict` preserves insertion order", but that premise is
   **outdated**: D-0235 made built-in `dict` iteration order **unspecified and
   per-run seeded** (wyhash, #1064). An insertion-ordered map is therefore a
   genuine capability gap, so `OrderedDict[K, V]` ships here (composed, as above).

3. **Delivery — seam module + prelude generics.** Generic types cannot be built
   by a non-generic module-facet constructor (the monomorphizer keys on bare
   names), so each is a prelude generic struct + bare `new_*` constructor with
   the `collections.Type[...]` seam sugar — the idiomatic shape under #812, and
   the same path `concurrent.Queue` already uses. The `collections` module is
   registered only so `import collections` is recognized; it has **no**
   module-facet methods and **no** `seam_sigs.rs` rows (see Consequences).

4. **Name clash with `concurrent.Counter` — resolved by a hidden backing name.**
   The prelude already ships a `struct Counter` (the lock-free counter over
   `Atomic[int64]`, D-0078). The Python name `Counter` is kept for the
   collections multiset via the seam sugar, backed by the distinct prelude struct
   `CounterMap`, so `collections.Counter` and `concurrent.Counter` are different
   types that coexist with no collision.

## Rationale

- **Why prelude generics, not a seam module facet or runtime C.** `decimal` /
  `fractions` proved the "generic/opaque prelude struct over an existing
  primitive, built by a module constructor" shape with zero runtime C; the
  built-in `dict`/`list` already provide the GC-traced element storage, `.get`,
  `.items`/`.keys`/`.values`, iteration, `del`, and the sort/tuple machinery
  `most_common` needs. Composition needs no `zaprt.c`/GC descriptor work. A
  generic type **must** take the seam-generic path because a module-facet
  constructor returns a single concrete `Ty` and cannot drive per-instance
  monomorphization — this is exactly why `concurrent.Queue` is built by the bare
  `new_queue` (D-0078/D-0202), and `collections` follows it.
- **Why the hidden backing name instead of renaming the surface.** AGENTS.md
  requires the library to match the popular Python shape as closely as possible;
  `collections.Counter` is the Python spelling. `concurrent.Counter` is already
  shipped (Experimental, D-0078) and cannot be renamed, and two prelude structs
  cannot share the base name `Counter` (the monomorphizer / id table key on it).
  Extending the seam table with a bare-type column is a one-line-per-row,
  fully-reviewable change that keeps the Python surface and the no-collision
  invariant.
- **Why `most_common` order among equal counts is unspecified.** The ordering is
  by count **descending** and is fully deterministic for distinct counts. Among
  **equal** counts the order follows the backing dict's iteration order, which is
  unspecified and seeded (D-0235), so it is documented as unspecified — a
  deliberate divergence from CPython's insertion-order tie-break. `most_common`
  is sorted with a manual insertion sort by count so that `K` needs **no `Ord`
  capability** (dict keys are only hashed / equality-compared, never ordered).
- **Rejected: a generic `sort(key=…)`.** A generic free function cannot be passed
  as a first-class value (closures are monomorphic), so the count sort is written
  by hand rather than via `sort(key=…)`.

## Consequences

- **No `seam_sigs.rs` rows.** The constructors are seam-generic (rewritten to
  bare prelude free functions **before** module dispatch) and `most_common` /
  `add` / `subtract` are prelude-struct methods, so keyword binding (`items=`,
  `n=`, `key=`, `factory=`) comes from the ordinary prelude function/method
  machinery (I#298/I#481), not the seam registry — verified by the
  `collections_*` cases in `crates/sema/tests/seam_kwargs.rs`. `seam_sigs.rs` is
  only for seam **module-facet** functions; nothing to add there.
- **Prelude injection.** `collections`, the three surface names, the three
  backing struct names, and the three constructors are added to the
  `references_prelude` needle set so a program that names the module (or any of
  the names) gets the prelude items injected.
- **Divergences from Python** (also recorded in `docs/PYTHON_DIFFERENCES.md`):
  `most_common` tie order among equal counts is unspecified (not insertion
  order); `Counter`/`defaultdict` key iteration order is unspecified (D-0235) —
  use `OrderedDict`, `most_common`, or `sorted` for determinism; `namedtuple` is
  not provided (use `value struct`); no `**kwargs` / mapping Counter constructor
  and no `Counter`-of-mapping (construct from an iterable, then `update`/`add`);
  `defaultdict`'s factory is a `fn() -> V` (no implicit `list`/`int` default
  class); a missing `OrderedDict[k]` read yields the dict's zero word pending a
  language-wide `KeyError` (#22). The Pythonic `d[k]` autovivification,
  `k in d` (non-creating), `len`, and `c1 ± c2` all match CPython.
- **Two pre-existing core defects surfaced, reported, not worked around** (per
  the workspace golden rule): (a) the `__getitem__`/`__setitem__` index dunder
  (D-0047) does not type-check the index argument against the declared parameter
  type; (b) a `fn(...) -> T`-typed parameter does not type-check its argument
  (a non-function is accepted). Both reproduce with a plain generic struct /
  plain function — independent of `collections`. The collections types are
  correctly typed; the gaps only mean *misuse* (a wrong key type, a non-function
  factory) is not rejected at compile time. Filed in `zaplang-io/zap`; no
  work-around is taken here.
- **Follow-ups (not in scope):** `OrderedDict.popitem(last=)` and a reverse
  iteration order; a `Counter` that tracks insertion order for a Python-exact
  `most_common` tie-break; the two core-defect fixes above (owned by the
  maintainer).

<!-- Related: concurrent generics D-0078 (../ not applicable — same tree); the
dict-order decision D-0235; the seam-generic sugar I#996/D-0202. -->
