---
title: A parked select is woken by a scope cancellation or deadline
status: Accepted
issue: 1414
pr:
date: 2026-11-24
domain: runtime
supersedes:
superseded-by:
---

# A parked select is woken by a scope cancellation or deadline

**Status:** Accepted

## Context

`design/0005-concurrency.md` lists `select` as a cancellation point, alongside a
bare channel send/receive. It was not one. A `select` with no `default` and no
ready case parked in `zap_select`'s pass 2: it enqueued one per-case `Waiter`
(`w->sel = &sel`, `w->sel_case = i`) on each channel's send/receive queue and
`copark`ed once on `sel.mu`. Unlike `chan_block` — which, since I#1398, registers
a `ScopeWait` node against the coroutine's innermost lexical scope
(`G.cancel_scope`) — pass 2 registered no `ScopeWait` and recorded no scope.

`scope_request_cancel` wakes channel-blocked waiters only by walking the
`g_scope_waits` list (the `ScopeWait` nodes `chan_block` publishes) and
`scope_cancel_io` only wakes netpoller `PollDesc`s. Neither could see a parked
`select`'s waiters, and every `coready(sel->pw)` on the channel paths is a
counterpart completing a case, never a cancellation. So a `select` parked inside
`scope(timeout=T)` was **not** woken at the deadline: it resumed only when a case
actually fired, or — inside a timed scope — when the coroutine was severed and
leaked after the grace window (#825). Its `finally` / `with` cleanup never ran on
cancellation. A bare `<-ch` in the same position was woken cooperatively and
raised `Cancelled`; wrapping that receive in a single-case `select` silently
changed the semantics.

This was the last cancellation point not wired to scope cancellation. I#1398
fixed the same *family* of bug for `time.sleep`, channel send/receive, and socket
I/O (each now binds its waiter to the active lexical scope); `select` was the
remaining gap.

## Decision

Register a parked `select` against the coroutine's `cancel_scope` exactly as
`chan_block` does, reusing the existing machinery rather than adding a parallel
one.

- **In `zap_select` pass 2**, after the per-case waiters are enqueued and while
  every channel lock is still held, push one `ScopeWait` node per case
  (`{scope: cancel_scope, channel: cases[i].ch, waiter: &wcases[i]}`) onto
  `g_scope_waits`. This mirrors `chan_block` (which links its node under the
  channel lock before `copark` releases it), so a canceller — which takes the
  channel lock first — always sees a consistent waiter+node state. A cancel that
  arrives between the entry `zap_check_cancel()` and the park is caught under
  `g_scope_wait_mu` (as `chan_block` does): the waiters are removed and
  `Cancelled` is raised without parking. On resume, each node is detached
  (`waiter = NULL`) and released under the same channel + registry locks the
  canceller reads it under, so a concurrent canceller never touches a waiter
  whose stack frame is leaving.

- **In `scope_request_cancel`**, when a dequeued waiter has `w->sel` set, claim
  the select the single-fire way a channel counterpart does
  (`sel_claim_cancel`): lock `sel->mu`, and if no case has fired, mark
  `fired = 1`, `chosen = -1` (no case value) and `coready(sel->pw)`. A select
  already claimed elsewhere — a real case fired, or another of its cases was
  cancelled first — is dropped, so `sel->pw` is woken at most once. The post-park
  `zap_check_cancel()` in `zap_select` then raises.

D-0094 labelling is applied unchanged at scope exit: the cancellation point only
raises `Cancelled`; the scope whose OWN deadline fired re-labels it to
`ScopeTimeout`, while an ENCLOSING deadline surfaces as `Cancelled`. Because the
per-case nodes bind to `cancel_scope` (the innermost lexical scope, a descendant
of every enclosing one), both a nested scope's own deadline and any enclosing
deadline reach the parked select — consistent with I#1398.

### Locking

Lock order stays acyclic. The canceller holds the case's channel lock and
`g_scope_wait_mu`, then takes `sel->mu` innermost — the same channel → sel order
the counterpart paths (`deliver_to_receiver`, `sel_try_*`, `zap_chan_close`) use,
with `sel->mu` always a leaf. No path acquires a channel lock or
`g_scope_wait_mu` while holding `sel->mu`, so no cycle forms, and concurrent
cancels serialize on the single `g_scope_wait_mu`.

## Consequences

- A parked `select` is now a true cancellation point: it wakes at its scope's
  deadline (or on any cancel reaching that scope), runs its `finally` / `with`
  cleanup, and surfaces `ScopeTimeout` (own deadline) or `Cancelled` (enclosing
  cancel / sibling failure). This matches a bare `<-ch` in the same position.
- No surface change; a `select` already satisfiable by a ready case, or made
  non-blocking by `default`, is unaffected. Existing `select` goldens
  (`select_stmt`, the `time.after` / ticker timeout idioms) are unchanged — a
  timer channel is an ordinary channel whose send already wakes the select.
- Scope: all in `crates/codegen/src/zaprt.c` (`zap_select`,
  `scope_request_cancel`, the new `sel_claim_cancel` helper). No language
  surface, no seam, no sema/lowering changes. The C runtime path is exercised
  only on the LLVM-18 full/arm64 CI legs (the host build does not compile it), so
  the new `select_scope_*` goldens are the regression wall.
