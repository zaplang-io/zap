---
title: Omitted decorator name defaults to the decorated declaration's name
status: Locked
issue: 1665
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Omitted decorator name defaults to the decorated declaration's name

**Status:** Locked

## Decision

A returning decorator's **outermost** layer, when its receiver method declares a
`str name` parameter that the decorator call does **not** bind, has that parameter
defaulted to the **decorated declaration's name** — Click-style `@command`
naming. So a package can recover the decl name when the author omits `name=`
(#1647's consumer expects `greet.name == "greet"`):

```zap
struct Cli:
    def command(self, str name = "", fn(Namespace) -> str handler) -> Command:
        return Command(name=name, run_fn=handler)

@cli.command                 # => greet = cli.command(handler=greet$adapter, name="greet")
def greet(str who) -> str:
    return who
```

## Mechanism (a) — chosen

The desugar fills the name from the declaration (`fill_decl_name` in
`crates/driver/src/decorator_desugar.rs`), appending `name = "<decl>"` as a
**keyword** argument to the outermost decorator's call. It fires only when:

1. the receiver method declares a parameter literally named `name` of type `str`;
2. the call binds it **neither** by the `name=` keyword **nor** by a leading
   positional reaching its slot (an explicit name always wins); and
3. the decorator is the **outermost** (the last wrap built) — an inner
   `@option`/`@argument` names itself and is never auto-filled.

The marker (b) and adapter-dunder (c) alternatives were rejected: they add a new
magic identifier / fn-value reflection for no gain, whereas mechanism (a) is
desugar-only and matches the Click surface the CLI is modeled on.

## Note

The issue's "callback is `unnamed$adapter`" observation does **not** reproduce:
the synthesized adapter is always `<decl>$adapter` (`greet$adapter`), so there was
no adapter-naming bug — only the missing decl-name default, which this fixes.

## Consequences

- `crates/driver/src/decorator_desugar.rs`: `try_build_rebind`'s chain loop calls
  `fill_decl_name` on the outermost wrap.
- A decorator receiver with no `str name` parameter, or a call that supplies the
  name, is unchanged. Resolves the separable decl-name sub-ask of #1647.

<!-- Related: #1647 (the composition model this split from), #1544/#1621/#1626,
zaplang-io/cli#4. -->
