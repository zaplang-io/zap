---
title: Generic reflect.type_info(T) resolution — comptime-for unrolls per instantiation (E3b part 1)
status: Accepted
issue: 1291
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Generic `reflect.type_info(T)` resolution (E3b, part 1)

**Status:** Accepted — the first slice of E3b (#1291, the `@derive`
forwarding-impl synthesis, epic #1289). It supplies the **mechanism** a real
generic derive generator needs, which S2 (#1298) deferred: a `comptime for` over
`reflect.type_info(T)` where `T` is a **type parameter** now unrolls per
instantiation. The forwarding-stub synthesis itself is the next E3b slice.

## Context

S2 wired `comptime for` to the engine in mono, but `try_unroll_comptime_for`
resolved `reflect.type_info(T)` only when `T` named a **concrete** declaration.
A real `@derive` generator is generic — `def _derive_x[T](T self, …)` with a
`comptime for f in reflect.type_info(T).fields:` body — so `T` is a type
parameter, not a concrete name, and the loop would not unroll. That made the
whole derive path non-functional for generic generators.

## Decision

In `try_unroll_comptime_for`, resolve the type name extracted from
`reflect.type_info(<name>)` through monomorphization's **active type-parameter
substitution** (`cur_subst`) before looking it up among the concrete
declarations. When specializing a generic function body (I#161), `cur_subst`
already maps the function's type parameters to this instantiation's concrete
types; so inside the specialized `_derive_x[Point]`, `reflect.type_info(T)`
resolves `T → Point`, the loop unrolls over `Point`'s fields, and each
`reflect.field(self, comptime f.index)` desugars to an unboxed field access. A
bare concrete type name is left as-is (the S2 behavior).

This reuses the existing substitution machinery — no new mono state — and is the
minimal change that makes a generic comptime generator usable, which is the
prerequisite for the E3b forwarding stub (a `@derive(X)` on `T` forwards to the
registered generic generator specialized at `T`).

## Consequences

- A generic `def g[T](T p): comptime for f in reflect.type_info(T).fields: …`
  called at a concrete type unrolls per instantiation (host-tested over the
  monomorphized AST: `crates/mono/tests/comptime_surface_wiring.rs`), and runs
  end-to-end on the LLVM matrix (golden `tests/golden/comptime_for_generic.zap`,
  prints a `Point`'s two fields via `p.x`/`p.y`).
- No behavior change for a non-generic `comptime for` or a program with none.

## Still deferred (the rest of E3b)

- **Forwarding-impl synthesis + hygiene (HQ7):** `@derive(Trait)` on a type
  synthesizes the trait interface's method(s) as thin stubs forwarding to the
  registered generic generator (E3a's registry → the generator), specialized at
  the type. That is the next E3b slice, building directly on this.
- Bound propagation (E3c, #1292); derive diagnostics wiring (E3d, #1293).
