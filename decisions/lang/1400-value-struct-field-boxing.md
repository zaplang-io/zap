---
title: A value struct stored into an aggregate field is boxed onto the heap
status: Locked
issue: 1400
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# A value struct stored into an aggregate field is boxed onto the heap

**Status:** Locked

## Decision

When a `value struct` is stored into an **aggregate field** — a struct field
(`fill_struct_fields`), a tagged enum-variant payload (`build_variant`), or an
`Optional.Some` payload built by the `#1262` auto-wrap (`expr_coerced`) — it is
**boxed onto the heap** (a fresh `StructNew` + field-wise copy, `copy_value_struct`)
when either:

* the source is an **lvalue read** (value semantics: the stored instance must be
  independent of the source), or
* the value struct is **inline-eligible** (a flat value struct / tuple, D-0051),
  whose by-value calling convention materializes it in a register / `sret`
  **stack spill**.

The shared helper is `FnLower::copy_field_value`. A *non*-inline-eligible value
struct (one with a nested value-aggregate field) is always a heap instance, so a
non-lvalue one still needs no copy — preserving the existing behaviour and
avoiding a redundant allocation. Reference structs and non-struct types pass
through unchanged. Local-variable binding (`auto x = f()`) is unaffected: the
spill and the binding share the one frame, so no boxing is inserted there.

## Rationale

A `value struct` field is physically a **pointer** to the value struct's storage
(D-0051 / `basic(Ty::Struct) == ptr`): the containing aggregate holds a pointer,
not the inlined bytes. For a heap/reference container that outlives the current
frame, that pointee **must** be heap-resident.

An inline-eligible value struct returned (or passed) by value lives in the
caller's **stack spill** (`abi_result` / the register/sret ABI). Storing that
spill's pointer straight into a heap aggregate field left the field pointing into
the caller's frame. Once that frame returned the field **dangled**; a later read
of an interior pointer faulted on the reused/zeroed slot.

Concretely (zaplang-io/zap#1400): the datetime-utils parser took a user
`tzresolver` that returned a `TimeZone` (a flat `value struct` with a `str name`)
**by value**, auto-wrapped into `Optional.Some`. The `Some` payload pointed at the
resolver's stack spill; after the resolver's frame returned, `TimeZone.name` read
`NULL`, and `datetime.resolve` → the runtime `zap_tz_zone_id` segfaulted on the
null name. A single-module control that built the zone *inline* (`datetime.zone`,
which lowers to a heap `StructNew`) survived, which is why the defect only
surfaced across the by-value call boundary.

The codebase already boxes a value struct for exactly this reason when coercing
it to an **interface value** ("its storage may be a stack slot under the
register/sret ABI", `crates/ir/src/lower/coerce.rs`) and when storing it as a
**collection element** (`box_if_value` / `box_elem_for_store`, D-0051 "stored in
a collection … is boxed"). Aggregate **fields** were the remaining storage site
that did not box, using the lvalue-only `copy_if_value`. This decision brings
field storage in line with the interface and collection paths: a value struct
stored anywhere behind a heap pointer is boxed so the container owns storage that
outlives every frame.

## Alternatives considered

* **Box unconditionally for every value-struct field store** (as collections do).
  Correct but adds a redundant heap copy for a non-inline value struct whose
  source is already a uniquely-owned heap instance; the `is_lvalue_read ||
  inline_eligible` condition boxes exactly the two cases that need it.
* **Heap-allocate every by-value return (`abi_result`)** instead of stack-spilling.
  Over-broad: it penalizes every value-struct/tuple return — the common case that
  never escapes a frame — to fix the escaping-store case, defeating the inline
  layout's whole purpose (D-0051).
* **Reject the escaping store in sema.** The store is legal and ubiquitous
  (`Optional.Some(f())`, `Struct(field=f())`); rejecting it is not an option.

## Consequences

* `FnLower::copy_field_value` is the single site deciding field-store boxing;
  `fill_struct_fields`, `build_variant`, and the `#1262` auto-wrap use it.
* A flat value struct stored into an aggregate field now costs one heap copy even
  when the source is a fresh constructor result. This is confined to value-struct
  fields of reference aggregates (a narrow case) and is the price of memory
  safety; hot paths that need no aggregate storage are unaffected.
* Value semantics are unchanged and strengthened: a stored value struct is always
  an independent, heap-owned instance.
