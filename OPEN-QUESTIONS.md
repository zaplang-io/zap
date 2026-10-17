# Zap — Open Questions

Tracked, unresolved language decisions — the backlog we work down in priority
order (in the spirit of Go's proposal queue). When one is settled it moves to
`DECISIONS.md` and is removed here. Tags: 🔴 blocking · 🟡 soon · ⚪ later.

_Last updated: 2026-10-17_

---

## Type system & semantics

- 🔴 **Function parameters: default & keyword arguments, variadics.** Python devs
  rely on these heavily; not yet decided. Affects call syntax and the type
  checker.
- 🟡 **Sum types + `match`** (enum tier 3). Data-carrying variants (Rust-style)
  and a pattern-matching statement. High value (state machines, AST nodes), but
  deferred past tier-1 enums (D-0018). Interacts with generics (D-0017).
- 🟡 **`set` collection type** + set comprehensions (comprehensions are locked,
  D-0026, but a `set` type isn't).
- 🟡 **Operator overloading for user types.** Currently built-in types only
  (D-0011). Decide if/how user structs define `+`, `==`, etc.
- 🟡 **Iteration protocol.** How a user type becomes iterable / works with `for`
  and comprehensions (a `__iter__`-style contract, or an `Iterable` interface).
- 🟡 **Tuple syntax & multiple return.** Tuples are locked (D-0019); their
  literal/destructuring syntax and whether functions return multiple values via
  tuples is TBD.
- 🟡 **Method-definition syntax** for structs (D-0015): inline methods vs a
  separate receiver/`impl` block.
- ⚪ **Unsigned integers.** Deferred (D-0019); likely reintroduced only for C
  interop and array dtypes (e.g. `uint8` for image data).
- ⚪ **`char` details** — grapheme-cluster handling, char/codepoint API surface.
- ⚪ **const / compile-time evaluation** — how far `const` folds at compile time.

## Concurrency

- 🟡 **Structured-concurrency surface syntax** (D-0033 model is locked) — the
  scope-block form and how results/cancellation are expressed.
- ⚪ **Formal memory-model spec** (principle locked, D-0034).
- ⚪ **Race detector** tooling.

## Modules, packaging, tooling

- 🟡 **Package-manager concrete design** (D-0035 direction locked): manifest
  format (`zap.toml`?), resolver (MVS vs SAT/semver), registry vs decentralized.
- 🟡 **Standard-library scope & layout** for v1.
- 🟡 **Build/run command names** (`zap build`, `zap run`, …).
- ⚪ **In-language test framework** and **doc conventions**.
- ⚪ **Compatibility / versioning promise** (Go-style "Go 1 compatibility"?).

## Features deferred with a direction

- 🟡 **Decorators / annotations** (D-0036) — needs its own design: pre/post
  wrappers *and* Java-style metadata annotations that also apply to variables.
- ⚪ **C FFI** (D-0012) — deferred; generalizes interop to all languages.
- ⚪ **Precise GC** (Immix-style) replacing Boehm (D-0009).
- ⚪ **Array dtype promotion** rules for mixed-dtype ops (`i64 + f64`), and
  `uint8`/`bool` dtypes (`design/0002-arrays.md`).

## Design docs still to write

- `design/0004-concurrency.md` — channels, `select`, sync primitives, structured
  concurrency, memory model.
- `design/0005-types-and-generics.md` — structs, interfaces, generics, enums,
  tuples.
- `design/0006-strings.md` — the UTF-8 string model (D-0025).
- `design/IR.md` — the typed-IR schema (phase-boundary contract).
- `design/RUNTIME-ABI.md` — object layout, calling convention, GC hooks.
