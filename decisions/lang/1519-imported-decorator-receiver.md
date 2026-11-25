---
title: A dotted decorator resolves an imported-package function as its receiver
status: Locked
issue: 1519
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# A dotted decorator resolves an imported-package function as its receiver

**Status:** Locked

## Decision

A **dotted** function decorator (`@recv.method`, `@recv.method(args)`) whose
root `recv` is an **imported module** — bound by `import pkg` or `import pkg as
alias` — resolves its receiver to the package's exported **free function**, and
then applies the same return-type-driven rule the local struct-receiver case
already does (mechanism **A**, [#1509](1509-decorator-binds-object.md)):

- the function's declared return type is **non-void** → *rebind* the decorated
  name to the returned object (the Click command-object surface), passing the
  original function (renamed `<name>$impl`) as the call's trailing argument;
- the return type is **void** → *register* — the synthesized call runs for its
  effect and the name stays the raw function (the fastzap `@app.get` surface
  across a package boundary).

```zap
import factory                         # factory.command(fn) -> Command

@factory.command                 ==>   auto greet = __zm…factory__command(greet$impl)
def greet(str name) -> str: ...        greet.run()        # greet is the Command
```

`import factory as f` + `@f.command` binds identically — the alias is the
receiver root the decorator resolves through. A **stacked** imported chain
composes bottom-up into one nested call exactly as the local case does, each
decorator's own arguments kept as its library function's leading arguments:

```zap
@cli.command                           auto greet = __zm…cli__command(
@cli.option("--count")           ==>       __zm…cli__option("--count",
@cli.argument("World")                         __zm…cli__argument("World", greet$impl)))
def greet(str name) -> str: ...
```

## Rationale

[#1509](1509-decorator-binds-object.md) locked mechanism A but, to isolate the
binding mechanism from package imports, resolved only a **local struct
receiver**; its own Consequences recorded imported-module receivers as "not yet
resolved … the documented extension for the package consumer surface," gating
zaplang-io/cli#4. This decision is that extension. It adds **no new surface**:
the rebind/register choice is still the receiver's declared return type, and an
ordinary cross-package call `factory.command(x)` already compiles through the
same installed `path` dependency — only the decorator spelling was missing.

The one subtlety is *where* the receiver is resolved. The module linker
(modlink, D-0081) merges the whole import graph into one module with
mangle-prefixed symbols, rewriting every ordinary `factory.command(x)`
expression to the mangled free function `__zm…factory__command` **and then
consuming the `import` statements**. A decorator name, however, is a plain
string the merge never rewrites, and it reaches the decorator desugar *after*
the imports are gone. So the linker now also hands the desugar the **entry
module's import alias → mangle-prefix map**; the desugar resolves
`alias.method` to `{prefix}method`, reads that function's declared return type
from the already-merged module, and synthesizes a bare-`Name` free-function
call (vs. the `recv.method` attribute call a struct receiver produces). The
synthesized binding is then type-checked by sema like any other, so a genuine
mismatch is a precise error at the binding.

Alternatives considered and rejected:

- **Rewrite the decorator in the linker's merge.** The alias map lives there,
  but the rebind/register decision, the `$impl` rename, and the stacked-chain
  composition are the decorator desugar's job and read the whole merged module;
  splitting them across two passes, or encoding the resolved target back into
  the decorator's flat name string, is more machinery than threading the small
  alias map. The map is the minimal fact the desugar lacks.
- **Re-resolve imports in the desugar.** Duplicates the linker's resolution
  (stdlib vs. dependency vs. local, D-0052) in a second place.

## Consequences

- **No new AST / ZIR variant.** As in #1509, the pass only synthesizes existing
  nodes (`Stmt::Let`, `Expr::Call`/`Attr`/`Name`) and renames a function; the
  imported case differs only in using a bare mangled-function `Name` callee
  instead of a struct-method `Attr` callee. The feature-gated exhaustive
  `Expr`/`Inst` matches (repl, modlink, codegen) and GC-rooting are untouched.
- **The receiver is resolved in the desugar**, from the entry module's import
  alias→prefix map the linker now returns alongside the merged module
  (`modlink::Linked`). A **local struct receiver is tried first**, so a local
  value shadows an import of the same name; a root that is neither a resolvable
  struct receiver nor an import alias stays **unresolvable and keeps the
  registering behavior** (no guess, no crash), so nothing that compiles today
  changes.
- **An imported receiver has no `let` binding to anchor on**, so its spliced
  rebinding lands at the top of `main` (the existing unanchored-receiver
  fallback) — exactly where the decorated name is used — rather than after a
  receiver's binding, as the local case does.
- Resolution uses the **entry module's** alias map. A dotted decorator in an
  *imported* module is not an intended surface (an imported module cannot carry
  the top-level executable binding a rebind splices), and is unchanged.
- **Unblocks zaplang-io/cli#4**: `@cli.command` / `@cli.option` /
  `@cli.argument` from an imported `cli` package now bind and compose.

<!-- Related: the local struct-receiver mechanism and the register/void case in
     decisions/lang/1509-decorator-binds-object.md; the module linker in
     crates/driver/src/modlink.rs; the desugar in
     crates/driver/src/decorator_desugar.rs. -->
