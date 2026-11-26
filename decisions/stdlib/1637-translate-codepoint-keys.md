---
title: str.translate keys by whole codepoint, not by byte
status: Accepted
issue: 1637
pr:
date: 2026-11-26
domain: stdlib
supersedes: DECISIONS.md#D-0214 (str.translate "1-char key" clause only)
superseded-by:
---

# str.translate keys by whole codepoint, not by byte

**Status:** Accepted

## Decision

`str.translate(table)` looks up each **codepoint** of the subject as a whole
1-character key in `table` (`dict[str, str]`): a present key substitutes its
value, an empty value deletes the character, and an absent key passes the
character through unchanged. Multibyte character keys now match, so they can be
remapped or deleted, matching CPython's per-character translation. ASCII
(single-byte) keys are unaffected.

Zap keeps its `dict[str, str]` surface (1-character **string** keys), rather than
CPython's integer-ordinal keys — that surface choice from **D-0214** stands. This
decision supersedes only D-0214's *implementation* clause that keyed the lookup
one byte at a time (so "1-char key" silently meant "1 ASCII byte").

## Rationale

- Keying by byte made it impossible to remap or delete any non-ASCII character:
  `"café".translate({"é": "e"})` left the string unchanged, because the lookup
  only ever built single-byte keys that could not equal a two-byte `"é"`. The
  method silently did nothing for the exact inputs a user reaches for it with.
- "Map each character" (the method's documented contract) means a codepoint, not
  a byte. Codepoint keying is the only reading consistent with the `dict[str,str]`
  surface, where a key like `"é"` is one character.
- Keeping the `dict[str,str]` surface (vs adopting CPython's ordinal-keyed dict)
  avoids a `maketrans` step and reads naturally in Zap; it is an intentional,
  already-recorded divergence (D-0214) and is noted in `docs/PYTHON_DIFFERENCES`.

## Consequences

- The runtime iterates the subject by codepoint and builds a whole-codepoint key
  for each lookup (`zap_utf8_width`), copying the codepoint's bytes on a miss.
- Because keys are strings, the surface is unchanged; only multibyte matching
  improves. SPEC §9.2 and `docs/STABILITY.md` note codepoint keying; a changelog
  `fixed` fragment records it. Part of epic #1638.

<!-- Related: decisions/stdlib/1636-strip-codepoint-charset.md; #1630; D-0214 (surface). -->
