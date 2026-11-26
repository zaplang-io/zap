---
title: CSPRNG entropy failure raises a catchable OSError instead of aborting
status: Accepted
issue: 1580
pr: 1591
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# CSPRNG entropy failure is a catchable OSError

**Status:** Accepted. Resolves the second of the three policy items **Held** under
[lang/1566-catchable-input-errors](1566-catchable-input-errors.md) (its "Held for
maintainer decision" item 2).

## Decision

A failure to obtain entropy from the operating-system CSPRNG raises a **catchable
`OSError`** that a matching `try`/`except` handles, instead of terminating the
process with `abort()`. Any recoverable path **discards partial output**: a
caller never observes a partially-filled buffer from a raising generation call.

This matches SPEC §10, where `OSError` is the generic operating-system / I/O error
base: an exhausted or unavailable system entropy source is an operating-system
failure, not bad caller input. `OSError` and `IOError` share the runtime type id
`ZAP_EXC_OS_ERROR` (19), so `except OSError` or `except IOError` both catch it.

## Background

`zap_crypto_fill_random` (`crates/codegen/src/runtime/crypto.inc.c`) is the single
OS-CSPRNG chokepoint (`getentropy` → `/dev/urandom` fallback on POSIX;
`BCryptGenRandom` on Windows). It had three `fprintf(stderr, …); abort()` sites:

1. Windows `BCryptGenRandom` returns non-zero.
2. POSIX `getentropy` fails *and* `open("/dev/urandom")` fails.
3. A `read` from `/dev/urandom` returns `<= 0`.

It backs the crypto secrets surface (`random_bytes`, `token_hex`, `token_urlsafe`,
`randbelow`, `choice`) and the `uuid` module's random generators (`uuid1`/`uuid4`/
`uuid7`, `nanoid`). An `abort()` there terminated the process even inside a
matching `try`/`except` — the #1566 failure mode, applied to entropy failure.

## Implementation

Two defects are fixed, exactly as for the other #1566 surfaces:

1. **Runtime direct termination.** Each of the three `abort()` sites now
   `zap_panic_typed(ZAP_EXC_OS_ERROR, zap_str_new(m, …))` with a clear message,
   joining the C++-unwinder path. The `/dev/urandom` descriptor is closed before
   the read-failure raise so the unwind leaks no fd.
2. **Missing unwind edge.** Every CSPRNG-backed lowering now emits a
   `throwing_call` (an `Inst::Invoke` whose unwind edge targets the active landing
   pad inside a `try`; a plain `call` otherwise) instead of a plain `Inst::Call`,
   so the raise reaches the local handler. In `crates/ir/src/lower/modules/`:
   - `crypto.rs`: `zap_crypto_random_bytes` and `zap_crypto_token_hex` added to the
     throwing set (`token_urlsafe`/`randbelow` were already throwing for their
     `ValueError` paths; `choice` routes through `randbelow`).
   - `uuid.rs`: `gen0` gained a `throwing` flag; `uuid1`/`uuid4`/`uuid7` pass
     `true`, and `nanoid` was already throwing. The constant generators
     (`nil`/`max`/the namespace constants) and the hash-based `uuid3`/`uuid5` draw
     no entropy and stay non-throwing.

## Partial output is discarded

Traced every `zap_crypto_fill_random` caller: on the raising path the destination
is never caller-visible.

| caller | destination | on raise |
|---|---|---|
| `zap_crypto_random_bytes` | `ZapStr` allocated in the wrapper, returned only on success | GC object, never returned → discarded |
| `zap_crypto_token_hex` | `zap_buf` scratch, hex-encoded after the fill | scratch, never returned → discarded |
| `zap_crypto_token_urlsafe` | `malloc` scratch, base64url-encoded after the fill | scratch, never returned → discarded (see Consequences) |
| `zap_crypto_randbelow` / `choice` | a stack `uint64_t` | stack, discarded on unwind |
| `uuid.uuid1`/`uuid4`/`uuid7` | `ZapStr`/stack `rnd`, stamped and returned only on success | GC object / stack, never returned → discarded |
| `uuid.nanoid` | a stack `uint8_t buf[256]` decoded into the `ZapStr` built after | stack, discarded on unwind |

In every case the result object is built and returned only *after* a successful
fill, and the fill is the last operation that can raise, so unwinding past the
wrapper drops the (never-returned) buffer. No caller passes a caller-visible
buffer into `zap_crypto_fill_random`, so no scratch-and-copy was needed.

## Consequences

- `except OSError` (or `except IOError`) recovery now works for the crypto secrets
  and `uuid` random surfaces; the **uncaught** form still prints the normal
  `Uncaught exception: <message>` diagnostic and exits 1. Intentional termination
  elsewhere (`sys.exit`, signals, heap-exhaustion fail-stop) is unchanged.
- ~~`zap_crypto_token_urlsafe` leaks its one `malloc`'d scratch buffer on the
  raising path (the unwind bypasses its `free`). This is immaterial: a CSPRNG
  failure is a process-level, typically one-shot catastrophic condition, and the
  buffer is a single small allocation that is not caller-visible. Rewriting it to
  free on the unwind path was judged not worth the churn; noted here so it is a
  deliberate choice, not an oversight.~~ **Superseded by
  [lang/1600-token-urlsafe-scratch](1600-token-urlsafe-scratch.md):** a caller
  that catches the `OSError` and retries leaks one buffer per failure, so the
  scratch is now freed on the unwind path. `token_urlsafe` fills through a
  non-raising CSPRNG core (`zap_crypto_fill_random_try`) and `free()`s its scratch
  before raising; the other CSPRNG callers keep the raising
  `zap_crypto_fill_random` wrapper because their buffers are GC- or stack-owned
  and are safely dropped on the unwind.
- A CSPRNG failure is **not reproducible on a healthy host**, so there is no
  end-to-end golden for the failure path. Host ZIR checks
  (`crates/ir/src/lib.rs`) pin the invoke edge for every CSPRNG-backed entry point
  and pin the `OSError` type-id mapping (19 == `ZAP_EXC_OS_ERROR`); only the LLVM
  CI build exercises the unwinding itself. Runs under both Immix and Boehm.

<!-- Related: [lang/1566-catchable-input-errors](1566-catchable-input-errors.md). -->
