---
title: Representation built-ins repr / ascii / bin / oct / hex
status: Locked
issue: 1384
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Built-in `repr` / `ascii` / `bin` / `oct` / `hex`

**Status:** Locked

## Decision

Add the five Python string-representation built-ins, all returning `str`. They
touch the one value/number rendering path (#274, D-0124), exposing its debug
side and adding integer base formatting.

- **`repr(x)`** — the unambiguous/debug form: the `RenderMode::Repr` side of the
  Display protocol. A `str` is Python-quoted (`'hi'`), `bytes` is the `b'…'`
  form, a container renders with its elements in `repr` (`['a', 'b']`, not
  `[a, b]`), numbers/`bool`/`char` render as their value, and a user struct /
  enum resolves `__repr__` / `@derive(Repr)`. Accepts exactly the operand set
  `str()` accepts.
- **`ascii(x)`** — `repr(x)` with every **non-ASCII codepoint** backslash-escaped
  (`\xNN` for cp ≤ 0xff, `\uNNNN` for cp ≤ 0xffff, else `\UNNNNNNNN`). ASCII
  output of `repr` passes through unchanged.
- **`bin`/`oct`/`hex`** — an integer rendered in the Python **sign-magnitude**
  base form with the `0b`/`0o`/`0x` prefix and lowercase digits: `hex(255) ==
  "0xff"`, `bin(-5) == "-0b101"` (never two's-complement), `bin(0) == "0b0"`.
  Accepts any integer (`int*`, `uint*`, `usize`, `bigint`); an unsigned value
  renders its true magnitude with no sign.

### The `repr` vs `str` dunder contract (the decision this issue raised)

Python's model: `str(x)` calls `__str__` (falling back to `__repr__`), `repr(x)`
calls `__repr__`, and a **container always renders its elements with `repr`** —
so `str([point])` uses `repr(point)` too. Zap previously had one renderer per
type that preferred `__str__` and was used for both `str()` and container
elements.

**Locked: Python-exact.** A render site now carries a preference threaded
through `render_value` → `struct_to_str` → `struct_repr_fn` (`prefer_repr`):

- **str contexts** — top-level `str()` / `print()` / an f-string field — prefer
  `__str__` (unchanged).
- **repr contexts** — `repr()` and **every container element** (in both `str()`
  *and* `repr()` of a container) — prefer `__repr__`.

For a type defining only one of the two dunders, or neither, the preference is
immaterial (the single hand-written renderer, or the `@derive(Repr)` / default
field-wise renderer, is used either way). The change is therefore observable
only for a type defining **both** `__str__` and `__repr__`, and only when such a
value is rendered via `repr()` or as a container element — no existing golden
exercises that, so there is no behavior change to committed fixtures.

## Rationale

- **Reuse, don't re-derive.** `repr` is the `RenderMode::Repr` path that already
  backs container-element and nested rendering (`zap_str_repr`,
  `zap_bytes_repr`, the per-container and per-struct repr renderers), so
  exposing it as a built-in is a thin wiring, correct by construction across
  every type the Display protocol supports.
- **Python-exact repr/str, because libraries depend on it.** A type that
  distinguishes a friendly `__str__` from a debuggable `__repr__` must get the
  Python contract (`repr()` → `__repr__`, containers → `__repr__`), or library
  authors would have to re-check Zap's rendering every time they add a second
  dunder. Threading one `prefer_repr` bit is the minimal faithful mechanism and
  leaves every single-dunder and no-dunder type unchanged.
- **Sign-magnitude `bin`/`oct`/`hex`.** Matching CPython exactly (`-0b101`, the
  `0x`/`0o`/`0b` prefixes, lowercase, `x == 0` → `0b0`) keeps ported code and
  differential tests honest. Unsigned types print their true magnitude (no
  phantom sign), consistent with `zap_u64_to_str` (#552). `bigint` formats by
  limb short-division, the same kernel shape as `zap_bigint_to_str`.
- **Never raise.** The base kernels take a compile-time-constant base and an
  operand sema has already proven integer, so they are plain (non-throwing)
  calls. `ascii` escaping is total (a malformed UTF-8 lead byte escapes as the
  raw `\xNN`).

### Typing

- `repr`/`ascii`: accept the `str()` operand set; return `str`. Reject a
  non-renderable operand (ZAP4024), wrong arity (ZAP4025).
- `bin`/`oct`/`hex`: accept any integer (`is_int()`); return `str`. `bool`,
  `float`, and `complex` are rejected (ZAP4024) — no Python int-subclass
  coercion. Positional only; `seam_sigs.rs` is not touched.

## Consequences

- `repr`/`ascii`/`bin`/`oct`/`hex` now shadow user free functions of those
  names, like the other built-ins.
- New runtime kernels: `zap_ascii_escape` (post-processes a `repr` string),
  `zap_int_to_base` / `zap_uint_to_base` / `zap_bigint_to_base` (integer base
  formatting); their behavior is exercised by goldens and gated by the LLVM-18
  CI job.
- **Known follow-up (#707 family):** an interface value carries a single
  renderer slot per itable, kept str-preferred so `print`/`str` of an interface
  value is unchanged. Therefore `repr()` of a **bare interface value** whose
  concrete type defines both dunders uses the str-preferred renderer, not
  `__repr__`. Adding a second (repr) renderer slot to the itable is the fix;
  it is not Iterator-/#1384-blocking and affects only dual-dunder types reached
  through an interface value.
- Follow-ups (not in this change): the interface repr slot above; a
  decimal-exact note is not needed here (base formatting is integer-exact).
