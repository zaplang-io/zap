---
title: http.Client response-body byte limit (max_response_bytes) + ResponseTooLargeError
status: Locked
issue: 1448
pr:
date: 2026-11-24
domain: stdlib
supersedes:
superseded-by:
---

# A response-body byte limit on `http.Client`

**Status:** Locked

## Decision

Give the keep-alive HTTP client an optional **response-body byte cap enforced
during the receive**, and a dedicated catchable error when a response would
exceed it.

- **API.** A new keyword on the `http.Client(...)` factory:

  ```zap
  auto client = http.Client(max_response_bytes=65536)   # cap each response body
  auto client = http.Client()                            # 0 / omitted = unlimited
  ```

  `max_response_bytes` is an `int64`; `0` (the default) means **no limit**, the
  same "`0` = unbounded" convention the sibling `timeout` keyword already uses on
  the same factory. A **negative** value is a `ValueError` raised before any dial
  (next to the existing `timeout` / `max_redirects` validation). The cap composes
  with `timeout`, `follow`, and `max_redirects` — it is an independent fourth
  control on the client, carried on every request the client makes.

  The cap lives on the **client**, not on the one-shot module verbs
  (`http.get` / `http.post` / `http.request`): those have no client object to
  carry policy and keep today's unbounded behavior. This matches the issue's
  repro exactly (`http.Client(max_response_bytes=…)`).

- **Error type.** A new first-class built-in exception
  **`ResponseTooLargeError`** (`ZAP_EXC_RESPONSE_TOO_LARGE_ERROR = 21`, parent
  `OSError`): `ResponseTooLargeError` ⊂ `OSError` ⊂ `Exception`. So `except
  ResponseTooLargeError` selects just the over-limit case, `except OSError`
  catches it alongside the rest of the I/O-error family, and an existing `except
  Exception` still catches it. It is slotted into the existing `OSError`
  hierarchy introduced for filesystem reads (see
  [lang/1311-filesystem-notfound-error](../lang/1311-filesystem-notfound-error.md)),
  not a parallel error path.

- **Enforcement during receive, all three framing modes** (in
  `zap_http_read_one`):

  | Framing          | When the cap fires                                                        |
  |------------------|---------------------------------------------------------------------------|
  | `Content-Length` | declared length `> max` → rejected **before reading the body**            |
  | `chunked`        | decoded size crosses `max` while accumulating → rejected mid-stream       |
  | EOF-framed       | buffered size crosses `max` as bytes arrive → rejected mid-stream         |

  A body of **exactly** the limit succeeds; the first byte **over** the limit
  produces the error (strict `>` comparison in every mode). On rejection the
  connection is **closed before the raise**, so a partial over-limit response
  never returns to the keep-alive pool and is never reused.

## Rationale

- **A cap keyword, not a streaming reader.** The issue asks to bound memory use
  *while receiving*, and its own repro proposes `max_response_bytes=N`. A simple
  cap is the smallest surface that satisfies every acceptance criterion
  (including "reject an over-large `Content-Length` without waiting for the
  body") and keeps the existing buffer-and-return response shape — the client
  still hands back a fully-formed `bytes` body, just a bounded one. A streaming
  / `iter_content`-style reader is a larger, separable surface; a bound does not
  need it, and nothing here forecloses adding one later.

- **`0` = unlimited.** Reuses the factory's own established sentinel (`timeout=0`
  is "no deadline"), so the three numeric client controls read uniformly and the
  default (`http.Client()`) is byte-for-byte the pre-#1448 client.

- **`ResponseTooLargeError` ⊂ `OSError`, a named type.** An over-size response
  is an I/O-layer condition, so `OSError` is the right home — the same base the
  filesystem surface uses (#1311). A *named* subtype (rather than a bare
  `OSError` with a telltale message) lets a caller select exactly the
  over-limit case — e.g. OAuth provider discovery rejecting an oversized
  metadata document (zaplang-io/oauth#6) — without parsing a message string,
  while `except OSError` / `except Exception` still catch it. This mirrors the
  `FileNotFoundError` ⊂ `OSError` precedent rather than inventing a new tree.

- **Enforced in the reader, not post-hoc.** A `len(bytes(resp))` check after the
  fact cannot bound memory: the client has already buffered the whole body. The
  cap must gate the receive loop itself, which is why it threads down to
  `zap_http_read_one` and is checked in each framing branch as bytes are framed.

- **Reject `Content-Length` early.** When the declared length already exceeds
  the cap, no body bytes are read at all — the error is raised off the header
  alone, so an attacker-sized `Content-Length` costs nothing.

- **Close on rejection.** On every over-limit raise the reader closes the
  connection before unwinding, so it is neither released to the pool nor
  (unlike a clean framed response) eligible for reuse. The raise propagates
  through the already-`throwing_call` `zap_http_pool_do` invoke, so an enclosing
  `try/except` catches it with no lowering change.

- **Internal pool primitive stays positional.** `http._pool_do` gains a 9th
  **positional** argument (`max_response_bytes`). It is an underscore-prefixed,
  prelude-only seam primitive deliberately kept positional-only (no
  `seam_sigs.rs` row), exactly like its siblings `_pool_new` / `_pool_close` and
  `json._lex_*` (D-0204). The user-facing keyword is on the prelude `def
  __http_client` factory, bound by the ordinary user-function keyword binder
  (the same path that already binds `follow` / `timeout` / `max_redirects`),
  which is why no `seam_sigs.rs` row is needed or added for the factory — it is
  D-0202 constructor sugar onto a prelude `def`, not a C-backed seam verb.

## Consequences

- **New built-in exception id `21`.** Wired at exactly the sites `OSError` /
  `FileNotFoundError` (#1311) and `MemoryError` (#882) occupy: the `ZAP_EXC_*`
  enum and `zap_exception_parent` in `zaprt.c`, sema's builtin name table and
  `is_exception_type`, and lowering's `exception_type_id_name`. No Python-interop
  name→id row is added — there is no CPython `ResponseTooLargeError`, so the
  pyinterop map would never reach it.

- **Scoped to the `http.Client` path.** Only responses read through
  `zap_http_read_one` (i.e. via a `Client`) are capped. The one-shot
  `http.get` / `http.post` / `http.request` verbs are unchanged. Extending a cap
  to those would need a new per-call keyword and is left out of this issue.

- **Granularity is per-frame, bounded by the limit.** The chunked / EOF checks
  fire as soon as the running decoded/buffered size exceeds the cap, so peak
  memory is bounded by roughly one fill/chunk beyond the limit, never the whole
  oversized body. The pass/fail boundary is on the *total* body size: exactly
  the limit passes, one byte more fails.

- **Message text.** The error message is
  `"http: response body exceeds max_response_bytes"` on every framing mode; tests
  pin the *type* and hierarchy, not the message.
