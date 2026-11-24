---
title: Soft (contextual) keywords — scope / select / match / case demoted from hard keywords
status: Locked
issue: 1405
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Soft (contextual) keywords: `scope`, `select`, `match`, `case`

**Status:** Locked

Tier 1 of the soft-keyword epic #1413. Closes #1405 (`scope`) and #1411
(`select`, `match`, `case`) together.

## Decision

Demote `scope`, `select`, `match`, and `case` from **hard** reserved keywords to
**contextual (soft) keywords**. Each is a keyword **only** in its single
statement/clause slot and an ordinary identifier everywhere else — permitted as
a struct field name, function/method parameter name, named-argument label,
attribute (`.member`) read, and general expression (variable / call target):

```zap
struct Token:
    str scope          # field named `scope`
    str match          # field named `match`

def authorize(str scope, str match) -> str:   # params
    return scope + match

def main():
    auto t = Token(scope="read", match="exact")   # named-argument labels
    print(t.scope)                                # attribute reads
    r = match(t.scope)                            # `match` as a call target

    match t.match:          # the match STATEMENT, unchanged
        case "exact":       # the case ARM, unchanged
            print("ok")
        case _:
            print("no")

    scope:                  # the scope STATEMENT, unchanged
        spawn worker()
    select:                 # the select STATEMENT, unchanged
        case v = <-ch:
            use(v)
```

The keyword slots keep their existing syntax and behavior unchanged:

- `scope` — statement-leading `scope`, optional `(` arg_list `)`, then `:` +
  indented block (structured concurrency, D-0044).
- `select` — statement-leading `select` **immediately** followed by `:` + block
  (channel select, D-0031).
- `match` — statement-leading `match`, a subject expression, then `:` + block
  (D-0040).
- `case` — only inside a `match` body (pattern arm) or a `select` body (select
  case); an identifier anywhere else, including inside an arm body.

**Mechanism.** The demotion is done at the lexer: these four words are removed
from the reserved-word table (`Keyword::from_ident`), so the lexer emits them as
ordinary `Ident` tokens (joining the existing contextual words `pass`, `from`,
`del`, `value`, `default`, and the regex prefixes `m`/`s`). Because every
identifier-accepting position in the parser already consumes `Ident`, the four
words become usable in all of them with no per-position change — which is what
makes the change provably backward-compatible. The parser recognizes a word as a
keyword only at the head of its slot (`crates/syntax/src/parser.rs`,
`try_soft_keyword_stmt`), gated on the slot marker.

**`match` / `case` disambiguation (the one with a genuine ambiguity).** At
statement start, `match` could begin either the match statement (`match EXPR:` +
block) or an expression (`match(x)`, `match = v`, `match.attr`, `match[i]`, or a
bare name). The parser resolves it with a **bounded backtracking trial parse** —
the same rule CPython uses for its soft `match`: checkpoint, tentatively consume
`match`, parse a subject expression, and treat it as the statement **iff** that
trial parse is error-clean **and** is immediately followed by `:` NEWLINE INDENT;
otherwise roll back and parse the line as an ordinary expression/assignment where
`match` is an identifier. Resolve toward the statement when the `:`+block marker
is present, otherwise identifier. `case` needs no such backtrack: it is only ever
read inside a `match`/`select` body. `scope` and `select` are pure lookahead —
no identifier use of either is statement-leading and `:`+block-terminated, so the
marker admits no false positive (`select` requires the `:` immediately; `scope`
allows one balanced `(...)` option list first).

## Rationale

- **Reserve the minimum (epic #1413 locked principle).** A keyword may be
  demoted iff it occupies a unique syntactic slot with a disambiguating marker
  (statement/clause-leading `:` + block) and never appears in general expression
  or type position. All four satisfy this: each lives only statement/clause
  leading with a `:`+block and never mid-expression or in type position, so
  softening introduces no grammar ambiguity (bar `match`'s call-vs-statement
  case, which the marker + trial parse settles exactly as CPython's does).

- **Real library demand.** `scope` is the originating case: the official OAuth
  package cannot expose the `scope` token field / `scope=` keyword argument
  (RFC 6749 §5.1; Authlib's `OAuth2Client`), tracked in #1405. `select`
  (SQLAlchemy `select(...)`, the `select`/`selectors` syscall wrappers),
  `match` (`re.match(...)`, match objects), and `case` (test-case/switch
  builders) are common identifiers in the Python libraries Zap mirrors.

- **Precedent, in Python and in-repo.** Python's own `match`, `case`, and `type`
  are soft keywords. Zap already carries the contextual words `pass`, `from`,
  `del`, `value`, `default` and the soft `type` (#1296) / `fn` (I#163) — this
  reuses that established model rather than inventing one.

- **Strictly widening.** Every program that parsed before still parses
  identically; the grammar only starts accepting more. So the change is
  backward-compatible and needs no deprecation window.

- **Lexer demotion over per-position allow-listing.** Removing the words from
  the reserved set (rather than keeping the `Keyword` variants and allow-listing
  them in each identifier position) is both smaller and safer: it cannot miss a
  position, and it guarantees the words behave exactly like any other identifier
  by reusing the existing declaration/assignment/postfix/atom machinery
  unchanged — the direct source of the backward-compatibility guarantee.

## Consequences

- `scope`, `select`, `match`, `case` are now valid identifiers in every
  non-slot position; the OAuth package (and the Python libraries above) can use
  them as names. The four keyword statement forms are unchanged.

- No new AST/IR node: the words resolve to the existing `Expr::Name` / field /
  parameter / argument-label forms, so sema, lowering and codegen are untouched
  (the change is confined to `crates/syntax`).

- `match` at statement start now costs one bounded trial expression parse to
  disambiguate; the cost is linear in the subject expression and incurred only
  when a line begins with the identifier `match`.

- Sets the pattern (lexer demotion + in-slot recognition + the `:`+block marker +
  `match`-style trial parse, plus walled-garden parser **and** native-execution
  tests proving both meanings coexist in one program) that Tier 2 (#1412:
  `spawn`, `when`, `do`, `unsafe`, `python`) reuses.

See also epic #1413 (soft-keyword tiering), #1411 (the `select`/`match`/`case`
slice), and the `docs/GRAMMAR.ebnf` soft-keyword note above `compound_stmt`.
