---
title: Borrowed-bytes `bytesview` — non-owning view over `bytes`
status: Locked
issue: 1082
pr: 1106
date: 2026-11-19
domain: lang
supersedes:
superseded-by:
---

# Borrowed-bytes `bytesview`

**Status:** Locked. The `bytes` counterpart of `strview`
([lang/1082](1082-borrowed-bytes-strview.md), merged in #1101), implementing the
`bytesview` follow-up that ADR deferred.

## Decision

`bytesview` is a non-owning view over an existing `bytes` value's storage, the
direct analog of `strview`, with the same zero-alloc / zero-copy read-only
consumers and the same affine soundness rule (`ZAP5135`).

- **Surface.** Created with `b.view()` / `b.view(lo, hi)` and sub-viewed with
  `v.view(lo, hi)`. Bounds are **byte** indices (I#337 — `bytes` is byte-indexed,
  not codepoint-indexed like `str`). Methods are the read-only subset:
  `startswith`/`endswith`/`contains` (argument `bytes` or `bytesview`), `len()` /
  `len(v)` (byte count), `__hash__`, `==`/`!=` (vs `bytes` or a view), and
  `to_bytes()` to materialize an owned copy. `v in d` and `d.get(v[, default])`
  probe a `bytes`-keyed dict with the borrowed bytes.
- **Representation.** Lowers to a reserved by-value tuple `(bytes, int64, int64)`
  with its own `push_layout` id — distinct from `strview`'s and from any natural
  `(bytes, int64, int64)` tuple — so the three are never confused for `==` /
  `print` / method dispatch. The backing `bytes` pointer is traced and rewritten
  on a moving-Immix evacuation via the aggregate-root `(base, desc)` pairs (#586),
  exactly as for `strview`.
- **Runtime reuse.** A `bytesview` and a `strview` have identical `(base, off,
  len)` storage over the same `ZapStr` representation, so the probes, `==`, hash,
  dict lookup and the `to_str`/`to_bytes` materializer **reuse the `zap_strview_*`
  and `zap_dict_*_strview` entry points** verbatim. Only creation / sub-view are
  `bytes`-specific (`zap_bytesview_{off,len,sub_off,sub_len}` — byte clamping, no
  UTF-8 walk), and `len` is the byte-length field itself.

## Rationale

The `bytes` view is the obvious completion of the borrowed-bytes feature and was
explicitly listed as a follow-up in [lang/1082](1082-borrowed-bytes-strview.md).
Modeling it as a second reserved aggregate (rather than reusing `strview`'s id or
a generic "view") preserves the same no-collision guarantee that keeps a view
distinct from a look-alike tuple, while the shared `ZapStr` storage lets the
entire read-only runtime be reused — the only genuinely new code is byte-indexed
bound clamping. Byte indexing (not codepoints) matches `bytes`'s existing
slice/`len` semantics (I#337), so `b.view(lo, hi)` lines up with `b[lo:hi]`.

## Consequences

- Adds the `bytesview` type + keyword; `bytes[a:b]` copy semantics are unchanged.
- The affine rule, the dict-key acceptance, and the `Let`-collection guard are
  generalized to cover both views, so a `bytesview` in a struct field / global /
  collection / return position is the same `ZAP5135` error as for `strview`.
- Deferred items from [lang/1082](1082-borrowed-bytes-strview.md) that remain:
  `int()`/`float()`/`print()` taking a view directly (today: materialize);
  `find`/`index`/`count`/`is*`; and the implicit borrow at the call boundary.
- GC soundness for the moving collector is CI-only (gc-immix x86 + arm64); the
  `tests/golden/bytesview_gc_survive.zap` golden exercises a `bytesview` held
  across forced collections.
