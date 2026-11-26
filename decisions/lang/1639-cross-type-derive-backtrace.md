---
title: A derive backtrace links a failing nested type to the outer field that required it
status: Locked
issue: 1639
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# A derive backtrace links a failing nested type to the outer field that required it

**Status:** Locked

## Decision

When a `@derive` failure's `required because of …` instantiation backtrace
(#1293) occurs in a nested *derived* type `Inner`, and `Inner` was pulled in by a
field of an enclosing type `Outer` that also derives the same trait, the chain
now prepends a **cross-type frame** for that field above `Inner`'s own
within-type chain, giving the full `Outer → … → Inner` path:

```
error[ZAP7003]: cannot derive `Ser` for `Inner`
  note: required because of field `b` of `Inner`
  note: required because of field `inner` of `Mid`
  note: required because of field `mid` of `Outer`
  note: the generator reported: `Ser` is not implemented for `Bad`
```

The header still names `Inner` (where the unsupported leaf actually is); the new
frames climb outward through each enclosing deriving type's field. The frame
crosses container levels (`list`/`dict`/`Optional`/tuple) exactly as the
within-type chain does, and names the variant for an enum-outer payload
(`payload field `inner` of variant `Wrap` of `Outer``, #1616). A standalone
derive failure — one not required by any other deriving type — is unchanged.

## Rationale

This is the cross-type follow-up deferred from #1616. The within-type chain
(field → container levels → variant → leaf) was complete; the one missing
dimension was the **edge between types** — when `Inner`'s generator fails, which
outer type's field pulled `Inner` in.

The issue sketched threading instantiation-request provenance through mono's
worklist. That approach does not work, for two concrete reasons in how derive
forwarding lowers:

1. A nested derive is reached through a **synthesized forwarding stub**
   (`Inner.ser` / `Inner.to_json`), a non-generic method of a non-generic type.
   Mono's pass 1 (`rewrite_struct_nongeneric`) rewrites every such method
   eagerly, so `gen$Inner` is first requested with **no derive context live**
   (`cur_derive = None`); that first request wins worklist dedup.
2. The outer generator's recursion into a nested field
   (`reflect.field(value, i).ser()`) is a **plain, non-generic method call**,
   which mono does not route through `request_method` (only generic methods are).
   So the outer expansion makes no worklist request for the nested generator from
   which to recover a `(trait, field span)`.

So there is no live `(Outer, field)` context at the point `gen$Inner` is
requested. Instead, the cross-type frame is reconstructed **structurally** at
failure time from data mono already holds:

- `Outer` *requires* `Inner` iff `gen$Outer` was actually instantiated this build
  (its forwarding stub forced the generator instance, so `gen$Outer` is in
  `requested_funcs` — i.e. `Outer` really derives the trait), **and** some field
  of `Outer` (a struct field, or an enum variant payload field) has a declared
  type that structurally contains `Inner`.
- Walk `Inner → requirer → requirer → …` through that requires-graph, building
  the frames per hop with the existing `struct_anchors` / `enum_anchors` /
  `derive_top_label` / `push_container_frames` helpers, anchored on the outer
  type's own source field spans, and prepend them outermost-first.

The requires relationship is always *true* — `Outer`'s derivation of the trait
genuinely requires `Inner`'s — so the frame never misleads, and the restriction
to instantiated generators keeps a type that merely *mentions* `Inner` but does
not derive the trait out of the chain. The change is confined to `crates/mono`;
the `crates/comptime` renderers (#1256) and the `DeriveErrorInfo` wire format are
untouched (the new frames ride the existing `frames` vector).

## Consequences

- **Diagnostic (Experimental) wording change.** `ZAP7003` is an Experimental
  surface (`docs/STABILITY.md`); this is additive (a standalone failure renders
  byte-identically to before, and a nested one only gains outer frames).
- **Host-tested; runtime via CI.** The structural helpers (`find_named_span`,
  `field_frames_referencing`, `cross_type_requirer`, `cross_type_chain`) are pure
  and host-tested in `crates/mono`, and the rendered end-to-end chain is asserted
  through `zap ir` in `crates/driver` (`zap_cli`) — the `comptime.error` fires in
  monomorphization, before codegen, so the diagnostic is observable without LLVM.
  The CI LLVM-18 matrix remains the runtime gate (Docker/LLVM is down on the
  maintainer box).
- **Deterministic representative path.** When several outer types require the
  same `Inner` (a diamond), the backtrace shows one representative requirer,
  chosen by a stable name-sorted scan; a recursive / mutually recursive type is
  walked once per type and then stops (`visited` guard). A *generic* outer whose
  generator instance is requested only after the inner failure aborts the
  worklist drain may not appear as a requirer — acceptable for the common
  non-generic nesting these diagnostics target.
