---
title: net IP presentation↔wire conversion — inet_pton / inet_ntop
status: Locked
issue: 1261
pr:
date: 2026-11-22
domain: stdlib
supersedes:
superseded-by:
---

# `net` IP presentation ↔ wire conversion

**Status:** Locked

## Decision

Add a pure, I/O-free pair to the core `net` module that converts between IP
address **text** and its **network-order octets**, in both directions, matching
Python's `socket.inet_pton` / `socket.inet_ntop`:

- `net.inet_pton(family, text) -> bytes` — parse an IPv4 or IPv6 literal into its
  network-order octets (**4 bytes** for `"ipv4"`, **16 bytes** for `"ipv6"`).
- `net.inet_ntop(family, packed) -> str` — format 4-byte / 16-byte octets back
  into canonical text (RFC 5952 for IPv6, with defined IPv4-mapped handling).

Both are pure computation: **no sockets, no DNS, no hostname resolution.** Until
now the stdlib exposed IP text only *to* socket operations (`net.connect`,
`net.connect_ip_deadline`, `net.udp_bind`, …); there was no way to parse or format
an address on its own. This unblocks the `dns` library's typed A/AAAA records
(zaplang-io/dns#3), whose `address` field is text that the wire codec must turn
into 4/16 octets and back, and reverse lookup (zaplang-io/dns#2).

The Python/Go parallel:

| Zap | Python | Go |
| --- | --- | --- |
| `net.inet_pton("ipv4", s) -> bytes` (4) | `socket.inet_pton(AF_INET, s) -> bytes` | `netip.ParseAddr(s).As4()` |
| `net.inet_pton("ipv6", s) -> bytes` (16) | `socket.inet_pton(AF_INET6, s) -> bytes` | `netip.ParseAddr(s).As16()` |
| `net.inet_ntop("ipv4", b) -> str` | `socket.inet_ntop(AF_INET, b) -> str` | `netip.AddrFrom4(b).String()` |
| `net.inet_ntop("ipv6", b) -> str` | `socket.inet_ntop(AF_INET6, b) -> str` | `netip.AddrFrom16(b).String()` |

### Signatures and argument style

- `net.inet_pton(family, text)` — `family` and `text` are both keyword-bindable
  (`seam_sigs.rs`), so `net.inet_pton(family="ipv6", text="::1")` and the
  positional `net.inet_pton("ipv6", "::1")` both bind. `family` is a *selector*,
  the canonical case for a keyword argument (per AGENTS.md's recommendation to
  make family-style params keyword-bindable).
- `net.inet_ntop(family, packed)` — likewise keyword-bindable on `family`,
  `packed`.

### Address family — a portable string, not an `AF_*` integer

`family` is the string `"ipv4"` or `"ipv6"` (exactly those two spellings;
anything else is a `ValueError`). This is a **deliberate divergence** from
Python's `socket.inet_pton(family, …)`, which takes the integer constants
`socket.AF_INET` / `socket.AF_INET6`:

- Those constants are **platform-specific** — `AF_INET6` is `10` on Linux, `30`
  on macOS, `23` on Windows — so a numeric family would not be portable source,
  which is unacceptable for an AOT cross-platform language.
- Zap exposes **no `AF_*` constants** anywhere, and the `net` module already
  selects address family by *explicit spelling* rather than a flag: `net.lookup_ipv4`
  / `net.lookup_ipv6` (#992), `net.udp_socket` / `net.udp_socket6` (#991). A
  `"ipv4"` / `"ipv6"` string keeps `net` internally consistent and reads well.

Go's `net/netip.ParseAddr` infers the family from the text and needs no family
argument; the issue's acceptance criteria explicitly want an **explicit address
family**, so a family-selecting string (not inference) is the chosen surface.

### Errors — typed `ValueError`, documented

Every rejection raises a catchable `ValueError` (via `zap_panic_typed(
ZAP_EXC_VALUE_ERROR, …)`), the canonical input-validation exception in Zap and
uniformly `try`/`except Exception`-catchable:

- **Unknown family** — `family` not `"ipv4"`/`"ipv6"`.
- **Malformed `text`** (`inet_pton`) — see the rejection rules below.
- **Family / text mismatch** — IPv6 text with `family="ipv4"` (and vice-versa)
  is rejected by the selected family's parser, so it surfaces as the same
  "invalid IPv4/IPv6 address" `ValueError`. (The family is authoritative; the
  text is validated against it.)
- **Invalid packed width** (`inet_ntop`) — `"ipv4"` requires exactly 4 bytes and
  `"ipv6"` exactly 16; a 3-, 5-, 15- or 17-byte `packed` is a `ValueError`.

Python raises `OSError` for malformed *text* and `ValueError` for a bad *family*;
Zap uses one typed exception (`ValueError`) across the board so a caller writes a
single `except Exception` (or `except ValueError`) arm. This is recorded in
`docs/PYTHON_DIFFERENCES.md`.

### IPv6 output — RFC 5952 §4 canonical form

`inet_ntop("ipv6", …)` emits the **canonical** representation, independent of any
libc `inet_ntop` formatting quirks (we hand-write the formatter for deterministic
output across Linux / macOS / aarch64 CI):

- lowercase hexadecimal;
- no leading zeros within a group (`db8`, never `0db8`);
- the **longest** run of consecutive all-zero groups is compressed to `::`
  (**leftmost** run on a tie);
- a run must be **≥ 2** groups to be compressed — a single zero group is written
  `0`, never `::` (RFC 5952 §4.2.2);
- `::` for all-zeros, `::1` for the loopback.

### IPv4-mapped handling — RFC 5952 §5

Only the **IPv4-mapped** prefix (five zero groups followed by `0xffff`, i.e.
`::ffff:a.b.c.d`) is rendered with the embedded **dotted-decimal** IPv4 tail;
the leading zeros still compress to `::`. Example: the 16 octets
`00…00 ff ff c0 00 02 01` format as `::ffff:192.0.2.1`.

Every other address — including the **deprecated IPv4-compatible** form
`::a.b.c.d` (six leading zero groups, no `0xffff`) — is rendered as pure hex
groups (`::c000:201`), never with an embedded dotted tail. This keeps the
IPv4-mapped special-case narrow, well-defined and testable.

`inet_pton("ipv6", …)` *accepts* a trailing embedded IPv4 in either form
(`::ffff:1.2.3.4`, `::1.2.3.4`, `0:0:0:0:0:ffff:1.2.3.4`); only the **output**
of `::ffff:…` is special-cased.

### Parse rejection rules (intent documentation)

- **IPv4** rejects: an octet `> 255`; a **leading zero** (`01`, `00`, `1.2.3.04`);
  the wrong field count (too few / too many `.`-separated octets); an empty
  octet; any non-digit or trailing garbage (`1.2.3.4x`, `1.2.3.`, `1..2.3`).
- **IPv6** rejects: more than one `::`; a group with more than 4 hex digits; a
  `::` that expands to *zero* groups (a full 8-group address with a stray `::`);
  a wrong total length (fewer or more than 16 octets); an embedded IPv4 anywhere
  but the trailing position; any non-hex garbage.

### Typed address — deferred, non-blocking

A Go `netip.Addr`-style typed wrapper (`.as4()` / `.as16()` / `.to_str()` /
family predicate) is **not** added here. The family-explicit `inet_pton` /
`inet_ntop` pair is the required surface and is exactly what the `dns` codec
consumes. A typed address, if it earns its place later, can be layered in **pure
Zap** on top of these primitives without a core change; it is deliberately out of
scope for #1261.

## Rationale

- **Home in `net`, not a new module.** The socket surface that already consumes
  IP text lives in `net`; the parse/format pair belongs beside it, and `dns`
  (which already depends on `net`) picks it up with no new import.
- **Pure runtime C, in the existing `net_http.inc.c` fragment.** The conversion
  is byte manipulation with no syscalls; it adds no new `#include` (so no
  `lib.rs`/`jit.rs`/`build.rs`/manifest churn and no `zap test --doc`-only
  breakage). The functions are emitted as **throwing calls** (invoke) so the
  `ValueError` is catchable inside `try`.
- **Hand-written parser/formatter, not libc `inet_pton`/`inet_ntop`.** Parsing is
  controlled for exact rejection semantics (leading zeros, family mismatch) and
  formatting follows RFC 5952 strictly with a defined IPv4-mapped rule, so output
  is byte-identical across platforms (libc `inet_ntop` IPv6 compression and
  v4-mapped behaviour varies).

## Scope

`net` remains **Experimental** (§4 of `STABILITY.md`); this is an additive pure
function pair under that umbrella. No change to `DECISIONS.md` / `CHANGELOG.md`
(frozen); a `changelog.d/1261.added.md` fragment carries the user-visible entry.
