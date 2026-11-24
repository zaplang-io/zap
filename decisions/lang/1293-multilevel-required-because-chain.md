---
title: "A nested @derive failure renders a multi-level required-because chain"
status: Accepted
issue: 1293
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Multi-level derive `required because of …` chain (E3d #1293)

**Status:** Accepted — the remaining E3d slice (epic #1289), unblocked by the
nested-type use-site span (#1346). A `@derive` failure buried in a
container-of-containers now renders one `required because of …` frame per
container level crossed, inner-first, so the reader climbs from the exact
offending type out to the field.

## Problem

E3d shipped the consumer `ZAP7003` header, the dual-span generator fault
(#1335/#1341), and (#1346) a one-level attribution: a nested cause pointed the
primary caret at the element but produced only a single `required because of
field X of T` frame. For a type like `dict[str, list[Widget]]` the diagnostic
did not say *how* the failure reached the element — which container levels were
on the path.

The renderer and the carried payload were already N-frame capable: `#1256`'s
`DeriveBacktrace`/`render_derive_error` walk frames inner-first, and mono's
`DeriveErrorInfo.frames` is a `Vec<(label, span)>`. What was missing was the
per-level span to anchor the intermediate frames — delivered by #1346.

## Decision

When a generator aborts (`comptime.error`) and mono has not been handed explicit
frames, `enrich_derive_error` builds the chain from the declared type of the
field whose declaration contains the cause span:

1. The **field** frame (outermost): `field `buckets` of `Bag``, at the field span.
2. One frame per **container level** crossed on the path from the field's
   declared type down to the node the generator pointed at, each naming its
   container type (reconstructed from the AST, since mono has no source text):
   - list → `the element type of `list[Widget]``
   - dict key / value → `the key type of …` / `the value type of …`
   - optional → `the element type of `Optional[Widget]``
   - tuple → `element N of `(…)`` (1-based)

   Each frame's span is the container node it describes, so the carets zoom
   inward. Only the child on the path to the cause is followed (an unrelated
   dict key is never a frame). The walk stops at the node the generator pointed
   at, and is depth-bounded defensively.

Frames are appended outermost-first; the renderer (`append_backtrace`) prints
them **inner-first** — nearest the failure, climbing to the field — followed by
the library's own root-cause note. No renderer change: the shape #1256 defined
was already correct, only under-populated.

## Consequence

```
error[ZAP7003]: cannot derive `Ser` for `Bag`   (caret: the element Widget)
note: required because of the element type of `list[Widget]`
note: required because of the value type of `dict[str, list[Widget]]`
note: required because of field `buckets` of `Bag`
note: the generator reported: `Ser` is not implemented for this type
```

A bare struct field (`comptime.error(f.span, …)`) crosses no container level and
still renders the single field frame, unchanged. Regression-guarded by mono unit
tests (`crates/mono/src/lib.rs`: outermost-first order, nested spans enclosing
inward, tuple position, path-only descent, the no-container case) and driver
end-to-end tests (`crates/driver/tests/zap_cli.rs`: the deep `dict[str,
list[Widget]]` chain with explicit inner-first ordering, and the tuple
positional frame).

This closes the multi-level chain scope of E3d #1293. Rendering the chain for the
LLVM `build`/`run` path (versus the host check/`ir` path) remains a separate
follow-up.
