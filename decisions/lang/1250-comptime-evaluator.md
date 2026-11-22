---
title: Comptime value model + pure tree-walking evaluator (Tier B1, E2)
status: Accepted
issue: 1250
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Comptime value model + pure tree-walking evaluator (Tier B1, E2)

**Status:** Accepted — this ADR records the *implemented shape* of phase **E2**'s
greenfield core, issue #1250 (epic #1249, proposal #1248 / Discussion #1243). It
is the first cut of the comptime evaluation engine: the value model and the
tree-walking evaluator every other E2 issue builds on. The resolved design and
its rationale live in Discussion #1243 (HQ1–HQ9) and the sub-epic #1249 locked
decisions.

## Context

Nothing in the compiler evaluates expressions today: monomorphization
(`crates/mono`) *substitutes types and rewrites references*, it never computes;
the REPL/JIT runs via LLVM codegen, not a host interpreter; there is no
const-eval / CTFE anywhere in sema, ir or mono. E2 therefore needs a **new,
pure-Rust, host-side tree-walking evaluator** over comptime values. E1 (#1257)
shipped the read-only reflection model (`TypeInfo`/`FieldInfo`/…) in
`crates/sema_types`; E2 turns that value into executable compile-time code.

## Decision

### A new leaf crate, `crates/comptime` (`zapc_comptime`)

The evaluator lives in its own crate depending only on `zapc_syntax` (the AST it
walks) and `zapc_sema_types` (the E1 reflection records, **reused, never
redefined**). It deliberately does **not** depend on the sema `Checker` or on
codegen/LLVM, so monomorphization — itself a leaf over `zapc_syntax` +
`zapc_sema_types` that does not depend on sema — can drive it later (#1251)
without a dependency cycle or an LLVM toolchain. It is in both `members` and
`default-members`, so the ordinary host build and test run it.

### Tier-B1 value model (`Value`)

`int64`, `bool`, `str`, `char`; `list`/`tuple` of comptime values (immutable);
and the E1 reflection records `TypeInfo`/`FieldInfo`/`VariantInfo`/`AttrInfo`/
`AttrArg` plus the `TypeKind` tag, wrapped in `Rc` for cheap clone. Floats,
bigint, `bytes`, `None`/optionals, and dict/set values are **deliberately
absent** — reaching for one is out of fragment.

### The fragment the evaluator reduces

`let` bindings (lexically scoped, shadowing); `comptime if` (first true branch,
short-circuited); the ternary and short-circuiting `and`/`or`; field-access-by-
index on the reflection records; `int64`/`bool`/`str`/`char` arithmetic,
comparison, bitwise and boolean ops; string concat/ASCII-case/codepoint-slice;
`len`/`ord`/`chr`/`char`/`str` builtins; and **non-recursive calls to registered
user functions** (the HQ9 unblock, so a library resolves attribute meaning and
`rename_all` transforms at compile time).

### The soundness backstop (epic #1249, locked decision 2b)

Everything else is out of the fragment: I/O, FFI, `spawn`/channels, global
mutation (assignment), time/RNG, dict/set iteration (#1066, unspecified order),
recursion (direct **and** transitive, detected via the active call stack),
`match`-over-comptime-values, `for`/`while` loops, comprehensions, lambdas,
regex, and float/bigint/`bytes`/`None` values. The evaluator is the **physical
backstop**: it has no impure primitive, so reaching one is a clean
span-attributed `CtError` carrying the exact phrase `not available at comptime`,
never a panic. The static early-reject lint and shared leaf table are a separate
issue (#1254); this crate only makes the evaluator fail cleanly. A fixed step
budget and call-depth cap (not tunable quotas, HQ5) fail fast on a runaway.

### Numeric parity (the "easy to get wrong" requirement)

`int64` `+`/`-`/`*` wrap modulo 2^64; `//`/`%` are floor division / floor-sign
modulo with the `INT64_MIN`/`-1` special cases; `<<`/`>>` saturate at count ≥ 64
and `>>` is arithmetic; division/modulo by zero and a negative shift count are
faults. Each mirrors the runtime C helper line for line (`zap_ifloordiv`,
`zap_imod`, `zap_ishl`, `zap_ishr`), and `str.upper()`/`.lower()` mirror the
runtime's **ASCII-only** fold (`zap_str_upper`/`zap_str_lower`). Slicing clamps
bounds Python-style (`zap_slice_pos`/`zap_slice_neg`) and faults only on a zero
step, matching `zap_str_slice`/`zap_list_slice`; indexing faults when out of
range. A parity suite cross-checks a matrix of operands against an independent
i128 reference.

## Scope boundaries owned here

- **No parser `comptime` keyword.** The user surface (`comptime for` /
  `comptime <binding>`) is really wired by #1251; like E1, this ships its model
  host-tested over ordinary parsed/constructed AST, with no Zap syntax. So
  GRAMMAR.ebnf / README / STABILITY are unchanged (no user-facing surface yet).
- **No mono wiring, no `comptime for` unrolling, no `.kind` switch, no typed
  `reflect.field` lowering** — those are #1251/#1252/#1253. A field's own type
  (`field.ty` recursion, the kind-switch) is explicitly reported as deferred to
  #1253 rather than evaluated.
- **Arithmetic restricted to `int64`/`bool`/`str`/`char`**; true division `/`
  (float), `**`, `@`, and wider numerics are out of fragment, keeping the parity
  surface small. Float and bigint comptime values are B2 (#946 / #1084).
- **`AttrArg.name` optionality** is surfaced as a `.is_named` bool plus a
  faulting `.name` on a positional argument (B1 has no optional value type).

## Consequences

The evaluator is the sole soundness authority for comptime purity (the lint only
ever *rejects*). Its output will be well-typed specialized AST for mono to splice
(#1251). No behavior change for existing programs: the crate is compiler-internal
and host-tested (`crates/comptime/tests/`), with no runtime or codegen surface.
