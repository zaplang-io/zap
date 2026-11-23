---
title: A for-loop variable binds the iterable's element type in monomorphization
status: Locked
issue: 1329
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# A for-loop variable binds the iterable's element type in monomorphization

**Status:** Locked

## Decision

When the monomorphizer (`zapc_mono`) rewrites a runtime `for target in iter:`
loop, it now declares a single-name loop variable with the iterable's **element
type** — `Mono::iter_elem` applied to the iterable's best-effort concrete type —
instead of `CTy::Unknown`. This is the same binding a comprehension clause
already gets (`Mono::rw_comp_clauses`, #1131). A non-iterable or opaque iterable
(a `py` handle, a user iterator mono cannot type) still leaves the variable
`Unknown`, exactly as before — the change only ever *adds* a type, never removes
the fail-closed behaviour.

Consequently a generic call on the loop variable (`item.collect() ->
Optional[T]`) is typed by mono's own inferer, so a `match` on that result pins
the generic enum's instance and lowers, including when the iterable is a
`list`-typed struct **field** (`for counter in self.counters:`).

## Rationale

`match counter.collect()` failed to lower with `ZAP6001` ("cannot determine the
instance of generic enum 'Optional' for this match pattern") whenever the loop
receiver was iterated from a `list` struct field. The match-pattern rewrite
(`Mono::rewrite_pattern`) takes the generic enum's instance args from the
**scrutinee's type**; with the loop variable declared `Unknown`, the
`counter.collect()` call could not be typed, so the scrutinee type was unknown
and the instance could not be pinned.

The asymmetry the issue reported — a local `list[Counter]` worked, a field
`list[Counter]` did not — was a consequence of the `TypeMap` backfill (#1169):
sema records a type at the scrutinee span only for forms it types. Sema types a
loop variable iterated from a **local** `list` (its element type flows from the
local's declared type), so the backfill closed the local case; sema does **not**
type a struct **field read** at all (`infer`'s `Expr::Attr` value arm returns
`Ty::Unknown` — a separate, broader sema defect, reported as #1337), so the
field case reached mono untyped and the backfill had nothing to supply.

Two fix sites were weighed:

1. **Bind the for-loop variable's element type in mono** (this decision). Mono
   already resolves a struct field read's type (`field_ty`) and a container's
   element type (`iter_elem`); the only gap was that the plain `for` loop — unlike
   a comprehension clause (#1131) — never bound the element type. This makes
   mono's own inferer self-sufficient for every built-in-container loop (local or
   field), independent of the sema backfill, and is the "general element-type
   binding" fix rather than a special-case at the match site.
2. **Fix sema to type struct field reads** (#1337). Correct and desirable, but
   broader type-inference/diagnostics machinery (it also changes which
   assignment/argument mismatches sema catches) and orthogonal to this loop gap:
   even with field reads typed, the plain-`for` binding gap would still force
   every field-list loop through the backfill rather than mono's own inferer.
   Left to its own issue; this fix deliberately does **not** work around or
   depend on it (per the core-defect house rule).

Site 1 mirrors an established precedent (#1131), reuses existing helpers
(`iter_elem`, `field_ty`), and keeps the `ZAP6001` diagnostic reachable for a
genuinely un-annotatable element (#1190 boundary).

## Consequences

- Every runtime `for v in ITER:` over a built-in container (`list`/`set`/
  `heap`/`deque`/`oset`/`dict`/`range`/`str`) now binds `v` with its element
  type in mono, so a generic call / `match` on `v` infers without an explicit
  `Optional[...]` annotation, whether `ITER` is a local or a struct field.
- Tuple-destructuring targets (`for k, v in d.items()`) and non-`Name` targets
  are unchanged (they declared nothing before and still do); an opaque or
  user-iterator element stays `Unknown`, so the match-pattern `ZAP6001` remains
  reachable and is still reported cleanly.
- The sema `TypeMap` backfill (#1169) is unchanged and remains the fallback for
  forms mono's own inferer cannot type.
- Does not address #1337 (sema not typing struct field reads); that defect still
  lets some field-read type mismatches go uncaught and is tracked separately.

<!-- Related: [lang/1169-mono-consumes-sema-types](1169-mono-consumes-sema-types.md) (the TypeMap backfill), #1131 (the comprehension-clause element-type binding this extends), #1337 (sema field-read typing gap). -->
