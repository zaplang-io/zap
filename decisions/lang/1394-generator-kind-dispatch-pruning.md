---
title: "A derive generator's top-level comptime `if` is pruned, enabling kind dispatch"
status: Accepted
issue: 1394
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Generator top-level comptime-`if` pruning (#1394)

**Status:** Accepted — the enabler for a real `@derive` codec and nested
deriving (epic #1371), unblocking #1375. A `@derivable` generator can now
dispatch on `reflect.type_info(T).kind` at its **top level**, so one generator
(one registered per trait) handles every kind the trait is derived on.

## Problem

A trait derived on both a struct and an enum registers **one** generator, which
must branch on `T`'s kind:

```zap
@derivable(Sum)
def _derive_sum[T](T value) -> int64:
    if reflect.type_info(T).kind.name == "enum":
        match value:
            case comptime v in reflect.type_info(T).variants as payload: …
        return 0
    else:
        int64 acc = 0
        comptime for f in reflect.type_info(T).fields: …
        return acc
```

Comptime-`if` pruning (#1330) only ran **inside a `comptime for` body** (the
specializer path). A generator's top-level `if reflect.type_info(T).kind…:` was
rewritten as an ordinary runtime `if`, so `reflect` had no runtime value
(`unknown variable 'reflect'`) and the enum `match` in the dead branch reached
mono for a struct `T` (`a comptime-variant match requires 'P' to be an enum`).
Structs alone never hit this — their whole body is a top-level `comptime for`,
which already specializes.

## Decision

Two pieces:

1. **Evaluator type-parameter awareness.** `Evaluator` gains a `type_subst`
   (parameter → concrete type name). `reflect.type_info(T)` resolves `T` through
   it, so a `reflect.type_info(T).kind`-based condition is comptime-evaluable
   inside a specialized generic generator. (The `comptime for` path still
   resolves `T` in mono and binds `info`, so it leaves `type_subst` empty.)

2. **Prune top-level comptime-constant `if`s during specialization.** When mono
   rewrites a block **under an active substitution** (specializing a generic —
   i.e. potentially a generator), an `if` whose condition chain is a
   comptime-constant bool is pruned to its taken branch; the dead branches (and
   their kind-specific constructs) never reach lowering. The pruned branch is
   re-run through the block rewrite, so any `comptime for` / `comptime if` inside
   it unrolls/prunes too. A non-comptime condition stays an ordinary runtime
   `if`, and ordinary (non-generic) code is untouched (the gate is a non-empty
   substitution; the evaluator only folds genuinely comptime conditions).

A supporting fix (same reflect-desugar area): `desugar_stmt_fields` now desugars
`reflect.field` in assignment statements (`acc = acc + reflect.field(…)`,
`+=`, chained) and recurses into runtime `if`/`while`/`for` bodies — previously
only `Expr`/`Return`/`Let` were covered, so a codec that **accumulates** over
fields left a stray `reflect.field`. Needed for any fold-style codec.

## Consequence

One generator derives a trait on both structs and enums, and recursion where an
enum variant payload is a derived struct works (the payload's own derived method
is called through the shared generator). Host-verified: a kind-dispatch `Sum`
specializes to both `_derive_sum$Pair` (struct fold) and `_derive_sum$E` (variant
match) from one generator. Regression-guarded by
`crates/driver/tests/zap_cli.rs::a_derive_generator_dispatches_on_kind_across_struct_and_enum`
and runtime goldens `tests/golden/enum_derive_kindswitch.zap` /
`enum_derive_nested.zap`. The exhaustive breadth + a named codec + the E3d
enum-variant diagnostic frame are #1375, now unblocked.
