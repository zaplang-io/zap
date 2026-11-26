---
title: Argparse-backed handler adapter synthesis for returning decorators
status: Locked
issue: 1544
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Argparse-backed handler adapter synthesis for returning decorators

**Status:** Locked

## Decision

A library-defined **returning decorator** (the #1509/#1519 surface) can adapt a
decorated handler of **any signature** into one uniform dispatcher, so a package
no longer needs a fixed `fn(...)` parameter that matches a single handler shape.

When a returning decorator's receiver method declares its trailing (handler)
parameter as the **uniform argparse seam** `fn(Namespace) -> R`, the decorator
pre-pass synthesizes, per decorated handler, an adapter

```
def <name>$adapter(Namespace $ns) -> R:
    return <name>$impl($ns.get_int("p0"), $ns.get_flag("p1"), $ns.get_str("p2"), …)
```

that reads **each handler parameter from the parsed `Namespace` by name** with
the type-matched getter and calls the handler **in its declaration order**. The
decorator chain then threads this adapter — a `fn(Namespace) -> R` value — in
place of the raw handler, so the package stores and invokes one uniform callable
regardless of the handler's arity or parameter types. Adaptation is fully
static: there is **no runtime invoke-by-name**.

So the approved Click-style CLI (zaplang-io/cli#4, "backed by core `argparse`")
can declare

```zap
import cli

@cli.command
def greet(int64 count, bool shout, str name) -> str:
    …
```

and dispatch `greet` for any such `greet` through a single `fn(Namespace) -> str`
seam. Before this change the decorator received only the raw callable, renamed
`<name>$impl` (#1509); its declaration name and ordered parameter names/types
were unavailable, and a fixed `fn(str) -> str` parameter handled only one shape.

### Mechanism — A-argparse (of the three the issue reserved)

The issue reserved the adaptation mechanism as a core design decision. The three
candidates were:

- **A-box — a runtime boxed/variant value type** carrying mixed scalars, giving a
  framework-agnostic uniform seam. **Rejected (for now):** Zap has no such type
  (`py` is the CPython-interop handle only, with no typed→`py` boxing, and there
  is no `Variant`/`Any`), so this is a large new runtime surface (new IR value,
  GC, codec) — the E2-sized representation deliberately deferred in #1250/#1257
  and the #1527 minimal-token decision.
- **A-argparse — synthesize an adapter bound to core `argparse` (chosen).** The
  adapter reads from the `argparse` `Namespace` the parser already produces. No
  new runtime value type; it reuses the prelude `Namespace` and its typed
  getters. cli#4 is explicitly backed by core `argparse`, so this is the natural
  seam.
- **C — reflectable callable values** (`reflect_params(handler)` on a fn-value).
  **Rejected:** D-0089 keys reflection to a *named declaration* on purpose;
  making a fn-value reflectable is a runtime value-reflection surface the project
  has avoided.

### The getter protocol

The handler parameter's declared type selects the `Namespace` getter:

| parameter type                     | getter       |
|------------------------------------|--------------|
| `int` / `int64` / `i64`            | `get_int`    |
| `float` / `float64` / `f64`        | `get_float`  |
| `bool`                             | `get_flag`   |
| `str`                              | `get_str`    |

The parameter *name* is the namespace key, so a package's declared option/
argument names (e.g. `@cli.option("--count")` → `count`) must match the handler
parameter names — the by-name binding Click performs, done statically here.

### Scope (v1)

- **Argv-sourced scalar parameters only.** `int`/`float`/`bool`/`str`. A
  parameter of any other type (a struct, a `Context`, a container) **cannot** be
  bound from the namespace and is a precise decoration-site error (`ZAP5048`), not
  a silent miscompile.
- **No explicit `Context` parameter.** cli#4's first slice is a single command
  with typed options and arguments and declares no `Context`. Supplying a
  non-argv `Context`/injected parameter to the adapter is a documented follow-up
  (it needs a source decision analogous to D-0092's `Depends`).
- **The opt-in is the trailing-parameter type `fn(Namespace) -> R`.** A receiver
  method that declares the handler's own fixed signature (e.g. `fn(str) -> str`,
  the #1527 golden) still receives the raw handler, unchanged — full
  back-compatibility; only a method that asks for the uniform seam gets an
  adapter.
- Both **local struct receivers** and **imported-module receivers** (#1519, the
  cli#4 surface) are supported; the trailing-parameter type is read from the
  struct's methods or from the merged, mangled package function's signature.
- **Mapping declared option/argument names to handler parameters, and the
  declaration/handler mismatch diagnostics that depend on it, stay framework-side
  (cli#4).** Baking one framework's decorator protocol into the compiler is out
  of scope (consistent with D-0092 keeping request extraction framework-side).

## Rationale

- **No new runtime surface.** A-argparse reuses the prelude `Namespace` and its
  typed getters, so a handler of any scalar signature dispatches with no boxed
  value type, no new IR node, and no GC/codec work — the representation question
  that blocks a framework-agnostic seam is sidestepped, not forced.
- **Static, in the existing pre-sema desugar.** The adapter is ordinary
  synthesized AST spliced alongside the #1509/#1519 rebind chain
  (`crates/driver/src/decorator_desugar.rs`), reusing the D-0092 `synth_adapter`
  machinery's pattern. Sema type-checks the adapter against the `fn(Namespace) ->
  R` seam, so a getter/type mismatch is a precise error.
- **Opt-in by the seam type.** Keying synthesis to the trailing parameter being
  exactly `fn(Namespace) -> R` makes the feature additive: every existing
  returning decorator (which declares a concrete handler signature) is untouched.

## Consequences

- `crates/driver/src/decorator_desugar.rs`: the returning-decorator rebuild
  (`try_build_rebind`) detects the `fn(Namespace) -> R` trailing parameter on the
  innermost decorator's receiver method and, when present, synthesizes
  `<name>$adapter(Namespace) -> R` (reading each handler parameter by name with
  the type-matched getter, calling the handler in signature order) and threads it
  through the chain instead of the raw handler. An unsupported parameter type is
  `ZAP5048` at the parameter.
- No stdlib function is added; the adapter calls existing `Namespace` getters, so
  `seam_sigs.rs`/`seam_kwargs.rs` are unchanged.
- **Deferred:** a non-argv `Context`/injected parameter in an adapter; an A-box
  runtime value type for a framework-agnostic seam; the declaration/handler
  name-and-type mismatch diagnostics that are framework-specific (cli#4).

<!-- Related: #1509/#1519 (the returning-decorator surface), #1527 (type-value
decorator arguments), I#458/D-0089 (reflection, keyed to a named declaration),
I#466/D-0092 (the registering-decorator adapter synthesis this mirrors); unblocks
zaplang-io/cli#4. -->
