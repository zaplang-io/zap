---
title: Generic argument consistency through an Optional[T] parameter
status: Locked
issue: 1682
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Generic argument consistency through an `Optional[T]` parameter

**Status:** Locked

## Decision

The #1545 generic-argument-consistency rule applies **through an `Optional[T]`
parameter**. After a `Type[T]` argument binds `T`, a value supplied for an
`Optional[T]` parameter — whether a raw payload or an explicitly wrapped
`Optional[U]` — must be compatible with the bound payload type, or it is rejected
with `ZAP4021` before lowering.

The motivating case is the approved Click-style CLI option declaration, whose
library signature needs `Optional[T]` to distinguish an omitted default from an
explicit zero/false/empty-string default:

```zap
def select[T](Type[T] kind, Optional[T] fallback = None) -> Optional[T]:
    return fallback

def main():
    auto accepted = select(int64, fallback="oops")   # error[ZAP4021]
```

The `Type[int64]` argument establishes `T = int64` (#1527), so the `fallback`
payload must be compatible with `int64`; the string `"oops"` is not. Previously
the program compiled and lowered a bare string constant into an `Optional[int64]`
slot — the IR showed `@select$int64(i64, struct#40)` receiving a `strconst`.

## What a value supplied for `Optional[T]` contributes

A raw value supplied for an `Optional[T]` parameter lifts into the payload by
implicit `Some` wrapping (#1265). For the consistency pass it therefore binds and
is checked against `T` exactly as the payload type would be:

- a **raw payload** `fallback=7` binds `T` from the argument type (`int64`'s
  least upper bound with any `Type[T]` binding), and lifts when compatible;
- an **explicitly wrapped** `Optional[U]` argument recurses payload-to-payload
  (`Optional[T]` vs `Optional[U]` ⇒ bind `T` from `U`), so a wrong wrapped
  payload (`Optional[str]` into `Optional[int64]`) is rejected by the same
  payload-aware `Optional`-to-`Optional` rule (#1452);
- **omission** and an **explicit `None`** contribute nothing: `None` carries the
  wildcard `Unknown` payload, which neither binds `T` nor conflicts, so the
  default and explicit absence stay accepted;
- a **dynamic** (`py` / `Unknown`) argument is a wildcard, as everywhere else.

Numeric widening is preserved: an `int32` payload lifts into an `Optional[int64]`
bound, matching the least-upper-bound widening #1545 already applies to a bare
`T` argument.

## Rationale

- **Root cause, not a special case.** `resolve_type_scoped` — which records a
  generic function's *un-erased* parameter types so #1545 can bind type
  parameters — had no `Optional` arm, so an `Optional[T]` parameter fell through
  to the unscoped `resolve_type` and erased its payload to `Ty::Unknown`. That
  severed the payload's connection to the function's type parameters, so the
  consistency pass saw `Optional[Unknown]` and the permissive payload lift (#1265)
  admitted any raw value regardless of the bound `T`. Adding the scoped `Optional`
  arm preserves the payload type variable; the bare-payload lift is then matched
  as a binding of `T`, the same general rule #1545 established — no special case
  for the decorator or the CLI library.
- **Consistent with #1265.** The lift stays permissive in exactly the ways #1265
  intends (a bare `str`/`float`/narrowing-numeric payload still lifts into a
  matching bound); the only values newly rejected are those genuinely
  incompatible with a type parameter already bound by another argument.

## Scope

- **Free functions only (v1)**, as in #1545. Generic *methods* are not yet
  recorded for this pass.
- **Additive.** Before this change the `Optional[T]` payload erased to `Unknown`,
  so the payload lift accepted anything; the new pass only adds rejections for
  genuinely inconsistent calls. No previously-accepted *consistent* program
  changes behaviour.

## Consequences

- `resolve_type_scoped` gains an `Optional` arm, so a generic function's recorded
  parameter types preserve an `Optional[T]` payload's type variable.
- `collect_tyvar_pairs` gains a bare-payload-lift arm: a non-`Optional` argument
  against an `Optional[T]` parameter recurses payload-to-argument, binding `T`.
- `subst_ty` gains `Optional` and `VarTuple` arms. Preserving the payload type
  variable exposed a latent gap in the #1623 generic-return substitution: an
  `-> Optional[T]` return left its payload as the `Ty::Named(T)` type variable
  (previously masked because the payload erased to `Unknown`), so a call such as
  `wrap[T](T x) -> Optional[T]` now substitutes its payload and types the result
  as `Optional[str]` rather than the unsubstituted `Optional[T]`.
- A mismatched `Optional[T]` payload is now `error[ZAP4021]`, located at the
  offending argument, with the message naming the type parameter and its binding.

<!-- Related: #1545 (the general rule this extends), #1265 (bare-payload lift),
#1452 (payload-aware Optional assignability), #1527 (first-class type values),
#564, zaplang-io/cli#4. -->
