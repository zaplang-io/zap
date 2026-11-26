---
title: ljust/rjust/center take a one-codepoint fill, not an ASCII fill byte
status: Accepted
issue: 1648
pr:
date: 2026-11-26
domain: stdlib
supersedes: DECISIONS.md#D-0210 (pad/justify "ASCII fill byte" clause only)
superseded-by:
---

# ljust/rjust/center take a one-codepoint fill, not an ASCII fill byte

**Status:** Accepted

## Decision

`str.ljust(w[, fill])`, `str.rjust(w[, fill])` and `str.center(w[, fill])` take a
fill that is **exactly one codepoint** and repeat its full UTF-8 bytes at each
padding position, so a multibyte fill like `"é"` produces valid UTF-8. Width is
counted in codepoints; the allocation counts bytes (checked for overflow). A fill
that is **not exactly one codepoint** — empty, or two or more characters — raises
a catchable `TypeError` ("The fill character must be exactly one character
long"), matching CPython, and the check runs even when the width needs no
padding. The default fill (argument omitted) remains a space. `zfill` is
unaffected (it pads with the ASCII `'0'`).

This supersedes the clause of **D-0210** that specified an "ASCII fill byte".

## Rationale

- The byte-based form copied only the fill's first byte, so any multibyte fill
  produced **invalid UTF-8** (`"x".ljust(3, "é")` → bytes `78 C3 C3`), breaking
  the `str` UTF-8 invariant (SPEC §4.2). Like the char-set strip fix (#1636),
  emitting a malformed `str` is wrong under any contract, not merely a CPython
  divergence.
- Repeating the whole codepoint is the only behavior consistent with "fill is a
  character" and with width being counted in codepoints.
- Requiring exactly one codepoint matches CPython and removes two silent
  footguns the old code had: an empty fill silently became a space, and a
  multi-character fill silently used only its first byte. A `TypeError` surfaces
  the mistake; it is catchable (#1566), so programs can handle it.

## Consequences

- The shared `zap_str_pad` helper validates the fill up front and raises
  `TypeError` on a bad one; the three methods now lower through a throwing call so
  `try`/`except` can intercept it. Padding size arithmetic (`len + pad * fillWidth`)
  is overflow-checked and raises a catchable `MemoryError` rather than wrapping.
- ASCII fills and the default-space behavior are unchanged, so existing fixtures
  and callers are unaffected.
- Out of scope: `center`'s odd-padding split (smaller half on the left) still
  differs from CPython's right-smaller rounding — a separate, pre-existing
  D-0210 behavior, not addressed here.
- SPEC §9.2, `docs/STABILITY.md` and a changelog `fixed` fragment updated. Part
  of the epic #1638 Unicode audit follow-up.

<!-- Related: decisions/stdlib/1636-strip-codepoint-charset.md; SPEC §4.2; #1566 (catchable faults). -->
