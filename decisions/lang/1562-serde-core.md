---
title: "E6.1: serde core — monomorphized Serialize/Deserialize over reflection"
status: Accepted
issue: 1562
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# serde core — `Serializer`/`Deserializer` backends + generic `serialize`/`deserialize` (#1562)

**Status:** Accepted — the first E6 (#1552) slice, the payoff phase of the
comptime/`@derive` epic #1248. It establishes a **general** (de)serialization
layer over comptime reflection so that a codec (msgpack, cbor, …) is a **pure
library** — a backend plus `@derive`'d intent — with **no per-codec compiler
change**. This slice ships the core for the scalar (`int64`/`str`/`bool`) + nested-
struct surface; it is explicitly NOT Go/Java runtime reflection.

## Shape: free generic functions + interface backends (the key decision)

```zap
interface Serializer:   # emit_* / begin_struct / field_name / end_struct …
interface Deserializer: # read_* / begin_struct / end_struct …

def serialize[T, S: Serializer](T value, S s)      # compile-time field walk → s.emit_*
def deserialize[T, D: Deserializer](D d) -> T      # reflect.construct + per-field set_field
```

The backend is a **type parameter bounded by the backend interface**
(`[S: Serializer]`), so dispatch is **statically monomorphized per concrete
backend — no fat pointer / vtable** (the I#164/I#84 bound rule; cf.
`generics_bounds.zap`, monomorphizer docstring `crates/mono/src/lib.rs`). The
field walk is a `comptime for f in reflect.type_info(T).fields` loop with dead-arm
elision on `f.ty.name` / `f.ty.kind.name`, reading each field with
`reflect.field(value, comptime f.index)`. The lowered ZIR is direct `fieldget` /
`fieldset` + direct `@Backend__emit_*` calls, with a nested struct field recursed
inline through its own `serialize` specialization — fully **unboxed**, zero
runtime type inspection. This is the #1248 thesis made concrete.

### Why the backend is NOT threaded through a `@derive`'d method

The natural shape — `@derive(Serialize)` synthesizing `def serialize[S:
Serializer](self, S s)` — is **not expressible** today, and this is a deliberate
finding of the slice:

1. **Interface methods cannot carry their own generics** — `interface X: def
   m[S: I](self, S s)` fails to parse (ZAP2001).
2. The **`@derive` forwarding stub is non-generic** (`crates/driver/src/derive_synth.rs`,
   `forwarding_stub`, `generics: Vec::new()`), forwarding only the Self type
   argument to `generator[Self](self, …)`. A backend type parameter in the method
   body would be an unbound free type.

So a monomorphized backend must live on a **free generic function**, which is the
idiomatic Zap form (the msgpack library already uses `packb_with_default[T]`).
This is **not a workaround of a defect** — it is the correct expression, and it
fully satisfies the monomorphized/unboxed thesis. A future ergonomic (generic
interface methods, or a backend-threading `@derive`) could additionally offer a
`value.serialize(s)` method surface; it is not required for E6 and is deferred.

### `deserialize` call convention

Both type arguments are **explicit** at the call site — `deserialize[T, D](d)`.
`T` cannot be inferred (it is the `reflect.construct` target / the return type),
and a partially-explicit type-argument list does not resolve. A codec library
hides this behind a `unpack[T](bytes) -> T` wrapper that constructs the concrete
deserializer and supplies both arguments.

## Primitives relied on (all already shipped — no new core primitive)

- `reflect.type_info(T).fields` declaration-order iteration (E1 #1257).
- `reflect.field(value, comptime i)` typed field read; `reflect.construct(T)` +
  `reflect.set_field(out, comptime i, …)` (E4 #1454/#1490).
- `[S: Serializer]` interface-bounded generic params, static dispatch (I#164).
- `f.ty.kind.name` / `f.ty.name` comptime dispatch with dead-arm elision (#1326).
- `comptime.error(f.span, …)` for an unsupported field kind (E2 #1256).

## Scope boundaries and blocked work

- **Enum / generic-struct DESERIALIZE** has no construction primitive —
  `reflect.construct` is non-generic-struct only (`crates/mono/src/lib.rs`). Filed
  as **#1561 under epic #1248** and, per the golden rule, NOT worked around here.
  Enum *serialize* (variant read) is feasible and is a later slice.
- **No static field-count primitive**: `comptime len(reflect.type_info(T).fields)`
  does not reduce (`reflect` survives to codegen). A length-prefixed backend
  (msgpack map/array headers) buffers and backfills — a standard streaming
  technique, not a core gap.
- **The `@derive(Serialize, Deserialize)` opt-in marker** is deferred: a vacuous
  bound is not expressible (an interface needs method signatures and there is no
  universal method), so slice 1 ships the free-function core unbounded. The
  Rust-parity opt-in surface is a follow-up (a trivial derivable marker so
  `[T: Serialize]` gates participation).

## The proof (CI-LLVM-gated)

`tests/golden/serde_core_roundtrip.zap` ships a self-contained serde core with a
trivial token backend and asserts, over four structs: a flat struct round-trips
every field; the encoded token stream is exactly begin+fields+end; edge scalar
values (negative int, empty string, `False`) round-trip; a single-field struct
round-trips; and a nested struct field encodes by recursing into its own
specialization. Every assertion prints `True` and the stream equals its literal.
The runtime (round-trip) equality is gated by the **CI LLVM-18 matrix** (Docker
down on the maintainer box). A host test in `crates/driver/tests/zap_cli.rs`
asserts the lowering specializes `@serialize$T$Backend` / `@deserialize$T$Backend`
per (type, backend) and calls backend methods directly — the monomorphization
property — runnable without LLVM.

The core was also runtime-verified on the x86 LLVM-18 box ahead of CI (flat
round-trip `True`; nested `Outer(7, Point(1,2,"q",False))` → `<,7,<,1,2,q,False,>,>`).
