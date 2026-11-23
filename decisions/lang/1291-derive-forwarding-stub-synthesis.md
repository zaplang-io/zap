---
title: "@derive(Trait) forwarding-stub synthesis for library-registered derivable traits (E3b)"
status: Accepted
issue: 1291
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# `@derive(Trait)` forwarding-stub synthesis (E3b)

**Status:** Accepted — the second slice of E3b (#1291, epic #1289). It builds on
the generic-`T` resolution (the first slice,
[1291-comptime-generic-t-resolution](1291-comptime-generic-t-resolution.md)):
with a generic `comptime for` generator now able to unroll per instantiation,
this slice makes `@derive(Trait)` on a type actually synthesize the conforming
method(s) that forward to that generator.

## Context

E3a (#1290) let a library register a generic generator with `@derivable(Trait)`
and declare the trait as a structural `interface Trait`. But `@derive(Trait)` on
a type was still inert: lowering reported it as "deferred to E3b". The contract
chosen for E3b (maintainer decision, Option A) is a **thin forwarding stub**:
all derive logic lives in the library's generic generator, and `@derive(Trait)`
only makes the type conform by gaining the interface's method(s), each forwarding
to the generator specialized at the type (HQ7 — no logic is synthesized into the
type beyond the forward).

## Decision

Add a **pre-sema** driver desugar `expand_library_derives` (runs in the front end
right after the behavioral-decorator expansion, before `analyze_types`). For a
non-generic type that derives a library-registered trait, it:

1. Scans the module for `@derivable(Trait)` generators (trait → generator name)
   and `interface Trait` declarations (trait → method signatures). Sema's own
   derivable registry is not yet built at this point, so the pass scans the AST
   directly.
2. For each interface method the type does not already define, appends a
   synthesized method with the interface method's exact signature whose body
   forwards every parameter (including `self`) positionally to the generator with
   an **explicit** type argument: `generator[Type](self, …)`. The type argument
   is explicit rather than inferred because the synthesized nodes share the
   enclosing type's span, so sema's span-keyed type map cannot distinguish the
   `self` argument's type for monomorphization to pin `T` from — and the concrete
   type is known at synthesis anyway.
3. Strips the synthesized trait from the type's `@derive(...)` (removing an empty
   `@derive`), so lowering's deferral backstop no longer fires. Built-in traits
   and any un-synthesized name are left in place.

Running before sema means the synthesized method is type-checked, structurally
conformance-checked, monomorphized and lowered exactly like a hand-written one; a
hand-written method of the same name wins (no stub is synthesized over it). The
`@derivable` marker on the generator is a compile-time-only registration with no
codegen, so lowering now treats it as a consumed no-op (added to
`DECORATED_NO_CODE`) rather than a deferred decorator.

## Scope (first cut) and consequences

- Supported: a **non-generic** type deriving a library trait whose `interface`
  declares one method (the Dump / Hash / Serialize shape) of any arity. Host-
  tested in `crates/driver/src/derive_synth.rs` (stub shape, trait strip, HQ7
  override, missing-interface error, built-in left untouched, generic-struct
  deferral); exercised end-to-end by `tests/golden/derive_forwarding_stub.zap`
  (a `@derive(Dump)` `Point` whose `p.dump()` prints both fields).
- One stub is synthesized per interface method, each forwarding to the trait's
  one registered generator; a multi-method trait therefore forwards every method
  to that generator, so **distinct per-method generators** are a follow-up.
- A **generic** type is left with its `@derive` intact (the lowering backstop
  still reports it): bound propagation is E3c (#1292). Enum deriving and the
  derive-diagnostics wiring (consuming the #1256 backtrace) are E3d (#1293).
- A library derive generator must **not** name its generic parameter `self`: a
  generic free function with a `self`-named parameter currently mis-lowers
  `reflect.field` (`ZAP6001` "field access on a non-struct value"). This is a
  pre-existing defect in the reflect-surface machinery, orthogonal to the
  synthesis, and is reported separately; the synthesis itself is independent of
  the generator's parameter name.
