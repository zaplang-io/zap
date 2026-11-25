---
title: A returning dotted decorator binds the decorated name to its object
status: Locked
issue: 1509
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# A returning dotted decorator binds the decorated name to its object

**Status:** Locked

## Decision

A **dotted** function decorator (`@recv.method`, `@recv.method(args)`) whose
receiver method has a **non-void declared return type** *rebinds* the decorated
name to the object that method returns — the Click command-object surface. The
original function is kept, renamed `<name>$impl`, and handed to the method as a
typed `fn(...)` callable; the decorated name is bound by splicing
`auto <name> = recv.method(args..., <name>$impl)` right after the receiver's
binding (reusing the registering-decorator splice). After

```zap
struct Cli:
    def command(self, fn(str) -> str handler) -> Command: return Command()

@cli.command
def greet(str name) -> str: return "Hello " + name

def main():
    auto cli = Cli()
    greet.run()            # greet is now the Command; dispatches to Command.run
```

`greet` is the `Command`, so `greet.run()` type-checks and dispatches, where
today it reports `ZAP6001: method call on a non-struct value`.

The **void** case is unchanged: a dotted decorator whose method returns nothing
(e.g. fastzap `@app.get(...)`) keeps *registering* — the synthesized
`recv.method(args..., fn)` runs for its effect and the name stays the raw
function (I#457/D-0087). The return type is thus the single, explicit signal
that chooses between the two forms (mechanism **A**, return-type-driven).

**Stacking** composes bottom-up into one nested call: the decorator closest to
`def` is innermost, each outer decorator receives the inner one's returned
object as its trailing argument, and the name binds to the outermost result.
The stacked `@option`/`@argument` arguments are retained verbatim as those
methods' call arguments, so a later statically-typed dispatcher can read the
option/argument metadata and the command object can bind parsed values and
supply an explicit `cli.Context` when requested:

```zap
@cli.command                  auto greet = cli.command(
@cli.option("--count")   ==>      cli.option("--count",
@cli.argument("name")                 cli.argument("name", greet$impl)))
def greet(...): ...
```

The decision is made in the existing pre-sema desugar
(`crates/driver/src/decorator_desugar.rs`), which runs on the already-linked
module, by reading the method's declared return type from the AST; the
synthesized binding is then type-checked by sema like any other, so a genuine
mismatch (the method does not accept the function's type, the chain's types do
not line up) is a precise error at the binding.

## Rationale

Zap already had two decorator semantics and #1509 needs a third. A **bare**
declared `decorator` (#651/#652) rebinds the name but only to a *same-signature*
wrapper — it cannot yield a differently-typed `Command`. A **dotted** decorator
(D-0087) registers and discards the return. Neither exposes a returned object
under the decorated name.

The issue left "the particular syntax and compiler design" a core decision. The
maintainer chose **A (return-type-driven)** over the alternatives:

- **B — an explicit library-side marker** (a built-in annotation on the method,
  or a compiler-known `Decorator` return-type protocol). Rejected: it adds a new
  surface (keyword/protocol) to say what the return type already says. A
  value-returning decorator *is* the explicit opt-in, matching Python/Click,
  where a decorator returns the new binding; Zap's void registering form is the
  deviation, not this.
- **C — a marker at the use site** (on the decorated `def`). Rejected: noisiest,
  least Click-like, and redundant with the method signature.

Return-type-driven adds **no new syntax**, is Python/Click-faithful
(AGENTS.md: match the popular library), and keeps every existing registering
program working because a void method still registers. The one cost is that a
*registering* method that is later given a non-void return type would flip to
rebinding; that is acceptable (and visible) because a registering method
returning a value it intends to discard is already unusual, and the flip is a
type-checked binding, not a silent miscompile.

## Consequences

- **No new AST / ZIR variant.** The pass only synthesizes existing nodes
  (`Stmt::Let`, `Expr::Call`/`Attr`/`Name`) and renames a function, so the
  feature-gated exhaustive `Expr`/`Inst` matches (repl, modlink, codegen) and
  GC-rooting are untouched; it lowers through the ordinary binding + method-call
  paths.
- **Rebind vs register is decided from the declared return type**, read in the
  pre-sema desugar from the linked module. A receiver whose struct type the pass
  can resolve syntactically — a typed `let`/parameter, or `auto x = T()` naming
  a known struct — participates; an **unresolvable receiver falls back to
  registering** (no guess, no crash), so nothing that compiles today changes.
- **Imported-module receivers** (`import cli; @cli.command`, where `cli` is a
  module, not a local struct value) are **not yet** resolved here and fall back
  to registering. This is the documented extension for the package consumer
  surface and **gates zaplang-io/cli#4**: it needs the desugar to read an
  imported free function's return type (or the decision lifted into sema). The
  issue's reproduction deliberately uses a local receiver to isolate the binding
  mechanism from package imports; this ADR covers that mechanism.
- Stacked dotted decorators on a rebinding head compose into one chain; a stack
  whose head is a void method keeps registering each decorator as before.
- `@request_source` / `Depends(...)` adapter synthesis stays on the registering
  path only; the rebinding path hands the original function to the method
  directly (per-parameter injection and `cli.Context` are cli#4's dispatcher
  concern).

<!-- Related: the registering/behavioral decorator paths (I#457/D-0087,
     #651/#652) in crates/driver/src/decorator_desugar.rs and
     crates/driver/src/behavioral_decorator.rs. -->
