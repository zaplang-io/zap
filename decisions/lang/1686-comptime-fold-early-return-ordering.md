---
title: "A pruned comptime-`if` drops statements made unreachable by an early return"
status: Accepted
issue: 1686
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Comptime-`if` folding preserves early-return ordering (#1686)

**Status:** Accepted — a correctness fix to the comptime-`if` pruning introduced
for kind dispatch (#1394). Blocks typed visible prompts in zaplang-io/cli#11.

## Problem

When mono specializes a generic function, a top-level `if` whose condition chain
is a comptime constant is pruned to its taken branch (#1394): the branch's
statements are spliced into the enclosing block in place of the `if`, and
rewriting then continues with the statements that followed the `if`.

That splice ignored control flow. In the idiomatic "check the type, else fall
through to a fallback" shape:

```zap
def resolve[T](Type[T] kind, T value) -> T:
    if reflect.type_info(T).name == "str":
        return value
    raise ValueError("unsupported")
```

the supported specialization `resolve$str` folded the `if` to `return value` and
then **also** spliced the trailing `raise`, so the body became

```text
return value
raise ValueError("unsupported")   # textually after the `if` — unreachable!
```

The `raise` ran only on the original fall-through path (when the `if` was *not*
taken), but after folding the condition to `true` it was lowered straight into
the taken path. `zap ir` emitted the `raise` ahead of the `ret`, and the
supported path raised a `UsageError` fallback instead of returning its value
(observed in CI on zaplang-io/cli#18, run `45966411241`). The same hazard
applied to any effectful statement after the early return, not only a `raise`.

## Decision

A pruned `if` stood in for a conditional: the statements after it ran **only
when control fell through** the (now folded-in) branch. So after splicing the
taken branch's rewritten statements, mono asks whether that branch can fall
through:

- **It cannot** (it unconditionally `return`s / `raise`s — or ends in a loop
  `break`/`continue`, or a total `if`/`else` whose every arm diverges): the
  statements after the original `if` are statically unreachable in this
  specialization and are **not** spliced in.
- **It can** (it ends in an ordinary statement, or — the false specialization —
  it is empty): the trailing statements are kept and execute as before.

Divergence is a deliberately conservative, purely syntactic over-approximation
(`block_diverges` / `stmt_diverges` in `crates/mono/src/lib.rs`): any shape it
does not recognise as diverging (an exhaustive `match`, a `with` whose body
diverges, …) reports "falls through", which keeps the trailing statements — the
pre-#1686 behaviour — and is always sound (it never drops a statement that might
run). It only ever *removes* code that cannot be reached.

The gate is unchanged from #1394: pruning (and therefore this dropping) happens
only under an active type substitution — i.e. while specializing a generic. A
non-comptime condition stays a runtime `if`, and ordinary non-generic code is
untouched.

## Consequence

The supported specialization returns cleanly; the unsupported specialization
still reaches its fallback `raise`. For `resolve$str`, `zap ir` now emits exactly
`ret %value` with no `zap_exception_new` / `zap_raise`. Regression-guarded by
mono unit/integration tests (`crates/mono/tests/comptime_surface_wiring.rs`:
early-return-then-raise, early-return-then-side-effect, the false specialization
reaching the fallback, a non-diverging folded branch keeping its tail, a total
diverging `if`/`else`, and runtime/non-generic negative controls; plus
`block_diverges` unit tests in `crates/mono/src/lib.rs`) and the runtime golden
`tests/golden/comptime_early_return_fold.zap`. Unblocks typed visible prompts in
zaplang-io/cli#11.
