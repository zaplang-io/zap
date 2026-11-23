---
title: comptime/reflect surface — sema acceptance + end-to-end (S2b)
status: Accepted
issue: 1298
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# comptime/reflect surface — sema acceptance + end-to-end (S2b)

**Status:** Accepted — the second half of issue #1298 (S2 of the comptime/reflect
surface epic #1296). The first half wired the surface to the E2 engine in mono
(decisions/lang/1298-comptime-surface-engine-wiring.md); this half makes **sema
accept and type the comptime fragment**, so a real `.zap` program using
`comptime for` / `reflect.*` compiles and runs **end-to-end**.

## Decision

S1 (#1297) parsed the fragment but sema reported it as not-yet-implemented
(`ZAP0002`), which stopped compilation before mono's unrolling could run. S2b
removes that deferral and types the fragment leniently:

1. **`reflect.{type_info,field,has_override}(...)` are comptime builtins.** In
   `check_call`, a callee `reflect.<fn>` with one of these names type-checks as
   `Ty::Unknown` and its arguments are **not** inferred — crucially
   `reflect.type_info(T)`'s argument is a *type name*, not a value, so inferring
   it would wrongly raise an unknown-name error. The comptime engine consumes
   and unrolls these during monomorphization.
2. **`comptime for`** infers its iterable (the reflection sequence, `Unknown`),
   binds its loop variable as `Unknown` (a comptime reflection record), and
   checks its body — like a runtime `for`, minus element typing.
3. **`comptime <expr>`** is transparent to the type system: it has its inner
   expression's type (a comptime-only operand such as `f.index` or
   `reflect.type_info(T)` infers `Unknown`).

`Unknown` is the front end's permissive type (attribute/call/arg checks on it do
not cascade errors), so the fragment type-checks without special per-node
machinery, and a program with no `comptime` is unaffected.

## Consequences

- A `comptime for` over a concrete `reflect.type_info(T).fields`/`.variants` now
  passes the **whole host front end** (parse → sema → `lower_module`, where mono
  unrolls it and desugars `reflect.field` to typed field access). The
  grammar-coverage `comptime_for_stmt` entry **graduates from `TrackedGap` to
  `Compiles`** accordingly, and its `GAP_NOTES` entry is removed.
- A runtime golden (`tests/golden/comptime_for_fields.zap` + `.expected`) proves
  the end-to-end path on the LLVM CI matrix: a `comptime for` over `Point`'s two
  fields prints `10` / `20` via unboxed `p.x` / `p.y` accesses.
- New host tests: `crates/sema/tests/comptime_surface_accept.rs` (6) — the
  fragment type-checks, no `ZAP0002`, `reflect.*` is not an unknown name, a plain
  program is unaffected.

## Scope still deferred

- **Generic `reflect.type_info(T)` with `T` a type parameter** (a generic derive
  generator unrolling per instantiation) — mono resolves only a concrete type
  name today; threading type-parameter substitution into the comptime-for
  resolution is the remaining piece before E3b (#1291) is fully usable.
- Standalone `comptime <expr>` folding outside a loop; richer comptime typing
  (treating `reflect.*` as precise comptime types rather than `Unknown`).
