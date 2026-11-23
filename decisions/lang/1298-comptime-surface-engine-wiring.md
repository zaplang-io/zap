---
title: comptime/reflect surface wired to the E2 engine in mono (S2)
status: Accepted
issue: 1298
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# comptime/reflect surface → E2 engine wiring (S2)

**Status:** Accepted — records the *implemented shape* of issue #1298 (S2 of the
comptime/reflect surface epic #1296). S2 wires the S1 (#1297) parser surface to
the **existing** E2 engine (nothing new in the engine). Builds on S1 and on the
E2 bridge in `crates/mono/src/comptime_for.rs` (#1251/#1252/#1253).

## Decision

### 1. `reflect.type_info(T)` is an evaluator builtin
`crate::eval` resolves `reflect.type_info(<TypeName>)` by reading its argument as
a **type** (not a value) and resolving it via the attached `TypeRegistry` to a
`Value::Type`. This makes `reflect.type_info(T).fields` / `.variants` an evaluable
comptime sequence the loop unroller consumes. (A bare type name is supported;
generic/container spellings in that position are deferred.)

### 2. mono unrolls `comptime for` and desugars `reflect.field`
`Checker::rewrite_block` detects a `Stmt::ComptimeFor { target, iter, body }`
whose `iter` is `reflect.type_info(T).fields` / `.variants` and whose `T` names a
concrete `struct`/`enum` in the post-substitution module, and **splices** the
unrolled, `reflect.field`-desugared statements in its place (a block-level
rewrite, since one node yields many statements). It drives the existing bridge
(`unroll_struct_fields_with_access` / `unroll_enum_variants`). A `comptime for`
whose type does not resolve here (e.g. a still-generic `reflect.type_info(T)`) is
left intact for a later pass / the backstop — mono never panics or mis-splices.

### 3. `comptime <expr>` unwraps at mono
The comptime operands a generator uses (`comptime f.index`, `comptime f.name`)
are folded by the unroller before mono sees the body, so `Expr::Comptime` is
transparent by the time mono rewrites it: mono unwraps it to the rewritten inner,
so the `comptime` marker never reaches lowering.

## Scope deferred (owned here)

- **Generic-`T` in `reflect.type_info(T)`.** v1 resolves a `T` that names a
  concrete declaration; substituting a type *parameter* used as the call argument
  (so a generic `def ser[T](...)` generator unrolls per instantiation) is the
  follow-up — it needs mono's type-parameter substitution threaded into the
  comptime-for resolution.
- **sema acceptance + end-to-end.** sema still reports the fragment as
  not-yet-implemented (`ZAP0002`, from S1), so a real `.zap` program using
  `comptime for` does not yet compile end-to-end: sema must first recognize and
  type the comptime fragment (`reflect.*` comptime values, the `comptime for`
  binding, `comptime <expr>`). That sema work — and the LLVM end-to-end golden —
  is the remaining S2 step, tracked on #1298/#1296. This PR lands the **engine
  wiring**, host-tested over the monomorphized AST (the same discipline #1251
  shipped the bridge under).
- **Standalone `comptime <expr>` folding** outside a loop (`comptime
  len(info.fields)`) — v1 unwraps; folding is a follow-up.

## Consequences

No behavior change for existing programs (no `comptime` → untouched). New host
tests: `crates/mono/tests/comptime_surface_wiring.rs` — `comptime for` over
struct fields unrolls and desugars `reflect.field` to typed field access; over
enum variants unrolls; an unresolvable type is left for the backstop; a plain
program is unchanged. Unblocks E3b (#1291) once sema acceptance lands.
