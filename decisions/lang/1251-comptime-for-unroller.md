---
title: comptime for loop unroller wired to mono (E2)
status: Accepted
issue: 1251
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# `comptime for` loop unroller wired to mono (E2)

**Status:** Accepted — records the *implemented shape* of issue #1251 (epic
#1249, phase E2 of #1248 / Discussion #1243). Builds directly on the #1250
comptime evaluator.

## Context

`comptime for f in info.fields:` is a **compile-time-unrolled** loop over a
type's reflected fields (or `.variants`): the body is cloned and specialized once
per field, not executed as a runtime loop over `any` (no boxing). It belongs at
monomorphization — mono already clones per concrete type — but mono does **not**
run the sema `Checker` (#1249: the generator/verification boundary), so the
unroller cannot lean on sema's resolver.

## Decision

### The unroll engine (`zapc_comptime`)

`zapc_comptime::unroll(loop_var, iterable, body, base_env, ev)` evaluates
`iterable` once (must be a comptime `list`, e.g. `info.fields`) and clones the
body once per element with `loop_var` bound to it. In each clone, every
sub-expression that **evaluates** (via the #1250 evaluator) to a
**materializable** value — an `int64`/`bool`/`str`/`char`, or a `list`/`tuple`
of such — is folded to a literal AST node (`materialize`); anything referencing
a runtime binding is left as runtime AST with its comptime operands folded.

- **HQ3 (unroll ≠ forced inline):** the engine folds only comptime *operands*; it
  never inlines a called function. `emit(reflect.field(value, comptime f.index))`
  specializes to `emit(reflect.field(value, 0))` — still an ordinary,
  outlineable **call**. Code size is bounded by field count. This is structural,
  not an optimization choice.
- **HQ9:** a comptime user-function call that reduces to a constant *is* folded
  (e.g. a `rename_all` transform computed at compile time) — that is computing a
  constant, not inlining a runtime call.
- A negative/`INT64_MIN` int materializes as `-<magnitude>` (unary negation of a
  non-negative literal, as the parser produces signed literals); a `char`
  materializes as `char(<codepoint>)` (no char literal surface).

### The no-sema reflector (`zapc_comptime::reflect_ast`)

`type_info_of_struct(&StructDef)` / `type_info_of_enum(&EnumDef)` build the E1
`TypeInfo`/`FieldInfo`/`VariantInfo` records **directly from the concrete AST**
(names, declaration order #1066, attributes, spans), with no sema dependency.
`FieldInfo::ty` is a best-effort primitive classification of the declared type;
`decl_ty` keeps the exact declared type, and a field's full comptime type (the
kind-switch recursion) is #1253 — the evaluator reports `.ty` as out of fragment
until then, so an unresolved element type never silently misleads.

### Mono wiring (`crates/mono`)

Mono depends on `zapc_comptime` and exposes `unroll_struct_fields` /
`unroll_enum_variants`: reflect the **post-substitution** concrete declaration
(so `info.fields` reflects the actual instantiation, #154/#164), bind `info`, and
run the engine. Output is constructed-correct-by-type AST for the existing
lowering to consume; mono does not re-run sema.

## Scope deferred (owned here)

- **No parser `comptime` surface.** Like E1 (#1257) and #1250, this lands the
  engine host-tested over constructed/parsed AST; the `comptime` keyword,
  `comptime for` parsing, `comptime <expr>` (the *forcing* prefix that surfaces a
  reduction error), and the `rewrite_stmt` auto-detection that calls the mono
  entry are the follow-up. GRAMMAR.ebnf/README/STABILITY unchanged.
- **Typed `reflect.field(value, i)`** is #1252; **container recursion / nested
  `comptime for` via the kind-switch** is #1253; those test cases land with that
  work. A comptime hole inside a dict/set/comprehension/lambda body form is not
  folded in this cut (documented in `unroll.rs`).

## Consequences

No behavior change for existing programs: compiler-internal, host-tested
(`crates/comptime/tests/unroll.rs`, `crates/mono/tests/comptime_for.rs`). The
engine is the reusable core; mono's call site auto-invokes it once the surface
lands.
