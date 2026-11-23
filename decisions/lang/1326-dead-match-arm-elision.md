---
title: A statically-impossible match arm is elided before lowering
status: Accepted
issue: 1326
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# A statically-impossible match arm is elided before lowering

**Status:** Accepted

## Decision

When lowering a `match`, an arm whose pattern can be proven — from the
scrutinee's type alone — never to match at this specialization is **dropped
before its bindings and body are lowered**, exactly as the pre-existing
tuple-shape check already dropped shape-incompatible tuple arms. A single
predicate, `FnLower::pattern_statically_dead(sty, pattern)`, decides it and now
covers every refutable leaf whose match is a pure type question:

- a nominal struct pattern `S(...)` against any type other than `struct S`
  (a scalar, or a different struct);
- a tuple pattern against a non-tuple or wrong-arity scrutinee (the former
  tuple-only check, folded in unchanged);
- a sequence pattern against a non-`list` scrutinee;
- an `as` binding, which is dead exactly when its inner pattern is;
- an or-pattern, which is dead only when **every** alternative is dead.

Anything whose outcome is decided at runtime — literals, captures/wildcards,
enum discriminants, and the nested field/element tests inside a live
pattern — is never reported dead and is lowered as before. The elision is
behaviour-preserving: a statically-false arm never executes at runtime, so
dropping it removes only dead code.

## Rationale

A generic function specialized to a scalar can contain an arm that is dead *at
that specialization* but live at another — e.g. `case Date() as day:` in
`promote[T]` is real for `promote$Date` and impossible for `promote$int64`.
`pattern_cond` already folded such an arm's test to a compile-time-constant
`false`, but lowering still emitted the arm's `then` block: its `as` binding
bound `day` to the `i64` scrutinee, and `datetime.combine(day, …)` then emitted
a `FieldGet` reading `Date`'s fields off that `i64`. The constant-false branch
is unreachable, but the backend lowers every block it is handed and tried to GEP
into a scalar, so inkwell panicked (`Found IntValue … expected PointerValue`).
This surfaced once #1310 unblocked native inference and let these scalar
specializations instantiate (zaplang-io/datetime-utils#22).

Two depths were considered:

- **A compile-time diagnostic** that rejects the instantiation. Rejected: the
  instantiation is *legal*. `promote(1)` is meant to compile and raise
  `TypeError` at runtime through the `case _:` fallback; erroring would break a
  valid, useful program (a generic that accepts several types and rejects the
  rest at runtime).
- **Dead-arm elision** (chosen). It is the general form of a mechanism the
  lowerer already trusted for tuples, it needs no new IR or type machinery, and
  it keeps the invalid field reads out of the IR entirely rather than relying on
  a later pass to prune the dead block. It also preserves the fallback arm and
  the nominal binding for the specializations where the arm *is* live.

Emitting a false branch and leaning on LLVM to delete it was rejected: the
builder faults while *constructing* the GEP, before any dead-code pass runs, so
the invalid instruction must never be built.

## Consequences

- No new ZIR `Inst`/`Elem`/`Expr`/`Ty` variant and no new exhaustive-match arm:
  the fix only *skips* emission, so there is no new GC-rooting (`inst_dest`)
  entry and no feature-gated match to update.
- The special-case tuple-shape check at the top of `lower_match` is replaced by
  the general predicate; tuple behaviour is unchanged (its logic moved verbatim
  into `pattern_statically_dead`).
- A match all of whose arms are statically dead now falls straight through to
  the merge block. For a non-enum scrutinee that is the existing Python no-op
  fall-through; nothing that previously executed stops executing.
- Out of scope / unchanged: enum-variant patterns (`Enum.Variant`) are not
  reported dead here — they lower through discriminant/tag tests, not nominal
  struct field reads, and never hit the crashing shape; sema's exhaustiveness
  and type checking are untouched (this is purely a lowering-time dead-code
  decision).

<!-- Related: the lowering lives in crates/ir/src/lower/enums.rs
     (`lower_match` / `pattern_statically_dead` / `pattern_cond`). Surfaced
     after lang/1309 + the native-inference work in #1310. -->
