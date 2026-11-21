---
title: The module linker mangles the interface name inside a type-parameter bound
status: Locked
issue: 1222
pr:
date: 2026-11-21
domain: lang
supersedes:
superseded-by:
---

# The module linker mangles the interface name inside a type-parameter bound

**Status:** Locked

## Decision

When the module linker qualifies an imported module's symbols with its mangle
prefix, it now also rewrites the **interface name inside each generic type
parameter's bound** (`struct Wrapper[T, C: Codec[T]]`), exactly as it rewrites
the interface declaration and every other type reference. The bound's own type
arguments — the surrounding type parameters, e.g. `T` in `Codec[T]` — stay
unmangled (they are type-locals), and a built-in capability bound
(`Comparable`, `Add`, …) is left alone because it is not a module symbol.

## Rationale

The linker (`crates/driver/src/modlink`) merges every imported module into one
AST, mangling each module's own top-level symbols with a unique prefix so same
-named declarations in different modules do not collide. The `Rewriter` walked
field types, method signatures, return types and bodies — but never the `bound`
field of a `TypeParam`. So an imported `interface Codec` was mangled to
`__zm1_pkg__Codec` at its declaration and at the generic struct's own name and
concrete field type, while the bound `C: Codec[T]` kept the bare name `Codec`.

Monomorphization resolves a bound by looking the interface name up in the
merged interface table, which is keyed by the mangled name. The unmangled
`Codec` missed, and the instantiation failed with `'Codec' is not an interface`
— a type that compiled and ran when declared in a single module failed as soon
as the interface and conformer moved to an imported module. This blocked the
typed serde wrapper in zaplang-io/memcached#8/#21.

The fix belongs in the linker, not the monomorphizer: the bound is an ordinary
type reference, and the linker is the single place that decides a module
symbol's qualified name. Making the monomorphizer re-guess a prefix would
duplicate that authority and reintroduce the skew. Rewriting the bound through
the same `ty` path the linker already uses for every other reference keeps the
bound's name derivation identical to the interface declaration's by
construction.

Alternatives rejected:

- **Resolve the bound by bare name in the monomorphizer (strip/try prefixes).**
  Reintroduces two places that compute a symbol's qualified name; they will
  drift. The linker already owns qualification.
- **Special-case the serde/memcached names.** Rejected on principle — the defect
  is general to any imported generic with an interface bound.

## Consequences

- An imported generic struct/enum/function/type-alias/interface with an
  interface bound on a type parameter now resolves that bound to the imported
  interface and is checked for conformance like a same-module one; a
  non-conformer is rejected with the ordinary "does not satisfy the bound"
  diagnostic (which names the missing/mismatched method) instead of the
  misleading "is not an interface".
- Static dispatch is unaffected: the bound's methods still lower to direct calls
  on the concrete conformer per instantiation (no vtable), so there is no
  runtime or GC change.
- Parameterized bounds that reference a sibling type parameter (`C: Codec[T]`,
  the self-referential `[T: Scaler[f64, T]]` shape) keep that argument bare, so
  the per-instantiation substitution is unchanged.
- The bound rewrite is inert for the entry module (empty prefix), so the
  single-module behavior established in I#164/#1205/#1206 is byte-identical.
