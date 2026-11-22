---
title: type_info(T).kind switch + container recursion (E2)
status: Accepted
issue: 1253
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# `type_info(T).kind` switch + container recursion (E2)

**Status:** Accepted — records the *implemented shape* of issue #1253 (epic
#1249, phase E2 of #1248 / Discussion #1243). Builds on #1250 (evaluator), #1251
(`comptime for`), #1252 (`reflect.field`).

## Context

Containers (`list[T]`/`dict[K,V]`/`T?`/tuple) are handled by **comptime
branching on `type_info(T).kind`** and recursing on the element/key/value type —
*not* by `impl`s (Zap has no `impl`; the nominal-traits RFC was withdrawn, #1248
HQ2). The hard part is going from a container's element **type** to that
element's own `TypeInfo` without the sema `Checker` (mono doesn't depend on it).

## Decision

### The no-sema `Type`/`Ty` → `TypeInfo` resolver (`zapc_comptime::TypeRegistry`)

A `TypeRegistry` is built once from a module's `struct`/`enum` declarations
(`TypeRegistry::from_module`) and resolves:

- `resolve_ty(&Ty)` — a resolved `Ty` (as stored in a container `TypeShape`) to
  its `TypeInfo`: primitives, `list`/`dict`/`tuple` structurally, a user
  `Ty::Named` via the registry, and the erased `Named("Optional")` as a bare
  optional. The mirror of E1's `Checker::reflect_type_info`, registry-backed.
- `resolve_type(&Type)` — a *declared* `Type` (e.g. a field's `decl_ty`), which
  recovers an `Optional[T]`'s element from the declared argument (E1 parity).

Resolution stays **one level** (never flattens the type graph), so a
depth-bounded walk over recursive type shapes terminates.

### Evaluator accessors (#1253)

With a `TypeRegistry` attached (`Evaluator::with_registry`), the evaluator gains:

- Container element accessors on a `TypeInfo` value: `.elem` (list), `.key` /
  `.value` (dict), `.inner` (optional), `.elems` (tuple → a list of nested
  `TypeInfo`) — each resolving to a nested `TypeInfo` so a kind-switch can
  recurse.
- `field.ty` — a field's own comptime type (finishing #1250's deferral),
  resolved from its declared type.
- The seven `TypeKind` constants (`Struct`/`Enum`/`Tuple`/`Primitive`/`List`/
  `Dict`/`Optional`) seeded as comptime values (`seed_kind_constants`, and
  automatically inside every comptime function), so a kind-switch is written
  `if info.kind == List:`. The switch itself composes from `.kind` equality +
  `comptime if` (#1250) + the accessors + recursion — no new "switch" primitive.

### Determinism (#1066)

The dict arm walks **key then value structurally**, never by iterating a map, so
no seed-dependent dict order leaks into output. Only declaration-ordered struct
fields / tuple elements are stable, which they already are.

### Override hook

`reflect.has_override(info, "serialize")` → `bool`, true when the type declares
that method (tracked in the registry). A comptime kind-switch short-circuits to a
user-supplied handler instead of recursing (#1248 resolved decision). This is the
*mechanism*; the full override *policy* (which method, how invoked) is E3.

### Recursion & the cap

The kind-switch recursion is **type-structural** — a depth-bounded walk (and, in
mono, per-instantiation specialization) over strictly-smaller type structure —
**not** comptime user-function recursion, so #1250's no-recursion rule for the
evaluator stands. A recursive type shape (e.g. `struct Node: list[Node] kids`) is
bounded by a fixed depth cap and terminates with a diagnostic, never hangs; the
centralized cap is #1254.

### Mono wiring

`zapc_mono::type_registry(module)` builds the registry from the
post-substitution concrete declarations, which mono attaches to the evaluator.

## Scope deferred (owned here)

- **No `reflect`/`comptime` parser surface** — host-tested over constructed/
  parsed AST like #1250/#1251/#1252. The surface, the `Kind` enum it exposes, and
  the `rewrite_stmt` driving of recursion land with that follow-up.
- An `Optional` nested *inside* a container (`list[int?]`) reflects as a bare
  optional (element `Unknown`) — the one-level `Ty`-erasure limitation inherited
  from E1's model; the listed container tests don't hit it.
- Full override *policy* and the codec that consumes the kind-switch are E3/E6.

## Consequences

No behavior change for existing programs: compiler-internal, host-tested
(`crates/comptime/tests/kind_switch.rs`, `crates/mono/tests/comptime_for.rs`).
This completes the E2 mechanism trio (`comptime for` + typed `reflect.field` +
kind-switch) that E3 `@derive`/serde builds on directly.
