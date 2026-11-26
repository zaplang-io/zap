---
title: Explicit typed context through the argparse-backed handler adapter
status: Locked
issue: 1621
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Explicit typed context through the argparse-backed handler adapter

**Status:** Locked

## Decision

The argparse-backed handler adapter (#1544) is extended so a decorated handler
may declare an **explicit typed context as its first parameter**, which the
dispatcher supplies directly rather than reading from argv. This is the capability
the approved CLI contract selected for its first slice (zaplang-io/cli#7: "a
handler may declare `cli.Context ctx` as its first parameter; the dispatcher
supplies it directly").

A returning decorator opts in through the trailing-parameter seam on its receiver
method, now generalized from `fn(Namespace) -> R` to admit one leading context
parameter:

- `fn(Namespace) -> R` — no context (unchanged, #1544).
- `fn(C, Namespace) -> R` — an explicit typed context `C`.

Under a `fn(C, Namespace)` seam the synthesized adapter takes `(C $ctx,
Namespace $ns)`. The dispatcher passes the context value as `$ctx`; the adapter
forwards it to the handler's context parameter and reads every other parameter
from the namespace by name with the type-matched getter, calling the handler in
its own signature order:

```zap
@cli.command
def greet(Context ctx, int64 count, str name) -> str:
    ...
# synthesizes:
def greet$adapter(Context $ctx, Namespace $ns) -> str:
    return greet$impl($ctx, $ns.get_int("count"), $ns.get_str("name"))
```

## Which parameter is the context

The context is the handler's **first** parameter, and only the first. Under a
context seam, the first parameter is treated as the context when its type is
**not an argv-sourced scalar** (`namespace_getter_for` returns `None` — i.e. not
`int`/`float`/`bool`/`str`); it then receives `$ctx`. Every other parameter must
be an argv scalar bound from the namespace; a non-scalar parameter anywhere else
(or a non-scalar first parameter under a plain `fn(Namespace)` seam) is the same
precise decoration-site error as #1544 (`ZAP5048`).

The adapter's context parameter carries the seam's declared context type `C`
verbatim, so sema checks the handler's context parameter against `C` (a mismatch
is an ordinary type error at the synthesized call).

### The context is optional on the handler

A `fn(C, Namespace)` seam still adapts a handler that declares **no** context: the
adapter keeps the uniform `(C, Namespace)` shape, binds every parameter from the
namespace, and leaves the context argument unused. So one seam serves both
context-taking and context-free handlers, and the package stores a single
`fn(C, Namespace) -> R`.

## Rationale

- **Matches the pinned CLI surface.** cli#7 fixes the context as the handler's
  first parameter supplied by the dispatcher; the `fn(C, Namespace)` seam is the
  smallest generalization of #1544's `fn(Namespace)` that expresses it, and it
  keeps the adapter a statically-typed call (no runtime invoke-by-name).
- **No new runtime surface.** `C` is the package's own struct; the compiler copies
  its type from the seam and passes the dispatcher-supplied value through. Nothing
  about the context is materialized or reflected at run time.
- **Non-scalar-first-parameter detection, not type-name matching.** Deciding the
  context by "first parameter that is not an argv scalar" avoids resolving a
  package type name across the module boundary (the imported-receiver case), while
  still letting sema validate the context type at the synthesized call.
- **Additive and back-compatible.** A plain `fn(Namespace)` seam is unchanged, and
  a non-context handler under a context seam behaves as before for its scalars.

## Consequences

- `crates/driver/src/decorator_desugar.rs`: `is_namespace_seam` becomes
  `namespace_seam`, returning whether the trailing type is a seam and, if so, the
  optional leading context type. `synth_namespace_adapter` takes that context
  type, prepends a `$ctx` parameter of that type, and forwards it to the handler's
  first (non-scalar) parameter.
- Resolves the explicit-context deferral recorded in
  decisions/lang/1544-handler-adapter-synthesis.md.
- **Deferred (unchanged):** general `Depends`-style injected (non-context)
  parameters beyond a single leading context; wider scalar parameter types
  (#1625); a framework-agnostic boxed-value seam (A-box).

<!-- Related: #1544 (the adapter this extends), zaplang-io/cli#7 (the pinned CLI
surface) and zaplang-io/cli#4. -->
