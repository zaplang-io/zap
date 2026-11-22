---
title: Parameter-aware forward/reflected operator fall-through
status: Accepted
issue: 1200
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Parameter-aware forward/reflected operator fall-through

**Status:** Accepted

Governing issue I#1200 (deferred follow-up of the reflected-operators epic
I#1189). Amends the operator-dispatch rule set by
[`lang/1180-reflected-operators`](1180-reflected-operators.md) — specifically its
**Decision edge** ("the left defines `D` but its parameter does not accept the
RHS") — and the matching paragraph of
[`design/0006-types-and-generics.md`](../../design/0006-types-and-generics.md)
§2.5. RFC: Discussion #1182 (follow-up comment for #1200).

## Decision

For `a OP b` with forward dunder `D` (`__add__` …) and reflected dunder `R`
(`__radd__` …), dispatch resolves **statically** and **parameter-aware**:

1. **Left wins when it accepts the RHS.** If the built-in rules type the
   operator, or the left type is a struct that defines `D` **and `D`'s first
   non-`self` parameter accepts `b`**, use `a.D(b)`. (Unchanged outcome from
   I#1180 for every program whose left forward dunder already accepts its RHS.)
2. **Reflected fall-through.** Otherwise — including when the left defines `D`
   but its parameter does **not** accept `b` — if the right type is a struct
   whose `R`'s first non-`self` parameter accepts `a`, use `b.R(a)` (`self` =
   right operand). The result type is `R`'s declared return type.
3. **Otherwise** `ZAP4071`, naming both the forward (`D` on the left) and
   reflected (`R` on the right) escape hatches.

This **reverses** the I#1180 Decision edge: a left type that defines `D` by name
no longer unconditionally commits the operator to the left. It now commits only
when `D`'s parameter accepts the RHS — matching Python's `left.__add__(rhs)` →
`NotImplemented` → `rhs.__radd__(left)`.

**One forward dunder per name.** A type still defines at most one `D` per
operator (name-keyed, no overload set). "Parameter-aware" means *this one*
forward dunder is selected only if its parameter accepts the RHS — not that the
compiler chooses among several same-named forward dunders.

### Where the accept test lives (the erased-generic crux)

The accept test is `coercible(rhs, D.params[1])` on the left, `coercible(lhs,
R.params[1])` on the right.

- **Concrete left** (a non-generic struct, or a generic left whose forward
  parameter is a concrete type such as `Duration`): the parameter type is fully
  visible in sema, so the whole rule is resolved in sema
  (`check_struct_binop`).
- **Generic left whose forward parameter is a type variable** (`Container[T]`'s
  `__add__(self, T)`): sema erases `Container[int]` → `Named("Container")` and
  `T` → `Unknown`, and `coercible(_, Unknown)` is permissive, so sema **keeps
  the forward commit** and does not fall through. The concrete accept/reject
  decision is made in **lowering**, where the monomorphized signature is
  available (`self.sigs`, keyed by the monomorphized struct name): if the
  monomorphized forward parameter rejects the RHS and a reflected handler
  applies, lowering reroutes to `b.R(a)`; otherwise it is a clean mono-time
  type error.

This two-site split is what makes the full rule sound without a function-type
model in sema: sema only ever falls through on a **provable** concrete mismatch
(monotone — anything sema proves mismatched is also mismatched after
monomorphization), and lowering is authoritative on the monomorphized types.
Lowering's acceptance predicate rejects only a provable mismatch (a
scalar/pointer boundary crossing, or two distinct non-interface struct types);
where it is unsure it keeps the forward commit, so no sound dispatch is ever
rerouted.

## Rationale

- **Closes a latent miscompile, not only an ergonomic gap.** Under the I#1180
  name-level rule, `check_struct_binop` returned the forward dunder's declared
  return type without checking the RHS against the parameter, and lowering's
  representation gate (`arg_repr_compatible`, #1103) is `__getitem__`/
  `__setitem__`-only. So `Duration + Delta` (left `__add__(self, Duration)`, an
  unrelated same-representation `Delta` RHS) **type-checked and lowered to a
  pointer reinterpret** of the `Delta` as a `Duration`, rather than erroring.
  The parameter-aware rule turns that into either a correct reflected call or a
  clean `ZAP4071`.
- **Still purely additive for matching programs.** When the left forward
  dunder's parameter accepts the RHS (every existing single-forward-dunder
  program that compiles today), step 1 selects it exactly as before. The
  fall-through is reached only on a parameter mismatch — paths that previously
  errored or silently reinterpreted.
- **Generics handled by deferral, not by de-erasing sema.** The full Pythonic
  rule for a generic left was deferred in I#1180 because sema has no function-
  type model for erased generics. Rather than rework sema's result typing, the
  decision confines the only hard case (a type-variable forward parameter) to
  lowering, where the monomorphized parameter is concrete.

### Alternatives rejected

- **Keep the I#1180 name-level commit.** Leaves the `Duration + Delta`
  reinterpret in place and blocks the Python `NotImplemented` analogue the issue
  asks for.
- **Forward overload sets** (multiple same-named forward dunders): a much larger
  change (resolver, ambiguity rules, mangling) with no consumer; the single
  forward dunder plus a parameter-aware gate is sufficient.
- **Move all operator result typing into lowering.** Would make the concrete
  case (which sema resolves cleanly today) unnecessarily late and pollute
  downstream inference with a deferred operator result type. The two-site split
  keeps the common case in sema.

## Consequences

- A left type that defines a forward dunder for one RHS shape no longer blocks a
  right operand's reflected dunder for a different RHS shape — e.g. `Duration`
  defining `__add__(self, Duration)` does not prevent `dur + delta` from reaching
  `Delta.__radd__(self, Duration)`.
- `ZAP4071` gains a parameter-mismatch form naming both escape hatches; the
  missing-forward-dunder form (no `D` at all) is unchanged.
- The I#1180 Decision edge ("ordinary type error, not a reflected reroute") and
  the matching sentence in `design/0006` §2.5 are replaced by the rule above.
  `DECISIONS.md` (frozen) is not edited — this ADR is the going-forward record.
- Reflected calls continue to lower through `throwing_call` (invoke), so a
  raising reflected dunder stays catchable by `try/except`.
- Runtime value round-trips for the fall-through are pinned by CI goldens (the
  maintainer's local LLVM is unavailable; host tests cover sema typing and
  mono+lowering dispatch — see `crates/sema/src/lib.rs` tests and
  `crates/ir/tests/reflected_ops_lower.rs`, including the monomorphized
  generic-left reroute).
