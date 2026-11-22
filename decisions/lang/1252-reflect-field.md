---
title: Typed reflect.field(value, i) access (E2)
status: Accepted
issue: 1252
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Typed `reflect.field(value, i)` access (E2)

**Status:** Accepted — records the *implemented shape* of issue #1252 (epic
#1249, phase E2 of #1248 / Discussion #1243). Builds on the #1250 evaluator and
the #1251 unroller.

## Context

`reflect.field(value, comptime f.index)` must yield a value of the field's
**statically-known type** (`int`, `str`, a nested struct, `list[T]`, `T?`),
never `any`, with **no boxing** — mirroring the fully-monomorphized/unboxed goal
of generics (#154/#164) and mono-recv (#671). The index is a comptime constant
(produced by #1250, folded by #1251). The hard requirement is that this compile
for all three struct layouts: value-struct (inline fields), heap-struct (pointer
+ offset, identity preserved, no copy), and `@scoped` (#876 region discipline).

## Decision: desugar to ordinary field access

`reflect.field(value, <const>)` resolves to the field at that declaration index
and **desugars to `value.<name>`** — an ordinary `Expr::Attr`. The existing
per-layout field-access lowering already compiles `s.field` for all three
layouts (value-struct inline read via `StructLayout::inline` D-0051; heap-struct
pointer+offset load; `@scoped` arena/region field discipline #876). So the
typed, unboxed, direct field load and the three-layout coverage come from the
existing, already-golden `s.field` path — this phase adds the comptime
index→name resolution, **not a new codegen path** (no new ZIR/LLVM, no golden
risk while the LLVM gate is CI-only).

This is the epic's principle in action: comptime is a restricted subset that
*resolves* to ordinary language constructs, not a second code-emission
mechanism.

### API (`zapc_comptime`)

- `reflect_field(value, index, info, span) -> Result<Expr, CtError>` — the
  resolution: `value.<name>` for the field at declaration `index` of `info`; an
  out-of-range (or negative) index is a compile error **at the comptime site**,
  never a runtime out-of-bounds.
- `desugar_reflect_field(expr, info)` — rewrite every
  `reflect.field(value, <int literal>)` in an expression to its typed access
  (recursing via the shared `walk::map_child_exprs`, now factored out of the
  #1251 unroller so both reuse it).
- `field_decl_type(index, info)` — the field's declared `Type`, for a consumer
  that needs the static type alongside the access.

### Mono wiring (`crates/mono`)

- `reflect_field_access(def, value, index)` — resolve over a concrete struct.
- `unroll_struct_fields_with_access(def, loop_var, body, funcs)` — the combined
  #1251 + #1252 pipeline a derive codec drives: unroll `comptime for f in
  info.fields:` then desugar each emitted `reflect.field(value, f.index)` to
  `value.<name>`.

## Scope deferred (owned here)

- **Read access only (v1).** Field *write* is out of scope until a codec
  consumer (E4/E6) needs it — a follow-up, per the issue.
- **No `reflect` parser surface / sema typing yet** — like #1250/#1251, this is
  the host-tested resolution primitive the deferred surface and derive codecs
  call. The statement-position auto-wiring (full `rewrite_stmt` integration)
  lands with that surface; the mono entry here desugars the expression forms a
  comptime-for body emits.
- **Nested `reflect.field` across *different* types** (a field that is itself a
  struct, recursing) is the container kind-switch, #1253.

## Consequences

No behavior change for existing programs: compiler-internal, host-tested
(`crates/comptime/tests/reflect_field.rs`, `crates/mono/tests/comptime_for.rs`),
and the field load reuses the existing per-layout lowering.
