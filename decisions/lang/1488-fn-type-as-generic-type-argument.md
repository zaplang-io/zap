---
title: A function type may be written as an explicit generic type argument
status: Locked
issue: 1488
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# A function type may be written as an explicit generic type argument

**Status:** Locked

## Decision

A function type (`func_type`, `fn(A, …) -> R`) may appear as a generic type
argument in a subscript: `run[fn(int64) -> int64](...)`, for both a free
function `g[…](…)` and a method `recv.m[…](…)`. The grammar's `index_elem`
(the comma-separated contents of a `[...]` subscript) now admits a `func_type`
directly, alongside the existing `expr` and slice-spec forms.

It is admitted only at the *top* of an index element, where `fn(` is
unambiguous — it can never begin a value index expression. A function type
*nested inside a tuple* type argument (`run[(fn(int64) -> int64, int64)]`)
remains unsupported, because the tuple there is parsed as a parenthesized value
expression and a function type is not a value expression; that shape is left as
a follow-up.

## Rationale

Every other type argument — a name, a generic instantiation (`list[int64]`), a
tuple (`(A, B)`) — already round-trips through ordinary expression parsing, so
the monomorphizer reconstructs the written type from the parsed value expression
(`expr_to_type`). A function type has no value-expression form, so
`run[fn(A) -> B]` failed to parse (`ZAP2001: expected ']'`). Nor could inference
supply it: a bare function name or `lambda` does not pin a generic type
parameter, so a function value could reach a generic `T` only by first binding
it to an `fn`-typed variable — a non-obvious workaround for a type the grammar
already spells everywhere else (annotations, aliases, fields).

Two alternatives were rejected:

- **Parse the subscript contents as types, not value expressions.** This is the
  "correct" long-term shape but a large, invasive change: the whole subscript
  grammar is deliberately shared between value indexing (`a[i]`, `m[:, j]`) and
  type arguments, with sema/mono classifying them after the fact. Reworking that
  split is out of proportion to this gap.
- **Infer `T` from a bare function name / lambda.** A lambda has no type without
  a target, and a bare function name is intentionally not pinned either; forcing
  inference here would change unrelated inference behaviour.

Instead, the parser recognizes the one form that cannot otherwise parse (`fn(`
at the head of an index element), parses it with the existing `parse_type`, and
carries it as a new leaf expression `Expr::TypeArg { ty, span }` — a type
written where an expression is syntactically expected. `mono`'s `expr_to_type`
unwraps it to its `Type`; it is never a runtime value (inference yields
`Unknown`, lowering never sees it because the explicit-type-argument paths
consume it first).

## Consequences

- New `ast::Expr::TypeArg` variant. It carries no sub-expressions, so the
  exhaustive-`Expr` visitors across the front/middle end (span accessors,
  free-name/scope walks, comptime walk/eval, fmt render, mono/sema rewrites,
  the modlink and decorator rewriters, and the LLVM-gated REPL span helper) gain
  a trivial leaf arm; codegen operates on ZIR and never matches `ast::Expr`.
- `fmt` renders it through the shared type renderer (`type_str`), so
  `run[fn(int64) -> int64](g)` re-emits byte-for-byte.
- No new ZIR `Inst`/`Elem`/`Ty` variant, no GC-rooting entry, no raising runtime
  call: the type argument is resolved entirely in monomorphization and never
  reaches lowering as a value.
- The `fn`-typed-binding workaround keeps working unchanged; this only adds the
  direct spelling.
