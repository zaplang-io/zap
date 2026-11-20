---
title: net.connect_deadline — hostname-aware bounded TCP dial
status: Locked
issue: 1138
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net.connect_deadline` — hostname-aware bounded TCP dial

**Status:** Locked

## Decision

Add `net.connect_deadline(host, port, deadline_ns) -> conn`: a TCP dial to a
**DNS hostname** bounded by one absolute `CLOCK_MONOTONIC`-ns deadline covering
the whole operation — name resolution and every resolved-address attempt.
Complements the two existing entry points, both preserved:
`net.connect(host, port)` (hostname, no deadline) and
`net.connect_ip_deadline(ip, port, deadline_ns)` (numeric IP only, bounded).

It is implemented through the exact bounded path `connect_ip_deadline` already
uses (`zap_net_connect_impl` with the numeric-only flag off), so the deadline is
enforced identically: checked before resolution (expiry-before-work), at the
first address attempt after resolution (expiry during/after a DNS lookup), and
per-attempt while dialing. The four outcomes stay distinguishable by exception
type: a timeout is `TimeoutError`, a cancellation is `Cancelled`, a DNS failure
and a connect failure are the respective `net:` errors. A zero deadline is
unbounded (as `net.connect`); a negative one raises `ValueError`.

The argument list is **positional**, matching `connect_ip_deadline`.

## Rationale

- **Reuse the proven bounded dial path, don't fork it.** `zap_net_connect_impl`
  already takes a `deadline_ns` and a numeric-only flag; `connect_ip_deadline`
  passes the flag on, this passes it off. No new timeout/cancellation machinery,
  so the deadline semantics are the same code the numeric dial is tested against
  (#559/D-0099).
- **No separate bounded-resolver API.** #1138 asked for either a hostname
  bounded dial *or* a bounded resolver that composes with the numeric dial. A
  single `connect_deadline` is the smaller, more direct surface — the budget
  spans lookup + dial in one call, which is exactly the memcached-client need
  (zaplang-io/memcached#2) that `connect_ip_deadline` + the #992 DNS functions
  could not express (neither resolves under a deadline).
- **getaddrinfo is bounded at its boundary, best-effort.** `getaddrinfo` has no
  portable mid-call cancellation, so a resolver that hangs is detected only when
  it returns (the deadline is re-checked immediately after). This matches how the
  whole runtime treats blocking resolution and is the honest limit of a portable
  implementation; the common cases (cached/fast DNS, expiry before work, expiry
  while dialing) are all bounded precisely.
- **Positional args, like `connect_ip_deadline`.** That entry point was
  deliberately left off the kwargs seam "pending a settled name for its deadline
  parameter". `connect_deadline` stays consistent; settling a kwargs name
  (`deadline_ns`?) for both bounded connects is a separate follow-up.

## Consequences

- A caller can enforce one budget across hostname resolution and connection —
  the gap #1138 identified. `net.connect` and `net.connect_ip_deadline` are
  unchanged.
- Recommend a follow-up to give both `connect_deadline` and
  `connect_ip_deadline` a kwargs-enabled deadline parameter name once settled.
- `net` remains **Experimental** (STABILITY.md §4.4). Builds on the #992 DNS
  surface and the #559/D-0099 absolute-deadline dial.
