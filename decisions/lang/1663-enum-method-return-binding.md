---
title: Bind a generic enum method's return type into the call's result
status: Locked
issue: 1663
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Bind a generic enum method's return type into the call's result

**Status:** Locked

## Decision

When a generic **enum** instance-method's type parameters are bound from a call's
arguments, the binding is substituted into the method's **return type**, so the
call's result is concrete rather than the erased `Unknown` it was before, and a
result assigned to an incompatible target is rejected. This is the enum
counterpart of #1643 (struct methods), making enum and struct instance methods
symmetric on both argument consistency (#1642/#1622) and return binding
(#1643/#1623).

```zap
enum Color:
    Red
    Green

    def make[T](self, Type[T] kind, T default) -> T:
        return default

def main():
    auto c = Color.Red
    int64 n = c.make(int64, 1)   # result is int64 — OK
    str s = c.make(int64, 1)      # result is int64, not str — rejected
```

#1642 brought argument type-checking (plain + generic consistency) to enum
instance methods, but left the return erased (`-> T` resolves to `Unknown`), so
`str s = c.make(int64, 1)` was silently accepted while the identical struct form
was rejected after #1643.

## Mechanism

The enum instance-method call path already calls `check_generic_arg_consistency`
(#1642), which returns `Option<Ty>` — the result type with the bound type
parameters substituted into the *un-erased* return type (`Some` only when the
callee is generic; a type parameter the arguments did not bind is substituted to
`Unknown`). The path now uses that result in place of the erased `sig.ret`, under
the exact rule #1643 uses for struct methods: **only when it is `Some` and the
substitution produced a concrete type** (`!= Unknown`). A non-generic enum method
returns `None` here → keep `sig.ret`; a generic enum method whose return did not
bind returns `Unknown` → keep `sig.ret`. The two instance-method branches now
handle return binding identically.

## Scope

- **Enum instance methods** (the enum-receiver branch in the method-call path).
- Mirrors #1643's struct behaviour exactly; arity stays loose and the per-argument
  / generic-consistency checks from #1642 are unchanged.
- An unbound generic enum-method return — a type parameter appearing only in the
  return — stays `Unknown`. (Not expressible as valid source: the body would have
  no value of that type to return.)

## Consequences

- `crates/sema/src/lib.rs`: the enum instance-method call path uses the
  `Option<Ty>` from `check_generic_arg_consistency` as the result type when the
  method is generic and the substitution is concrete, else keeps `sig.ret`.
- A generic enum-method call's result assigned to an incompatible target is now a
  type error; a matching target and numeric widening still compile; a non-generic
  enum method result is unchanged.
- Generic argument consistency AND return binding are now symmetric across free
  functions (#1545/#1623), struct instance methods (#1622/#1643), and enum
  instance methods (#1642/#1663).

<!-- Related: #1643 (struct method return binding, the symmetric sibling), #1623
(free-function return binding), #1642 (enum instance-method argument type-checking),
#1622. -->
