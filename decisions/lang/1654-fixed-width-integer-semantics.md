---
title: Width-stable fixed-width integer semantics; int8/int16 first-class
status: Locked
issue: 1654
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Width-stable fixed-width integer semantics (int8/int16 first-class)

**Status:** Locked.

This decision is driven by #1654 (`int8`/`int16` as first-class scalar value
types) but is deliberately **family-wide**: it defines the arithmetic semantics
for *every* fixed-width integer type, and in doing so it **supersedes the
promote-to-64 clause of D-0019 / #552**. `int8`/`int16` then fall out of the
general rule rather than being special-cased.

## Decision

Fixed-width integer arithmetic is **width-stable** and wraps at the operand
width (two's-complement, matching C `-fwrapv` / Go / Rust `wrapping_*`). The
result of an operation is a fixed-width type of a definite width, not a silent
promotion to 64-bit.

1. **Same-width typed operands → that width, wraps.**
   `int16 + int16 → int16` (wraps mod 2¹⁶); `uint8 + uint8 → uint8` (mod 2⁸).

2. **Mixed-width typed operands → widen to the larger operand, wrap there.**
   `int8 + int16 → int16`. (Zap keeps implicit widening here; this is *not*
   Rust, which would reject `i8 + i16`.) Signed↔unsigned mixing remains a
   compile error (ZAP4015, #552) — unchanged.

3. **An untyped integer literal is inferred to its context type (Rust-style).**
   A literal adopts the type its context demands — the typed operand in an
   arithmetic expression, or the declared/annotated target — and **must be
   representable** in that type or it is a compile error. So:
   - `int16 c; c + 1 → int16` (the `1` is an `int16`, wraps at 2¹⁶);
   - `int8 x = 300` and `int8_v + 300` are compile errors (literal out of range);
   - the explicit `int8(300)` remains the **wrap escape hatch** (C-cast
     semantics, D-0019/D-0020).

4. **No typed context → `int64` default, unchanged.** `x = 5` is `int64`;
   `5 + 3` is `int64`. "int64 is the default int" and "literals infer to
   context" coexist exactly as in Rust (default `i32`) and Go (default `int`):
   context overrides the default, it does not fight it.

5. **Widening on assignment/initialization stays implicit** (`int32 x =
   int16val`); **narrowing on assignment stays explicit** (`intN(...)`).

6. **`int8` / `int16` become first-class scalar value types** (#1654) and
   inherit rules 1–5. They previously existed only as array `Dtype`s / ZIR
   types (`I8`/`I16`) with no scalar `Ty`; this adds `Ty::Int8`/`Ty::Int16`
   threaded through resolve, convert, coerce, dicts, generics, Display,
   reflection, and FFI, plus the `int8(...)`/`int16(...)` value conversions.

7. **Division/modulo overflow at a narrow width** (`intN::MIN / -1`,
   `intN::MIN % -1`) wraps two's-complement, consistent with the existing
   `int64` rule (#742).

8. **Reserved, not implemented.** The following type names are recognized and
   emit a "reserved for future use" diagnostic (they are *not* `Unknown` and
   *not* usable as identifiers), with no semantics yet: `float8`/`f8`,
   `float16`/`f16`, `int128`/`i128`, `uint128`/`u128`, `isize`.

## Rationale

The pre-existing model (D-0019 / #552) promoted **all** fixed-width integer
arithmetic to 64-bit: `num_join(uint8, uint8) == uint64`,
`num_join(int32, int64) == int64` (tested invariants, `sema_types/src/lib.rs`).
Under it the declared width had **zero effect on arithmetic** — it mattered only
at a store or an explicit `intN(...)` cast. `uint8(200) + uint8(100)` was `300`,
not `44`. A declared `int16` that does not wrap at 2¹⁶ makes the width a storage
annotation, not a real arithmetic type — which is confusing precisely because
the type claims otherwise.

Options considered:

- **(A) Keep promote-to-64; make int8/int16 join it.** Uniform with the shipped
  family and already-decided, but bakes the confusing behavior into two more
  types and keeps the width a lie for computation. Rejected.
- **(B) Make int8/int16 alone width-stable.** Gives the intuitive behavior for
  the new types but splits the family (`int16 + int16 → int16` while
  `uint16 + uint16 → uint64`) — a worse inconsistency. Rejected.
- **(C) Make the whole fixed-width family width-stable (this decision).** The
  only end state where "the declared width *is* the arithmetic width, and it
  wraps there" holds uniformly, so int8/int16 need no special case.

Parity: pure Python has no equivalent (`int` is unbounded, never wraps), so
these scalars are a Zap extension regardless. The relevant anchor is where
Python programmers meet fixed-width ints — **NumPy**, which under **NEP 50**
(NumPy 2.0) is exactly this model: width-stable, promote-to-larger on mixed
width, weak (context-inferred) literals. NEP 50 exists *because* NumPy 1.x's
value-based promotion was too confusing — the same complaint that motivates this
decision. **Rust** and **Go** likewise keep integer arithmetic at the declared
width and infer literals to the context type. The promote-to-64 model matched
none of these.

The literal rule (3) takes Rust/Go/NumPy's context-inference specifically for
**literals**, while Zap keeps its own more permissive **typed** widening (rules
2, 5) for Python-readability (`int32 x = myInt16` should just work). The
mechanism already exists in-tree: untyped-literal-adopts-target with a checker
range-check is how the unsigned family handles assignment today
(`sema_types/src/lib.rs`, negative-into-unsigned caught with the literal's
value); rule 3 extends it to arithmetic context and signed narrows.

## Consequences

- **Semantic break for already-shipped types.** `uint8`/`uint16`/`uint32` and
  `int32` arithmetic changes behavior (`uint8(200) + uint8(100)` becomes `44`,
  not `300`; narrow `int32` loops that relied on 64-bit intermediates now wrap
  at 2³¹). Accepted as pre-1.0; these types are niche. This is tracked as a
  **separate family-wide "width-stable integer arithmetic" change** so the
  shipped-type break is visible apart from the int8/int16 introduction.
- **The counter case needs no special rule.** Because a literal infers to the
  operand width, `int16 c; c += 1` and `c = c + 1` stay `int16` and wrap — the
  augmented-assign-narrows rule considered under the rejected int64-promote
  variant is unnecessary.
- **The out-of-range-literal question is folded into rule 3** — one unified rule
  rather than a separate narrow-literal-overflow decision.
- **Mechanical surface is centralized.** The typing change lives at `num_join`
  (when one operand is `UntypedInt` and the other a concrete fixed-width int,
  adopt the concrete type instead of `default_type`-ing the literal to `int64`;
  the checker range-checks the literal) plus the mixed-width rule. Codegen is
  already width-aware (ZIR `I8/I16/U8…` already wrap, D-0005), so the gap is in
  the front-end type system, not codegen. Existing tested invariants
  (`unsigned_num_join_promotes_to_uint64`, the `int32`/`int64` join tests) and
  any uint8/int32 arithmetic goldens are rewritten.
- **Stale issue text.** #1654's "Design decision required" section asks about an
  implicit *widening lattice* `int8→int16→int32→int64`; this decision rejects
  implicit arithmetic widening in favor of width-stable + widen-to-larger, so
  that section must be rewritten before implementation.
- Overflow is silent (wrap), consistent with D-0019/D-0020. Catching overflow
  remains a separate, orthogonal future feature (checked arithmetic); promote-
  to-64 was never real protection against it.

<!-- Supersedes the promote-to-64 clause of D-0019 / #552 (unsigned family). -->
