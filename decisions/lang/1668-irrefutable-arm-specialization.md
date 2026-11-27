---
title: Elide match arms made unreachable by a preceding irrefutable arm
status: Locked
issue: 1668
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Elide match arms made unreachable by a preceding irrefutable arm

**Status:** Locked

## Decision

In a `match`, an arm whose pattern is **statically irrefutable** for the
scrutinee's type — and that carries **no guard** — is a catch-all: every later
arm is unreachable at that specialization and is dropped before its body is
lowered. This is the type-directed match pruning #1668 asked for, and the exact
dual of the statically-*dead* arm elision of #1326: there the arm can never
match; here a preceding arm always matches.

It lets an ordinary generic distinguish an already-composed object from a
callable by matching on its own type parameter, with one body serving every
specialization:

```zap
def build[Inner](Inner previous, str label) -> Command:
    match previous:
        case Command() as composed:   # irrefutable for Inner = Command
            return composed
        case _:                       # dead for Inner = Command; kept otherwise
            fn(Context, Namespace) handler = previous
            return Command(name=label, handler=handler)
```

- For `Inner = Command` the struct arm always matches, so the `case _:` arm is
  unreachable and is **not lowered** — its `fn(..) handler = previous` body
  (which cannot coerce a `Command` to a function) never reaches
  `check_fn_coercion`, where it previously failed with `ZAP4001: value of type
  Command is not a function, but fn(Context, Namespace) -> void is expected`.
- For `Inner = fn(Context, Namespace)` the struct arm is statically dead (#1326)
  and the `_` arm is kept and builds the command.

## Irrefutable patterns

A pattern is irrefutable for scrutinee type `sty` when it is:

- a wildcard `_` or a bare capture;
- `<pat> as name` whose inner pattern is irrefutable;
- an or-pattern **any** of whose alternatives is irrefutable;
- a nominal struct pattern `S(...)` whose path resolves to **exactly** `sty`'s
  struct, with every field sub-pattern itself irrefutable (a bare `S()` or
  `S(field=capture)` trivially so; a refutable field test such as `S(n=0)` is
  not); or
- a tuple pattern against a tuple struct of matching arity, no `*rest` hole, with
  every element sub-pattern irrefutable.

A literal, a sequence pattern, an **enum-variant** pattern (`E.Variant` — a sum
type has other variants), a guarded arm, and anything runtime-decidable are
**not** irrefutable, so their following arms stay live.

## The null-reference edge

A nominal struct pattern on a **reference** struct lowers with a defensive
non-null test (it guards the arm's field reads). This decision treats
`case S()` against an exactly-`S` scrutinee as irrefutable anyway: a null
non-`Optional` reference is a `NullReferenceError`-class fault (#1489), not a
match miss that a trailing `_` arm is expected to catch. A null scrutinee that
reaches such a match therefore flows to the match's unreachable merge rather than
a dropped `_` arm — consistent with the language's null-safety direction, where a
non-`Optional` reference is never a valid null. (A `value struct` has no null
form, so the question does not arise.)

## Rationale

- **Reachability, not type magic.** The compiler does not overload on parameter
  type (rejected in #1647); recognizing that an unconditional match makes later
  arms unreachable is ordinary dead-code elimination, so a library composes its
  own object in plain generic code — "no library-specific built-in required".
- **Behaviour-preserving.** An arm after an unconditional match never runs, so
  dropping it cannot change the output of any program that previously compiled;
  it only stops the compiler lowering a body that was reachable solely in a
  *different* specialization.
- **Lowering, not sema.** The predicate needs the specialized scrutinee type and
  the struct/value-struct layout, both of which exist at IR lowering (after
  monomorphization), where the sibling #1326 elision already lives.

## Consequences

- `crates/ir/src/lower/enums.rs`: `lower_match` computes
  `pattern_statically_irrefutable(sty, pattern)` for each unguarded arm and
  `break`s after lowering the first irrefutable one; the new predicate and its
  `struct_field_patterns_irrefutable` helper sit beside `pattern_statically_dead`.
- Independent of inferring the generic argument from the callable itself (#1669)
  and of the omitted-decorator-name follow-up (#1665). Unblocks the normalization
  step of zaplang-io/cli#4.

<!-- Related: lang/1647-uniform-generic-seam.md, #1326 (statically-dead arm
elision — the dual), #1489 (null-reference faults), #1669, zaplang-io/cli#4. -->
