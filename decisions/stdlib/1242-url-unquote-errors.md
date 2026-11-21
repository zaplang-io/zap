---
title: url.unquote errors mode — replace (default) | strict, over a UTF-8 replacement decode path
status: Accepted
issue: 1242
pr: 1246
date: 2026-11-21
domain: stdlib
supersedes:
superseded-by:
---

# `url.unquote` / `url.unquote_plus` `errors` mode

**Status:** Accepted — the maintainer reviewed the open signature/default
question (recorded below) and chose **default `"replace"` (option B, Python
`urllib.parse.unquote` parity)**.

## Decision

Add an optional trailing `errors` argument to the percent decoders:

```zap
url.unquote(s: str[, errors: str]) -> str          # errors in {"replace","strict"}, default "replace"
url.unquote_plus(s: str[, errors: str]) -> str      # same; also maps '+' -> space
```

backed by a replacement-capable UTF-8 decode of the percent-decoded octets:

- `errors="replace"` (the default) — substitute **U+FFFD** for each *maximal
  invalid subpart* (Unicode §3.9 best-practice substitution / WHATWG Encoding
  utf-8 decoder), so a lenient W3C-Baggage value decode never raises. The
  replacement count matches CPython's `urllib.parse.unquote(errors="replace")` /
  `bytes.decode("utf-8","replace")` codepoint-for-codepoint. This is also
  Python's own default for `unquote`.
- `errors="strict"` — validate the decoded octets as well-formed UTF-8 and raise
  a **catchable `ValueError`** on any invalid sequence (surrogates, overlong
  encodings and code points above U+10FFFF all rejected), exactly as
  `str(bytes)` does.
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

The implementer recommended (A); the **maintainer chose (B)** — Python
`unquote` parity, and the least surprising for existing default callers (the old
default never raised either, and replace keeps that property while yielding a
*valid* `str`). The bare (no-`errors`) entry point is therefore the replace
decode and never raises; `errors="strict"` is the opt-in to raising.

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

- Unblocks `zaplang-io/otel#4` (W3C Baggage value decoding): `url.unquote(v)`
  (or explicit `errors="replace"`) now yields U+FFFD for invalid UTF-8.
- `url.unquote` / `url.unquote_plus` with no `errors` now return a **valid** `str`
  (invalid octets → U+FFFD) instead of silently constructing a malformed `str`
  from raw octets. The default still never raises, so no caller gains a new
  exception; `errors="strict"` is the explicit opt-in to raising on invalid UTF-8.
- `str(bytes)` and the `parse_query*` readers are untouched.
- Follow-ups: (1) make `errors` keyword-only (seam registry needs a keyword-only
  marker); (2) expose `bytes.decode(encoding="utf-8", errors=...)` reusing the
  same `zap_utf8_decode_replace` primitive.
