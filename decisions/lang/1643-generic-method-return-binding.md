---
title: Bind a generic method's return type into the call's result
status: Locked
issue: 1643
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Bind a generic method's return type into the call's result

**Status:** Locked

## Decision

When a generic **method**'s type parameters are bound from a call's arguments, the
binding is substituted into the method's **return type**, so the call's result
type is concrete rather than the erased `Unknown` it was before. A result assigned
to an incompatible target is then rejected. This is the method-side counterpart of
#1623, which did the same for free functions.

```zap
struct Opt:
    def make[T](self, Type[T] kind, T default) -> T:
        return default

def main():
    Opt o = Opt()
    int64 n = o.make(int64, 1)   # result is int64 — OK
    str s = o.make(int64, 1)      # result is int64, not str — rejected
```

#1622 bound `T` across the method's **arguments**, and #1642 extended that to enum
methods, but the method's return stayed erased (`-> T` resolves to `Unknown` in the
stored signature), so `str s = o.make(int64, 1)` was silently accepted.

## Mechanism

The struct instance-method call path already calls `check_generic_arg_consistency`
(#1622), which — since #1623 — returns `Option<Ty>`: the call's result type with
the bound type parameters substituted into the *un-erased* return type (`Some`
only when the callee is generic; a type parameter the arguments did not bind is
substituted to `Unknown`). The path now uses that result in place of the erased
`sig.ret`, but **only when it is `Some` and the substitution produced a concrete
type** (`!= Unknown`):

- a **non-generic** method returns `None` here → keep `sig.ret` (its declared
  concrete type);
- a **generic** method whose return **bound** to a concrete type (`-> T` with
  `T = int64`, or `-> list[T]` with `T = int64`) → use the substituted type;
- a **generic** method whose return did **not** bind (`Unknown`) → keep `sig.ret`.

Keeping the non-generic and unbound cases loose preserves the method-chain operator
typing this path drives `&`/`|`/`~` dispatch off the returned struct from (the
fluent condition-builder shape, operators #52): a method returning a concrete
struct keeps that struct type (its return has no type parameter, so the
substitution would return it unchanged anyway, and the `None`/unchanged paths keep
it exactly). The substitution reuses the same binding the #1622/#1642 consistency
pass already computes, so return binding and argument consistency share one
mechanism.

## Scope

- **Struct instance methods** (the `self.struct_info(&bt).and_then(…)` path). Enum
  instance methods route through the sibling branch added in #1642, which also
  calls `check_generic_arg_consistency`; extending the return binding there is a
  mechanical follow-up if wanted, not required by this issue.
- An unbound generic method return — a type parameter appearing only in the return,
  bound by no argument — stays `Unknown`. (It is not expressible as valid source
  anyway: the body would have no value of that type to return.)

## Consequences

- `crates/sema/src/lib.rs`: the struct instance-method call path uses the
  `Option<Ty>` from `check_generic_arg_consistency` as the result type when the
  method is generic and the substitution is concrete, else keeps `sig.ret`.
- A generic method call's result assigned to an incompatible target is now a type
  error; a matching target and numeric widening still compile; a non-generic
  method result and an operator-overloading method chain are unchanged.
- Generic **method** calls now bind their return type like generic **free
  function** calls (#1623), completing the follow-up #1623's ADR deferred.

<!-- Related: #1623 (free-function return binding, the shared mechanism), #1622
(generic method argument consistency), #1642 (enum instance-method argument
type-checking). -->
