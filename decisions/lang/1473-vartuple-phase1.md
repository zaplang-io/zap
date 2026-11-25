---
title: Variadic tuple[T] — Phase 1 (type, construction, core ops)
status: Locked
issue: 1473
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `tuple[T]` Phase 1

**Status:** Locked. Phase 1 of the variadic-tuple epic #1461 (RFC discussion
#1472). Surface decisions locked on the epic: `tuple[T]` spelling (single type
arg = variadic homogeneous; `(A, B)` stays the fixed tuple), Python-parens repr,
full-sequence v1. This phase ships the type and its construction plus
index/iterate/len/repr; equality+hash (#1474) and ordering/concat/slicing
(#1475) follow.

## Decision

Introduce `tuple[T]`: a dynamic-length, immutable, homogeneous sequence — the
type Python's `tuple(iterable)` returns, and (from #1474) a hashable `dict`/`set`
key. It is a GC-managed reference type like `list`, but immutable.

- **Type**: a new `Ty::VarTuple(Box<Ty>)` (sema) / `Ty::VarTuple(Elem)` (IR).
  `tuple[T]` type syntax resolves to it (one type arg); `(A, B)` and the fixed
  `Ty::Tuple` are unaffected.
- **Construction**: `tuple(iterable)` (element type inferred like `list(...)`),
  `tuple[T]()` (empty), `tuple[T](iterable)` (typed).
- **Core ops (this phase)**: `t[i]` (runtime index, negative allowed — unlike
  the const-index fixed tuple), `for x in t`, `len(t)`, and the repr/str form
  `(1, 2, 3)` / `(1,)` / `()`.

## Representation

A `tuple[T]` is physically a **frozen `ZapList` buffer**: it reuses `zap_list_new`
/ `zap_list_push_*` to build, `zap_list_get_*` to index, `zap_list_len`, and the
list iterator and GC tracing. **No new runtime entry points** this phase — the
only runtime-visible difference from `list` is the `(…)` repr (a dedicated
parenthesized container renderer, with the single-element trailing comma). In
codegen `Ty::VarTuple` maps to `ptr` exactly like `Ty::List`.

The type is kept distinct from `list` purely so the value layer renders it
Python-tuple style, so it stays immutable (it exposes no mutators), and so it can
be made hashable in #1474.

## Why a new `Ty` variant (not `list` reuse)

A `list`-typed value renders `[…]` and is mutable; `tuple[T]` must render `(…)`,
be immutable, and (next phase) hashable. `render_value`/method dispatch key on
the IR/sema type, so the distinction has to live in the type. The new variant
is ABI-identical to `list` (a traced pointer), so it threads through the
exhaustive `Ty` matches by sitting alongside `Ty::List` — a single codegen
`llvm_type` arm, the element-kind/`iterate`/index/len/repr arms — and adds no
new runtime object.

## Consequences

- New `Ty::VarTuple` variant across sema_types, ir_core, sema, lowering, codegen
  (one `ptr` arm). The end-to-end behaviour (construct/index/iterate/len/repr) is
  LLVM-CI-gated via golden `vartuple_basics` + CPython differential `vartuple`;
  sema is covered host-side by `vartuple_codes`.
- Follow-up phases: #1474 (`==`/hash → dict/set key), #1475 (ordering/concat/
  slicing), #1476 (exhaustive goldens + SPEC/STABILITY). Nested-`tuple[T]`
  read-back repr (a `tuple[T]` element inside another collection) is deferred to
  the exhaustive phase.
