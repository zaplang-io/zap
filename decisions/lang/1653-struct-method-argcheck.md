---
title: Type-check struct instance-method call arguments
status: Locked
issue: 1653
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Type-check struct instance-method call arguments

**Status:** Locked

## Decision

A struct **instance-method** call `recv.method(args)` type-checks its arguments:
each positional argument must be `coercible` to its declared parameter type and
each keyword argument to its named parameter. A mismatch is `ZAP4021` (keyword:
`ZAP4022`) before lowering. This is the struct-side parity counterpart of #1642,
which added the same checking to enum instance methods.

```zap
struct S:
    def plain(self, int64 n) -> int64:
        return n

def main():
    S s = S()
    s.plain("not an int")   # error[ZAP4021]: argument 1 has type str, expected int64
```

Previously the struct instance-method path ran **only** the generic
type-parameter consistency pass (#1622) and otherwise inferred arguments loosely,
so a plain (non-generic) argument mismatch was silently accepted — the asymmetry
recorded as a follow-up in `decisions/lang/1642-enum-method-argcheck.md`, where
the equivalent enum method call already rejected it. Struct and enum instance
methods now type-check their arguments identically.

## Mechanism

In the struct instance-method call path (`crates/sema/src/lib.rs`, the
`self.struct_info(&bt).and_then(|i| i.methods.get(name))` block), after the
associated-function (`ZAP4085`) and `@deprecated` guards, the implicit `self` is
`sig.params[0]` (a receiverless `def` has already returned as an associated
function, so the remaining methods have a receiver — named `self` or the
empty-named synthesized receiver of a `@derive`d dunder). So:

- a positional argument `i` is checked `coercible` against `sig.params[i + 1]`;
- a keyword argument is checked against its named parameter via `check_keyword_arg`.

Generic consistency then runs as before, keyed `"Type.method"`. This mirrors the
enum branch added in #1642 exactly, keeping the two receiver paths in step.

## Scope — what is deliberately unchanged

- **Arity stays loose.** An extra or missing argument is not flagged here, and the
  variadic / `**kwargs` collector parameter is never treated as a fixed
  positional — matching the other receiver paths and the enum branch.
- **The return type stays loose (`sig.ret`).** This path feeds the method-chain
  operator typing that drives `&`/`|`/`~` dispatch off the returned struct (the
  fluent condition-builder shape, operators #52). Per-argument checking does not
  change the returned type, so that dispatch and chaining are preserved. Binding a
  generic method's return type is the separate change #1643.
- **Only instance-method argument *types*.** No new keyword-only / positional-only
  placement diagnostics beyond what `check_keyword_arg` already raises.

## Consequences

- `crates/sema/src/lib.rs`: the struct instance-method call path gains a
  per-argument `coercible` check (`ZAP4021`) and keyword check
  (`check_keyword_arg`), alongside the existing generic-consistency pass.
- A struct instance-method call with a mismatched argument is now `ZAP4021` /
  `ZAP4022`, located at the offending argument; a consistent call, numeric
  widening, and an operator-overloading method chain still compile; arity stays
  loose.
- Struct and enum instance-method argument checking are now symmetric, closing the
  follow-up #1642's ADR recorded.

<!-- Related: #1642 (enum instance-method argument type-checking, the symmetric
sibling), #1622 (generic method consistency), #1623 / #1643 (generic method
return binding). -->
