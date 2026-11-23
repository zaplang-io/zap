---
title: comptime/reflect parser surface — comptime keyword + comptime-for / comptime-expr (S1)
status: Accepted
issue: 1297
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# comptime/reflect parser surface — `comptime` keyword (S1)

**Status:** Accepted — records the *implemented shape* of issue #1297 (S1 of the
comptime/reflect surface epic #1296, under #1248). S1 is the **parser surface
only**; comptime evaluation/unrolling is S2 (#1298). It realizes locked decision
4 of epic #1249 (the `comptime` keyword), the surface E1 (#1257) and the E2
engine (#1249) deferred.

## Decision

### 1. `comptime` is a keyword

A new lexer keyword (`comptime`). It is now reserved — a program using it as an
identifier is a parse error (acceptable pre-1.0; noted in the changelog).

### 2. Two new forms, one preserved

- **`comptime for <target> in <expr>:` block** → a distinct AST node
  `Stmt::ComptimeFor`, separate from the runtime `Stmt::For`, so a later pass
  (S2) can locate and unroll it. (The runtime `for` is unchanged.)
- **`comptime <expr>`** → `Expr::Comptime`, a prefix (same precedence level as
  `not`) that forces compile-time evaluation of the operand. It covers the
  comptime operand (`reflect.field(v, comptime f.index)`) and the comptime
  *binding* via `auto x = comptime <expr>` — so no separate declaration syntax is
  needed for locked decision 4's "comptime <binding>".
- **`comptime.<builtin>`** (e.g. `comptime.error(...)`, #1256) is **preserved**:
  when `comptime` is followed by `.`, the parser yields the `comptime` *name* so
  the postfix `.error(...)` attaches as an ordinary `Attr`/`Call`. The token
  after `comptime` disambiguates: `for` → `ComptimeFor`; `.` → qualified call;
  anything else → the `comptime <expr>` prefix.

### 3. Parsed, not evaluated (build-green deferral)

S1 ships the surface without wiring. To keep the build coherent and honest until
S2:

- **sema** reports `ZAP0002` ("feature not yet implemented") for `ComptimeFor`
  and `Expr::Comptime` — a program using the surface fails cleanly rather than
  type-checking as if it ran.
- **mono** type-substitutes through both nodes (so a specialized declaration is
  well-formed) but does not unroll/fold.
- **lowering** (`zapc_ir`) returns a clean lowering error for a `comptime for`
  that reaches it (sema gates first; this is a backstop).
- `render_expr`, `zap fmt`, cfg, the generator/yield analysis, scope/escape
  analysis, the LSP analyzer, doc, and the driver's AST rewriters all handle the
  new nodes (a new `Stmt`/`Expr` variant touches every exhaustive match).

S2 (#1298) removes the sema/lowering deferral and wires the nodes to the E2
engine (unroll via #1251, `reflect.field` desugar via #1252, fold via #1250).

## Scope deferred

- Evaluation/unrolling and `reflect.*` recognition → S2 (#1298).
- Whole-method `comptime def` → future (locked decision 4).
- `@derive` synthesis → E3 (#1289), which this unblocks.

## Consequences

`docs/GRAMMAR.ebnf` gains `comptime_for_stmt` and the `comptime` alternative in
`not_test`. New host tests: `crates/syntax/tests/comptime_surface.rs` (parse +
`render_expr` round-trip + reserved-word) and
`crates/fmt/tests/comptime_surface.rs` (`zap fmt` canonical + idempotent). A
program with no `comptime` is byte-identical through parse and fmt.
