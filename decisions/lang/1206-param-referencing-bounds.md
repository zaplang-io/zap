---
title: Bounds referencing other type parameters — capture and substitute the bound's arguments
status: Accepted
issue: 1206
pr:
date: 2026-11-20
domain: lang
supersedes:
superseded-by:
---

# Bounds referencing other type parameters — capture and substitute the bound's arguments

**Status:** Accepted (implemented)

Second slice of the generic-interfaces epic #1204; builds on
[lang/1205-generic-interface-conformance](1205-generic-interface-conformance.md)
and, with it, unblocks #1199
([lang/1199-operator-capability-interfaces](1199-operator-capability-interfaces.md)).
Governing design `design/0012-generics.md` (Fork B Deferred: "bounds
referencing other type params").

## Decision

A type-parameter bound may name a generic interface whose **type arguments
reference other type parameters** in scope, including the parameter being bound:
`def scale_all[T: RMul[f64, T]](…)` and `struct S[E: Scaler[f64, E]]`. The
bound's full argument list is **captured** (previously sema kept only the
interface name and dropped the arguments) and the references are **resolved**
against the type parameters in scope — the declaration's own parameters plus,
for a method, the enclosing type's parameters — and preserved as type variables.

The captured bound is used at two sites:

- **Definition (check-at-definition, Fork M1).** Inside the bounded body a call
  through the parameter resolves the interface's method signature with the
  interface's own parameters substituted by the bound's arguments
  (`In := f64`, `Out := T`), so the call's result type is precise (e.g. `T`),
  not opaque. A call to a method the bound does not provide is still rejected
  here. A generic-interface bound supplied with the wrong number of arguments is
  rejected at the definition (`ZAP4084`).
- **Instantiation (mono).** The generic's own parameters are substituted into
  the bound's arguments first (`T := Matrix` turns `RMul[f64, T]` into
  `RMul[f64, Matrix]`), then #1205's generic-interface conformance substitutes
  the interface's parameters and checks the concrete type structurally. The
  diagnostic names the fully substituted bound.

## Rationale

- **It is the realization of the capability-bound surface.** #1199's reflected
  operator capability is exactly `[T: RMul[L, Out]]` with the result named by a
  parameter; without argument-referencing bounds the capability cannot be stated
  as a bound. This slice is the missing mechanism, not new surface.
- **Two existing substitution points, reused.** Definition-side resolution reuses
  #1205's `subst_fn_sig` over the interface's signature; instantiation reuses the
  monomorphizer's `subst_type`. The only genuinely new work is *capturing* the
  arguments (sema dropped them) and resolving their references.
- **Check-at-definition stays the model** (Fork M1): the body is checked once
  against the bound; concrete conformance is checked per instantiation.

## Consequences

- sema: `tyvar_bounds` / `enclosing_type_bounds` carry a `TyBound { iface, args }`
  (arguments resolved to `Ty`, references preserved as `Ty::Named(param)`), not a
  bare interface name; `type_param_bounds` resolves the arguments against the
  parameters in scope; a call through a bounded parameter substitutes them into
  the resolved signature; `ZAP4084` reports an arity mismatch at the definition.
- mono: `check_bounds` substitutes the generic's own parameters into the bound
  before the #1205 conformance check and names the substituted bound in the
  diagnostic.
- **Unblocks #1199** together with #1205. Its implementation (declaring the
  prelude operator interfaces and routing operator calls through them) is a
  separate change, not part of this epic.
- **Deferred:** multiple bounds per parameter, where-clauses, and associated /
  output types (`type Out`, #1207). A bound argument referencing an enclosing
  type parameter from a *method's own* parameter list is resolved; deeper nested
  generic-interface arguments beyond the built-in container families follow the
  same `Ty::Named` erasure noted in #1205.
- No change to `DECISIONS.md` (frozen); this ADR is the record.
