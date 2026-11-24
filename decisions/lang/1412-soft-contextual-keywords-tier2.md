---
title: Soft (contextual) keywords Tier 2 — spawn / when / do / unsafe / python
status: Locked
issue: 1412
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Soft (contextual) keywords, Tier 2: `spawn`, `when`, `do`, `unsafe`, `python`

**Status:** Locked

Tier 2 of the soft-keyword epic #1413, reusing the Tier-1 mechanism
(decisions/lang/1405-soft-contextual-keywords.md). Closes #1412.

## Decision

Demote `spawn`, `when`, `do`, `unsafe`, and `python` from **hard** reserved
keywords to **contextual (soft) keywords**. Each is a keyword **only** in its
single statement/clause slot and an ordinary identifier everywhere else —
struct field name, parameter name, named-argument label, attribute read, and
general expression (variable / call target):

```zap
struct Job:
    str do              # field named `do`
    bool unsafe         # field named `unsafe`

def start(str spawn, str python) -> str:   # params
    return spawn + python

def main():
    auto j = Job(do="build", unsafe=False)   # named args
    print(j.do)                              # attribute read

    # keyword meanings unchanged:
    do:                 # do-while statement
        step()
    while go
    unsafe:             # target-feature-call gate
        lowlevel()
    spawn worker()      # spawn statement
    when target.os == .linux:   # conditional compilation
        a()
```

As in Tier 1, the words are removed from the reserved-word table
(`Keyword::from_ident`), so the lexer emits them as `Ident` and the parser
recognizes each only at the head of its slot. **Strictly widening** — every
program that parsed before still parses identically. The statement/import forms
are unchanged.

**Slot recognition (`crates/syntax/src/parser.rs`).** Three groups:

1. **Block statements with a `:` marker** — `do` and `unsafe` join `select` in
   `try_soft_keyword_stmt`: the statement only when statement-leading and
   **immediately** followed by `:` NEWLINE INDENT (`at_block_stmt`); otherwise an
   identifier (`do(x)`, `do = v`, `unsafe.attr`, …).

2. **`when` — predicate marker, no bare `:`** — the conditional-compilation
   statement/item only when statement/item-leading and followed by its `target`
   `.` predicate head (`at_when_pred`); a `when` predicate always begins with the
   contextual word `target` then `.` (see `parse_cfg_predicate`), so `when = v`,
   `when(x)`, `when.attr`, and the member access `when.target` all stay
   identifier uses. Recognized in both item position (top-level cfg over
   imports/declarations) and statement position.

3. **No `:` marker — markerless keywords (prefer-keyword-at-statement-start)** —
   per the issue, `spawn` and `python` have no trailing marker, so the rule is:
   - **`spawn`** is a *simple* statement `spawn [detached] call_expr`, dispatched
     in `parse_simple_stmt` (so it still works after `;`). At statement start it
     is **always** the keyword; it is an identifier only off the statement head —
     as an assignment target/RHS, an argument or named-arg label, a
     sub-expression, a field, or a parameter. `detached` remains a contextual
     modifier after `spawn`.
   - **`python`** is a keyword only in the interop import head, recognized when it
     immediately follows `import` **and** is immediately followed by the module
     string literal (`import python "numpy" as np`). Otherwise an identifier, so
     `import python` / `import python as p` now import a logical module named
     `python`.

No new `ast::Expr`/IR node — the words resolve to the existing `Expr::Name` /
field / parameter / argument-label forms, so sema, lowering and codegen are
untouched; the change is confined to `crates/syntax`.

## Rationale

- **Same locked principle as Tier 1 (epic #1413).** Each word occupies a unique
  statement/clause/import slot with a disambiguating marker and never appears in
  general expression or type position, so softening introduces no ambiguity. The
  only words without a `:` marker — `spawn` and `python` — are resolved by the
  issue's explicit rule (prefer the keyword at the statement head / interop head,
  identifier elsewhere), which never breaks a program that parses today because
  statement-leading `spawn` and `import python STRING` keep their exact meaning.

- **Real, if lower, identifier demand.** `spawn` (subprocess libraries), `when`
  (condition/event helpers), `do` (task/command builders), `unsafe` (FFI
  wrappers), `python` (interop shims / config fields) are all plausible names in
  the Python-adjacent libraries Zap mirrors.

- **Reuse Tier 1's mechanism and test patterns.** `is_soft_kw` / `expect_soft_kw`
  / `peek_nth` / `try_soft_keyword_stmt` already exist; Tier 2 adds
  `at_block_stmt` and `at_when_pred` and extends the dispatcher. The same
  walled-garden parser **and** native-execution test shape is reused.

- **Lexer demotion over per-position allow-listing** — identical reasoning to
  Tier 1: it cannot miss an identifier position and makes the words behave
  exactly like any other identifier.

## Consequences

- The five words are now valid identifiers in every non-slot position. In
  particular `import python` changed meaning: it was a parse error (the interop
  form required a string), and now imports a logical module named `python` — a
  strict widening of a previously-rejected program, so no existing valid program
  is affected.

- Statement-leading `spawn` is always the spawn statement, so `spawn` cannot be
  an assignment target or stand-alone call at the statement head (`spawn = v`,
  `spawn(x)`, `spawn.m()` as statements). These did not parse before either; the
  issue accepts this explicitly. `spawn` as a value/argument/field/param is
  unaffected.

- Completes epic #1413's committed scope (Tier 1 #1405/#1411 + Tier 2 #1412).
  The optional declaration-leading set (`struct`/`enum`/`interface`/
  `annotation`/`decorator`) remains uncommitted, to be filed on demand.

See also decisions/lang/1405-soft-contextual-keywords.md (Tier 1, the shared
mechanism), epic #1413, and the `docs/GRAMMAR.ebnf` soft-keyword note above
`compound_stmt`.
