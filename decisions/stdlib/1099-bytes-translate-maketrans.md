---
title: Bulk byte-table translation — bytes.maketrans + bytes.translate
status: Locked
issue: 1099
pr:
date: 2026-11-19
domain: stdlib
supersedes:
superseded-by:
---

# Bulk byte-table translation — `bytes.maketrans` + `bytes.translate`

**Status:** Locked

## Decision

Add Python's byte-table translation primitive to the built-in `bytes` type,
closing #1099. This is **additive** on `bytes`; `str.translate(dict)` (#1034) is
unchanged.

- **`bytes.maketrans(frm, to) -> bytes`** — an associated constructor
  (classmethod-like, called on the type: `bytes.maketrans(...)`) that returns a
  freshly-allocated **256-byte** translation table: the identity map
  (`table[i] == i`) except that each byte `frm[i]` maps to `to[i]`. `frm` and
  `to` are both `bytes` and **must have equal length** — a mismatch raises a
  catchable **`ValueError`** (CPython raises `ValueError`). A later `frm[i]`
  overrides an earlier one for the same source byte (left-to-right fill, matching
  CPython).

- **`b.translate(table) -> bytes`** and **`b.translate(table, delete) -> bytes`**
  — rewrite the whole buffer in **one native pass**: `out[i] = table[b[i]]`.
  `table` must be exactly **256 bytes**; any other length raises a catchable
  **`ValueError`**. With a `delete` set (also `bytes`), any input byte whose value
  appears in `delete` is **dropped before translation** — i.e. **deletion wins
  over remapping**, matching CPython. Bytes are unsigned (`0..=255`), so the high
  bytes `0x80..0xFF` index the table with no sign extension.

Both results are freshly-allocated `bytes` (a `ZapStr` over a pointer-free leaf
buffer), reusing the existing `zap_buf` / `zap_str_adopt` allocation path, so the
precise collector traces them like every other `bytes`-producing op.

**Arguments stay positional (no keyword form).** `b.translate(table[, delete])`
and `bytes.maketrans(frm, to)` are **positional-only**, matching CPython (whose
C-level `bytes.translate` / `bytes.maketrans` reject keywords). Built-in
intrinsic methods on `bytes`/`str` are hand-written check/lower dispatch in
`zapc_sema` / `zapc_ir` — exactly like `str.translate`, `str.split`, and
`bytes.view` — and are **not** seam module functions. The seam keyword binder
(`crates/syntax/src/seam_sigs.rs`) keys on a **module** name (`zlib`, `http`, …);
there is no `bytes` seam module, so these methods never reach it. Consequently no
`seam_sigs.rs` row and no `seam_kwargs.rs` case applies; the recommendation is
**positional**, recorded here per the workspace stdlib rule (AGENTS.md).

## Rationale

- **Replaces a 25M-iteration interpreted loop with two native passes.** Surfaced
  by the benchmarks review (zaplang-io/zap-benchmarks#52, reverse-complement):
  without a bulk primitive the Zap port must do a per-byte `table[seq[i]]` loop
  over the whole input, a large part of why that leg ran **>6.6x slower than
  CPython** (which does the complement as one `bytes.translate` plus a bulk
  reverse). `bytes.translate` makes the hot path one C pass. It is also the
  natural primitive for any byte-rewriting workload (encoders, codecs, text
  normalisation), not just this benchmark.

- **Why a separate `bytes` byte-table method, not overloading `str.translate`.**
  `str.translate(dict[str, str])` (#1034) is a **code-point dict map**: it keys on
  1-char strings and allocates per mapping. That is the wrong shape for byte
  workloads — it cannot accept a 256-byte table or a delete set, and it iterates
  code points, not bytes. Python itself keeps these distinct (`str.translate`
  takes a dict/mapping; `bytes.translate` takes a 256-byte table + optional
  delete set), and so does Zap. Overloading one method name across the two
  receiver types with two incompatible argument shapes would be a trap; dispatch
  is by receiver type (`str` vs `bytes`), and each keeps its Python shape.

- **Deletion wins over remapping — CPython parity.** CPython's
  `bytes.translate(table, delete)` deletes first, then translates the survivors,
  so a byte that is both in `delete` and remapped by `table` is **dropped**. Zap
  matches this exactly (the delete-set check precedes the table lookup in the
  single pass).

- **Faults are raised the catchable way.** The length checks
  (`maketrans` unequal `frm`/`to`; `translate` table ≠ 256 bytes) raise
  `ValueError` via `zap_panic_typed(ZAP_EXC_VALUE_ERROR, …)`, and the lowering
  reaches both runtime calls through `throwing_call` (an `invoke` inside a `try`),
  so `try/except ValueError` catches them — the same discipline the existing
  `bytes(list)` / `str(bytes)` / `str.format_map` faults follow. A plain
  `emit(Call)` would `std::terminate` instead of unwinding to the handler.

- **No new ZIR instruction, so no GC-rooting hazard.** Both methods lower to the
  existing `Inst::Invoke` shape (`throwing_call`) with a `Ty::Bytes` dest, which
  `codegen inst_dest()` already roots like every other call/invoke result; no new
  `Inst`/`Elem`/`Expr` variant is introduced, so no exhaustive-match site changes.
  The runtime entries live in the existing `crates/codegen/src/runtime/bytes.inc.c`
  fragment (no new `#include`, so no `lib.rs`/`jit.rs`/`build.rs` write sites).

### Rejected alternatives

- **Extend `str.translate` to also accept a 256-byte `bytes` table.** Rejected:
  it conflates two Python APIs with incompatible argument shapes on one name, and
  `str` is code-point-indexed while the table is byte-indexed. Keeping a
  `bytes`-only method mirrors Python and keeps each surface single-shaped.
- **Make `delete` a keyword argument.** Rejected: CPython's `bytes.translate` is
  positional-only, and these intrinsics are not seam functions, so there is no
  keyword-binder machinery to carry a default — positional matches both Python
  and the existing `bytes`/`str` method surface.
- **Raise an untyped panic (as the older `zap_bytes_raise` codec faults do).**
  Rejected in favour of a typed `ValueError`, matching CPython's exception type
  and letting callers write `except ValueError`.

## Consequences

- The `bytes` surface gains two methods (`maketrans` associated constructor +
  `translate` instance method); `str.translate(dict)` is untouched and still
  covered by `tests/golden/str_translate.{zap,expected}`.
- New runtime symbols only (`zap_bytes_maketrans`, `zap_bytes_translate`) — no
  RUNTIME-ABI break.
- Behaviour is pinned by `tests/golden/bytes_translate.{zap,expected}` (identity
  / single-byte remap / DNA complement / empty input / delete-set
  overlapping+non-overlapping+empty / deletion-wins / non-ASCII `0x80..0xFF` /
  maketrans+translate round-trip), the catchable-fault golden
  `tests/golden/bytes_translate_reject.{zap,expected}` (table too short / too
  long / maketrans unequal length), and the host sema walled-garden tests
  `bytes_translate_maketrans_type_check` / `bytes_translate_maketrans_reject_bad_args`.

<!-- Related: str.translate(dict) code-point map is #1034; the bytes value type is #337; bytes.view / bytesview is #1082. -->
