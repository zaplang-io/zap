---
title: "E4 dogfoods @derive(Eq) by shadow-proving, not replacing, the built-in"
status: Accepted
issue: 1431
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Shadow-prove `@derive(Eq)` (#1431)

**Status:** Accepted — the first E4 (#1429) dogfood slice. It proves the library
`@derivable` mechanism can express the compiler's structural equality **without
replacing** the built-in.

## Why shadow-prove, not replace

E4's initial framing was "incremental replace" (remove each built-in `gen_*`).
For `Eq` that runs into working, well-integrated machinery that is not worth
tearing out:

- The built-in `Eq` has two entry points — the explicit `@derive(Eq)` *and* the
  **implicit value-struct auto-derive**, which synthesizes `Eq` **and** `Hash`
  together at IR lowering from post-sema hashability analysis, and backs the
  prelude's own value structs (`Duration`, `MemStats`, …).
- Rerouting that implicit, type-info-dependent auto-derive through the pre-sema
  library-stub pipeline would add real risk for no user-facing gain.

So E4 keeps the built-in (and the implicit auto-derive) untouched and instead
**shadow-proves**: a library `@derivable` generator expresses the same structural
equality, and a behavior-identical golden shows it agrees with the built-in. The
payoff is the proof plus a readable, forkable reference generator — not less
compiler code.

## What this slice adds

- A library generator + trait (named `StructEq`, not `Eq`, so it does not touch
  the built-in `Eq` registration) whose body is the structural field-wise
  comparison `comptime for f in reflect.type_info(T).fields: … reflect.field(a, i)
  != reflect.field(b, i) …`. Its per-field `!=` lowers to the same primitive the
  built-in `Eq` and the `==` operator emit (`FCmp Eq` for float, `zap_str_eq` for
  str, the field's own `__eq__` for a nested struct), so the two agree.
- Golden `tests/golden/derive_eq_shadow.zap`: a struct derives both built-in `Eq`
  (via `==`) and the library `StructEq`, and the two results match for int / str
  / bool / float fields and a nested derived struct (runtime output, CI LLVM
  matrix).
- Host test `a_library_generator_shadows_the_built_in_eq_without_replacing_it`:
  both `@P____eq__` (built-in, untouched) and `@_derive_structeq$P` (shadow) are
  emitted.

**No wiring changes.** `DERIVABLE_TRAITS`, `gen_eq_method`, the value-struct
auto-derive, and the prelude are all unchanged.

## Known, intentional difference

The built-in `Eq` requires a nested struct field to be **hashable** (its
key-plan needs `__hash__` + `__eq__`), while the shadow via `!=` needs only
equality. The golden gives the nested `Inner` both (`@derive(Hash, Eq)`) so the
two paths are comparable; the laxer shadow acceptance is noted, not reconciled —
nothing here changes the built-in's rule.

## Consequence

The `@derivable` mechanism is shown to express the compiler's structural
equality. Ord/Hash/Repr can be shadow-proved the same way; Repr still waits on a
source-level `repr()` (#1384). The derive-synth built-in-sig enabler (#1433)
stays available for a future explicit-path replacement but is not used by this
shadow slice.
