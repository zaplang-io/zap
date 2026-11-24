---
title: "Derive synthesis supplies a ported built-in trait's method signature internally"
status: Accepted
issue: 1433
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Derive-synth built-in method signatures (#1433)

**Status:** Accepted — the E4 (#1429) enabler that lets a compiler built-in
derivable trait be ported to a library `@derivable` generator, cleared ahead of
the per-trait port slices (Eq #1431, then Ord/Hash/Repr).

## Problem

The library-derive synthesis (E3b, `crates/driver/src/derive_synth.rs`) learns
the method(s) to synthesize a forwarding stub for by matching a `@derivable(Trait)`
to a **same-named `interface Trait`**. Porting a built-in trait to a library
generator needs such an interface, but the names collide: `Eq` / `Ord` already
denote the **generic, reference-gated** operator-overloading interfaces `Eq[R]` /
`Ord[R]` (the operator prelude, #1199). There is no reliably-present, non-generic
`interface Eq` to read, and adding one would clash by name — so the port was
blocked before it began.

## Decision

When no same-named interface is present, the synthesis supplies the method
signature for a **ported built-in** trait from a small internal table
(`builtin_derive_method_sigs`), mirroring the built-in codegen's shapes
(`DerivedTrait::signatures`, D-0047):

- `Eq` → `__eq__(self, Self other) -> bool`
- `Ord` → `__cmp__(self, Self other) -> int64`
- `Hash` → `__hash__(self) -> int64`
- `Repr` → `__repr__(self) -> str`

The codec traits (`Json` / `Csv` / `Yaml`) carry associated functions and are out
of scope here; their port slices supply their own shapes.

## Why it is inert today

The fallback changes no existing program. The current built-ins are
`DerivableEntry::Builtin` with no `@derivable` generator, so the synthesis's
`generators` map never contains them and a type's `derived` list never names them
at this site. The table takes effect only once a trait gains a `@derivable`
generator — its port slice. A library `@derivable(X)` with no `interface X` for a
non-built-in `X` still gets the existing clear "no interface" diagnostic (no
regression), since the table returns `None` for any other name.

## Consequence

A built-in derivable trait can now be ported to a library generator without a
name-colliding interface. Eq (#1431) is the first to use it; Ord/Hash/Repr
follow. The derive and the operator-overloading interface stay separate: the
operator `Eq[R]` is untouched.
