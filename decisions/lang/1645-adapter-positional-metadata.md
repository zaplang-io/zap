---
title: Handler adapter routes positional metadata around a non-last handler parameter
status: Locked
issue: 1645
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Handler adapter routes positional metadata around a non-last handler parameter

**Status:** Locked

## Decision

A returning decorator whose receiver method declares the argparse seam parameter
`fn([C,] Namespace) -> R` (#1544/#1621) **not last** may now carry **positional**
metadata arguments, and they compose correctly: the positional arguments fill the
non-handler parameters, routed *around* the handler's slot, while the synthesized
adapter continues to reach the handler parameter **by name** (#1626).

```zap
def argument(str name, fn(Namespace) handler, str metavar = "") -> Arg: ...

@cli.argument("x", "META")   # => greet = cli.argument("x", metavar="META",
def greet(str v) -> str:     #                         handler=greet$adapter)
    return v
```

The handler is at index 1. The first positional (`"x"`) fills the pre-handler
`name` slot; the second (`"META"`) is bound **by the receiver parameter's name**
(`metavar`), shifted one past the handler, so it never lands on — or double-binds
— the handler slot.

Before this change #1626 kept the decorator's positional arguments verbatim and
appended the adapter as the `handler=` keyword. With a non-last handler and enough
positionals to reach it, a positional landed on the handler's slot and collided
with the by-name adapter (the handler was bound twice).

## Mechanism

When building the innermost decorator's call, the desugar knows the receiver
method's full `(name, type)` parameter list (`ResolvedRecv::params`, #1626) and the
handler parameter's name. It counts the decorator's leading positional arguments;
a routing pass runs **only** when a positional would reach the handler's index
(`positional_count > handler_index`). In that pass each positional **before** the
handler's index keeps its position, and each positional **at or after** it is
emitted as a keyword argument named after `params[pos + 1]` (the receiver
parameter one past the handler), so the handler's slot is skipped. Keyword
metadata binds by its own name, unchanged, and the adapter is appended as the
`handler` keyword as in #1626.

## Scope

- Applies to the single, handler-receiving (innermost) decorator. Stacked
  decorators thread the composed object positionally, unchanged.
- The routing pass is a no-op when no positional reaches the handler, so the
  handler-last case (#1544) and the keyword-only-metadata case (#1626, the
  approved CLI surface's `name=`/`help=`) are **byte-for-byte unchanged**.
- An overflow (more positionals than parameters) is left as a user arity error for
  sema; the desugar preserves the argument count rather than inventing a binding.

## Rationale

- **Order-independent composition.** Binding positionals by the non-handler
  parameter names, skipping the handler, makes the decorator factory robust to the
  handler's position regardless of how many positional metadata arguments precede
  or follow it — the natural generalization of #1626's by-name handler pass.
- **Additive.** Only the new case (positional metadata reaching a non-last handler
  slot) changes; every existing adapter path is unchanged.

## Consequences

- `crates/driver/src/decorator_desugar.rs`: a new `innermost_wrap_args` helper
  builds the innermost decorator's arguments, routing positional metadata around
  the handler index; `try_build_rebind`'s chain loop calls it for the innermost
  seam wrap.
- Resolves the positional-metadata deferral recorded in
  decisions/lang/1626-adapter-metadata-binding.md.

<!-- Related: #1544/#1621 (the adapter and seam), #1626 (by-name handler),
zaplang-io/cli#4, zaplang-io/cli#7. -->
