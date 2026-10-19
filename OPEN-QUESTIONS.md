# Zap — Open Questions

Tracked, unresolved language decisions — the backlog we work down in priority
order (in the spirit of Go's proposal queue). When one is settled it moves to
`DECISIONS.md` and is removed here. Tags: 🔴 blocking · 🟡 soon · ⚪ later.

_Last updated: 2026-10-19_

---

## Type system & semantics

- ⚪ **`**kwargs`-style arbitrary named arguments.** Deferred (D-0038) — fights
  static typing; keyword args + defaults cover the common cases. Revisit if a
  real need appears (e.g. forwarding).
- ✅ **Data-carrying enum variants** (enum tier 3, Rust-style sum types) —
  RESOLVED (D-0046, issue #51): value-first tagged representation (pointer only
  for recursive/oversized fields), qualified construction + `match` destructuring
  (positional or named), generic enums reserved now and monomorphized when user
  generics land. See `design/0006-types-and-generics.md` §3.3.
- 🟡 **`set` collection type** + set comprehensions (comprehensions are locked,
  D-0026, but a `set` type isn't).
- ✅ **Operator overloading for user types** — RESOLVED (D-0047, issue #52):
  dunder methods satisfying compiler-known structural operator interfaces; single
  `__cmp__` for ordering, `__eq__` for equality; `@derive(Eq, Ord, Hash, Repr)`;
  `__call__` and `__hash__` (struct keys) included; in-place `__iadd__` reserved,
  implementation deferred. See `design/0006-types-and-generics.md` §2.5.
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

- ✅ **Structured-concurrency surface syntax** — RESOLVED (D-0044/D-0045, issue
  #50): the `scope:` block, `spawn detached`, typed `OnError` policy, and
  aggregate `ExceptionGroup` on concurrent failure. See
  `design/0005-concurrency.md`.
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

- 🟡 **User-defined compile-time decorators (macros)** — the decorator model +
  built-ins are locked (D-0036, `design/0004-decorators.md`); v1 lands
  `@derive(Eq, Repr)` and a string-keyed runtime-reflection surface
  (`reflect_has` / `reflect_names`, see `design/RUNTIME-ABI.md` §"Decorators &
  reflection", #16). Still open: the macro API (what a decorator sees/emits) and
  hygiene rules; the remaining built-ins (`@cache`, `@default`, `@validate`,
  `@range`, `@deprecated`, `@inject`, `@derive(Ord/Json/Hash)`); field/parameter/
  function decorators; richer reflection (field-level metadata, annotation
  argument values). The `@range` name also collides with the `range` keyword and
  needs a parser accommodation before it can be written.
- ⚪ **C FFI** (D-0012) — deferred; generalizes interop to all languages.
- ⚪ **Precise GC** (Immix-style) replacing Boehm (D-0009).
- ⚪ **Array dtype promotion** rules for mixed-dtype ops (`i64 + f64`), and
  `uint8`/`bool` dtypes (`design/0002-arrays.md`).

## Design docs

All planned design docs are now written: `design/0001-perl-regex.md`,
`0002-arrays.md`, `0003-python-interop.md`, `0004-decorators.md`,
`0005-concurrency.md` (#14), `0006-types-and-generics.md` (#15),
`0007-strings.md` (#16), plus the two phase-boundary contracts `IR.md` (#12) and
`RUNTIME-ABI.md` (#13). New features get a design doc per the proposal process
(`design/TEMPLATE.md`).
