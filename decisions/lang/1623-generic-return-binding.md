---
title: Bind a generic return type into the call's result
status: Locked
issue: 1623
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Bind a generic return type into the call's result

**Status:** Locked

## Decision

When a generic free function's type parameters are bound from a call's arguments,
the binding is substituted into the **return type**, so the call's result type is
concrete rather than the erased `Unknown` it was before. A result assigned to an
incompatible target is then rejected.

```zap
def option[T](Type[T] kind, T default) -> T:
    return default

def main():
    int64 n = option(int64, default=1)   # result is int64 — OK
    str s = option(int64, default=1)      # result is int64, not str — rejected
```

#1545 bound `T` across the **arguments** but left the result erased (`-> T`
resolves to `Unknown` in the stored signature), so `str s = option(int64,
default=1)` was silently accepted.

## Mechanism

`GenericFnInfo` now also stores the function's *un-erased* return type (type
parameters preserved via `resolve_type_scoped`). `check_generic_arg_consistency`,
after establishing the binding `subst`, substitutes it into that return type and
returns the result; `check_user_call_plain` uses it in place of the erased
`sig.ret`. Every type parameter is substituted — a bound one by its inferred
type, an **unbound** one by `Unknown` — so a return type the arguments did not
pin stays permissive and a non-generic call is unaffected.

## Scope

- **Free functions** (the `check_user_call_plain` path). A **method's** return
  type is left loose (`sig.ret`): the instance-method path feeds method-chain
  operator typing, and substituting there is a separate change not required by
  this issue.
- The substitution reuses the same binding the #1545/#1622 consistency pass
  computes, so return binding and argument consistency share one mechanism.

## Consequences

- `crates/sema/src/lib.rs`: `GenericFnInfo` gains an un-erased `ret`;
  `check_generic_arg_consistency` returns `Option<Ty>` (the substituted result
  type) and `check_user_call_plain` returns it when the callee is generic.
- A generic call's result assigned to an incompatible target is now a type error;
  a matching target and numeric widening still compile; an unbound generic return
  stays `Unknown`.
- **Deferred:** binding a generic *method's* return type.

<!-- Related: #1545 (argument consistency, the shared mechanism), #1622 (generic
method consistency, the sibling follow-up). -->
