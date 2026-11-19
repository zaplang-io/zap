---
title: A typed channel's element keeps its precise type on receive
status: Locked
issue: 1100
pr:
date: 2026-11-19
domain: runtime
supersedes:
superseded-by:
---

# Channel elements use the shared value-position classifier

**Status:** Locked

## Decision

`channel[T]` construction classifies its element type through the same
value-position classifier every other collection uses (`expr_to_type` +
`value_elem_of` → `elem_kind`), so the channel type carries the element's
**precise** `Elem` kind — a struct is `Elem::Struct(sid)`, a nested collection
`Elem::Nested`, and `bytes`/`complex`/`enum`/`float`/`char`/`bool` each their
own — rather than collapsing every reference element to an opaque `Elem::Ptr`.
A value received with `<-ch` (or a `select` receive case) therefore comes back
with `T`'s real type, so field access, `len()`, indexing, and Display on the
received value dispatch correctly. A tuple element stays rejected cleanly
(channel-ABI boxing of a value aggregate is a documented follow-up, #703).

## Rationale

The channel constructor classified its element from the subscript's bare *name*
(`chan_elem_from_name`): integers/bool/char → an `i64` word, `float` → an `f64`
word, and **everything else → `Elem::Ptr`**. A `channel[Entry]` thus had type
`Ty::Chan(Elem::Ptr)`, and `<-ch` yielded `Elem::Ptr.ty() == Ty::Ptr` — a bare
pointer that read back as `str`. The front end type-checked the program against
the typed-channel contract, but IR lowering of `entry.amount` saw a non-struct
receiver and failed with `error[ZAP6001]: field access on a non-struct value`.
This is exactly the read-back-identity bug family already fixed for nested
collections (#625) and `bytes` (#1074, D-0241): a collection/channel element
must stay recoverable, never flattened to `Ptr`.

Options considered:

- **Special-case structs** in the name classifier (map a struct name to
  `Elem::Struct`). Rejected: it leaves every *other* reference element
  (`channel[list[int]]`, `channel[bytes]`, `channel[complex]`) still collapsed,
  and re-implements, by hand and incompletely, the resolution the shared
  classifier already does. The failure is general, so the fix must be general.
- **Fix the receive site** to re-derive the element type. Rejected: the type is
  already lost at construction (`Ty::Chan(Elem::Ptr)` keeps no struct id), so the
  receive has nothing precise to recover. The element kind must be correct where
  the channel type is built — the single source both send and receive read.
- **Route element classification through `expr_to_type` + `value_elem_of`**
  (chosen). This is the identical path `dict` values, `list` elements, and
  `AtomicArray` elements already take, so channels inherit precise read-back for
  free and stay consistent with the rest of the collection surface.

## Consequences

- `channel[Struct]`, `channel[list[...]]`/other nested collections,
  `channel[bytes]`, `channel[complex]`, and `channel[enum]` now round-trip with
  their real type on receive. Scalar `channel[int]`/`channel[float64]` are
  unchanged (same `i64`/`f64` entry points).
- No ABI, runtime-C, or GC-rooting change: every reference element still crosses
  the channel through `zap_chan_send_ptr`/`zap_chan_recv_ptr` as a traced pointer
  word (the `Elem`'s `val_suffix()` is still `ptr`); only the *lowering* type the
  received value flows with changed. No new ZIR `Inst`/`Elem` variant.
- `channel[(T, U)]` (tuple element) remains rejected with a specific message,
  since a tuple is a value aggregate whose channel-ABI boxing is unimplemented
  (#703); the shared classifier would otherwise intern it as an anonymous struct.
- The dead `chan_elem_from_name` helper is removed.

See the read-back-identity precedents
[lang/1075-bare-none-optional-lowering.md](../lang/1075-bare-none-optional-lowering.md)
and the `bytes`/nested-collection element work (#625, #1074, D-0241).
