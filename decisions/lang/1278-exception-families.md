---
title: Package-defined exception families via a single @exception marker
status: Locked
issue: 1278
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Package-defined exception families (`@exception`)

**Status:** Locked

## Decision

Add a single struct marker, `@exception`, that makes a `struct` a **catchable
exception**, with an optional parent argument that groups it into a family:

```zap
@exception
struct DNSException:
    str message

@exception(DNSException)
struct NXDOMAIN:
    str message
```

- `@exception` — the struct is an exception: it may be `raise`d and named in an
  `except` clause.
- `@exception(Parent)` — additionally groups the struct under `Parent`, so
  `except Parent` catches it (and every other member of the family). `Parent`
  must itself be an `@exception` struct.

**Only an `@exception` struct may be raised or caught.** `raise`/`except` on a
plain struct is a compile error (ZAP5139 / ZAP5140), closing the former
`raise Point("oops")` footgun where any struct was silently throwable.

A family **root** is not a distinct kind — it is an ordinary `@exception` that
other exceptions point at, and is itself raiseable (like the built-in
`LookupError ⊃ IndexError` and Python's `OSError ⊃ FileNotFoundError`). Families
are **emergent**, may be **multi-level**, and each member has **at most one
parent** — an acyclic forest. A cycle (ZAP5138) and a non-`@exception` parent
(ZAP5137) are rejected; a malformed marker application (ZAP5136) too.

This is strictly a catch-family relationship: it adds exactly **one directed
parent edge** consulted only by the exception matcher's chain walk. There is
**no** field inheritance, **no** method inheritance/override, and **no** subtype
assignability (a `DNSException` is not usable where an `NXDOMAIN` is expected, or
vice versa, outside `except`). General inheritance remains the deferred Full-OO
epic #66; this feature does not pre-commit any of it.

## Rationale

- **One marker, not two.** An earlier draft had `@exception_base` (an abstract,
  non-raiseable root) plus `@errors(Base)` on members. Once a parent can be *any*
  `@exception`, the second marker is redundant and the abstract-root distinction
  buys nothing for the common case — every author would reach for `@exception`.
  Collapsing to one marker is the smallest surface that covers family-less
  exceptions, roots, and members uniformly. An opt-in abstract category remains
  available as a future additive `@exception(abstract=true)` if a real need
  appears.

- **The runtime already does the hard part.** A `ZapException` carries a
  name-hashed `type_id`; `zap_exception_is_a` walks a parent chain via
  `zap_exception_parent`; and cross-module catch already works because ids are
  hashes of the consistently module-mangled type names. The *only* missing piece
  was that `zap_exception_parent` hard-wired every user id's parent to
  `Exception`. The feature reduces to registering the declared member→parent
  edges in a runtime table that the user-id arm consults before that fallback —
  so source-order precedence, `except Exception`, and unrelated-propagates all
  carry over unchanged.

- **Decorator, not `struct Child(Base)`.** A parenthesized-base syntax reads as
  nominal inheritance and telegraphs #66. A struct marker reuses the existing
  decorator infrastructure (`check_decorator` / `require_target(Struct)`,
  type-name arguments like `@json`/`@inject`) and keeps the relationship
  explicitly exception-only.

- **Close the footgun now.** Gating raise/catch on `@exception` is a small
  pre-1.0 break that removes a real hazard (any struct being throwable with its
  fields silently dropped) and makes "this type is an error" legible at the
  definition site.

## Consequences

- **Breaking (pre-1.0):** a plain `struct` can no longer be raised or caught —
  it must be marked `@exception`. Existing user-defined exceptions add one line.
  Built-in exceptions and string raises are unaffected.

- **Multi-level families work from day one:** the runtime chain walk is
  depth-agnostic, so the only new structural obligation is sema acyclicity
  (needed anyway for termination).

- **Blast radius is additive at runtime:** the built-in hierarchy
  (ids 1..18, incl. `MemoryError`, decisions/lang/882-memoryerror-exception.md)
  is untouched; a built-in `zap_exception_parent` switch arm always wins, so user
  families never perturb it. The family table holds plain `i64` id pairs (no GC
  roots) and is populated once at startup in `@__zap_init_globals` — before any
  coroutine spawns — so its read needs no lock.

- **New diagnostics:** ZAP5136 (malformed `@exception`), ZAP5137 (parent not an
  exception), ZAP5138 (family cycle), ZAP5139 (raise non-exception), ZAP5140
  (catch non-exception).

Unblocks the DNS resolver error API (zaplang-io/dns#6), which wants
`except DNSException` to catch a whole family of concrete resolver errors. Full
design discussion: #1280.
