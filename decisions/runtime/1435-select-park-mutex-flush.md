---
title: A parked select flushes its stack-local park mutex before destroying it
status: Accepted
issue: 1435
pr:
date: 2026-11-24
domain: runtime
supersedes:
superseded-by:
---

# A parked `select` flushes its stack-local park mutex before destroying it

**Status:** Accepted

## Decision

When a blocking `select` resumes, `zap_select` re-acquires and releases its park
mutex `sel.mu` once — after unlinking the non-chosen waiters — immediately before
`pthread_mutex_destroy(&sel.mu)`. `sel.mu` lives on the select'ing coroutine's
stack frame, so it must be provably unreferenced by any other thread before that
frame is destroyed and reused.

## Rationale

A `select` that must block allocates `sel.mu` and its per-case `Waiter`s on the
select'ing coroutine's stack and parks on `sel.mu` via `copark`. A channel
counterpart that claims one of the cases runs, *while holding `sel->mu`*:

```c
sel->fired = 1; sel->chosen = ...; ...
coready(sel->pw);                 /* makes the select coroutine runnable NOW */
pthread_mutex_unlock(&sel->mu);   /* trailing unlock, AFTER the wake */
```

`coready` enqueues the select coroutine immediately, so with more than one worker
M it can be picked up by another M and return from `zap_select` — which destroys
`sel.mu` and lets the stack frame be reused — *before* the claimer's trailing
`pthread_mutex_unlock(&sel->mu)` executes. That unlock (and any concurrent lock of
the now-reused bytes) then writes pthread-mutex state over the reused frame. On
x86-64 this corrupted the next `galloc` allocation size; on aarch64, where the
`pthread_mutex_t` layout differs, the stray write landed on a reused mutex's
`__kind`, which glibc read as `PTHREAD_MUTEX_PRIO_PROTECT` and aborted inside
`__pthread_tpp_change_priority` (`tpp.c:86`). It was a recurring merge-queue flake
on the aarch64 full-matrix legs under the `time.ticker`/`time.after` + `select`
goldens (issue #1435).

Alternatives considered and rejected:

- **Unlock `sel->mu` before `coready` in the three claim sites**
  (`deliver_to_receiver` / `take_from_sender` / `chan_buf_refill_from_sender`).
  Workable, but it reorders the wake in three hot channel paths and still requires
  capturing `sel->pw` before the unlock; the single-site flush in `zap_select`
  changes no claimer behaviour and is far easier to audit.
- **Heap-allocate `SelState`.** Removes the stack-reuse hazard but adds an
  allocation (and GC pressure) to every blocking `select`, and the claimer's
  trailing unlock of a concurrently-freed heap mutex is still undefined — the
  lifetime problem would just move.

The flush is the same shape the scope-join path already relies on: `scope_join`
re-locks `sc->mu` after each `copark`, which is what makes the claimer's
coready-under-lock there safe. The other park mutexes — `channel->mu`,
`PollDesc->mu` — are long-lived and never destroyed, so they need no flush.

## Consequences

- A blocking `select` takes one extra uncontended lock/unlock of `sel.mu` on
  resume. Negligible, and only on the slow (parked) path.
- The non-chosen-case cleanup already acquires every non-chosen channel lock,
  which flushes any late claimer that popped one of those waiters; the added
  re-lock flushes the chosen-case claimer that actually woke us. Together they
  guarantee `sel.mu` is private before it is destroyed.
- Regression coverage: `tests/golden/time_ticker_select_stress.zap` (with a
  `MAXPROCS` `.env`) keeps a dense `select` claim/resume storm running under
  several worker Ms. The race is probabilistic — pre-fix the fixture aborted on
  the order of a third to a half of runs under AddressSanitizer at MAXPROCS=8
  (comparable to `time_ticker_many`) and never post-fix — so it catches a
  reintroduced regression within a few runs rather than on every run.

See [runtime/1360-coroutine-sleep-fd-less-timer-park](1360-coroutine-sleep-fd-less-timer-park.md)
and [runtime/1409-periodic-ticker](1409-periodic-ticker.md) for the timer surface
that exercises this path hardest.
