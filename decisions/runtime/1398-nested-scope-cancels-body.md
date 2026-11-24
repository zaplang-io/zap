---
title: A nested scope's deadline cancels a wait in its own body
status: Accepted
issue: 1398
pr:
date: 2026-11-24
domain: runtime
supersedes:
superseded-by:
---

# A nested scope's deadline cancels a wait in its own body

**Status:** Accepted

## Context

`scope(timeout=)` cancels a blocked wait by matching the waiter's recorded scope
against the cancelled scope's subtree: `scope_cancel_io(sc)` (and the channel-
waiter path in `scope_request_cancel`) wakes a waiter whose recorded scope is
`sc` or a descendant of `sc`.

Each blocking cancellation point recorded the waiter against `g->scope` — the
coroutine's **spawn** scope, the scope it was `spawn`ed into. That scope is fixed
for the coroutine's lifetime. A lexically nested `scope:` opened inside the
coroutine's body (`zap_scope_enter`) did **not** change it. So when a coroutine
ran a wait inside a nested `scope(timeout=)`, the waiter was still recorded
against the enclosing spawn scope — an **ancestor** of the nested scope. The
nested scope's deadline (`scope_fire_timeout` -> `scope_request_cancel` ->
`scope_cancel_io`) only matches itself and its descendants, never an ancestor, so
it could not select the wait. The wait ran to its full natural duration (e.g. a
60 s `time.sleep_for` under a 25 ms nested deadline), and an enclosing scope
reported the coroutine still parked (I#1398, surfaced by otel PR #17's metric
reader).

## Decision

Track the **innermost lexically-active cancellation scope** per coroutine,
separately from spawn ownership, and bind every blocking cancellation point to
it.

- A new `G.cancel_scope` field holds the scope whose body the coroutine is
  currently executing. It starts equal to `G.scope` (the spawn scope) at
  `zap_spawn_scoped`, is re-pointed to each nested `scope:` the coroutine enters
  (`zap_scope_enter`), and is restored to the enclosing scope (`sc->parent`) at
  the matching exit — normal join, early-exit cancel, and the landing-pad
  variant. It is a GC-managed pointer and is listed in `zap_off_gobj` so the
  precise/relocating collector traces it.
- The cancellation points — `zap_time_timer_wait` (fd-less sleep timer), the
  fd-backed netpoll read/write wait, `chan_block` (channel send/receive), and
  `g_is_cancelled` (the `checkpoint()` / pre-registration check) — select against
  `cancel_scope` instead of `g->scope`.

Because `cancel_scope` is a descendant of every enclosing scope, a deadline on
*any* ancestor still reaches the wait, so enclosing-scope cancellation keeps
working. For a coroutine that opens no nested scope, `cancel_scope == scope`, so
the common case is byte-for-byte unchanged.

**Spawn ownership is deliberately left on `g->scope`.** The deadline report
(`scope_fire_timeout`) and the non-draining sever (`scope_sever_live`) decide
tree membership from `g->scope`, so a nested scope's deadline reports and severs
exactly the coroutines spawned into its subtree — not the parent coroutine merely
executing its body. `zap_spawn_scoped` binds a child to the explicit scope handle
from the lexical scope stack, never to `cancel_scope`, so ownership is unchanged.

## D-0094 labelling

Unchanged and verified both ways:

- A scope's **own** deadline cancels the body: the woken wait raises `Cancelled`,
  unwinds to that scope's landing pad, and `zap_scope_exit_cancel_exc` re-labels
  it `ScopeTimeout` (the scope `timed_out`).
- An **enclosing** scope's deadline cancels the body: the wait still wakes (the
  ancestor contains `cancel_scope`), raises `Cancelled`, and the inner scope —
  which did not time out — propagates it unchanged; the enclosing scope that
  fired re-labels it `ScopeTimeout` at its own boundary.

## Alternatives considered

- *Mutate `g->scope` on lexical enter/exit instead of adding a field.* Rejected:
  `g->scope` is load-bearing for report and sever, and `scope_sever_live`
  atomically CAS-exchanges it from a timer thread, which would race a lexical
  push/pop on the coroutine.
- *Have the nested deadline also match ancestors.* Rejected: the runtime cannot
  tell which coroutine is running a given scope's body without tracking it, and
  matching ancestors would wake unrelated siblings.

## Consequences

Any future blocking cancellation point must record its waiter against
`cancel_scope`, not `g->scope`, or it will reintroduce this class of bug for
nested scopes. The scope report/sever paths must keep reading `g->scope`.
