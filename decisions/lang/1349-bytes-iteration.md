---
title: Iterating a bytes value yields its bytes as unsigned int64s in 0..=255
status: Locked
issue: 1349
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Iterating a `bytes` value yields its bytes as unsigned `int64`s in 0..=255

**Status:** Locked

## Decision

A `bytes` value is **iterable**. Iterating it — in a `for` loop, a
comprehension / generator-expression clause, a `*bytes` display splat, or the
`set(...)` / `oset(...)` constructors — yields each raw byte in order as an
unsigned `int64` in `0..=255` (so `0x00`→`0`, `0xFF`→`255`, never
sign-extended to a negative value). Length is the **byte** count (not a
codepoint count), so empty `bytes` takes zero iterations. This is byte-wise and
**not** UTF-8-decoded — deliberately distinct from `str` iteration, which yields
Unicode codepoints as `char` (D-0025). It matches Python, where `for b in
b"...":` binds integers.

Concretely: `iterable_elem(bytes) = int64` in sema (`zapc_sema`), `iter_elem`
maps `bytes → int64` in the monomorphizer (`zapc_mono`, the empty-`TypeMap`
native-`zap test` path), and the `for`-loop / general-iteration lowerings
(`zapc_ir`) grow a `bytes` arm (`lower_for_bytes` / `iterate_bytes`) that walks
`0..zap_str_len` reading each byte via `zap_bytes_index`, the same unsigned
accessor `b[i]` already uses (I#337).

## Rationale

The loop type-checked under some type assignments (e.g. a `bytes` value that
reached codegen typed `Unknown` / `py`, as in the datetime-utils parser
iterating a bytes token) yet fell through the `for`-loop lowering's type
dispatch to the catch-all ZAP6001 ("only `for ... in range(...)`, … are
supported by codegen yet") — accept-then-fail, the worst outcome. The issue
offered a fallback: make sema reject `bytes` iteration with the ordinary
non-iterable diagnostic. That was rejected because `bytes` **is** iterable in
Python and a downstream consumer (zaplang-io/datetime-utils#4) needs it; closing
the surface would push every such consumer to an index-loop workaround.

Element representation — the live hazard — was fixed by mirroring `b[i]`
(I#337), which returns `(int64)(unsigned char)`, rather than the `str` path:

- **Unsigned `int64`, not a 1-char `str`.** The #1074 bytes-collapse hazard
  (a `bytes` element decaying to a raw `Ptr` read back as `str`) would have made
  the loop variable a string; it is an integer, so arithmetic, comparison, and
  use as a list index all type-check, and `str` methods on it are rejected.
- **Unsigned, not sign-extended.** Relatedly #1344 fixed a `bool`
  sign-extension bug; here `0x80`/`0xFF` stay `128`/`255`, never negative,
  because `zap_bytes_index` zero-extends the `unsigned char`.
- **Byte length, not codepoint length** (`zap_str_len`, as `len(bytes)` already
  uses), so iteration visits every byte of a multi-byte sequence.

Making `bytes` iterable in sema necessarily opens every iteration construct, not
just the statement `for`. The lowering fix therefore extends the **shared**
D-0048 iteration protocol (`iterate` → new `iterate_bytes`), so comprehensions,
generator expressions, `*bytes` splats, and the `set`/`oset` constructors all
lower too — no construct is left accept-then-fail. The closure form
(`iterate_bytes`) and the statement form (`lower_for_bytes`) share the same
index-walk shape as `lower_for_list`, confirmed against list iteration via
`zap ir`; no new runtime helper is introduced.

## Consequences

- `for b in data:`, `[b for b in data]`, `{b for b in data}`, `(b for b in
  data)`, `[*data]`, `set(data)`, and `oset(data)` over a `bytes` `data` all
  compile; each byte is an `int64` in `0..=255`, in order; empty `bytes` is zero
  iterations.
- `str` iteration is unchanged (codepoints as `char`); the two element types
  stay distinct, so a change that regressed `bytes` iteration to `char` is
  walled off by the sema/lowering tests.
- Builtins and operators that take a `bytes` value *directly* rather than
  through the iteration protocol — `sum(bytes)`, `sorted(bytes)`, and `x in
  bytes` — remain unsupported in codegen (they already type-checked and failed
  before this change; this change does not alter their acceptance). They are a
  **pre-existing**, independent gap on the `bytes` type, reported separately and
  deliberately not fixed here (one issue, one PR; the core-defect house rule).
  `min(bytes)` / `max(bytes)` already reject cleanly (ZAP4024), so they are not
  gaps.

<!-- Related: #1074 (bytes-collapse hazard), I#337 (bytes indexing/`zap_bytes_index`), #1344 (bool sign-extension), D-0025 (str codepoint iteration), D-0048 (the Iterable protocol). -->
