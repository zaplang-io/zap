---
title: "@derivable(Trait) registration model (E3a)"
status: Accepted
issue: 1290
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# `@derivable(Trait)` registration model (E3a)

**Status:** Accepted — records the *implemented shape* of issue #1290 (the first
slice of E3, epic #1289 / #1248). E3a lands the **registration model + resolution
+ diagnostics** only; forwarding-impl synthesis and codegen are **E3b** (#1291).

## Context

`@derive(X)` resolved against a **closed, hard-coded** set
(`DERIVABLE_TRAITS = [Eq, Ord, Json, Csv, Yaml, Repr, Hash]`, D-0036). The E3
thesis is that a derive is a thin forwarding stub to a library-registered
generic generator (run at compile time over `type_info(T)`), so a new codec is a
library, not a compiler arm. That requires the derivable set to be **open**.

## Decision

### 1. A derivable-trait registry replaces the closed list

The `Checker` holds `derivable: HashMap<String, DerivableEntry>`, where
`DerivableEntry` is either `Builtin` (a D-0036 trait, lowered by the existing
scaffold) or `Library { generator, span }` (a `@derivable`-registered trait). It
is **pre-seeded** with the built-ins at `Checker::new`, so every existing
`@derive(...)` program compiles unchanged, and `@derive(X)` resolution consults
`derivable` instead of the const list.

### 2. `@derivable(Trait)` surface

`@derivable(Trait)` on a **generic free function** registers that function as the
generator for `@derive(Trait)`. It takes exactly one positional trait-name
identifier. Registrations are collected in `collect_types` **pass 1**, so
`@derive(Trait)` resolves regardless of which is declared first (whole-module).

### 3. Diagnostics (`ZAP5057` "invalid @derivable", plus `ZAP5035`)

- Malformed application (no / multiple / keyword / non-identifier argument) and
  wrong target are rejected (`ZAP5057` for arg-shape; `ZAP5047` wrong target,
  reusing the existing target check).
- A non-generic generator → `ZAP5057`.
- **A `@derivable` name that collides with a built-in → `ZAP5057`** (v1: a
  built-in derivable cannot be redefined by a library — revisit when E4 dogfoods
  the built-ins through the registry).
- **Registering the same trait twice → `ZAP5057`.**
- `@derive(X)` naming a trait neither built-in nor registered → `ZAP5035`
  (existing code), message reworded to "not a built-in derivable, and not
  registered with @derivable".

### 4. Resolution is recorded, not yet generated (deferred to E3b)

A `@derive(<library trait>)` **resolves** (sema-clean) and the binding is
surfaced on `Reflector::library_derivables()` — the sema→downstream channel E3b
consumes. No dunder synthesis fires for a library trait (that path stays keyed to
the built-ins), so nothing half-generates. **Lowering** reports a
`@derive(<library trait>)` as *deferred to E3b* (#1291) rather than "unknown
derivable trait", keeping sema and lowering consistent. The library-derivable
names lowering needs are collected from the **pre-monomorphization** module
(mono drops an uninstantiated generic generator).

## Scope deferred

- **Forwarding-impl synthesis + hygiene** → E3b (#1291).
- **Bound propagation** → E3c (#1292). **Derive diagnostics wiring** → E3d (#1293).
- **Overriding a built-in** with a library `@derivable` (the E4 dogfood path).
- **Foreign-type derive** (only a type you own), per HQ1.

## Consequences

No behavior change for existing programs (built-ins pre-registered). New host
tests: `crates/sema/tests/derivable_registry.rs` (16) and
`crates/ir/tests/derivable_defer_lower.rs` (3). A `@derive(<library trait>)` is
not yet buildable end-to-end (E3b), but it fails cleanly with an honest deferred
message rather than a contradiction or a crash.
