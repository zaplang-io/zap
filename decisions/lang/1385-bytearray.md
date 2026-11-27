---
title: Mutable growable byte buffer `bytearray`
status: Accepted
issue: 1385
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# `bytearray` — a mutable, growable byte buffer

**Status:** Accepted (surface + representation + GC model ruled by the maintainer
after RFC [discussion #1693](https://github.com/zaplang-io/zap/discussions/1693)).
Part of the Python-builtin-parity epic (#1387).

## Decision

Zap gains a first-class **`bytearray`**: the mutable, growable sibling of the
immutable [`bytes`](../../crates/codegen/src/runtime/bytes.inc.c). It is the
everyday "build up / patch bytes in place" buffer (wire protocols, encoders,
hashing, I/O framing) that previously had no direct spelling.

`memoryview` — the zero-copy view RFC'd alongside `bytearray` in #1693 — is
**not being built** (decision 2026-11-27, recorded on RFC
[discussion #1693](https://github.com/zaplang-io/zap/discussions/1693) and
issue #1385): it is a niche zero-copy tool, the zero-copy *read* case is already
covered by [`bytesview`](1106-bytesview.md) / [`strview`](1082-borrowed-bytes-strview.md)
(#1082/#1106) and `array` strided views, and the zero-copy-I/O-into-a-buffer case
is to be served later by a targeted `read_into`-style API rather than a view type.
Revisit only if a concrete need appears. So with `bytearray` landed and
`memoryview` explicitly not built, #1385 is fully resolved.

### Representation (R1)

A `bytearray` is a dedicated growable reference object

```c
struct ZapByteArray { int64_t len; int64_t cap; char *data; }
```

where `data` is a **separate, atomic (pointer-free) leaf** byte buffer — the
`ZapList` amortized-doubling growth model (`new_cap = cap < 8 ? 8 : cap*2`) with
the `ZapStr` atomic-buffer discipline. Growth allocates a **fresh** atomic buffer
and `memcpy`s into it (never C `realloc` — it mirrors `zap_list_grow`), then swaps
the `data` slot, so the **header identity is stable across appends** and every
reference observes a mutation.

- The header maps to `Ty::Ptr` in codegen exactly as `bytes`/`list` do — **no new
  IR scalar**; it coerces to/from `Ptr` as a no-op bitcast. A distinct
  `Ty::ByteArray` exists at the sema and ZIR levels purely so lowering can route
  the mutable byte operations (indexed get/set, `append`/`extend`/`+=`, `a + b`,
  `bytes(a)`, byte-wise iteration, content `==`) that the immutable `bytes` and
  the codepoint-based `str` do not share.
- One new runtime descriptor `zap_desc_bytearray` + a `ZAP_TD_BYTEARRAY` GC trace
  case forward **only** the `data` slot; the buffer holds bytes and is never
  pointer-scanned. The header is pinned in place (not in the evacuation move set),
  so its identity survives a moving collection while the `data` buffer relocates
  and its slot is rewritten — no new GC-root machinery.

This is R1 from the RFC, rejecting R2 (`ZapArray[U8]` + growth — carries N-D
`dims`/`strides`/`off` baggage a 1-D buffer does not want, and `ZapArray` has no
growth model) and R3 (`ZapList` with byte elements — an `int64*` word buffer, 8×
the storage, pointer-scanned).

### v1 API

- **Constructors.** `bytearray()` (empty), `bytearray(int)` (that many zero bytes;
  a negative count raises a catchable `ValueError`), `bytearray(bytes)` (copy),
  `bytearray(bytearray)` (copy), `bytearray(list[int])` (one byte per element, each
  `0..=255` or a catchable `ValueError`), and `bytearray(str)` (the UTF-8 bytes of
  the string — matching the bare `bytes(str)` encode, then owning a mutable copy).
  **Every form copies into a fresh buffer**, so the result never aliases the
  source.
- **Indexing.** `a[i]` → `int64` (`0..=255`, unsigned; negative indices count from
  the end); `a[i] = v` mutates in place. An out-of-range index raises a catchable
  `IndexError`; a value outside `0..=255` raises a catchable `ValueError`.
- **Grow / concat.** `append(int)`, `extend(bytes | bytearray | list[int])`, and
  `+=` (same operand types as `extend`, applied **in place**). `a + b` produces a
  **new** `bytearray` for `bytearray + bytearray`, `bytearray + bytes`, and
  `bytes + bytearray`.
- **Convert / compare.** `bytes(a)` materializes an immutable copy; `len(a)` is the
  byte count; iteration yields `int64` (`0..=255`); `==` / `!=` compare by content
  against `bytes` and `bytearray`. A slice read `a[i:j]` yields a fresh
  `bytearray`.
- **Aliasing guarantee.** `bytearray(b)` copies; mutating the result never writes
  the source `bytes`, and `bytes(a)` snapshots so later mutation of `a` never
  changes the returned `bytes`. Pinned by `tests/golden/bytearray_aliasing.zap`.

The built-in methods and constructors are **positional-only intrinsics** (the
`bytes.translate` precedent), so no `crates/syntax/src/seam_sigs.rs` row applies.

### Deferred (not in this PR)

Per the RFC, deliberately out of v1 to keep the surface tight:

- **Slice-assignment-with-resize** `a[i:j] = …` and **`del a[i]`** — both can change
  the length mid-buffer; a focused follow-up.

`memoryview` is **not** on this deferred list — it is not being built at all (see
the Decision section above).

A plain slice *read* `a[i:j]` → a fresh `bytearray` **is** included (it is cheap
and matches the `bytes`/`list` slice-read shape).

### Accepted Python-parity divergences

- Iteration and indexing yield `int64`, not a 1-byte `bytes` (matches `bytes`,
  #1349).
- `bytearray(str)` encodes as bare UTF-8 (no required `encoding` argument), exactly
  mirroring the existing `bytes(str)`.
- `a + b` (the expression) accepts only byte-like operands (`bytearray`/`bytes`);
  `list[int]` is accepted by `extend` and `+=` (the in-place forms) but not by the
  `+` expression, matching CPython where `bytearray + list` is a `TypeError`.
- Ordering (`< <= > >=`) is **not** provided in v1 (the required surface is content
  `==` / `!=`); it can be added if/when `bytes` ordering is relied upon more widely.

## Rationale

`bytearray` delivers the mutable-buffer value immediately with **no new GC
design**: R1 reuses the list growth model and the string atomic-buffer trace
discipline, and the single new trace case forwards exactly one slot. `memoryview`
is dropped rather than carried as a rider: an escapable, writable view under
moving Immix would be a genuinely new GC mechanism, and its value is already
covered by the existing borrowed views and a future `read_into`-style API, so it
is not worth that cost on current evidence.

## Consequences

- Adds the `bytearray` type name, the `Ty::ByteArray` sema/ZIR type, the
  `ZapByteArray` runtime object (`crates/codegen/src/runtime/bytearray.inc.c`), the
  `zap_desc_bytearray` descriptor, and the `ZAP_TD_BYTEARRAY` trace case.
- `bytes`'s copy/slice/compare semantics are unchanged; `bytes(bytearray)` is the
  one new `bytes` constructor form.
- Exhaustive host (sema) + runtime-golden + CPython-differential tests pin the
  surface, the catchable faults, and the no-aliasing guarantee.
