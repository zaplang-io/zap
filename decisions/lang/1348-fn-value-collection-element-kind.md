---
title: A function-value collection element keeps its signature as a dedicated element kind
status: Locked
issue: 1348
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# A function-value collection element keeps its signature as a dedicated element kind

**Status:** Locked

## Decision

A first-class function value (`fn(..) -> R`, I#163) stored as a collection
element — a `list`/`dict` value/`set`/`channel` element, directly or inside a
collection-typed struct field — is now carried by a dedicated element kind,
`Elem::Func(sig)`, where `sig` is the function's signature index (into
`Module::fn_sigs`). It mirrors the existing `Elem::Nested` (#625), `Elem::Bytes`
(#1074), and `Elem::Complex` (#928) kinds: physically the element is still a
single closure-record pointer word (so it pushes/reads through the `_ptr`
runtime entry points and GC-traces as a pointer, unchanged), but the kind
retains the function *signature* so read-back recovers the exact `Ty::Func(sig)`
rather than defaulting a bare pointer to `str`.

Consequently a function value read back from a collection — by **iteration**
(`for cb in callbacks:`), by **indexing** (`callbacks[0]`), or bound through an
unannotated `auto cb = callbacks[0]` — carries its `fn(..) -> R` type at the
call site, so `cb()` lowers to an indirect `CallClosure` through the closure
record. A function value is not `Hashable`, so `Elem::Func` never occupies a
dict/set *key* position; it is not orderable, so (like a struct/nested/complex
element) a `list[fn ..]` must be sorted via an explicit `key=`.

## Rationale

`callback()` — where `callback` is read from a `list[fn() -> int64]` by
iteration or indexing — failed IR lowering with `ZAP6001 "call to unknown
function 'callback'"` even though the program type-checked. The call lowerer
resolves a bare-name call by first looking the name up as a local of function
type (`lookup(name) -> Ty::Func`), which routes to the indirect
`lower_closure_call`; only if that fails does it fall back to a top-level
function-name lookup, which errored. The element kind collapsed every reference
element — including a `fn(..) -> R` — to `Elem::Ptr`, whose read-back type is
`Ty::Ptr`, so the loop variable / indexed value was typed as a bare pointer, the
`Ty::Func` branch was never taken, and the name fell through to the failing
top-level lookup.

An *annotated* read-back (`fn(int64) -> int64 g = ops[0]`) already worked,
because the annotation re-attaches the signature through `expr_coerced`; the
iteration and `auto`-binding forms have no annotation to re-attach it, so they
needed the element kind to carry the signature itself. This is the same
element-type-collapse family as #625/#1074/#1329: the general fix is to preserve
the element's type across the collection boundary, not to special-case the call
site.

Carrying the signature index directly in `Elem::Func(u32)` (rather than through
a side interner like `Elem::Nested`'s lowering-only `NestedInterner`) is
sufficient and simpler: the index is already module-stable (`resolve_ty` interns
the signature into `Module::fn_sigs` and hands back `Ty::Func(idx)`), so
`Elem::ty` can rebuild the exact `Ty::Func(idx)` with no extra state. Codegen
never inspects an `Elem` variant — it dispatches purely on the runtime
entry-point suffix (`ptr`), which is unchanged — so the new kind needs no
codegen change and every exhaustive `Elem` match is caught by the host build.

## Consequences

- A `fn(..) -> R` element read back from any value-position collection (`list`
  element, `dict` value, `set` member, `channel` element) — local or a struct
  field, iterated or indexed — recovers its function type and is callable; the
  call lowers to an indirect `CallClosure`. The otel #3 observable-gauge shape (a
  `list` of callbacks each returning a `list` of observations) now lowers and
  runs.
- Storage, GC tracing, and the runtime ABI are byte-identical to the previous
  `Elem::Ptr` lowering (a closure record is a traced pointer word), so existing
  fixtures that stored function values and read them back through an *annotated*
  binding are unchanged.
- A function value remains unusable as a dict/set key (unhashable) and a
  `list[fn ..]` remains unsortable without a `key=` projection — both reported
  with the existing diagnostics.
- The direct first-class-fn call path (a function value held in a plain
  variable, I#163) is unchanged and regression-guarded.

<!-- Related: #163 (first-class function values / closure records), #625 (Elem::Nested, the nested-collection element kind this mirrors), #1074 (Elem::Bytes), #1329 (for-loop element-type binding in the same collapse family). -->
