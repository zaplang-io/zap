---
title: dict / set initial-capacity hint on construction + reserve()
status: Locked
issue: 1083
pr:
date: 2026-11-19
domain: stdlib
supersedes:
superseded-by:
---

# dict / set initial-capacity hint on construction + `reserve()`

**Status:** Locked

## Decision

Add an optional **initial-capacity hint** to `dict` and `set` construction, plus
a post-construction **`reserve(n)`** method, so a known-large build pre-sizes the
open-addressing SoA table (#840) once instead of regrowing + rehashing from empty
~18 times.

### Surface (type-first)

```
dict[str, int64] d = dict[str, int64](capacity=200000)
set[str]         s = set[str](capacity=1000)
d.reserve(200000)
s.reserve(1000)
```

- **`capacity=n`** is a **keyword-only** constructor argument on `dict[K,V](...)`
  and `set[T](...)`. For `set`, it is an alternative to the existing positional
  iterable (`set[T](xs)`); the two are not combined (building from an iterable
  already sizes as it adds).
- **`reserve(n)`** is a method on a live `dict`/`set` with a single **positional**
  argument.

### Hint semantics

`capacity` / `reserve` are **hints**, not hard bounds: a *lower bound* on the
number of entries insertable before the first regrow.

- Rounded up to the table's existing size policy — the smallest power-of-two slot
  count that holds `n` entries below the 0.75 load factor (`n*4 < cap*3`),
  minimum 8 (the organic table's first size). One shared `zap_ht_cap_for(n)`
  computes this for both dict and set — the same policy the doubling path uses,
  not a second one.
- **`0` or omitted** = the current default (no pre-allocation; the first insert
  allocates the initial table exactly as a bare `{}` / empty `set` does).
- **Negative** — a negative **literal** (`capacity=-1`, `reserve(-1)`) is a
  **compile error** (`ZAP4024`), matching Rust `with_capacity` / Java
  `HashMap(int)` which reject a negative size. A run-time-negative (non-literal)
  value is clamped to a no-op.
- **Very large** — honored up to what the heap allows; past ~`1<<56` slots the
  backing-buffer arithmetic would overflow, so the hint fails through the
  **process-fatal OOM guard** (`zap_oom`, #697 / D-0119), exactly as an
  organically growing table would when the heap is exhausted. OOM is **not** a
  catchable exception here (see Rationale).
- **Smaller than the eventual size** still works — the table regrows past the hint
  as it does today, losing nothing.
- `reserve(n)` on a live table grows it (rehashing existing entries, losing none)
  and is a **no-op** when the table is already big enough; it **never shrinks**.

A pre-sized table is byte-for-byte what an organically grown one would be — same
probe sequence, same (still **unspecified**, #1066 / D-0235) iteration order —
because both the constructor keyword and `reserve` route through the one shared
rehash routine (`zap_dict_resize_to` / `zap_set_resize_to`), just allocated once.

### Python divergence (explicit)

CPython's `dict` and `set` have **no** capacity constructor and no `reserve`.
This is therefore a **Zap extension beyond the Python surface**, deliberately
modelled on the systems-language precedent instead:

- Java `new HashMap<>(n)` / `new HashSet<>(n)`,
- Rust `HashMap::with_capacity(n)` / `.reserve(n)`,
- Go `make(map[K]V, n)`.

`reserve` takes the Rust name and shape (positional `n`); `capacity=` is a
keyword because Zap's call syntax is Pythonic and a named hint reads better than a
bare positional on a constructor that otherwise takes none.

### Keyword binding — no seam row

`dict`/`set` construction and methods are **built-in**, hand-written check/lower
dispatch in `zapc_sema` / `zapc_ir`, **not** seam *module* functions. The seam
registry `crates/syntax/src/seam_sigs.rs` keys on a module name (`zlib`, `http`,
…) and feeds `bind_seam_args`; there is no `dict`/`set` module, so neither the
`capacity=` keyword nor `reserve` carries a `seam_sigs.rs` row or flows through
the keyword binder. `capacity` is keyword-only by construction (validated
directly); **`reserve`'s single argument stays positional** (one obvious arg, the
Rust shape). The keyword-surface behaviour is pinned in `crates/sema/tests/
seam_kwargs.rs` alongside the seam cases per the workspace stdlib rule, even
though no row applies.

## Rationale

- **The regrow churn, not the retained set, is what this removes.** `bench/
  perf-compare/dict_string.zap` (perf #836) builds a 200k-entry dict from `{}`;
  the SoA table doubles ~18 times, each regrow a fresh large `zap_ptrbuf` + a
  full rehash of every live key (where wyhash #1065 runs) + a GC-threshold spike,
  with the old buffer becoming immediate garbage. One pre-sized allocation
  removes those ~18 cycles. The live entry set is unchanged, so full-heap marking
  cost is unaffected — this targets the alloc + rehash path specifically.
- **OOM stays process-fatal, unlike an `IndexError`.** The dict/set buffer is
  allocated through `zap_ptrbuf` → `zap_oom` → `abort()`, the #697 clean-abort
  policy: OOM is deliberately **not** a catchable Zap exception because the
  mutator may hold GC locks when an allocation fails, so a `longjmp`/`zap_panic`
  unwind would be unsafe. Pre-sizing therefore aborts the same way an organic
  grow to that size would; it is **not** wrapped in `throwing_call`/`invoke` and
  is **not** `try/except`-able. (This is the one place the generic "built-in
  faults are catchable" rule does not apply, and it is intentional — see §
  `zaprt.c` OOM comment. A first-class, catchable `MemoryError` is a separate
  tracked follow-up under the arena epic #553.) The overflow guard routes to the
  same `zap_oom`, so a pathological `capacity` can never under-allocate and
  corrupt the table.
- **One size policy, one rehash routine.** `zap_ht_cap_for` derives the target
  bucket count from the *same* `(len+1)*4 >= cap*3` load trigger the insert path
  uses, and `zap_dict_reserve` / `zap_set_reserve` call the factored
  `*_resize_to(target)` that the doubling `*_resize()` now also calls. There is
  no second policy to drift, and a pre-sized table is indistinguishable from a
  grown one.
- **No new ZIR instruction, no GC hazard.** The constructor keyword lowers to the
  existing `zap_dict_new`/`zap_set_new` followed by a `zap_dict_reserve`/
  `zap_set_reserve` call, and `reserve` lowers to the same call — all ordinary
  `Inst::Call`s over the already-GC-traced dict/set pointer. No new `Inst` variant
  is introduced, so `inst_dest()` is untouched; because OOM aborts rather than
  throwing, no `invoke`/landing pad is needed.

### Rejected alternatives

- **A positional capacity (`dict[K,V](n)`).** Rejected: the workspace stdlib rule
  prefers a named argument, and a bare positional on a constructor that otherwise
  takes none reads as "this many elements", not "this much room". A positional
  argument stays a `ZAP4025` error.
- **Making a huge capacity a catchable `MemoryError`.** Rejected: it would diverge
  from how organic growth behaves (abort) and from the #697 process-fatal OOM
  policy; a catchable `MemoryError` is a cross-cutting follow-up, not this issue.
- **A `seam_sigs.rs` row for `capacity`/`reserve`.** Rejected: dict/set are not
  seam modules, so a row would be inert.
- **Combining `set[T](iterable, capacity=n)`.** Deferred: building from an
  iterable already right-sizes as it adds, so the combined form adds surface for
  little gain; it can be added later without breaking this one.

## Consequences

- New runtime surface in `zaprt.c` (the dict/set core, not yet split into a
  fragment): `zap_dict_reserve` / `zap_set_reserve`, the shared `zap_ht_cap_for`,
  the factored `*_resize_to`, and `zap_ht_resize_count()` behind the internal
  `__ht_resize_count()` builtin. The counter is **not** language surface — it is a
  test hook that lets the goldens assert a **zero-regrow** insert window
  deterministically (a wall-clock timing never could).
- Covered by `tests/golden/dict_capacity.{zap,expected}` and
  `tests/golden/set_capacity.{zap,expected}` (zero-regrow, smaller-hint-still-
  grows, reserve-on-live-table, `capacity=0` default, delete/reuse, full
  enumeration, string-key parity), the host sema suite in
  `crates/sema/tests/seam_kwargs.rs`, and the lowering asserts in
  `crates/ir/src/lib.rs`. The alloc/GC-golden validation rides the merge-queue
  full/arm64 gate (the LLVM build); local Docker/LLVM-18 is unavailable, so host
  checks + CI are the gate.
- Iteration order remains **unspecified** (#1066 / D-0235); pre-sizing does not
  make it stable and tests must not assert dict/set repr order.
