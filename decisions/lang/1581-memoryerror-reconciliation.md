---
title: Allocation-failure policy reconciled — bounded/temp allocations raise catchable MemoryError; GC infrastructure stays fail-stop
status: Accepted
issue: 1581
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Allocation-failure policy: catchable `MemoryError` vs fail-stop

**Status:** Accepted. Resolves the third policy item held under #1566. No
grammar/surface change; it aligns two sources of truth and closes three missing
unwind edges.

## Context

Two documents described allocation failure differently:

- **D-0119** (DECISIONS.md, #697) records a **fail-stop** policy: a `NULL`
  allocator result is out of memory, and the runtime prints `zap: out of memory`
  and `abort()`s rather than handing `NULL` to generated code. Its own "Why abort,
  not a catchable exception" note scopes this to the **GC allocation site**, where
  the mutator holds unrooted temporaries so unwinding through the exception path
  would itself need to allocate and traverse unsafe state. It explicitly names "a
  future catchable `MemoryError`" as "a possible increment, not a gap."
- **SPEC §10** states an allocation failure **raises `MemoryError`**.
- `MemoryError` already exists as a first-class type (`ZAP_EXC_MEMORY_ERROR = 18`,
  #882, decisions/lang/882-memoryerror-exception.md), parented to `Exception`, and
  the **arena** cap-overflow / heap-OOM path already raises it through an invoke.

The #1566 sweep of recoverable faults found three temp-allocation wrappers that
*already* raised on allocation failure but lowered through a **plain call**, so a
raised exception would unwind past (terminate) rather than reach a local
`try`/`except` — the same missed-unwind-edge defect class as #383 / #803 / #841.
It flagged the reconciliation as a policy decision and left it unchanged.

## Decision

The two documents are **not in conflict once the allocation is classified by
where it occurs**:

1. **Bounded / temporary allocations are recoverable → catchable `MemoryError`.**
   An allocation that backs a sized, transient result — a growable temp buffer, a
   bounded `malloc` for a built result, or an `arena` `cap`/heap request — raises
   `MemoryError`, which `except MemoryError` or `except Exception` catches, with
   `finally` / `with` cleanup on the unwind path. These sites are not inside the
   GC's unrooted-mutator window; the failing call is an ordinary libc
   `malloc`/`realloc` whose `NULL` the wrapper checks before touching any
   half-built state, so unwinding is safe.

2. **The GC's own infrastructure allocation stays fail-stop.** The collector's
   block-map / large-object-map failures, the `ZAP_GC_HEAP_LIMIT` mapped-memory
   and live-set caps, and the defensive guard on a `NULL` object-allocation
   result all remain a clean `zap_oom` → `abort()` (D-0119). This is exactly the
   site D-0119's rationale carved out: at a raising object-allocation the mutator
   holds unrooted temporaries, and the exception machinery would itself allocate.
   This path is unchanged.

SPEC §10 is reworded to state this split; D-0119's `abort()` remains correct for
its scope (the GC allocator), and this ADR records that it was always the
narrower policy, not a blanket one.

### Code changes (the three #1566-flagged wrappers + their raise types)

| wrapper | lowering before | after | runtime raise |
|---|---|---|---|
| `str.expandtabs` | plain `call` | `throwing_call` (invoke in a `try`) | `zap_strbuf_ensure` now `zap_panic_typed(ZAP_EXC_MEMORY_ERROR, …)` |
| `str.translate` | plain `call` | `throwing_call` | same (shared `zap_strbuf_ensure`) |
| `http.url` | `call_ret(.., false)` | `call_ret(.., true)` | `zap_http_url` OOM now `zap_panic_typed(ZAP_EXC_MEMORY_ERROR, …)` instead of the generic `zap_net_panic` |

`zap_strbuf_ensure` previously raised `ZAP_EXC_RUNTIME_ERROR` ("out of memory");
it now raises `ZAP_EXC_MEMORY_ERROR`. It is shared by `str.expandtabs`,
`str.translate`, and `str.format_map` — `format_map` already lowered through an
invoke (#1034/#1566), so it gains the correct type with no lowering change.

### Sibling audit

- **str / bytes:** `zap_strbuf_ensure` is the only temp-buffer OOM raise; all
  three of its callers now take the invoke edge. No other str/bytes method raises
  on a temp allocation through a plain call.
- **http accessors:** of the `http` functions that lower through a plain `call`
  (`status`/`body`/`header`/`final_url`/`redirects`/`req_*`/`pool_close`), none
  allocates-and-raises — they read response/request fields. `url` was the only
  plain-call `http` function that builds a buffer and can fail, and it is fixed.
- **net / http / tls internal OOM (`zap_net_panic("… out of memory")`):** these
  sit inside functions that **already** lower through an invoke (`http.get` /
  `post` / `request` / `do`, the `net`/`tls` I/O calls), so they are **already
  catchable** — as the generic net `Exception`, not `MemoryError`. Re-typing that
  whole live-socket-I/O surface to `MemoryError` is a broader, separate change and
  is deliberately **out of scope** here: it is not a catchability defect (the
  #1566 failure mode), only a type-precision nuance, and those allocations are
  interleaved with live connection state.
- `os`/`fs` already raise a typed `MemoryError` on their OOM path
  (`zap_sys_fail_typed(ZAP_EXC_MEMORY_ERROR, …)`).

## Consequences

- `try`/`except MemoryError` (and `except Exception`) now recovers a
  bounded/temp-allocation failure in `str.expandtabs` / `str.translate` /
  `str.format_map` / `http.url`, with `finally` / `with` cleanup honored. The
  **uncaught** form is unchanged: it still prints the normal
  `Uncaught exception: <message>` diagnostic and exits 1 (the uncaught reporter
  renders only the message string, so the `"out of memory"` / `"http: out of
  memory"` text is preserved).
- The GC heap-exhaustion `abort()` (D-0119) is **unchanged**, as are the other
  documented fail-stops (`sys.exit`, signals, the race-detector `_exit`,
  graceful-shutdown forced exit).
- **Type change (narrow):** a temp-buffer OOM in `str.expandtabs`/`translate`/
  `format_map` was `RuntimeError`, and an `http.url` OOM was a bare `Exception`;
  both are now `MemoryError`. Both descend from `Exception`, so `except Exception`
  is unaffected; only a handler that named the former type specifically would
  change, and these OOM paths are not reachable on a healthy host.
- Tests: host ZIR checks (`crates/ir/src/lib.rs`,
  `lower_temp_alloc_memoryerror_catchable_1581` and siblings) pin that the three
  call sites lower to `invoke @…` with an unwind edge inside a `try`, that the
  catch is also taken for an `except Exception` clause, and that the same wrapper
  stays a plain `call` in a frame with no active handler. Allocation failure
  cannot be forced on a healthy host, so there is no end-to-end OOM golden; the
  invoke lowering and the type mapping are the asserted contract. Runtime tests
  run under both Immix and Boehm in CI (the sole native gate).

<!-- Related: [lang/882-memoryerror-exception](882-memoryerror-exception.md),
[lang/1566-catchable-input-errors](1566-catchable-input-errors.md); D-0119
(DECISIONS.md, GC heap-exhaustion fail-stop). -->
