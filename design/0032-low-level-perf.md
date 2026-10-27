# Proposal: Low-level performance escape hatch (target intrinsics, CPU feature detection + multiversioning, `cfg` conditional compilation)

- **Author(s):** Zap toolchain team
- **Last updated:** 2026-10-27
- **Status:** Accepted (ratified 2026-10-27 as D-0079)
- **Discussion:** issue #139. Depends on I#121 (module-registration seam,
  PR#122), I#24/PR#41 (numeric arrays / portable SIMD), I#67/PR#118 (generators
  → stackless state machine), I#106 (zlib/gzip, CRC32C-vs-ISO-CRC32).
  Constrained by D-0076 (1.0 spec-freeze): everything here is **additive /
  Experimental** (STABILITY §4.2) and lands **post-1.0**.

> Formatting convention (from Go): wrap near the 80-column mark and start each
> sentence on a new line.

## Abstract

I#139 asks for an **opt-in escape hatch** so a power user (DB/storage, HFT,
codec, HPC) can reach target-specific hardware — CPU intrinsics, cache/prefetch
hints, hardware CRC/popcount — **without** changing Zap's safe, Python-readable
default surface.
The ticket is a broad sketch across three tiers plus several cross-cutting
mechanisms (CPU feature detection, `@multiversion`/ifunc, `cfg`); this note
turns it into a **decided, phased plan with a deliberately small v1**.

The core argument is that the ticket bundles two very different kinds of work:
a **small, memory-safe, backend-trivial** core (three intrinsics that already
have first-class LLVM lowerings, plus the compile-time `cfg` needed to call them
portably), and a **large, safety- and platform-heavy** remainder (runtime
multiversioning/ifunc, over-reading SSE4.2 string ops, inline asm, Mach-O
support, the MLP coroutine helper).
The recommendation (§7) is to **ship the core first** and gate the rest behind
it, and to **sequence every ifunc/Mach-O/Apple-Silicon piece behind the
Linux/ELF path** because that hardware is PARKED (no CI runner).

The decision is **D-0079** (ratified 2026-10-27); the four load-bearing calls
surfaced in §8 were ruled on as recommended.

**This is a design-only ticket.**
It adds this one file and changes no compiler, runtime, stdlib, parser, or
codegen code, and no `SPEC.md` / `docs/GRAMMAR.ebnf` / `docs/STABILITY.md`.

## Background

### What exists today

- **Module-registration seam (I#121, PR#122).**
  New stdlib/intrinsic modules are conflict-free parallel-addable: a module is a
  file plus one slice entry in each of three registries
  (`crates/sema/src/modules/`, `crates/ir/src/lower/modules/`,
  `crates/codegen/src/modules/`), with lowering that emits either a `declare_rt`
  call or inline IR.
  **An `arch` module hangs directly off this seam** — this is why Tier 2 is
  "mostly plumbing," not new compiler fundamentals.
- **Portable SIMD (I#24, PR#41), Experimental.**
  Numeric array / vector types that autovectorize and lower per target.
  This is Tier 1 and the **recommended first answer** to "make it faster"; it
  needs no arch knowledge and is out of scope for this ticket except as the
  layer we point users at first.
- **Generators → stackless state machine (I#67, PR#118).**
  A `yield` body lowers to a heap state struct (flat CFG, `BrIf` ladder, no
  per-call heap alloc beyond the one state object) that satisfies `Iterator`.
  This is exactly the mechanism the MLP coroutine-interleaving helper (§6.5)
  would reuse — later, not in v1.
- **zlib/gzip (I#106, D-0060).**
  `zlib.crc32`/`gzip` use **ISO CRC32** (poly `0x04C11DB7`) via system zlib.
  This is a **different polynomial** from the SSE4.2 `crc32` instruction
  (Castagnoli, §6.6); the two must never be conflated.
- **1.0 spec-freeze (D-0076).**
  The 1.0 surface is frozen with a platform matrix: **Linux x86-64 Stable,
  Windows Stable, macOS x86-64 tier-2 (not CI-gated), macOS arm64 +
  aarch64-Linux Experimental.**
  Anything new here is additive/Experimental and post-1.0.
- **ZIR (`design/IR.md`).**
  Typed SSA IR; reference types are opaque `ptr`.
  Intrinsics lower to ZIR ops / typed runtime calls, then to LLVM — so
  `@llvm.ctpop`, `@llvm.prefetch`, `@llvm.x86.sse42.crc32.*` are reachable with
  no new IR *schema* (they are new op/intrinsic *kinds*, not a new contract).

### What is missing

There is today **no way to name a CPU intrinsic**, **no `cfg`/`when target.*`
conditional compilation**, and **no CPU feature detection or runtime
multiversioning**.
A user who knows `_mm_crc32_u64` exists has no way to reach it and no portable
way to guard the call.

### What is explicitly PARKED (hardware-gated)

macOS / Apple-Silicon has **no CI runner** (per project memory and D-0076).
Therefore **Mach-O ifunc**, Apple-Silicon NEON/CRC verification, and any
"universal binary" packaging are **hardware-gated**: designed now, implemented
and verified only after a runner exists, and always sequenced **behind** the
Linux/ELF equivalent.
This mirrors the parked state of I#224 (macOS), I#223 (aarch64 GC), I#260
(macOS-arm64 JIT-unwinder).

## Proposal

### 0. The shape of the escape hatch (design rules, unchanged from the ticket)

1. **Off the default surface.**
   Everything lives under an explicit `arch.*` import or an `@`-attribute.
   A casual program never imports `arch` and never sees any of this.
2. **Keep the C names.**
   `crc32_u64`, `popcount_u64`, `prefetch` — muscle memory from
   `nmmintrin.h`/`core::arch` transfers directly.
   Friendly wrappers (e.g. `std.hash.crc32c`) sit *on top of* the raw intrinsic,
   never replace it.
3. **Honest portability contract — no silent fallback.**
   `import arch.x86_64` **does not compile** when targeting aarch64.
   The developer is forced to write both paths (via `cfg`, §4) or stay on the
   portable Tier-1 layer.
   This is the single most important ergonomic rule: a silent software fallback
   would make "I asked for the hardware CRC" quietly mean "maybe."
4. **`unsafe` only where memory safety is actually at stake.**
   `crc32`/`popcount`/`prefetch` are memory-safe (no load/store the compiler
   can't see) and need **no** `unsafe`.
   The SSE4.2 string compares (`PCMPxSTRx`) read a full 16-byte lane and *can
   over-read past a buffer* → those require `unsafe`.
   `@target_feature` functions are `unsafe` to call directly (§5.3).

### 1. v1 scope — the minimal core (RECOMMENDED cut line)

**v1 ships exactly three intrinsics, on Linux/ELF x86-64 and aarch64, plus the
`cfg` needed to call them portably, plus one friendly wrapper.**

| Intrinsic | Lowering | Safe? | x86-64 | aarch64 |
|---|---|---|---|---|
| `crc32c_u8/u16/u32/u64(crc, v) -> u32/u64` | `@llvm.x86.sse42.crc32.*` / ARM `__crc32c*` | safe | SSE4.2 | CRC ext |
| `popcount_u32/u64(v) -> u32` | `@llvm.ctpop.iN` | safe | always | always |
| `prefetch(ptr, locality, rw, cache)` | `@llvm.prefetch` | safe (hint, never faults) | always | `PRFM` |

Plus:

- **`std.hash.crc32c(data) -> u32`** — a safe, portable wrapper that uses the
  hardware intrinsic when the target has it and a small table-driven software
  CRC32C otherwise. This is the one place a *software fallback is correct*,
  because the **result is bit-identical** regardless of path (unlike "did I get
  the hardware instruction?", which `arch.*` answers honestly).

**Why this cut line.**
These three are the only ops that are simultaneously (a) memory-safe,
(b) backend-known with a stable, documented LLVM intrinsic on both target arches,
and (c) representative of the three distinct value propositions in the ticket —
a hardware *checksum*, a hardware *bit-count*, and a *cache hint*.
They need **no `unsafe`, no runtime dispatch, no new runtime ABI, and no parked
hardware**.
`popcount` and `prefetch` are architecture-universal; hardware CRC32C is the one
feature-gated op, and in v1 it is reached **only** through `cfg` (compile-time)
or the `std.hash.crc32c` wrapper (correct fallback) — so v1 does **not** require
runtime feature detection to be useful.

**Deferred out of v1** (justified in §6): `@multiversion`/`@target_feature` +
ifunc (§6.3), SSE4.2 string ops and non-temporal stores (§6.4), full SSE/AVX
vector intrinsics (use Tier 1), inline asm / Tier 3 (§6.7), Mach-O +
Apple-Silicon verification (parked), the MLP coroutine helper (§6.5), and
runtime `cpu.has()`-guarded direct intrinsic use (§6.3).

### 2. Namespacing and import surface (DECISION)

```
arch                 # portable facade: cpu feature enum + (v1) nothing else callable
arch.x86_64          # x86-64-only intrinsics; does NOT compile off-target
arch.aarch64         # aarch64-only intrinsics; does NOT compile off-target
```

- **`cpu.has()` and the feature enum live in the portable `arch` facade**, not
  in an arch submodule — feature *detection* is meaningful to write in portable
  code (inside a `cfg` arm), even though each `Feature` value is arch-specific.
  `arch.cpu.has(.sse4_2)` compiles everywhere; it simply can only be `true` on a
  build whose target arch defines `.sse4_2`.
  (In v1, `cpu.has` is **defined but only usable inside `cfg`/wrapper code** —
  see §6.3 for why unguarded runtime dispatch is deferred.)
- **Arch intrinsics live under `arch.x86_64` / `arch.aarch64`** and importing the
  wrong one for the target is a **compile error**, not a fallback (rule 0.3).
- **Keep C names** inside each submodule (`x86_64.crc32_u64`,
  `aarch64.crc32c_x`). The portable `std.hash.crc32c` wrapper is the friendly
  name for the common case.

```
import arch.x86_64 as x86        # compile error unless target.arch == x86_64

fn hash_block(h: u64, word: u64) -> u64:
    return x86.crc32c_u64(h, word)    # -> @llvm.x86.sse42.crc32.64.64
```

Mechanism: an `arch` module family on the I#121 seam.
Lowering emits the LLVM intrinsic directly; sema rejects the off-target import
by checking the active `--target` arch during module resolution.

### 3. Runtime resolution model — pin the two-problem split (DECISION)

The ticket's central conceptual point, ratified here:

- **Cross-architecture (x86-64 vs aarch64) is COMPILE-TIME only.**
  Native code is arch-specific; an AOT language has no runtime dispatch *across
  arches*. The answer is per-target builds (`zap build --target …`) + `cfg`
  (§4). "Universal/fat binaries" are a *packaging* step (Mach-O slices), not a
  Zap runtime mechanism, and are parked with macOS.
- **Same-arch, different-feature (SSE4.2/AVX2 present or not) is genuine
  RUNTIME resolution** — CPUID on x86, `getauxval`/hwcaps on ARM — surfaced as
  `cpu.has()` and, later, `@multiversion`/ifunc (§6.3).

This split is kept **conceptually separate from interface/vtable dispatch**
(D-0016 fat pointers / I#84): vtable dispatch is *per-call*; CPU multiversioning
resolves *once at load* and patches the call site (zero per-call cost after
startup). They share no machinery.

### 4. `cfg` / `when target.*` conditional compilation (DECISION: split out, do first)

`cfg` is the **first real implementation dependency** and should be its **own
sub-ticket, implemented before any `arch.*` intrinsic** (see §7 Phase 1).
It is independently useful (stdlib already needs target branching for Windows vs
POSIX) and is the mechanism that makes rule 0.3's "no silent fallback" *livable*
— without it, an off-target `import` is a hard error with no portable workaround.

Recommended surface — a compile-time `when` on a `target` predicate, evaluated
during sema against the active `--target`, with the dead arm **not type-checked
against the wrong arch's module** (so `arch.x86_64` names inside an
`x86_64`-only arm don't break an aarch64 build):

```
when target.arch == .x86_64:
    import arch.x86_64 as a
    fn step(h: u64, w: u64) -> u64: return a.crc32c_u64(h, w)
elif target.arch == .aarch64:
    import arch.aarch64 as a
    fn step(h: u64, w: u64) -> u64: return a.crc32c_x(h, w)
else:
    fn step(h: u64, w: u64) -> u64: return crc32c_sw(h, w)   # portable
```

Predicates for v1: `target.arch` (`.x86_64`, `.aarch64`), `target.os`
(`.linux`, `.windows`, `.macos`), and `target.has(.feature)` for
**compile-time-known** features (a feature baked into the build via
`--target-feature`, distinct from the runtime `cpu.has`).
`cfg` is a **block-level** construct (declaration/import/statement position); it
is *not* an expression macro and does no token pasting.
It needs a small addition to `docs/GRAMMAR.ebnf` **in its own implementation
ticket** (not here — this is design-only).

### 5. `@multiversion` / `@target_feature` — the attribute surface (DESIGN; built in a later phase)

Designed now so the v1 `arch` surface doesn't paint us into a corner, but **not
in v1** (§6.3).

```
@target_feature("avx2")           # building block; UNSAFE to call directly
unsafe fn sum_avx2(xs: []f32) -> f32: ...

@multiversion(.avx2, .sse2, .default)   # compiler emits variants + a resolver
fn sum(xs: []f32) -> f32: ...           # SAFE to call; dispatch is resolved
```

Three rules pinned:

1. **`.default` is MANDATORY.**
   The compiler **rejects** a `@multiversion` with no `.default` arm, so there is
   never a "no implementation for this CPU" hole. This is the safety spine.
2. **`@target_feature` functions are `unsafe` to call directly.**
   Calling an AVX2 function on a CPU without AVX2 is an illegal-instruction trap
   (SIGILL). Such a function is callable safely **only** from inside a matching
   `@multiversion` variant or after an explicit `cpu.has()` check. (Same rule as
   Rust; keeps the type system honest about a hardware precondition.)
3. **Resolution is once-at-load via ifunc where available** (ELF STT_GNU_IFUNC /
   a resolver), falling back to a **first-call-cached function pointer** where
   ifunc is unavailable (§6.3) — identical observable semantics, one predictable
   branch slower.

### 6. Decisions on the ticket's open questions

#### 6.1 Namespacing — **decided** (§2): `arch.x86_64`/`arch.aarch64` + portable `arch` facade holding `cpu`/`Feature`; C names kept; off-target import is a compile error.

#### 6.2 v1 intrinsic breadth — **decided** (§1): `crc32c` + `popcount` + `prefetch` only; everything else deferred.

#### 6.3 CPU feature detection + multiversioning — **model pinned (§3/§5), implementation deferred past v1, ELF-first.**
`cpu.has()` exists in v1 as a compile-usable primitive (inside `cfg`/wrapper),
but **unguarded runtime dispatch, `@multiversion`, and ifunc are a later phase**
because: (a) ifunc resolvers are fiddly and platform-specific; (b) the safety
model (`@target_feature` unsafe, mandatory `.default`) wants to be built once,
carefully; (c) **Mach-O ifunc is parked hardware** and must sit behind the ELF
path. v1 is useful without any of it (§1).

#### 6.4 SSE4.2 string ops / non-temporal stores — **deferred.**
`PCMPxSTRx` over-read a full 16-byte lane → `unsafe`, and their value is narrow
(mostly substring/charset scanning that Tier-1 SIMD or a careful scalar loop
often matches). Non-temporal stores need a matching fence story. Defer to a
dedicated "x86 string/stream intrinsics" ticket after the core lands.

#### 6.5 MLP coroutine interleaving — **confirmed: a later helper on I#67, not v1.**
The batched-independent-lookup pattern (overlap cache misses across N searches
via `prefetch` + `suspend`/resume; ~2-3x on memory-latency-bound bulk queries)
is a **collections helper built on the existing generator state machine**, not a
language feature. It depends on the generator frame being effectively zero-cost
(inlined, no per-call heap alloc) and on `prefetch` (which v1 provides). Schedule
after v1 + the multiversion phase.

#### 6.6 CRC32C vs ISO CRC32 — **documented, coordinated with I#106.**
SSE4.2 `crc32` computes **CRC32C (Castagnoli, poly `0x1EDC6F41`)**. It is **NOT**
the ISO/zlib CRC32 (`0x04C11DB7`) that gzip/PNG require.
Therefore:
- `arch.*.crc32c_*` and `std.hash.crc32c` are **Castagnoli only** and are named
  `crc32c` (never bare `crc32`) to prevent the confusion.
- gzip/zlib keep using `zlib.crc32` (ISO) from I#106 — unchanged.
- Hardware-accelerated **ISO** CRC32 is a *separate* path (PCLMULQDQ carry-less
  folding), explicitly **out of scope** here; if ever wanted it belongs to the
  zlib module, not `arch`.

#### 6.7 Inline asm (Tier 3) — **confirmed deferred, with trigger.**
Revisit only if a concrete, repeated need appears that Tier 2 cannot express
(an instruction with no LLVM intrinsic). Most "I want inline asm" is really "I
want a Tier-2 intrinsic." Not scheduled.

#### 6.8 Friendly wrappers vs raw-only — **both, layered.**
Raw `arch.*` intrinsics are the foundation; `std.hash.crc32c` is the one v1
wrapper. More wrappers are added as demand appears, always on top of the raw
layer.

### 7. Phased roadmap

| Phase | Content | Platform | Cost | Hardware-gated? |
|---|---|---|---|---|
| **1 — `cfg`** | `when target.*` conditional compilation (arch/os/compile-time-feature predicates); dead-arm skip | all | **M** | no |
| **2 — core intrinsics (v1)** | `arch` seam; `crc32c`/`popcount`/`prefetch`; off-target-import error; `cpu.has` primitive; `std.hash.crc32c` wrapper | Linux/ELF x86-64 + aarch64 | **M** | no |
| **3 — multiversioning (ELF)** | `@target_feature` (+unsafe rule), `@multiversion` (+mandatory `.default`), ifunc resolver, non-ifunc fp-cache fallback | Linux/ELF | **L** | no |
| **4 — x86 string/stream** | `PCMPxSTRx` (`unsafe`), non-temporal stores + fence story | x86-64 | **L** | no |
| **5 — MLP helper** | interleaved bulk-lookup collection helper on I#67 generators + `prefetch` | portable | **M** | no |
| **6 — Mach-O / Apple-Silicon** | Mach-O ifunc, Apple-Silicon NEON/CRC verification, universal-binary packaging | macOS x86-64 + arm64 | **L–XL** | **YES (parked)** |
| **(none) — Tier 3 inline asm** | `asm!` block | — | **XL** | deferred, unscheduled |

Phases 1→2 are the committed near-term work and are the full extent of what this
proposal recommends building first.
3→5 are additive and can proceed once the core is proven.
6 is **blocked on a macOS/Apple-Silicon CI runner** and sits behind the ELF
versions of phases 2–3 regardless.

### 8. Recommended sub-ticket breakdown (to OPEN after ack — not opened here)

1. **`cfg`/`when target.*` conditional compilation** (Phase 1, **M**).
   Grammar + sema; the first dependency. *Do first.*
2. **`arch` module seam + v1 intrinsics** (Phase 2, **M**).
   `crc32c`/`popcount`/`prefetch` lowering on I#121; off-target-import error;
   Linux x86-64 + aarch64 goldens.
3. **`std.hash.crc32c` wrapper + `cpu.has` primitive** (Phase 2, **S**).
   Hardware-or-software, bit-identical; `cpu.has` for use inside `cfg`.
4. **`@target_feature` + `@multiversion` + ELF ifunc** (Phase 3, **L**).
   Attribute surface, unsafe/`.default` rules, ifunc resolver + fp-cache
   fallback. ELF only.
5. **x86 SSE4.2 string + non-temporal stores** (Phase 4, **L**, `unsafe`).
6. **MLP interleaved bulk-lookup helper** (Phase 5, **M**, on I#67).
7. **Mach-O ifunc + Apple-Silicon verification + universal binaries**
   (Phase 6, **L–XL**, **HARDWARE-GATED / parked**).

(Issue #139 stays the epic; these are its children. This session does **not**
open them — recommendation only, per house rule.)

## Rationale

**Why cut v1 this hard.**
The ticket's own framing — "mostly plumbing on the I#121 seam" — is true *only*
for the three safe intrinsics.
The moment runtime dispatch, over-reading ops, or Mach-O enter, the work stops
being plumbing and becomes a safety-model + platform-ABI project.
Shipping the safe core first delivers the ticket's actual promise ("you can
reach the hardware CRC/popcount/prefetch, portably gated") at **M** cost and
zero parked-hardware dependency, and lets the larger pieces be judged on real
demand rather than speculation.

**Why `cfg` first and separate.**
Rule 0.3 (no silent fallback) makes an off-target `import` a hard error; without
`cfg` there is no portable way to write a both-arches program, so the intrinsics
would be unusable in portable code the day they land.
`cfg` is also needed independently by the stdlib (Windows vs POSIX) and is the
smaller, self-contained piece — the right first brick.

**Why `cpu.has` without full multiversioning in v1.**
Feature *detection* is cheap and safe; the hard, deferrable part is *dispatch*
(ifunc, resolver ABI, the `@target_feature`/`.default` safety spine, Mach-O).
v1 gives detection (usable from `cfg`/wrappers) and defers dispatch, which keeps
v1 at **M** while still letting the one feature-gated v1 op (`crc32c`) be reached
correctly.

**Why lean on LLVM.**
`@llvm.ctpop`, `@llvm.prefetch`, and `@llvm.x86.sse42.crc32.*` are backend-known
and stable; v1 adds new *op kinds* on the existing ZIR→LLVM path, not a new IR
contract — the same reason I#121 makes the module addition cheap.

**Alternatives considered.**
*Ship the whole Tier 2 set at once* — rejected: bundles unsafe over-reading ops
and parked Mach-O work into the critical path.
*`cpu.has` runtime dispatch in v1 instead of `cfg`* — rejected: runtime dispatch
doesn't solve the cross-arch problem (that's inherently compile-time), and a
proper dispatch story wants the `@multiversion` safety model, which is Phase 3.
*A single `arch` module with runtime arch checks* — rejected: there is no
runtime arch in AOT code; the honest model is compile-time `cfg` (§3).

## Compatibility

Additive only.
New import-gated `arch` module family, a new `std.hash.crc32c`, a new
`when target.*` block form, and (Phase 3) two new attributes.
No change to any existing type, no change to the default/portable surface, no
prelude change.
All of it is tiered **Experimental** under D-0076, carries no 1.0 stability
promise, and lands post-1.0.

Nothing here touches `SPEC.md`, `docs/GRAMMAR.ebnf`, or `docs/STABILITY.md` **in
this ticket**.
The eventual implementation tickets would: add `when target.*` to the grammar
(Phase 1), add `arch`/`std.hash.crc32c` to the stdlib inventory, and add
Experimental rows to the stability tables (Phase 2+).

## Implementation (sketch, after ack — NOT built here)

- **Phase 1 (`cfg`):** parser + sema evaluate `when target.*` against the active
  `--target`; dead arms are parsed but **not** name-resolved/type-checked against
  the off-target module set. Grammar addition in that ticket.
- **Phase 2 (`arch` + intrinsics):** `crates/{sema,ir/src/lower,codegen}/src/
  modules/arch_x86_64.rs`, `arch_aarch64.rs`, `arch.rs` (facade) on the I#121
  registries; lowering emits the LLVM intrinsic; sema rejects off-target import
  by inspecting `--target` arch. Goldens on Linux x86-64 **and** aarch64 (known
  CRC32C/popcount vectors; `prefetch` is a no-observable-effect compile+run
  smoke). **No `zaprt.c`/ABI change** for the three safe ops.
- **Phase 3 (multiversion/ifunc):** attribute parsing, variant codegen, ELF
  ifunc resolver emission + non-ifunc fp-cache fallback. **This is the first
  phase that touches `docs/RUNTIME-ABI.md`** (resolver/ifunc ABI). ELF only;
  Mach-O is Phase 6.
- **Tests throughout:** both GC collectors where relevant; **no exact-heap-byte
  asserts** (project rule); aarch64 goldens must tolerate the parked I#223 state
  (don't tie correctness of the *intrinsic* to the unrelated GC race).

Phase boundaries touched: **none in Phases 1–2** (no IR schema or runtime-ABI
change for `cfg` + the three safe intrinsics); `docs/RUNTIME-ABI.md` is touched
first in **Phase 3** (ifunc/resolver ABI).

## Open issues

- The four §8-referenced load-bearing calls below are the open decisions.
- Exact `prefetch` hint enum spelling (`.T0/.T1/.T2/.NTA` + rw + cache-type) —
  a Phase-2 detail, not load-bearing.
- Whether `std.hash.crc32c`'s software fallback ships in Phase 2 or waits for a
  demand signal (recommended: ship it — it's small and makes the wrapper honest
  on non-SSE4.2 targets).

## Recommendation + decision (D-0079, ratified 2026-10-27)

**Recommendation (one line):** ship a minimal, memory-safe escape hatch first —
`cfg` conditional compilation, then an `arch.{x86_64,aarch64}` module exposing
**`crc32c` + `popcount` + `prefetch`** (no `unsafe`, no runtime dispatch) on
Linux/ELF, plus a `std.hash.crc32c` wrapper — and defer multiversioning/ifunc,
SSE4.2 string ops, inline asm, the MLP helper, and **all Mach-O/Apple-Silicon
work (parked hardware)** to later additive phases sequenced behind the ELF path.

### Decision

> **D-0079 (Accepted — ratified 2026-10-27).**
> Zap gains an opt-in, Experimental (per D-0076), post-1.0 low-level escape
> hatch, delivered in phases.
> **v1 = `cfg`/`when target.*` compile-time conditional compilation (its own
> sub-ticket, done first) + an `arch.x86_64`/`arch.aarch64` module (portable
> `arch` facade holding `cpu`/`Feature`) exposing exactly `crc32c`, `popcount`,
> and `prefetch` — all memory-safe, no `unsafe`, no runtime dispatch — on
> Linux/ELF x86-64 and aarch64, plus a portable `std.hash.crc32c` wrapper.**
> Off-target `import arch.*` is a **compile error** (no silent fallback).
> Cross-arch selection is **compile-time only** (`cfg` + per-target builds);
> same-arch feature selection is runtime, but `@multiversion`/`@target_feature`/
> **ifunc** is a later phase with a **mandatory `.default`** arm and
> `@target_feature` functions **`unsafe` to call directly**, implemented
> **ELF-first**.
> SSE4.2 string ops (`unsafe`, over-reading), non-temporal stores, inline asm
> (Tier 3), the MLP coroutine-interleaving helper (a later helper on I#67
> generators), and **Mach-O ifunc + Apple-Silicon verification + universal
> binaries (HARDWARE-GATED / parked)** are all deferred to additive phases.
> SSE4.2 `crc32` is **CRC32C/Castagnoli** and is named `crc32c`, distinct from
> gzip/zlib's **ISO CRC32** (I#106), which is unchanged.
> No change to the default/portable surface, no new fundamentals; the `arch`
> module rides the I#121 seam and the three safe ops lower to existing
> backend-known LLVM intrinsics with no IR-schema or runtime-ABI change.

### The load-bearing calls (ruled on as recommended)

1. **v1 scope cut line (§1).**
   Confirm v1 = `crc32c` + `popcount` + `prefetch` only (memory-safe, no runtime
   dispatch, no parked hardware), with everything else deferred — vs. a broader
   first cut.
2. **`@multiversion`/`@target_feature` attribute surface + mandatory `.default`
   + `@target_feature`-is-unsafe rules (§5).**
   Confirm the designed-now/built-later attribute model, including that the
   compiler rejects a `@multiversion` with no `.default` and treats
   `@target_feature` functions as `unsafe` to call directly.
3. **Split `cfg` into its own sub-ticket and build it first (§4).**
   Confirm `when target.*` conditional compilation is sequenced ahead of any
   `arch.*` intrinsic.
4. **ELF-first / Mach-O-behind-parked-hardware sequencing (§3/§7 Phase 6).**
   Confirm all ifunc/Mach-O/Apple-Silicon work sits behind the Linux/ELF path
   and is gated on a macOS CI runner existing.

A secondary, non-blocking call: whether `std.hash.crc32c`'s software fallback
ships in Phase 2 (recommended) or waits for demand.
