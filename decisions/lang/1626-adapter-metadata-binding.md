---
title: Handler adapter reaches its parameter past optional decorator metadata
status: Locked
issue: 1626
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Handler adapter reaches its parameter past optional decorator metadata

**Status:** Locked

## Decision

A returning decorator's handler parameter — the one typed as the argparse seam
`fn([C,] Namespace) -> R` (#1544/#1621) — may appear **anywhere** in the receiver
method's signature, and the synthesized adapter is passed to it **by name**, so a
Click-style decorator factory with optional named metadata works:

```zap
struct Command:
    fn(Namespace) handler
    str name

def command(str name = "", fn(Namespace) handler) -> Command: ...

@cli.command                 # => greet = cli.command(handler=greet$adapter)
@cli.command(name="alias")   # => greet = cli.command(name="alias", handler=greet$adapter)
```

Before this change the adapter was appended as the trailing **positional**
argument and the seam was only recognized on the **last** parameter, so:

- `command(str name="", fn(Namespace) handler)` filled `name` with the handler
  (`ZAP6001: missing required argument 'handler'`), and
- `command(fn(Namespace) handler, str name="")` was not recognized as a seam at
  all (the raw `fn` was passed, `ZAP4021`).

## Mechanism

The desugar now records the receiver method's full parameter list (with `self`
dropped) and scans it for the seam parameter wherever it is (`seam_param`). The
innermost decorator — the one handed the handler — passes the synthesized adapter
as a keyword argument named after that parameter; the decorator's own metadata
arguments bind by default or by their own keywords as usual. A decorator with
**no** seam parameter (a raw handler, e.g. the #1527 fixed `fn(str) -> str`), and
the outer decorators that thread the composed object, keep the trailing
**positional** argument, unchanged.

## Scope

- The by-name pass applies to the handler-receiving (innermost) decorator and to
  a single decorator; stacked decorators compose as before, with the metadata
  decorators threading the composed object positionally.
- The decorator's metadata is expected to be **named/optional** (keyword) when the
  handler parameter is not last. Positional metadata combined with a non-last
  handler parameter is not part of the specified surface (zaplang-io/cli DECISIONS
  uses `name=`/`help=` keyword metadata) and is out of scope.

## Rationale

- **Robust to parameter order.** Passing the adapter by the handler parameter's
  name decouples it from the position of optional metadata, matching the Click
  `@command()` / `@command(name=…)` factory the CLI surface specifies.
- **Minimal and additive.** Only the handler-receiving decorator's argument gains
  a keyword name, and only when a seam is present; every raw-handler and
  composed-object path is byte-for-byte unchanged.

## Consequences

- `crates/driver/src/decorator_desugar.rs`: `ResolvedRecv` carries the method's
  `(name, type)` parameters (not just the trailing type); `seam_param` finds the
  handler parameter anywhere; the innermost wrap passes the adapter as
  `handler_param = <adapter>` by keyword.
- **Deferred:** positional decorator metadata alongside a non-last handler
  parameter.

<!-- Related: #1544/#1621 (the adapter and seam this threads), zaplang-io/cli#4,
zaplang-io/cli#7. -->
