---
title: A bool/char collection element reads back as its narrow scalar type
status: Locked
issue: 1241
pr:
date: 2026-11-21
domain: lang
supersedes:
superseded-by:
---

# A bool/char collection element reads back as its narrow scalar type

**Status:** Locked

## Decision

A value *read back* from a `bool` (`Elem::Bool`) or `char` (`Elem::Char`)
collection element into a **user program binding** — a statement `for` loop
variable (`for x in xs`, including the `set`/`dict`/`heap`/`deque`/`oset` loops,
which route through the same list-loop lowering) or an index expression
(`xs[i]`) — now carries its **narrow logical scalar type** (`bool` → `Ty::I1`,
`char` → `Ty::Char`), not the physical `i64` storage word it is read through.
The element is still physically read via the `i64`-word runtime entry point
(`zap_list_get_i64`), honoring the storage ABI; lowering then truncates that
word to `i1`/`char` at the read site so the value a program observes has its
true kind. Every other element kind is unchanged: it already reads back at its
own type, so the narrowing is a no-op for it.

The narrowing is applied at the two *user-facing* read-back sites
(`lower_for_list`, `lower_index_get`), **not** inside the shared internal
iterator (`iterate_list`). Several internal callers of `iterate_list` — the
container renderer (`repr.rs`), `list.extend`, and dict copy/merge — pass the
raw element word straight into an `i64`-word runtime entry point
(`zap_bool_to_str`, `zap_list_push_i64`, `zap_dict_get_*`/`set_*`) without
re-coercing, so they rely on `iterate_list` yielding the physical word.
Narrowing there would miscompile them (an `i1`/`char` passed where `i64` is
expected). Consequently a bool/char element read back by a **comprehension or a
`set(...)`/`list(...)` constructor** (which bind through `iterate_list`) does
**not** yet narrow; that is a documented follow-up, not part of this fix.

This makes a `bool`/`char` read-back round-trip through the element-kind check,
so `for value in a_list_of_bool: copied.append(value)` compiles exactly like the
`int64` / `float64` / `str` forms already did.

## Rationale

`bool` and `char` are the only element kinds whose `Elem::ty()` does **not**
map back to their own `Elem`: both report `Ty::I64` (the shared integer-word
storage, kept so the `zap_*_get_i64` ABI and GC tracing match an integer
element), so `list_elem_kind` mapped a read-back `bool`/`char` to `Elem::I64`.
A `list[bool]` loop variable was therefore typed `i64`, and
`list[bool].append(value)` saw `Elem::I64 != Elem::Bool` and rejected it with
`ZAP6001 "append() value has the wrong element type"` — even though the value
round-trips byte-for-byte. Every other kind round-trips (`int64`→`Elem::I64`,
`str`→`Elem::Ptr`, `float64`→`Elem::F64`, `enum`→`Elem::Enum`), which is why the
bug was bool/char-specific.

Narrowing the read-back is the consistent fix, not a special case:

- **It mirrors string iteration.** `for c in s` already yields a `char`
  (`Ty::Char`), because codepoints are read through a char-typed entry point.
  Collection iteration of a `char`/`bool` element should observe the same scalar
  kind; the asymmetry was the defect.
- **It keeps the wrong-kind error.** A read-back `bool` is now `Ty::I1` →
  `Elem::Bool`, so `append(value)` passes, while a bare `int64` literal
  (`append(5)`, `Ty::I64` → `Elem::I64`), a `str`, or an `f64` is still rejected
  with `ZAP6001`. The narrowing admits exactly the values that round-trip, not
  "any integer word."
- **The physical representation is untouched.** The read still uses the
  `i64`-word entry point; a literal `bool` was already stored by widening `i1`
  back to the `i64` word (`SExt`), and the re-appended read-back re-widens the
  same way, so the stored words are byte-identical to the literal path. Only the
  *ZIR type* the value flows with changes.
- **It is applied at the read sites, not in `Elem::ty()`.** `Elem::ty()` is the
  physical `i64` word that the runtime call returns; retyping it would mismatch
  the C ABI of `zap_list_get_i64`. Lowering reads the word at `elem_ty` and
  truncates to the logical `elem_read_ty` right after the call, so codegen never
  sees an `i1`-typed call result.
- **It narrows per user-facing read site, not in the shared iterator.** Making
  `iterate_list` narrow would be the smallest diff, but its internal callers
  (renderer, `extend`, dict copy/merge) consume the physical word directly, so
  the narrowing is scoped to the sites that actually bind a program value.

Alternatives rejected:

- **Relax the `append` check to accept the whole integer-word family** (as the
  indexed-store path does since #727). This would wrongly accept `append(5)`
  into a `list[bool]`, losing the element-type guard the issue requires us to
  keep; and it would not fix a comprehension / constructor, which type-check the
  pushed value the same way.
- **Retype `Elem::ty()` to `i1`/`char`.** It is the physical read-back word;
  changing it breaks the `i64`-word runtime ABI and every value-position read.
- **Narrow inside `iterate_list`.** Its internal consumers (container renderer,
  `list.extend`, dict copy/merge) push the raw element word into `i64`-word
  runtime entry points without re-coercing, so narrowing there emits an
  `i1`/`char` where an `i64` is required and fails LLVM module verification.

## Consequences

- One narrowing chokepoint (`narrow_read_elem`) is applied at the two
  user-facing read-back sites — statement `for` (`lower_for_list`) and `xs[i]`
  (`lower_index_get`). The `set` / `dict` / `heap` / `deque` / `oset` statement
  `for` loops funnel through `lower_for_list`, so they are covered too.
- A comprehension or `set(...)`/`list(...)` constructor over a `list[bool]` /
  `list[char]` binds through `iterate_list` and so still reads the element back
  as the `i64` word; `[v for v in bools]` into a `list[bool]` therefore remains
  rejected (unchanged from before this fix). Making that work consistently
  requires teaching the `iterate_list` internal consumers to re-widen, and is
  tracked as a follow-up. **Resolved by
  [1513](1513-ctor-comprehension-bool-char-narrows.md)**, which routes only the
  general iteration protocol through a separate logical wrapper
  (`iterate_list_logical`) rather than re-widening the internal consumers.
- `print(xs[i])` / `print(value)` for a `bool`/`char` element now renders
  `True`/`False` / `'c'` through the scalar Display path, instead of the element
  word's `-1`/codepoint integer (the same `str()`-matching rendering a container
  already gave, cf. `print_scalar_values.zap`). Container rendering was already
  correct and is unchanged.
- Indexed **stores** keep their existing integer-word leniency
  (`is_int_word_elem`, #727): `xs[i] = <int-word value>` is unaffected, and a
  read-back value (now narrow) still stores back into a same-kind collection.
- The `char` element gains the same round-trip as `bool`; the two kinds are the
  only int-word Display kinds (`Elem::Enum` keeps its own identity and is not in
  this family).

See also the integer-word store leniency in #727
(`tests/golden/list_index_store_int_word.zap`) and the `bool`/`char` element
Display kinds (#274 / #709, `tests/golden/repr_bool_elements.zap`,
`tests/golden/char_collections.zap`).
