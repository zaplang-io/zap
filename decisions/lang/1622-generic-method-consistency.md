---
title: Generic argument consistency for generic methods
status: Locked
issue: 1622
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Generic argument consistency for generic methods

**Status:** Locked

## Decision

Binding a type parameter `T` from one argument of a generic **method** constrains
every other argument declared with `T`, exactly as #1545 already does for generic
free functions. An inconsistent argument is rejected with `ZAP4021` before
lowering.

```zap
struct Util:
    def take[T](self, T x, T y) -> T:
        return x

def main():
    Util u = Util()
    u.take(1, "hello")   # error[ZAP4021]: `T` is bound to int, "hello" is str
```

Previously a generic instance-method call was inferred **without any argument
type check** (the receiver path infers arguments loosely and returns the method's
declared type), so `u.take(1, "hello")` compiled and monomorphized `take$int64`
with a string in an `i64` slot.

## Mechanism

`collect_sigs` records a generic method's call-site surface in `self.generic_fns`
keyed `"Type.method"`, mirroring the free-function path, with the implicit `self`
receiver dropped so a call's positional arguments map to the stored parameters by
index. The struct instance-method call path then runs the existing
`check_generic_arg_consistency` with that key, over the inferred argument types.

The pass is **purely additive**: it adds only the generic-inconsistency
`ZAP4021`; the method path's otherwise-loose argument typing and the method's
declared return type are unchanged (the return stays `sig.ret`, preserving the
method-chain operator typing that path relies on — generic *return* binding is
#1623, scoped to free functions).

## Scope

- **Struct instance methods** and any method call routed through `check_user_call`
  (e.g. an associated/`Type.func` call). The receiver's `self` is excluded from
  the recorded parameters.
- **Enum instance methods are out of scope (follow-up).** They are typed through a
  separate, fully loose receiver path (`struct_info` deliberately excludes enums),
  so there is no per-argument check to extend there yet; the generic surface is
  recorded for them, but the enum instance-call path does not consult it.

## Consequences

- `crates/sema/src/lib.rs`: `record_generic_method` populates `generic_fns` for
  struct and enum methods; the struct instance-method call path collects the
  argument types and calls `check_generic_arg_consistency("Type.method", …)`.
- A generic method call with inconsistent arguments is now `ZAP4021`, located at
  the offending argument.
- **Deferred:** enum instance-method argument consistency.

<!-- Related: #1545 (the free-function consistency this extends), #1623 (generic
return binding, the sibling follow-up). -->
