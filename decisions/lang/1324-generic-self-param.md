---
title: "A generic free function's `self`-named parameter is an ordinary parameter"
status: Accepted
issue: 1324
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# A free function's `self` parameter is ordinary (#1324)

**Status:** Accepted — a monomorphization fix. `self` is only a *method receiver*
when the function is a method; in a **free function** a parameter named `self` is
an ordinary positional parameter and must be treated as one.

## Symptom

```zap
def g[T](T self):
    comptime for f in reflect.type_info(T).fields:
        print(reflect.field(self, comptime f.index))
def main():
    g(Point(10, 20))
```

failed with `error[ZAP6001]: field access on a non-struct value` (or, with an
explicit `g[Point](…)`, after the first fix, `cannot infer type argument 'T'`).
Renaming the parameter `self` → `v` worked, and a non-generic `self` parameter
was already fine — so only the **generic** path broke. Surfaced by the E3b
`@derive` work (a derive generator naturally names its parameter `self`).

## Cause

Monomorphization special-cased every parameter literally named `self` as a method
receiver, in two places:

1. `rewrite_fn_inner` skipped type-substituting and env-declaring a `self`
   parameter (`if p.name != "self"`), relying on the method receiver type
   (`self_ty`). For a free function `self_ty` is `None`, so the specialized
   `g[Point]` kept `self: T` unsubstituted → lowering resolved `T` to a
   non-struct and `reflect.field(self, …)` failed.
2. `infer_fn_targs` filtered `self` out of the parameters it matches call
   arguments against (`filter(|p| p.name != "self")`). For a free call
   `g(Point(..))` the first argument maps to the `self` parameter, so filtering it
   left `T` uninferable.

Both are correct for a **method** (the receiver is bound via `self_ty` and passed
separately from the argument list) but wrong for a **free function**.

## Fix

Gate both on whether this is actually a method: a receiver is `self` **and** the
method context is present.

- `rewrite_fn_inner`: substitute + declare a `self` parameter unless it is a
  method receiver (`p.name == "self" && self_ty.is_some()`).
- `infer_fn_targs`: take a `skip_self` flag — `true` at the two method call sites
  (operator dunder, `recv.m(..)`), `false` at the free-function call site — and
  exclude `self` from the inference parameters only when set.

## Consequence

A generic free function parameter named `self` is substituted per instantiation
and participates in type-argument inference like any other parameter; methods are
unchanged. Regression-guarded by `tests/golden/reflect_field_self_param.zap`
(the repro, prints the two fields end-to-end on the LLVM matrix) and a
reflect-free mono unit test
(`crates/mono/tests/comptime_surface_wiring.rs`: `g[T](T self) -> T` specializes
with `self: Point` and infers `T` from the argument). No deprecation of the
`self` name is introduced — it is simply no longer privileged outside a method.
