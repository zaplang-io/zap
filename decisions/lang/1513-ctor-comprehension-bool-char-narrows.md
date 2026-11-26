---
title: The general iteration protocol yields logical (narrow) bool/char elements
status: Accepted
issue: 1513
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# The general iteration protocol yields logical (narrow) bool/char elements

**Status:** Accepted

## Decision

The general D-0048 iteration protocol (`iterate` / `iterate_value`) — the path
that backs comprehensions, the `list(…)` / `set(…)` / `tuple(…)` constructors,
the `oset[T]`/`set[T]` constructors, and `*xs` splats — now yields each
list-backed element at its **logical read-back type**, not its physical storage
word. A `bool` element yields `Ty::I1` and a `char` element yields `Ty::Char`;
every other element kind is unchanged (the narrowing is a no-op for it). This
extends the `bool`/`char` read-back narrowing that [the statement-`for` and
indexing read sites](1241-bool-char-element-read-back-narrows.md) already apply
to the one remaining user-facing read path that ADR left as a follow-up.

The narrowing is applied by a dedicated wrapper, `iterate_list_logical`, which
`iterate_value` routes its list-backed arms through (`list`, `tuple[T]`, `set`,
`heap`, `deque`, `oset`, and a `dict`'s keys). It calls `narrow_read_elem` on
each element the shared `iterate_list` reads at the physical `i64` word, before
handing it to the caller's body. The direct callers of `iterate_list` that
intentionally consume the raw storage word — the container repr renderer
(`repr.rs`), `list.extend`, dict copy/merge, and `sum` — are untouched and keep
reading the physical word.

As a result, `list([True, False])`, `tuple([True, False])`, `set([True])`,
`[b for b in bools]`, and `[*bools]` preserve the `bool`/`char` identity of their
elements and render `True`/`False` / `'c'` (CPython parity), instead of the
element's physical `i64` word (`1`/`0` / the decimal codepoint). A direct list
literal (`[True, False]`) always kept the identity, because it infers each
element's type before deriving the element kind; the constructors re-derived the
kind from the yielded physical word and lost it.

## Rationale

`bool` (`Elem::Bool`) and `char` (`Elem::Char`) are the only element kinds whose
`Elem::ty()` is not their own type — both report the shared `i64` storage word,
so a value read back through `iterate_list` and re-classified with
`list_elem_kind` collapsed to `Elem::I64`, dropping the identity. Narrowing at
the protocol chokepoint recovers it exactly where a program binds the value.

[ADR 1241](1241-bool-char-element-read-back-narrows.md) narrowed the two
statement-level read sites (`lower_for_list`, `lower_index_get`) but deliberately
did **not** narrow inside `iterate_list`, because its internal consumers (the
renderer, `extend`, dict copy/merge) push the raw word straight into `i64`-word
runtime entry points without re-coercing — narrowing there would emit an
`i1`/`char` where an `i64` is required and fail LLVM verification. It recorded
the comprehension/constructor case as a follow-up "requiring teaching the
`iterate_list` internal consumers to re-widen."

This decision resolves that follow-up a cleaner way: rather than narrow inside
`iterate_list` (and re-widen in every internal consumer), it adds a separate
`iterate_list_logical` wrapper on top of `iterate_list` and routes **only** the
general protocol through it. The internal consumers keep calling `iterate_list`
directly and are provably unaffected. This keeps one narrowing chokepoint
(`narrow_read_elem`, shared with the statement sites) and leaves the physical
storage ABI (`zap_list_get_i64`, GC tracing) untouched — only the ZIR type the
value flows with changes, re-widened to the `i64` word by the constructors'
existing `coerce(v, t, ek.ty())` before the `zap_*_push_i64` store.

Alternatives rejected:

- **Narrow inside `iterate_list` and re-widen in each internal consumer.** The
  option 1241 feared: it spreads re-widening across the renderer, `extend`, dict
  copy/merge, and `sum`, and is easy to get wrong (a missed site mis-renders or
  fails verification). The wrapper confines the change to the one protocol that
  should observe logical elements.
- **Re-derive the result element kind from the source collection's `Elem`
  instead of per element.** Workable for a first-class collection source, but the
  general protocol abstracts over sources with no first-class value (`range(…)`,
  a generator), so the builders cannot uniformly read a source `Elem`; the
  chokepoint narrowing covers every source the protocol accepts.
- **Leave it unfixed (status quo).** Keeps `list`/`set`/`tuple`/comprehension
  over `bool`/`char` rendering integers, breaking CPython parity and the
  element-identity contract the literal path already honors.

## Consequences

- Comprehensions, the `list`/`set`/`tuple`/`oset` constructors, and splats over a
  `list[bool]` / `list[char]` (or a set/heap/deque/oset/dict-keys of them) now
  produce a `bool`/`char`-element result and render it Python-exactly. The
  `oset[T]`/`set[T]` typed constructors, which compare the yielded element kind
  against the declared `T`, now accept a `bool`/`char` source that previously
  tripped a spurious "same representation" error.
- The physical storage and the wrong-kind element guards are unchanged: a value
  read back (now narrow) still round-trips byte-for-byte into a same-kind
  collection, and a bare `int64` pushed into a `list[bool]` is still rejected.
- The direct `iterate_list` consumers (container renderer, `extend`, dict
  copy/merge, `sum`) are untouched; they keep consuming the physical word.
- Statement `for` and indexing read-back are unchanged — they already narrowed
  (1241); the protocol now simply matches them.

See also [1241](1241-bool-char-element-read-back-narrows.md) (the statement/index
read-back narrowing this extends), the `bool`/`char` element Display kinds (#274
/ #709), and the companion container-repr recursion fix (#1516), which ships in
the same change.
