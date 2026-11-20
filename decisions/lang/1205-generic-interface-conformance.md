---
title: Generic-interface conformance — substitute the interface's type parameters
status: Accepted
issue: 1205
pr:
date: 2026-11-20
domain: lang
supersedes:
superseded-by:
---

# Generic-interface conformance — substitute the interface's type parameters

**Status:** Accepted (implemented)

First slice of the generic-interfaces epic #1204; a prerequisite for #1199
(naming an operator capability as a generic bound, see
[lang/1199-operator-capability-interfaces](1199-operator-capability-interfaces.md)).
Governing design `design/0012-generics.md` (the "generic interfaces — reserved;
realized in a later slice" item in Fork S / Fork B's Deferred list).

## Decision

A **generic interface** `interface X[T]` is realized for **structural
conformance**: a concrete type conforms to `X[A]` iff it provides every method
of `X` with the interface's own type parameters **substituted** (`T := A`). The
substitution is applied only to the interface's *own* declared method
signatures; the arguments are matched left-to-right against the interface's
declared parameters, and an arity mismatch is an error at the site that supplied
them.

The realized surface is the **bound** position — `def f[T: X[A]](…)` and
`struct S[T: X[A]]` — because bounds are checked by structural conformance **at
each instantiation** (static dispatch, no fat pointer; `design/0012` Fork B).
Interface *values* (`X[A] v = …`) are the separate dynamic, fat-pointer feature
(Fork X) and are **out of scope** here: the inferred-type representation
(`Ty::Named`, which carries no type arguments) cannot thread a value's interface
arguments, so a bare generic-interface value does not substitute. This slice
does not change that.

An interface method that names one of the interface's type parameters keeps it
as a type **variable** (`Ty::Named(param)` in sema; the bare AST `Type::Named`
in mono) rather than being erased to the opaque `Unknown` an unknown name
previously produced — that is what makes the parameter substitutable. The
variable is threaded through the element positions of the built-in container
families and tuples, so `list[T]` / `(T, U)` in an interface method conform
precisely.

## Rationale

- **Reuses the existing conformance checker.** Both the definition-side checker
  (`sema::Checker::conforms`) and the instantiation-side checker
  (`mono::satisfies_bound`) already match a concrete type's method set against
  an interface's; realization is a pre-pass substitution over the interface's
  declared signatures, not a new mechanism.
- **Bounds first, values deferred, matches the design.** `design/0012` already
  splits bounded generics (static, this slice) from interface values (dynamic,
  Fork X). Scoping conformance to the bound position avoids widening `Ty` to
  carry interface arguments — a larger change with no near-term consumer (#1199
  needs the bound form).
- **Per-argument, left-to-right substitution** is the obvious, Rust/`impl`-like
  semantics and keeps two-parameter interfaces (`X[L, Out]`, the #1199 MVP
  shape) working with no special case.

## Consequences

- `InterfaceInfo` (sema) gains `generics: Vec<String>` — the interface's own
  type-parameter names in order. A method signature that names one resolves to
  `Ty::Named(param)` via a new scoped resolver.
- `Checker::conforms_with_args(sname, iface, &[Ty])` substitutes and checks; the
  old `conforms` delegates with no arguments (the pre-#1205 behavior, so
  non-generic interfaces are unchanged — verified by the full existing suite).
- `mono::satisfies_bound` takes the bound's type arguments and substitutes the
  interface's parameters (via the existing `subst_type`) before the structural
  match; `check_bounds` threads them from the bound and names the bound with its
  arguments (`Container[int64]`) in the diagnostic.
- **Deferred (follow-ups):** resolving a *call through* a type parameter bounded
  on a generic interface to the substituted signature, and bounds whose
  arguments reference other type parameters (`[T: X[…, T]]`) — that is #1206.
  Generic *supertypes* of a refined interface are not substituted through the
  refinement edge (their arguments are not carried on it); moot while the
  refined capability supertypes are non-generic. Generic-interface **values**
  stay out of scope (Fork X). Associated/output types are #1207.
- No change to `DECISIONS.md` (frozen); this ADR is the record.
