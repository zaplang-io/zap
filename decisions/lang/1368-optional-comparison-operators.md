---
title: Optional[T] defines ==/!= (structural) and a total order with None first
status: Locked
issue: 1368
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Optional[T] defines ==/!= (structural) and a total order with None first

**Status:** Locked

## Decision

`Optional[T]` supports the comparison operators directly (not via a user
`__eq__`/`__cmp__` dunder):

- **Equality** `==` / `!=` is **structural**: `None == None` is true;
  `Some(x) == Some(y)` is `x == y`; a `Some` never equals a `None`. `!=` is the
  negation. Requires `T` to support `==`.
- **Ordering** `<` / `<=` / `>` / `>=` is a **total order with `None` before
  every `Some`** (the Rust `Option<T>` convention): `None < Some(_)` for all
  payloads, `None == None` (so `None <= None`), and `Some(x) OP Some(y)` is
  `x OP y`. Requires `T` to support ordering.
- A bare payload on the right (`opt == 7`, `opt < 7`) is compared as
  `opt OP Some(7)` (it is wrapped via the #1265 bare-payload widening).
- `== None` / `!= None` / `is None` / `is not None` are unchanged (they were
  already special-cased on the `None` literal).

The `Some`/`Some` payload comparison recurses into the ordinary operator
lowering, so the payload type's own operator is reused, and a payload type that
does not support the operator produces the same clean diagnostic a direct
comparison would (`'<T>' does not define '__eq__'/'__cmp__'`). This matches the
structural equality `Optional[T]` already exposed as a dict key / set element
(#703/#704); the operator-vs-dict-key inconsistency the issue noted is resolved.

## Divergence from CPython (deliberate)

Python has **no** total order across `None`: `None < 1` raises `TypeError`, and
`==` on `Optional`-typed values works only because `int | None` is a union, not a
distinct type. Zap instead defines a total order with `None` first, matching
Rust's `Option<T>: Ord` and SQL `NULLS FIRST`. The motivation: `Optional[T]` is a
real, first-class type here, and a total order makes a `list[Optional[T]]`
sortable and `Optional` usable as an ordered key without the programmer writing a
`None`-handling comparator. This is a conscious parity break, to be encoded in
the None/Optional conformance sweep (#1213) as an **intentional divergence**, not
a bug. Equality is unaffected (it matches Python's value semantics).

## Rationale / placement

The operators are lowered structurally in IR lowering (`crates/ir/src/lower/
operators.rs`), intercepted in `struct_binop` before the `__eq__`/`__cmp__`
dunder dispatch (an `Optional[T]` is a `Ty::Struct`, so it already routed there
and bailed). `optional_eq` / `optional_lt` / `optional_le` branch on
`gen_opt_is_none` and extract payloads with `gen_opt_payload` only in the
both-`Some` block (so a niche `None`'s absent payload is never read), then
recurse into `binop` for the payload comparison. `>` / `>=` are derived by
swapping operands (`a > b` == `b < a`). No new ZIR `Inst`, no new runtime call;
`T`-equatable / `T`-ordered requirements and their diagnostics fall out of the
recursive payload comparison.

## Consequences

- `==`/`!=`/ordering now work for `Optional[T]` where `T` supports the
  corresponding operator (including `Optional[float]`, which is equatable/ordered
  but not hashable, so it is usable with operators though not as a dict key).
- Nested `Optional[Optional[T]]` compares structurally at each level (the payload
  recursion re-enters this path).
- `None`-first ordering means `sorted(list[Optional[T]])` places all `None`s
  first; sort stability and the convention are part of this decision.

<!-- Fix in crates/ir/src/lower/operators.rs: struct_binop interception +
     optional_binop / optional_eq / optional_lt / optional_le. -->
