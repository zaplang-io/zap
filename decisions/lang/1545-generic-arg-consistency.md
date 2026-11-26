---
title: Generic argument consistency after a type parameter is bound
status: Locked
issue: 1545
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Generic argument consistency after a type parameter is bound

**Status:** Locked

## Decision

At a call to a generic free function, binding a type parameter `T` from one
argument **constrains every other argument declared with `T`**. An argument that
is incompatible with the bound type is rejected with `ZAP4021` before lowering.

The motivating case is the approved Click-style CLI option declaration, whose
library signature is generic:

```zap
def option[T](Type[T] kind, T default) -> T:
    return default

def main():
    int64 n = option(int64, default="oops")   # error[ZAP4021]
```

The `Type[T]` argument `int64` establishes `T = int64` (first-class type values,
#1527), so the `T default` argument must be assignable to `int64`; the string
`"oops"` is not. Previously the program compiled and lowered a string constant
into an `i64` parameter slot.

This is the property the #1527 ADR explicitly **deferred**:

> Zap's generic type-parameter consistency is currently loose program-wide —
> `def take[T](T x, T y)` accepts `take(1, "hello")` today — so binding `T` from
> the type value and enforcing it against `T default` is a property of generic
> checking in general, orthogonal to type values, and is left to a separate
> change rather than worked around here.

So the enforcement lives in **generic call checking in general**, not in the
decorator desugar or the type-value machinery. `def take[T](T x, T y)` now
rejects `take(1, "hello")` for the same reason — the first argument binds `T`,
the second must respect it.

### What establishes and checks a binding

A call's arguments are matched to the function's parameters in **declaration
order** (a positional argument by index, a keyword argument by name), so the
verdict does not depend on how the call lists its arguments. Each
`(parameter type, argument type)` pairing is matched structurally against the
function's *un-erased* parameter types (type parameters kept as type variables),
contributing `(type parameter, argument sub-type)` bindings:

- a bare `T` parameter binds `T` to the whole argument type (so `opt[T](T kind)`
  receiving `int64` binds `T = Type[int64]`, the #1527 surface, unchanged);
- a `Type[T]` parameter binds `T` to the **represented type** of a type-value
  argument (`Type[int64]` ⇒ `T = int64`) — an exact, authoritative binding, since
  type values are invariant;
- container/`Optional`/tuple parameters recurse element-wise.

Binding then runs in two rounds:

1. **establish** — type-value bindings (exact) are applied first, then ordinary
   value bindings bind an unbound parameter or **widen** it to the least upper
   bound of the two (`int32` then `int64` ⇒ `int64`, in either argument order);
2. **check** — every `T`-typed argument is re-tested against the final binding; a
   value assignable in neither direction is reported with `ZAP4021`, located at
   the offending argument.

### Scope

- **Free functions only (v1).** Generic *methods* (struct/enum/interface) are not
  yet recorded for this pass; a generic method call is unchanged. The #1545
  reproduction and its matrix are free functions, and the Click-style library
  decorator the gap blocks is matched as a free generic call. Generic-method
  consistency is a documented follow-up.
- **Additive.** Before this change every type parameter erased to `Ty::Unknown`
  in the stored signature, so a `T` position accepted anything. The new pass only
  *adds* rejections for genuinely inconsistent calls; no previously-accepted
  **consistent** program changes behaviour.
- **`Unknown`/`py` is a wildcard.** A dynamic or un-inferable argument neither
  binds a type parameter nor conflicts with one, so it never causes a false
  positive.
- The **return** type is unchanged: a generic return still erases to `Unknown` at
  the call site (binding `T` into the result is a separate, orthogonal inference
  improvement and is not required to close this gap).

## Rationale

- **Root cause, not a special case.** The gap was that `sig_of` resolves a type
  parameter to `Ty::Unknown`, severing the link between the `Type[T]` argument
  that binds `T` and the `T` argument that must respect it. Rather than
  special-casing the decorator or the `Type[T]` position, the function's
  **un-erased** parameter types (type variables preserved via
  `resolve_type_scoped`) are recorded per generic function and consulted at the
  call, so the fix is one general consistency pass.
- **Order-independent.** Separating *establish* from *check*, and giving the
  exact type-value bindings priority, makes the verdict the same whether the
  constraining type value is written before or after the value it constrains, and
  whether arguments are positional or keyword.
- **Least upper bound on widening.** Using assignability in both directions when
  refining a binding keeps a matching integer default (and ordinary numeric
  widening such as `int32`/`int64`) compiling, while a `str`-vs-`int` mismatch is
  rejected regardless of argument order.

## Consequences

- `sema` records a `GenericFnInfo` (type-parameter names + un-erased parameter
  types) for each generic free function and runs `check_generic_arg_consistency`
  on the plain call path. Calls carrying `*`/`**` spreads are skipped (their
  argument-to-parameter mapping is not statically known), matching the existing
  conservative spread arity checks.
- A mismatched generic argument is now `error[ZAP4021]` with a message naming the
  type parameter and the type it was bound to by another argument.
- **Deferred:** generic *method* argument consistency; binding a generic return
  type into the result type at the call site.

<!-- Related: #1527 (first-class type values — the deferral this resolves),
#1509/#1519 (the decorator surface), zaplang-io/cli#4. -->
