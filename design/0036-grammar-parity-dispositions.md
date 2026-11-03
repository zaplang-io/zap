# Proposal: Consolidated grammar-parity dispositions (#614)

- **Author(s):** Parvesh Garg
- **Last updated:** 2026-11-03
- **Status:** Accepted
- **Discussion:** epic #614 (CPython grammar-parity pass, decisions D1–D35);
  design-gated items #596, #600, #602, #603, #605, #606, #607, #611.

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line. This keeps review comments precise and diffs
> small.

## Abstract

Epic #614 triaged every grammar difference between CPython and Zap and recorded
a disposition (D1–D35) for each.
Most were settled on the spot — adopt as a pure desugar, or drop as off-strategy
for a nominal, type-first, AOT language.
Eight were flagged *needs a design decision before implementation* and were
settled in a dedicated interactive design session.
This note is the durable record of those eight decisions: seven adopt, one drop.
It is a design record only.
The authoritative grammar (`docs/GRAMMAR.ebnf`) and spec (`SPEC.md`) are *not*
edited here; each feature's grammar and spec changes land with its own
implementation PR, as the per-item sections below spell out.

## Background

The parity pass asked, difference by difference, whether a CPython construct
earns a place in Zap.
Zap is Python-readable but not Python: it is statically typed, type-first in its
declarations, nominally typed, and AOT-compiled with a moving Immix GC.
The test for adoption was never "Python has it" but "does it fit a nominal,
type-first language and pull its weight?"

For most items the answer was obvious.
The eight here were not, because each touched a load-bearing design axis —
laziness vs. eagerness, the "one obvious way to declare" rule, the published
callee contract, pattern-matching's capture-vs-type-test split, structured
concurrency's error model, type identity, or the nominal/structural boundary.
Each needed a decision recorded before anyone could implement it.

The dispositions below are transcribed from the final decision comment on each
issue.
They are the specification inputs for the implementation PRs, not the
implementation.

## Proposal

### Summary

| Item | Decision (D#) | Disposition |
| ---- | ------------- | ----------- |
| #596 | D4  — generator expressions `(x for x in xs)` | **Adopt** — lazy `Iterator` + sole-argument paren elision |
| #600 | D8  — walrus assignment-expression `(x := e)` | **Adopt** — restricted, parens always |
| #602 | D10 — positional-only parameters `/` | **Adopt** — symmetric mirror of keyword-only `*` |
| #603 | D11 — `as` sub-pattern capture in `match` | **Adopt** — recursive whole-value capture |
| #605 | D13 — `except*` exception groups | **Adopt** — full; nested groups flatten to one level |
| #606 | D14 — transparent type alias `type X = …` | **Adopt** — minimal, transparent-only (incl. generic form) |
| #607 | D15 — anonymous unions `A | B` in type position | **Drop** — enums-only (closed `wontfix`) |
| #611 | D18 — `is` / `is not` identity operators | **Adopt** — references + `None`; value-type `is` is a compile error |

Seven adopt, one drop.

### #596 — generator expressions (D4)

**Adopt — lazy generator expressions + sole-argument paren elision.**

- **Grammar:** `atom` gains `"(" comp_for ")"`.
- **Semantics:** produces a lazy, one-shot `Iterator`, reusing the
  generator/`Iterator` machinery from I#67 — no intermediate list.
  The eager form stays `[...]` (list comprehension), so the lazy/eager split is
  meaningful.
- **Sole-argument elision:** when a generator expression is the only argument to
  a call, the inner parens may be omitted — `sum(x*x for x in xs)`.
  Parser rule: inside a call's argument list, an argument followed by `for` is a
  genexpr, and it must be the only argument.
- **Evaluation:** the loop source is captured at creation; elements are produced
  lazily on iteration; consumed-once (the `Iterator` contract).

Cheapest of the parity items — a pure desugar onto existing generator lowering.

### #600 — walrus assignment-expression (D8)

**Adopt — restricted walrus assignment-expression, parens always.**

- **Grammar:** a `named_expression` `NAME ":=" expr`, allowed only in a
  parenthesized expression position, `if`/`while` conditions, and comprehension
  `if` filters.
  Parens are required in every position, including conditions
  (`while (line := read()) != "":`), for a dead-simple parse rule.
- **Non-goal (unchanged):** `:=` is NOT a general declaration shorthand
  (`a := 10`); use `auto a = 10`.
  This keeps the type-first "one obvious way to declare" property intact.
- **Semantics:** inference-bound (like `auto`) into the lexically enclosing
  **block** scope; visible after the construct; rebinds each loop iteration.
  The first `:=` introduces the name (inferred type); a later `:=`/use must be
  type-compatible.
- **Primary motivation:** comprehension-filter single evaluation —
  `[y for x in xs if (y := f(x)) > 0]` computes `f(x)` once.
  (The `while`-loop form is otherwise already writable with `auto` + `break`.)

### #602 — positional-only parameters (D10)

**Adopt — positional-only `/`, the symmetric mirror of the keyword-only `*`.**

- **Grammar:** `param_list` may carry a bare `/`; every parameter before it is
  positional-only (cannot be supplied by name at a call site).
  `/` appears at most once and must come before any `*`.
- **Rationale:** positional-only is part of the callee's *published contract*
  and must be spellable to be visible.
  Without `/`, a C-backed function's signature can't communicate "no keyword
  args," so a caller writes `f(x=1)` and hits a confusing error, and tooling
  (LSP/IDE/MCP) has nothing to key off — it can't suppress `x=` in completion,
  render `f(x, y, /, z)` on hover, or point a diagnostic at a visible `/`.
  Zap ships a C runtime, so C-backed callees are real consumers today.
  FFI/C-mapped signatures (I#542) render with `/` rather than needing a separate
  mechanism.
- **Diagnostics:** `def f(x, y, /, z)` rejects `f(x=1, ...)` for `x`/`y` with a
  clear message; `z` still accepts positional or keyword.

### #603 — `as` sub-pattern capture (D11)

**Adopt as proposed — recursive `as` sub-pattern capture.**

- **Grammar:** `pattern ::= or_pattern [ "as" IDENT ]`.
  Because the sub-pattern productions (`seq_pat`, `pat_arg`, tuple elements)
  already recurse through `pattern`, `as` capture is automatically available at
  any nesting depth — no extra rule is needed, and restricting it to top-level
  only would actually be *more* work.
  `as` binds looser than `|`, so `case 1 | 2 as x:` parses as `(1 | 2) as x`.
- **Semantics:** on a successful match of `<pattern>`, bind `name` to the matched
  value, typed as that value's static type at its position (scrutinee type for a
  top-level `as`; element/field type when nested).
  The guard (`if`) may reference the captured name.
  Or-pattern binding-consistency already covers the `as`-added name.
- Fills the real gap: today a bare `IDENT` pattern binds the whole value but
  can't destructure, and a struct/sequence pattern destructures but can't name
  the whole; `as` does both.
- Independent of the (dropped) anonymous-union match type-patterns — this is
  whole-value capture, not type matching.

### #605 — `except*` exception groups (D13)

**Adopt full `except*` exception-group handling.**

- **Grammar:** `try_stmt` gains `except* <type> [as name]` clauses.
  A `try` is all-`except` or all-`except*` (cannot mix).
- **Semantics:** each `except*` peels the sub-exceptions matching its type into a
  sub-`ExceptionGroup`, binds `name` to that GROUP (always a group, even for a
  single match), and runs the handler; the unmatched remainder auto-re-raises as
  a residual group.
  Keep the `ExceptionGroup` type; expose `ExceptionGroup.exceptions` iteration.
- **Bare bridge:** a bare (non-group) exception is treated as a one-element group
  for `except*` *matching only* (virtual wrap).
  If no `except*` clause matches, the original bare exception propagates
  **unchanged** (keeps its single stack trace, not artificially grouped).
  This lets one `try/except*` cover both the `scope` single-failure (bare) and
  fan-out (group) cases — note that a plain `except ExceptionGroup` would
  silently miss a bare exception.
- **Nested groups — decided: flatten-to-one-level.** All nested `ExceptionGroup`s
  collapse to a single flat group of leaf exceptions; `.exceptions` is always
  flat leaves; each leaf keeps its own traceback (so origin is preserved), while
  structural hierarchy is dropped; the residual re-raise is also flat.
  (The recursive/structure-preserving split — the Python behavior — was
  considered and rejected for simplicity.)
- **Tracebacks:** a group preserves each child's independent traceback, wrapped
  with the common (`scope`) prefix; the residual re-raise preserves the original
  per-child traces.

Rationale: `scope` already raises `ExceptionGroup` (SPEC §concurrency), and with
anonymous-union match type-patterns dropped (#607) the manual
`except ExceptionGroup` + `.exceptions` path can't lean on type-arms to
discriminate — so `except*` earns its keep as the ergonomic completion of
structured concurrency.

### #606 — transparent type alias (D14)

**Adopt — minimal, transparent-only type alias (incl. the generic form).**

- **Grammar:** `type_alias ::= "type" IDENT [ generics ] "=" type` as a module
  item.
  `type` is a **soft keyword**, recognized only at item start followed by `IDENT`
  then `=` or `[`, so `type` stays usable as an ordinary
  identifier/field/param everywhere else (reserving it would break the very
  common `type` name).
- **Semantics:** TRANSPARENT and total — the alias resolves to its target
  everywhere a type is resolved, *including* `match` and overload resolution (an
  alias is never a distinct type identity or a distinct overload key).
  It is NOT a new nominal type: struct/enum already cover nominal, and a
  one-field struct is the newtype.
- **Generic form:** `type Pair[T] = tuple[T, T]` — pure pre-monomorphization
  substitution; no new mono machinery (D-0017).
- **Cycles:** aliases must form a DAG.
  Any cycle or self-reference is a compile error (`type A = B; type B = A`; also
  `type Tree = (int64, list[Tree])` — infinite expansion, no nominal stop).
  Recursive types must go through a nominal struct/enum.
  Doc callout: Python-style recursive `type` aliases do NOT work here.
- **Scope note:** union aliases (`type J = A | B`) are NOT available — anonymous
  unions were dropped (see #607) — so this ships bare-alias-only.

### #607 — anonymous union types (D15)

**Won't-do — enums-only (anonymous unions dropped). Closed `wontfix`.**

Zap already has nominal tagged sum types — data-carrying `enum`s with
compile-time exhaustiveness — plus the `JsonValue` prelude enum (D-0059) for the
motivating ad-hoc "one of these" case.
Anonymous *structural* unions (`A | B` in type position) would add, for a
nominal-leaning AOT language:

- **implicit injection + widening** (a bare value flowing into a union) and the
  type-directed narrowing to go with it;
- **match type-patterns** (`case str as s:`) — a genuinely new pattern kind;
- **union subtyping** threaded through `assignable`/`join`;
- a **tagged-union representation** (discriminant + max-payload + per-arm GC
  descriptor), i.e. an anonymous enum under the hood.

That is a large surface for an ergonomic gain over what enums already express,
with no concrete consumer driving it.
Decision: do not add anonymous union types for now; revisit only if a concrete
need appears.

### #611 — `is` / `is not` identity operators (D18)

**Adopt — `is` / `is not`, references + `None`; value-type identity is a compile
error.**

- **Grammar:** `comp_op` gains `is` and `is not` (participates in chained
  comparison).
- **Semantics:**
  - `x is None` / `x is not None` → the `Optional[T]` empty-variant check or the
    null-reference (D-0014) check, per `x`'s static type (consistent with
    `== None` and `case None:`, #564).
  - `a is b` on **reference** structs (D-0023) → same-allocation identity
    (pointer comparison), distinct from `==`/`__eq__` which is structural.
    Coherent under the moving Immix GC: it compares two live references at a
    single instant (GC relocates both consistently) and never caches a raw
    address across a GC.
  - `a is b` where either side is a **scalar or value struct** (and not the
    `None` case) → **compile error**: "`is` compares object identity; value types
    have no identity — use `==`."
    (Designs out Python's `is`-on-small-ints footgun.)
  - **Generic `T`:** `x is None` is fine wherever `None` is valid for `T`;
    `a is b` on an unconstrained `T` resolves at monomorphization and errors at
    the instantiation site if `T` is a value type.

## Cross-cutting threads

Three threads run through the eight decisions and should be read together.

- **The enums-only cascade (#607 → the rest).** Dropping anonymous structural
  unions is the single decision with the widest reach.
  Its knock-on effects were recorded deliberately:
  - **#606 ships bare-alias-only.** Without anonymous unions there is no
    `type J = A | B` surface; the type alias is a transparent rename of an
    existing type and nothing more.
  - **Nullability stays `Optional[T]`.** There is no `T | None` type surface;
    the `None` checks in #611 are defined against `Optional[T]` and the
    null-reference model (D-0014), not against a union.
  - **No `match` type-patterns are introduced.** `case str as s:` would have been
    a genuinely new pattern kind and was dropped with the unions.
    This is precisely why #605 adopts `except*`: the manual
    `except ExceptionGroup` + `.exceptions` path can't lean on type-arms to
    discriminate, so the dedicated `except*` syntax earns its keep.
  - The `as`-capture in #603 is *whole-value capture*, not type matching, so it
    is unaffected by the drop — the two were explicitly disentangled.

- **The type-first declaration rule holds.** #600 restricts `:=` so it can never
  become a general declaration shorthand; `auto a = 10` remains the one obvious
  way to declare an inferred binding.
  The walrus is an expression-position convenience (chiefly comprehension-filter
  single-evaluation), not a second declaration form.

- **Lazy vs. eager stays legible.** #596 deliberately keeps `(…)` lazy and
  `[…]` eager, so the bracket a reader sees tells them whether an intermediate
  list is materialized.

## Resolved open design lines

A few sub-questions inside the adopted items were themselves open and were
closed in this session; they are called out here so implementation doesn't
reopen them.

- **#606 — total transparency + the acyclic rule.** The alias is transparent
  *everywhere* a type is resolved, including `match` and overload resolution
  (never a distinct identity or overload key), and aliases must form a DAG:
  any cycle or self-reference is a compile error, and recursive types must go
  through a nominal struct/enum.
- **#611 — generic `is` resolves at monomorphization.** `a is b` on an
  unconstrained `T` is not rejected at definition; it resolves per instantiation
  and errors at the instantiation site only when `T` turns out to be a value
  type.
- **#605 — nested groups flatten to one level.** `ExceptionGroup` nesting
  collapses to a single flat group of leaf exceptions (each leaf keeping its own
  traceback); the structure-preserving recursive split was considered and
  rejected for simplicity.

## Implementation

Nothing is built by this note.
Each adopted item (#596, #600, #602, #603, #605, #606, #611) is implemented by
its own PR, which is where that item's `docs/GRAMMAR.ebnf` and `SPEC.md` edits
land, together with sema tests and at least one golden (positive tests
throughout; #602 and #611 also add negative tests for their compile-error
cases).
#607 is closed `wontfix` and ships nothing.
The items are independent and may proceed in parallel, subject to the usual
same-repo sequencing for grammar/sema batches.
