---
title: url.unquote errors mode — strict (default) | replace, over a UTF-8 replacement decode path
status: Proposed
issue: 1242
pr:
date: 2026-11-21
domain: stdlib
supersedes:
superseded-by:
---

# `url.unquote` / `url.unquote_plus` `errors` mode

**Status:** Proposed — **the API surface below is PROVISIONAL and awaits
maintainer confirmation** (the issue leaves the exact signature and defaults
open for review). The runtime decode path is the committed part; the keyword
name, value set and default are the open question (see Rationale).

## Decision

Add an optional trailing `errors` argument to the percent decoders:

```zap
url.unquote(s: str[, errors: str]) -> str          # errors in {"strict","replace"}, default "strict"
url.unquote_plus(s: str[, errors: str]) -> str      # same; also maps '+' -> space
```

backed by a replacement-capable UTF-8 decode of the percent-decoded octets:

- `errors="strict"` (the default) — validate the decoded octets as well-formed
  UTF-8 and raise a **catchable `ValueError`** on any invalid sequence
  (surrogates, overlong encodings and code points above U+10FFFF all rejected),
  exactly as `str(bytes)` does.
- `errors="replace"` — substitute **U+FFFD** for each *maximal invalid subpart*
  (Unicode §3.9 best-practice substitution / WHATWG Encoding utf-8 decoder), so
  a lenient W3C-Baggage value decode never raises. The replacement count matches
  CPython's `urllib.parse.unquote(errors="replace")` /
  `bytes.decode("utf-8","replace")` codepoint-for-codepoint.
- An unrecognized `errors` name raises a **catchable `LookupError`** (as
  CPython's codec machinery does).

The `%`-escape handling itself stays lenient in both modes (a malformed `%` is
left literal). `errors` is **positional-or-keyword** in this first cut;
keyword-only is recommended (see Consequences).

Runtime: a new reusable primitive `zap_utf8_decode_replace` (in
`runtime/bytes.inc.c`, beside `zap_utf8_valid`) performs the U+FFFD maximal-
subpart substitution; `url.inc.c` gains `zap_url_decode_mode` /
`zap_url_errmode` and the `zap_url_unquote_e` / `zap_url_unquote_plus_e` entry
points. `str(bytes)` is unchanged. The internal `parse_query*` readers keep
their pre-existing lenient octet passthrough (they call `zap_url_decode`
directly) and are unaffected.

## Rationale

### The default is the real open question

The issue's framing ("preserve the strict behavior of existing `str(bytes)`")
assumed `url.unquote` was already strict. **It was not.** The pre-existing
decoder built the result string straight from the decoded octets via
`zap_str_new`, which does **no** UTF-8 validation — so `url.unquote("A%FFB")`
silently produced a `str` holding an invalid 0xFF byte (a latent correctness
bug: `len` miscounts codepoints, comparisons misbehave). So "default to strict"
and "preserve current behavior" could not both hold; a choice was required.

Precedent across the two reference languages diverges:

- **Python**: `urllib.parse.unquote(string, encoding="utf-8", errors="replace")`
  — `errors` is a kwarg whose **default is `"replace"`** (not strict); the
  decode delegates to `bytes.decode(encoding, errors)`.
- **Go**: `url.QueryUnescape` / `PathUnescape` return `([]byte→string)` and do
  not re-validate UTF-8 at all; separately `utf8.Valid` + `strings.ToValidUTF8(s,
  "�")` provide the replacement.

Three candidate defaults, for the maintainer to choose:

- **(A) default `"strict"` (chosen here)** — raises on invalid UTF-8, consistent
  with `str(bytes)` (which the issue highlights) and with Python's `"strict"`
  name. Verified to break **no** existing golden/test (the only in-tree
  `url.unquote` callers decode valid UTF-8 or pure ASCII). Downside: a
  behavior change from today's silent passthrough — any out-of-tree caller
  relying on invalid-byte passthrough would now raise.
- **(B) default `"replace"`** — exact Python `unquote` parity; most convenient
  for lenient consumers like W3C Baggage; never raises. Downside: diverges from
  `str(bytes)` strictness and silently masks malformed input.
- **(C) restore silent passthrough as a third legacy mode** — exact back-compat
  for the default, with `"strict"`/`"replace"` opt-in. Downside: keeps a mode
  that yields invalid `str`, which the rest of the language treats as an error.

**Recommendation: (A).** It makes the honest, `str(bytes)`-consistent choice the
default, breaks nothing in-tree, and still unblocks the otel consumer (which
passes `errors="replace"` explicitly). If the maintainer prefers Python parity,
(B) is a one-line default flip in the lowering (omitted → `_e(…, "replace")`).

### Keyword-only recommendation

Per the seam calling-convention guidance (#998), `errors` reads best as
keyword-only — `url.unquote(s, errors="replace")`, never a bare second
positional whose meaning is opaque at the call site. The seam registry has no
keyword-only marker yet, so this first cut accepts it positionally too;
promoting it to keyword-only is a small follow-up in the binder.

### Why an internal runtime helper, not `bytes.decode`

The issue allowed either a `bytes.decode(errors=...)` method or an internal
helper. The helper keeps this change small and focused on unblocking otel. A
general `bytes.decode(encoding, errors)` method (Python parity) is a natural
follow-up and would reuse `zap_utf8_decode_replace` unchanged.

## Consequences

- Unblocks `zaplang-io/otel#4` (W3C Baggage value decoding) via
  `url.unquote(v, errors="replace")`.
- `url.unquote` / `url.unquote_plus` with no `errors` now **raise** on invalid
  UTF-8 instead of silently passing bytes through (option A). This is the one
  behavior change; it is surfaced here for maintainer sign-off.
- `str(bytes)` and the `parse_query*` readers are untouched.
- Follow-ups if the maintainer agrees: (1) make `errors` keyword-only; (2)
  expose `bytes.decode(encoding="utf-8", errors="strict")` reusing the same
  replacement primitive; (3) if default (B) is preferred, flip the omitted-arg
  default.
