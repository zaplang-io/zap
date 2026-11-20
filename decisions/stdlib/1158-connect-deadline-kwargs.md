---
title: net bounded-dial deadline parameter name — deadline_ns (kwargs-enabled)
status: Locked
issue: 1158
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net` bounded-dial deadline parameter: `deadline_ns`

**Status:** Locked

## Decision

The two bounded TCP dials take their absolute-deadline argument as **`deadline_ns`**,
and both are now keyword-enabled (added to the seam signature table):

```
net.connect_ip_deadline(ip, port, deadline_ns)
net.connect_deadline(host, port, deadline_ns)
```

`connect_ip_deadline` had been left off the kwargs seam since #559 "pending a
settled name for its deadline parameter", and `net.connect_deadline` (#1138)
followed suit. This settles that name and enables keyword calls for both.

## Rationale

- **`deadline_ns`, not `deadline`.** The value is an absolute `CLOCK_MONOTONIC`
  **nanosecond** bound (computed from `time.monotonic_ns()`), not a relative
  duration and not seconds/ms. The `_ns` suffix makes the unit unmistakable at
  the call site and matches `net.set_deadline_ns`, which already uses `ns`.
- **Seam-only change.** The functions already exist and lower positionally; the
  central kwargs binder reorders named arguments to positional before lowering,
  so adding the two `seam_sigs.rs` rows is the whole change — no sema, lowering,
  codegen, or runtime change.

## Consequences

- `net.connect_deadline(host=..., port=..., deadline_ns=...)` and the
  `connect_ip_deadline` equivalent now type-check; positional calls are
  unchanged.
- `net` remains **Experimental** (STABILITY.md §4.4). Settles the naming left
  open by [stdlib/1138-connect-deadline-hostname.md](1138-connect-deadline-hostname.md)
  and D-0099 (#559).
