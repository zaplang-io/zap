---
title: str.rfind / str.rindex return codepoint indices, not byte offsets
status: Accepted
issue: 1630
pr:
date: 2026-11-26
domain: stdlib
supersedes: DECISIONS.md#D-0210 (reverse-search offset units only)
superseded-by:
---

# str.rfind / str.rindex return codepoint indices, not byte offsets

**Status:** Accepted

## Decision

`str.rfind(sub)` and `str.rindex(sub)` return the **codepoint index** of the last
match — the same units `str.find`/`str.index`, `len`, indexing and slicing use —
rather than the UTF-8 byte offset. A successful reverse search therefore yields a
value in `0..=len(s)` that is always a valid codepoint position, so
`s[s.rfind(x)]` and `s[s.rfind(x):]` are well-formed. An empty needle returns
`len(s)` (the codepoint length), including on an empty haystack; a miss returns
`-1` for `rfind` and raises a catchable `ValueError` for `rindex` (#1566),
unchanged. This matches CPython exactly.

This supersedes the clause of **D-0210** that recorded the reverse-search methods
as returning byte offsets. D-0210 asserted that `find`/`index` "already" returned
byte offsets and aligned `rfind`/`rindex` with them; in fact `find`/`index`
convert to codepoint indices, so the reverse methods were the sole outlier and
could return an index outside the string (`"éx".rfind("x")` gave `2` for a
length-2 string) or point at the wrong character when used in a slice.

## Rationale

- **Internal consistency over a surprising micro-optimization.** Every other
  position-valued `str` operation — `find`, `index`, indexing, slicing, `len` —
  speaks codepoints. One method reporting bytes is a footgun precisely because it
  coincides with codepoints on ASCII (where most tests live) and diverges only on
  multibyte input, so the bug survives casual testing and bites in production.
- **CPython is the oracle.** Zap is "Python-readable"; `str.rfind`/`str.rindex`
  are Python methods and must match CPython's codepoint semantics over the shared
  subset. The reverse-search differential case pins this.
- **Rejected: keep byte offsets and document it.** D-0210's original framing.
  Rejected: it makes the returned index unusable for the very indexing/slicing it
  is typically fed into, and contradicts the string contract (SPEC §4.2).

## Consequences

- ASCII results are unchanged (byte offset == codepoint index there), so existing
  ASCII fixtures and callers are unaffected.
- `zap_str_rpartition` slices the raw UTF-8 buffer, so it keeps a byte-offset
  helper (`zap_str_rfind_bytes`) rather than the public, now-codepoint `rfind`.
  The reverse-search byte offset is thus an internal implementation detail; no
  public surface exposes it.
- SPEC §9.2 and `docs/STABILITY.md` are updated to state codepoint indices for the
  reverse-search methods; a changelog `fixed` fragment records the behavior change.

<!-- Related: SPEC §4.2 (string contract), §9.2 (str methods); #1566 (catchable ValueError). -->
