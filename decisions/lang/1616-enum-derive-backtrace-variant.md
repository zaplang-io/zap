---
title: A derive backtrace names the enum variant that carries the failing payload
status: Locked
issue: 1616
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# A derive backtrace names the enum variant that carries the failing payload

**Status:** Locked

## Decision

When a `@derive` failure's `required because of …` instantiation backtrace
(#1293) anchors on an **enum variant payload field**, its top frame names the
variant:

```
required because of payload field `r` of variant `Circle` of `Shape`
```

A plain struct field is unchanged — `` required because of field `w` of `User`
``. Previously an enum failure flattened every variant's payload fields into one
list and emitted `` field `r` of `Shape` ``, which reads as a struct field of
the enum and hides which variant pulled the failing type in (two variants can
even share a payload field name). Naming the variant makes the chain's "which
type → which field → which derive" complete for enums.

## Rationale

The multi-level chain (#1293) already descends through container levels
(`list`/`dict`/`tuple`/`Optional`) within a field's declared type, and the
`ZAP7003` consumer header already names the derived trait and type. The one
dimension missing for an enum was the *variant*: the backtrace builder received
the variant payload fields as a bare `Vec<ast::Field>` (`variant_payload_fields`)
with the variant name dropped. This decision pairs each payload field with its
variant name at the anchor-building step (`enum_anchors`), so the top frame can
distinguish a struct field from a variant payload field.

The change is confined to `crates/mono` (the driver that builds the backtrace
data) and the frame label text; the `crates/comptime` renderers (#1256) are
untouched. The frame label is the only observable difference.

## Consequences

- **Diagnostic (Experimental) wording change.** The `ZAP70xx`/`ZAP7003` comptime
  diagnostics are an Experimental surface (`docs/STABILITY.md`), so this is a
  behavior change to a non-promised surface; it is additive in information (a
  struct field's frame is byte-identical to before).
- **Host-tested; runtime via CI.** The anchor builders, the label, and the
  within-type chain are pure and host-tested (`crates/mono/src/lib.rs` tests),
  and the rendered output is host-tested in `crates/driver` (`derive_render_tests`).
  The end-to-end derive path is gated by the CI LLVM-18 matrix (Docker/LLVM is
  down on the maintainer box).
- **Cross-type provenance is a deferred follow-up.** This slice does *not* add
  the Outer-field → Inner-derive frame (when a nested *derived* type's own
  generator fails, link it back to the outer type's field that required it).
  That needs instantiation-request provenance threaded through mono's worklist
  (entries are `(name, args)` with no requested-from span, and the request is
  made through a forwarding stub, not directly from the outer generator), and it
  can only be runtime-verified in CI — so it is scoped as a follow-up under
  #1616 / epic #1553 rather than landed here. See `docs/COMPTIME.md` §7.
