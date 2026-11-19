---
title: Borrowed-bytes `strview` — explicit non-owning string view for read-only consumers
status: Locked
issue: 1082
pr: 1101
date: 2026-11-19
domain: lang
supersedes:
superseded-by:
---

# Borrowed-bytes `strview`

**Status:** Locked (surface + representation ruled by the maintainer in-session,
2026-11-19, after RFC [discussion #1097](https://github.com/zaplang-io/zap/discussions/1097)).

## Decision

Zap gains an explicit, first-class **`strview`** type: a non-owning view over an
existing string's bytes, for read-only consumers (`dict.get`/`in`, `set`
membership, hash, `==`, `startswith`/`endswith`/`contains`, length,
materialization) that operate with **zero heap allocation and zero copy**.

- **Surface.** A `strview` is created explicitly — `s.view()` (whole string),
  `s.view(lo, hi)` / `s.view(lo)` (codepoint-indexed, D-0025), and sub-viewed with
  `v.view(lo, hi)`. `str[a:b]` slice semantics are **unchanged** (still a fresh
  owned copy); views are opt-in, so nothing in existing code changes behavior.
  `strview` methods are the read-only subset only: `startswith`/`endswith`/
  `contains` (arg may be `str` or `strview`), `len()` / `len(v)`, `__hash__`,
  `==`/`!=` (against `str` or `strview`), and `to_str()` to materialize an owned
  copy. Read-only consumers accept a `strview` where they take a `str`: `v in d`
  and `d.get(v[, default])` on a `str`-keyed dict probe the borrowed bytes.
  Owning / mutating string methods are deliberately absent — materialize with
  `to_str()` first.

- **Representation (runtime, cross-referenced).** A `strview` lowers to a by-value
  tuple `(backing: str, byte_off: int64, byte_len: int64)`. The backing is a real,
  traced GC pointer, so on a moving-Immix evacuation the collector rewrites it in
  place via the existing aggregate-root `(base, desc)` pairs
  ([runtime/586 machinery](../../crates/codegen/src/lib.rs)) — this is the issue's
  **"2b" base+offset interior-pointer** representation, obtained with **no new
  GC-root instruction and no new `inst_dest()` entry**. Read-only consumers gain a
  `(base, off, len)` **view ABI** (`zap_strview_*`, `zap_dict_*_strview`) that reads
  `backing->data` fresh on every call and never caches a raw data pointer across a
  safepoint. The hash is the same seeded wyhash the owned `str`/dict path uses
  ([#1064](https://github.com/zaplang-io/zap/issues/1064)), so a view hashes
  identically to its materialized string.

- **Affine soundness (`ZAP5135`).** A `strview` is affine: valid only while its
  backing lives. It may **not** be a struct field, a module global, a collection
  element / dict key-or-value, or a function **return** type. Because `strview` is a
  distinct type and is **not** assignable to/from `str`, those type-position
  rejections are a complete, sound fence — every remaining escape is blocked by
  ordinary type mismatch. An intra-frame local and a `strview` **parameter** are
  always sound: the view embeds its backing as a traced pointer, so it keeps its own
  backing alive and the collector rewrites it.

## Rationale

- **Why explicit, not implicit (Q1).** The RFC recommended an implicit borrow at
  the call boundary (no user-visible type) for the common case. The maintainer ruled
  for an explicit named type now: it gives a first-class value programmers can hold
  across a loop/call, which is exactly what makes `2b` (cross-safepoint) tracking the
  core rather than an edge case, and it is the piece the implicit analysis could not
  express for hot paths.
- **Why the tuple / `2b` representation (Q2/Q3).** The alternative "stack-materialized
  `ZapStr` header" (R1) cannot avoid copying a substring's bytes (`ZapStr` is one
  flexible-array object — a header cannot point at bytes living elsewhere), so it is a
  strict subset of the intra-frame stack-promotion in
  [#1081](https://github.com/zaplang-io/zap/issues/1081) and does not meet the
  zero-copy bar. The `(base, off, len)` view ABI (R2) is genuinely zero-copy. Requiring
  `2b` in v1 (not just the no-safepoint `2a` optimization) is what makes a `strview`
  safe to hold across a safepoint; modeling it as a value-struct tuple with one pointer
  field reuses the already-proven #586 aggregate-root machinery, so the collector
  rewrites the backing for free and the #120/#223-class "a new pointer-defining Inst
  must be in `inst_dest()`" hazard is sidestepped by construction.
- **Why type-position affine enforcement.** A `strview` keeps its own backing alive
  within a frame, so intra-frame use needs no dataflow analysis at all. The only
  unsoundness is escape to a longer-lived slot, and since `strview` is not assignable
  to `str`, the only way to express such a slot is to *name* `strview` in a long-lived
  type position — so rejecting those positions is both sound and complete, and reuses
  the `@scoped` home (`crates/sema/src/scoped.rs`, [lang/876](https://github.com/zaplang-io/zap/issues/876)).
- **Boundary with #1081 (Q5).** The `dict_string` `f"k{i}"` loop is an intra-frame
  build-and-die case already covered by #1081's escape/stack-promotion (the
  `call_is_non_retaining` allowlist in `escape.rs` already lists the read-only
  consumers). `strview`'s distinct, non-overlapping win is the **slice/substring +
  existing-value borrow** — `d.get(s[a:b])`, which today allocates and copies via
  `zap_str_slice`.

## Consequences

- One PR delivers the type, surface, view ABI, `2b` GC representation, affine rules,
  and the read-only consumer set (`dict.get`/`get_default`/`in`, `==`/`!=`, `__hash__`,
  `len`, `startswith`/`endswith`/`contains`, `to_str`, sub-view). The zero-alloc
  property is visible in the lowered IR: the `d.get(v)` / `v in d` path emits only the
  `zap_dict_*_strview` probes and the `strview` tuple build — no `zap_str_new` /
  `zap_str_slice`, and the view carries no owned-string shadow root of its own.
- **Follow-ups** (not in v1, no epic): `bytesview` (the same over `bytes`);
  `int()`/`float()`/`print()` taking a view directly (today: materialize with
  `to_str()`); `find`/`index`/`count`/`is*` on a view; and the implicit borrow at the
  call boundary (the RFC's Q1 preferred form) as an ergonomic layer over this explicit
  type. Returning a view of a parameter needs lifetime tracking deferred past v1.
- GC soundness for the moving collector is validated only by the full LLVM build
  (`gc-immix` x86 + arm64); the `tests/golden/strview_gc_survive.zap` golden exercises a
  view held across forced collections after heap churn.

<!-- Runtime/ABI detail (view ABI symbols, aggregate-root reuse) lives with this
decision rather than a separate runtime ADR, since the surface and the representation
were ruled together; cross-referenced from the view ABI block in
crates/codegen/src/runtime/array.inc.c and the dict probes in crates/codegen/src/zaprt.c. -->
