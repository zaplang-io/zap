---
title: A function type may appear inside a tuple generic type argument
status: Locked
issue: 1502
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# A function type may appear inside a tuple generic type argument

**Status:** Locked

## Decision

A function type (`func_type`) may be a *tuple-display element* in a generic type
argument: `run[(fn(int64) -> int64, int64)]`, extending #1488 (which admitted a
function type only at the head of a type-argument element, `run[fn(A) -> B]`).

`zap fmt` canonicalizes every tuple type argument to its paren-free spelling, so
`run[(fn(A) -> B, C)]` normalizes to `run[fn(A) -> B, C]` — which already parsed
after #1488, because the paren-free comma-separated form reaches `index_elem`
per element. This decision closes the remaining gap: the *parenthesized*
spelling is no longer a parse error.

**Accepted ambiguity.** `fn` is a soft keyword — a legal identifier outside type
position. In a subscript or tuple-display element, `fn(` is therefore read
*unconditionally* as a function type, so a variable named `fn` cannot be
**called** in that position: `xs[fn(1)]` (#1488) and `(fn(1), 2)` (#1502) are
parse errors when `fn` is a value. This is accepted as a deliberate, narrow
trade-off: a type argument is the overwhelmingly common reason to write `fn(`
there, and naming a value `fn` and calling it inside a subscript/tuple is
pathological. The alternative — a heuristic that tries to tell a function type
from a call of a variable named `fn` — is fragile for the bare `fn(x)` case
(where `x` could be a type or a value) and was rejected.

## Rationale

The subscript/tuple grammar is shared between value indexing / value tuples and
type arguments; sema/mono classify them after parsing. The only form that could
not round-trip through value-expression parsing was a function type (it has no
value-expression spelling), handled for the element head in #1488 via
`Expr::TypeArg`. A function type nested in a parenthesized tuple hit the same
wall one level down, so the parenthesized-tuple parser now applies the same
`fn(`-is-a-type rule to its elements (and only its elements — not shared
list/set display elements, where a type argument never appears). `mono`'s
`expr_to_type` already resolves a `Type::Func` nested inside a `Type::Tuple`, so
no monomorphizer change was needed.

## Consequences

- No new AST variant: reuses `Expr::TypeArg` from #1488. The parser hook is
  confined to tuple-display elements.
- `fmt` renders the tuple's `TypeArg` element through the shared type renderer
  and strips the redundant parentheses, so the canonical spelling is
  `run[fn(A) -> B, C]`.
- The accepted ambiguity means a value named `fn` cannot be called in a
  subscript or tuple-display element; it is usable as an identifier everywhere
  else. Guarded by a grammar-corpus reject case.
- A function type nested inside a *non-tuple* generic (e.g.
  `list[fn(A) -> B]` via the subscript's own grammar) is unaffected by this
  change and out of scope.
