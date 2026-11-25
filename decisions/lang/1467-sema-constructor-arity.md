---
title: Sema checks struct constructor arity and unknown field names
status: Locked
issue: 1467
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Sema checks struct constructor arity and unknown field names

**Status:** Locked

## Decision

A struct field constructor `StructName(args)` is checked in sema for two more
mistakes, bringing it to parity with enum-variant construction
(`check_variant_construction`):

- **Too many positional arguments** — more positional args than the struct has
  fields → `ZAP4062` (reported once, at the first excess argument).
- **Unknown field name** — a named argument that matches no declared field →
  `ZAP4063`.

Both were caught before only in IR lowering (too-many as `ZAP6001`; an unknown
named field not reliably at all), i.e. at `zap build`, not through the sema-only
`check` / MCP `check_source` / LSP surface. They now surface at type-check time;
the lowering checks stay as defense in depth. This continues the #1464 field-type
check (Phase 2) and matches how enum-variant construction already behaves.

## Explicitly NOT done: missing-required-field enforcement

#1467 also proposed rejecting a **missing required field** — an undefaulted field
(no literal `= expr`, no `@default(...)`) supplied neither positionally nor by
name. This is **deliberately not implemented**, because it would reject a pattern
the language currently supports: an undefaulted field may be left unset at
construction and assigned later. The motivating case is a `@scoped` owner that
constructs itself empty and fills its field in `__enter__`:

```zap
@scoped
struct Arena:
    int64 id = 0
    def __enter__(self) -> Arena: ...
    def __exit__(self): ...

struct Pool:
    Arena a                       # undefaulted
    def __enter__(self) -> Pool:
        self.a = Arena()          # field assigned here, not at construction
        return self
    def __exit__(self): ...

def main():
    with Pool() as p:             # Pool() omits `a` on purpose — must stay valid
        pass
```

The lowerer leaves an undefaulted omitted field null/zero (it fills only
*defaulted* omissions), which is a genuine null-deref footgun for pointer-typed
fields — but whether to make all undefaulted fields **required** at construction
(and how to reconcile it with deferred/`@enter__`-time initialization — e.g.
require them unless the struct is `@scoped`, or introduce a dedicated "late-init"
marker) is a **separate language-design decision**, left open on #1467. This ADR
does not take it.

## Placement

In sema, in `check_struct_field_types` (the `user_types` struct-constructor arm of
`check_call`, `crates/sema/src/lib.rs`) — the same helper the #1464 field-type
check lives in. Diagnostic codes reuse the enum-variant ones (`ZAP4062` arity,
`ZAP4063` unknown field) for parity. A struct with a user-defined designated
constructor takes the `check_user_call` path and is unaffected.

## Consequences

- `P(1, 2, 3)` for a two-field `P`, and `P(bogus=1)`, are now `ZAP4062` / `ZAP4063`
  at type-check time. These programs did not compile before either (too-many was a
  lowering error), so there is no loss of accepted behaviour; the diagnostic just
  moves earlier and gains the sema-only surface.
- Omitting an undefaulted field (`P(1)` for a two-field `P`, `Pool()` above) stays
  accepted — the missing-required decision is deferred.
- No new ZIR `Inst`, runtime call, or monomorph name. Follows #1464
  (`decisions/lang/1464-sema-constructor-field-check.md`), Phase 2 of #1452.

<!-- Core: the arity (ZAP4062) + unknown-field (ZAP4063) checks in
     check_struct_field_types, crates/sema/src/lib.rs. Missing-required-field is
     intentionally deferred (it would break the @scoped deferred-init pattern) and
     remains an open design question on #1467. -->
