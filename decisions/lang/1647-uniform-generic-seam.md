---
title: Uniform generic seam for repeatable returning decorators
status: Locked
issue: 1647
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Uniform generic seam for repeatable returning decorators

**Status:** Locked

## Decision

A returning decorator's handler parameter may be typed as the receiver method's
**own generic type parameter** (a *uniform generic seam*), in addition to the
literal argparse seam `fn([C,] Namespace) -> R` (#1544/#1621). This is the
compiler support for composing **repeatable** decorators where any layer can be
innermost (option A of #1647 — "a uniform factory input across the whole stack");
no library-specific built-in is introduced.

```zap
struct Command:
    fn(Context, Namespace) -> str run_fn

struct Cli:
    def command[Inner](self, Inner previous) -> Command:   # `previous: Inner` = seam
        return Command(run_fn=previous)

@cli.command
def greet(Context ctx, str who) -> str:                    # => cli.command(previous=greet$adapter)
    return who
```

- When the decorator is **innermost**, the synthesized adapter is bound to the
  generic seam parameter by name (as #1626 does for a literal seam), so the
  decorated handler of any signature is adapted — scalars bind from the
  `Namespace`, `Depends(...)` parameters are injected (#1646).
- When the decorator is an **outer** layer, the previous layer's return value is
  threaded into the generic seam parameter. Because the parameter is generic, both
  the adapter (innermost) and the composed object (outer) type-check, so one
  signature serves every stack position.

## Seam recognition

A parameter is a uniform generic seam when its declared type is a **bare
reference to one of the receiver method's own generic type parameters** — a
`Type::Named` with no type arguments whose name is in the method's `[...]`
generics. `Type[T]` (a type *value*, e.g. an `argument`'s `kind`) and concrete
types are not seams. The literal `fn([C,] Namespace)` seam is unchanged and takes
precedence; a method with neither keeps the raw-handler behavior (#1527).

## Context under a generic seam

A bare generic seam carries no context type `C` (unlike `fn(C, Namespace)`). The
adapter's context is therefore **inferred from the handler**: the handler's first
parameter is the context when it is a non-argv (non-scalar) type **with no
default** (excluding an injected `Depends(...)`/source parameter, which carries a
default) — the same "first parameter is the context" rule #1621 fixes, with the
type taken from the handler rather than the seam. A handler with no such leading
parameter yields a context-free `fn(Namespace)` adapter.

## Rationale

- **No overload needed.** Zap does not dispatch same-name definitions by parameter
  type, so option B (overload resolution) is rejected. A generic parameter is the
  statically-typed way one signature accepts both the adapted callback and the
  composed object.
- **Mechanism, not policy.** The compiler only recognizes the generic seam, adapts
  the innermost handler, and infers the context; how a package composes its own
  object across the stack is ordinary generic code in the library (zaplang-io/cli),
  consistent with "no library-specific built-in is required".
- **Additive.** The literal seam, bottom-up order, explicit context (#1621),
  optional/positional metadata (#1626/#1645) and `Depends` injection (#1646) are
  all unchanged.

## Consequences

- `crates/driver/src/decorator_desugar.rs`: receiver-method generic names are
  captured (`collect_struct_method_generics`, `fn_generics`) and carried on
  `ResolvedRecv`; `seam_param` returns a `SeamInfo { name, kind }` where `kind` is
  `Literal(Option<Type>)` or `Generic`; a generic seam infers its context via
  `infer_handler_context`.
- **Deferred (separate):** exposing the decorated declaration's name to the
  package when `name=` is omitted (#1647 sub-ask B) — it needs its own mechanism
  decision and is tracked in #1665.

<!-- Related: #1544/#1621/#1626/#1645/#1646 (the adapter and its seam), #1204
(generics), zaplang-io/cli#4. -->
