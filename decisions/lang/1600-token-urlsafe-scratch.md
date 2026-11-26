---
title: crypto.token_urlsafe — no scratch leak on entropy failure; allocation failure raises MemoryError
status: Accepted
issue: 1600
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# `crypto.token_urlsafe` scratch handling and allocation-failure type

**Status:** Accepted. Aligns `crypto.token_urlsafe` with the entropy-failure
policy ([lang/1580-csprng-oserror](1580-csprng-oserror.md)) and the
bounded/temp-allocation policy ([lang/1581-memoryerror-reconciliation](1581-memoryerror-reconciliation.md)).
No grammar/surface change; it removes a scratch leak and corrects one
exception type. It **supersedes the deliberate-leak note** in the #1580 ADR's
Consequences (that ADR's core OSError decision is unchanged).

## Context

`crypto.token_urlsafe(nbytes)` allocates a libc `malloc` scratch buffer, fills it
from the OS CSPRNG via `zap_crypto_fill_random`, base64url-encodes it (unpadded),
and `free()`s the scratch before returning. Two defects:

1. **Scratch leak on entropy failure.** `zap_crypto_fill_random` raises `OSError`
   on an entropy failure (#1580). That raise unwinds *past* `token_urlsafe`,
   skipping its `free(rnd)`. The size is caller-controlled, so a caller that
   catches the `OSError` and retries leaks one buffer per failure. The #1580 ADR
   recorded this as a deliberate choice "not worth the churn"; #1600 revisits it
   because the leak is caller-driven and unbounded under retry.

   `token_urlsafe` is the **only** CSPRNG caller that holds a raw `malloc` scratch
   across the fill. The others pass a GC `ZapStr` (`random_bytes`,
   `uuid.*`), a GC `zap_buf` (`token_hex`), or a stack buffer
   (`randbelow`, `uuid.nanoid`), all of which the unwind drops safely — so only
   `token_urlsafe` needs the free-before-raise.

2. **Wrong allocation-failure type.** If the scratch `malloc` itself fails,
   `token_urlsafe` raised `ValueError`. Under the #1581 policy a bounded/temp
   allocation failure is a catchable `MemoryError`.

## Decision

- **Allocation failure → `MemoryError`.** A `NULL` scratch `malloc` now raises
  `ZAP_EXC_MEMORY_ERROR` (#882), not `ValueError`. A **negative** `nbytes` is bad
  caller input and still raises `ValueError`; `nbytes == 0` still yields `""`.

- **Entropy failure frees the scratch.** The CSPRNG fill is split into:
  - `zap_crypto_fill_random_try(buf, n)` — a **non-raising** core returning `NULL`
    on success or a static error message on failure. It still closes the
    `/dev/urandom` fallback descriptor on a read failure before returning.
  - `zap_crypto_fill_random(buf, n)` — a thin **raising** wrapper that calls the
    core and `zap_panic_typed(ZAP_EXC_OS_ERROR, …)` on a message. Every GC-/stack-
    backed caller keeps using this wrapper; their behavior is unchanged.

  `token_urlsafe` now calls the **non-raising** core directly, and on a failure
  `free(rnd)`s the scratch before raising the same `OSError` (with the same
  message). The scratch is freed on every path — normal return, `MemoryError`
  (before fill), and `OSError` (entropy). No partial token is ever returned.

## Consequences

- `try`/`except OSError` around `token_urlsafe` recovers an entropy failure with
  **no leaked scratch**, even under catch-and-retry. `try`/`except MemoryError`
  recovers a scratch-allocation failure (the type is now correct). Both descend
  from `Exception`, so `except Exception` catches either.
- The other CSPRNG surfaces (`random_bytes` / `token_hex` / `randbelow` / `uuid.*`)
  are untouched: they still raise `OSError` through the wrapper, and their GC-/
  stack-owned buffers are dropped on the unwind as before (#1580).
- **Type change (narrow):** a scratch-allocation failure in `token_urlsafe` was
  `ValueError`; it is now `MemoryError`. A handler that named `ValueError` for the
  OOM case specifically would change; the negative-length `ValueError` is
  unchanged. The OOM path is not reachable on a healthy host.
- Supersedes the #1580 ADR's deliberate-leak note; #1580's OSError decision and
  its buffer-disposition table (for the other callers) stand.
- Tests: a host ZIR check (`crates/ir/src/lib.rs`,
  `token_urlsafe_alloc_failure_caught_by_except_memoryerror_1600`) pins the invoke
  edge and the `MemoryError` type id (18); the existing
  `csprng_failure_caught_by_except_oserror` pins the OSError (19) catch for the
  same call site. Host source-contract checks
  (`crates/codegen/src/tests_runtime_contract.rs`) pin that the OOM branch raises
  `MemoryError` (not `ValueError`), that the negative-length branch still raises
  `ValueError`, that `token_urlsafe` fills via the non-raising core and frees the
  scratch before the `OSError` raise, and that the core is non-raising and still
  closes the fallback descriptor on a read failure. Allocation and entropy
  failures cannot be forced on a healthy host, so there is no end-to-end golden
  for the failure legs; the existing `crypto_token_urlsafe` golden locks the
  success path (shape, unpadded base64url output, `nbytes == 0 → ""`, negative →
  `ValueError`). Runs under both Immix and Boehm in CI (the sole native gate).

<!-- Related: [lang/1580-csprng-oserror](1580-csprng-oserror.md),
[lang/1581-memoryerror-reconciliation](1581-memoryerror-reconciliation.md),
[lang/882-memoryerror-exception](882-memoryerror-exception.md),
[lang/1566-catchable-input-errors](1566-catchable-input-errors.md). -->
