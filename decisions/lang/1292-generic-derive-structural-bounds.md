---
title: "Generic @derive — inferred serde bounds realized structurally (E3c)"
status: Accepted
issue: 1292
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Generic `@derive` — inferred bounds, realized structurally (E3c)

**Status:** Accepted — the third slice of E3 (#1292, epic #1289). It lifts the
E3b restriction to non-generic types ([1291-derive-forwarding-stub-synthesis](1291-derive-forwarding-stub-synthesis.md)):
a **generic** type now derives a library trait. Per the HQ2 decision (#1243,
Option A — serde inferred bounds), a derive conforms only for instantiations
whose field types conform; this slice realizes that **structurally**, the way
Zap's type system actually works, and records the inferred-bound data for E3d.

## Context

The spec frames generic derive as "emit `impl[T_i: X]` for each type parameter
used in a non-skipped field." Two facts make that framing non-literal in Zap:

- **There is no `impl` construct.** Conformance is structural (D-0016): a type
  conforms to an interface iff it has the methods, checked per concrete
  instantiation.
- **A method's own type parameter shadows the enclosing struct's as a fresh,
  *unbounded* variable** (sema's `method_own_generic_shadows_enclosing_bound`).
  So a literal `[A: X]` cannot be attached to the synthesized method — it would
  shadow the struct's `A` and break `self`'s type.

Empirically, once the generic forwarding stub is emitted, a generic derive —
including a recursive type — already type-checks, monomorphizes and lowers:
monomorphization specializes the generator at each concrete instantiation and the
`SpecializationCache` (#671/#1255) terminates a recursive request by handing back
the in-progress instance ("assume-the-goal").

## Decision

Synthesize the forwarding stub for a generic type too, forwarding to the
generator specialized at **the type applied to its own parameters** —
`_derive_x[Pair[A, B]](self)` on `struct Pair[A, B]` (a bare subscript `Node[T]`
for a single parameter). Conformance and recursion termination are left to the
existing structural monomorphization path; **no literal bound is emitted** (none
can be) and **no new bound/`impl` logic** is added.

The inferred bounds are surfaced as **data**, not syntax: `expand_library_derives`
returns, for each generic derive, a `GenericDeriveBounds { struct_name,
trait_name, params }` where each `InferredBound { param, field_spans }` names a
type parameter mentioned by one or more fields and those fields' spans. The scan
walks `Named`/`Tuple`/`Func` type structure, so `list[A]`, `dict[A, A]`,
`(A, int64)` and `Optional[Node[T]]` are all caught; a parameter used in no field
(or only behind a concrete container like `list[int64]`) is omitted, and a
parameter mentioned by several fields collects each span. E3d (#1293) consumes
this to land an unsatisfied-bound diagnostic at the offending field; the current
pipeline ignores it.

## Consequences

- Generic types derive library traits (host-tested in
  `crates/driver/src/derive_synth.rs`: Self-applied type arg for multi- and
  single-parameter types, the inferred-bound scan over every spec shape, and a
  recursive `Node[T]`); end-to-end on the LLVM matrix via
  `tests/golden/derive_generic.zap` (`Pair[A, B]` → `p.dump()` prints both
  fields). The ZIR matches the non-generic E3b golden but specialized at the
  concrete instantiation.
- No change for a non-generic derive, or a program that derives only built-in
  traits (no inferred bounds recorded).

## Deferred

- **`@skip` field attribute:** not yet a surface, so every field counts in the
  scan. Excluding a skipped field's parameters is a follow-up.
- **A *recursing* generator** (serde-style dispatch of the trait method on each
  field) needs the E2 kind-switch (#1253); only then does recursion termination
  actually fire. The `SpecializationCache` that terminates it is already present.
- **Diagnostics rendering** of an unsatisfied inferred bound → E3d (#1293); this
  slice only produces the `GenericDeriveBounds` structure and spans.
