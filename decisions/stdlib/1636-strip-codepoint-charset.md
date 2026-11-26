---
title: char-set strip/lstrip/rstrip trims whole codepoints, not bytes
status: Accepted
issue: 1636
pr:
date: 2026-11-26
domain: stdlib
supersedes: DECISIONS.md#D-0210 (char-set strip "trim any byte" clause only)
superseded-by:
---

# char-set strip/lstrip/rstrip trims whole codepoints, not bytes

**Status:** Accepted

## Decision

`str.strip(chars)` / `lstrip(chars)` / `rstrip(chars)` treat `chars` as a set of
**characters (codepoints)** and trim whole codepoints from the ends, matching
CPython. A multibyte character in `chars` or in the haystack is handled as a
unit, and the result is always valid UTF-8. ASCII results are unchanged, and the
no-argument whitespace form is unaffected.

This supersedes the clause of **D-0210** that defined the char-set argument as
"trim any byte in the set".

## Rationale

- The byte-based form was not merely a Python divergence: it could **emit invalid
  UTF-8**. `"αβγ".strip("α")` seeded a 256-byte set with `α`'s bytes `CE B1`,
  then stripped the shared lead byte `CE` of `β` from the left, leaving the
  orphaned continuation byte `B2` — a malformed string, not `"βγ"`. No reasonable
  reading of "trim a char set" intends a corrupt result, so byte-based was wrong
  under any contract, not just against CPython.
- Zap is "Python-readable"; `str.strip(chars)` is a Python method and trims by
  character there. Codepoint semantics are the least-surprising and the only ones
  that keep the type's UTF-8 invariant.
- Rejected: keep byte-based but guard codepoint boundaries so the result stays
  valid UTF-8. It avoids corruption but still diverges from CPython on which
  characters are trimmed (a multibyte char in the set would trim any char sharing
  a byte), which is itself a footgun. Full codepoint semantics cost no more.

## Consequences

- The runtime scans `chars` a codepoint at a time (`zap_str_cp_in_set`) and
  advances the trim pointers by codepoint width, so only whole codepoints are
  removed. ASCII is byte == codepoint, so existing ASCII fixtures are unchanged.
- SPEC §9.2 and `docs/STABILITY.md` state codepoint semantics for char-set strip;
  a changelog `fixed` fragment records the change. Part of epic #1638.

<!-- Related: SPEC §4.2 (UTF-8 string contract); decisions/stdlib/1637-translate-codepoint-keys.md; #1630. -->
