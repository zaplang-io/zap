---
title: Generic operator dunders dispatch in operator syntax via a mono rewrite
status: Locked
issue: 1227
pr:
date: 2026-11-21
domain: lang
supersedes:
superseded-by:
---

# Generic operator dunders dispatch in operator syntax via a mono rewrite

**Status:** Locked

## Decision

A *generic* operator dunder — one carrying its own type parameters, such as
`def __radd__[T](self, T dt) -> T` or `def __add__[T](self, T other) -> R` — now
dispatches through binary-operator syntax (`a + b`, `a - b`, …), not only
through an explicit `x.__radd__(y)` call. The monomorphizer resolves the D-0047
/ #1180 operator dispatch (the left operand's forward dunder wins when its type
declares one; otherwise the right operand's reflected `__r*__`) and, **when the
resolved dunder is generic**, rewrites the operator expression into the
specialized method call `recv.<dunder>$<targs>(arg)` — exactly the AST an
explicit generic-method call already produces. A *non-generic* dunder is left as
an operator and dispatched unchanged by IR lowering, so every type that worked
before is byte-for-byte unaffected.

## Rationale

Monomorphization runs before IR→ZIR lowering and is the only pass that
instantiates a generic method body. IR lowering's operator path dispatches a
reflected/forward dunder by its *un-instantiated* mangled name
(`Delta____radd__`), which does not exist for a generic dunder — only the
template and any requested `Delta____radd__$T` specialization do. So operator
syntax reported `ZAP6001 "type 'DateTime' does not define '__add__'"` while the
explicit call form compiled, because only the explicit call reached the
monomorphizer's generic-method-call path that infers the type arguments and
requests the instance. The same gap hit a generic *forward* dunder
(`def __add__[T]`), not just reflected ones.

Rewriting the operator in the monomorphizer closes the gap at its source:

- **Mono, not IR lowering, is where it belongs.** IR lowering cannot synthesize
  a new generic method body; mono already owns `infer_fn_targs` /
  `request_method` and the proven generic-method-call rewrite. The operator case
  reuses that machinery verbatim, so the operator form and the explicit-call
  form lower to the identical specialized symbol.
- **Rewrite to the explicit call, don't special-case lowering.** The produced
  AST is exactly what the user could write by hand (`Delta(1).__radd__[T](x)`),
  which the issue confirms already works end to end. No new ZIR, no new lowering
  path, nothing for codegen to learn.
- **Only the generic case is rewritten.** A non-generic dunder already lowers
  correctly through IR lowering's operator dispatch (#1180/#1201). Leaving it an
  operator keeps the `operator_index_call` and reflected-operator goldens
  unchanged and avoids re-routing a working path.
- **Forward-wins precedence is preserved.** A declared forward dunder on the
  left operand suppresses the reflected fallback whether or not it is generic —
  the same order IR lowering applies — so mixing a left forward dunder with a
  right reflected dunder behaves identically to today.

Alternatives rejected:

- **Instantiate on demand during IR lowering.** Lowering runs after mono has
  emitted concrete bodies; it has no way to produce a new specialized body, so
  this cannot work without duplicating the monomorphizer.
- **Rewrite every reflected/forward operator to a call, generic or not.**
  Unnecessary churn that would move working non-generic operators onto a
  different path and perturb existing goldens for no behavioral gain.

## Consequences

- Both operands' types are consulted in the `Expr::Binary` monomorphization arm
  to decide dispatch; this reuses the inferred operand `CTy`s already computed
  there (#1128), so no extra inference work is added on the common path.
- A generic dunder whose own type parameter appears in neither the argument nor
  the return type cannot be inferred from an operator and is rejected with the
  same clean "cannot infer type argument" error the explicit-call form gives.
- The forward/reflected dunder name table is duplicated in the mono crate
  (`op_dunders`) alongside the IR crate's `reflected_op_dunder` and forward
  table; the IR table stays the authority, and any new overloadable operator
  must be added to both. A shared table is a follow-up cleanup.

See also [lang/1199-operator-capability-interfaces](1199-operator-capability-interfaces.md)
and the reflected-dispatch work in #1180 / #1201.
