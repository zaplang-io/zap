# Proposal: Low-level performance escape hatch — arch intrinsics, CPU feature detection + multiversioning, `cfg` conditional compilation

- **Author(s):** Zap toolchain team
- **Last updated:** 2026-10-27
- **Status:** Draft (design-only; awaiting maintainer review)
- **Discussion:** issue #139 (design/epic). Related: #121 module seam
  (mechanism), #24/PR#41 numeric arrays/SIMD (Tier 1), #67 generators (MLP
  coroutine pattern), #106 zlib/gzip (CRC32C vs ISO CRC32), #84/D-0016 interface
  dispatch (contrast). Constrained by D-0076 (1.0 spec-freeze) and
  `docs/STABILITY.md` (D-0070/D-0071).

> Formatting convention (from Go): wrap near the 80-column mark and start each
> sentence on a new line.

## Abstract

This proposal scopes #139 — an **opt-in escape hatch** that lets a power user
reach target-specific CPU features (hardware CRC, popcount, prefetch, later SIMD
string ops) **without** touching Zap's safe, Python-readable default surface.
Casual users never see it; a systems programmer who knows exactly what they want
can reach the metal through an explicit, honestly-gated `arch.*` namespace.

The ticket is a large sketch with three tiers, several cross-cutting features,
and an open-questions list.
This document turns that into a **decided, phased plan** with a deliberately
**minimal v1** and a sub-ticket breakdown.
The load-bearing recommendations are:

1. **v1 is three memory-safe, backend-known intrinsics** — `crc32c`,
   `popcount`, `prefetch` — on Linux/ELF for x86-64 **and** aarch64, plus a
   friendly `std.hash.crc32c` wrapper.
   Everything that needs `unsafe` (SSE4.2 string ops, non-temporal stores),
   full SSE/AVX, inline asm (Tier 3), and runtime multiversioning/ifunc is
   **deferred** to later phases.
2. **`cfg` / `when target.*` conditional compilation is split out as its own
   sub-ticket and done first**, because the `arch.x86_64` module cannot compile
   per-target without it.
   It is the one real implementation dependency of v1.
3. **ELF/Linux is the reference path; Mach-O ifunc and anything Apple-Silicon is
   hardware-gated** and sequenced behind the parked macOS runner — it is
   designed here but cannot be built or verified now.
4. **`@multiversion` / `@target_feature` with a mandatory `.default` fallback**
   is the runtime-resolution model (resolve-once-at-load via ifunc), kept
   conceptually separate from interface/vtable dispatch (D-0016/#84), and
   scheduled **after** v1.

All new surface is **additive, Experimental, and post-1.0** — nothing here
touches the frozen 1.0 Stable set (D-0076), `SPEC.md`, `docs/GRAMMAR.ebnf`, or
`docs/STABILITY.md`.
This is a **design-only** ticket: it proposes **D-0079** and a sub-ticket slate
for the maintainer to ratify and file; it writes no compiler, runtime, or
stdlib code.

## Background

### What exists today

- **Tier 1 — portable SIMD (#24, PR#41).** Numeric arrays with vector types
  that autovectorize and lower per target.
  This is the recommended "make it faster" path for most code and needs **no**
  arch knowledge.
  #139 (Tiers 2–3) is for when Tier 1 is insufficient, not a replacement.
- **The module seam (#121, D-0056/D-0058).** Each compiler-known namespaced
  module registers one data-table entry across three twinned facets —
  `crates/sema/src/modules/<name>.rs` (type-check), `zapc_ir::lower::modules`
  (AST→ZIR lowering), and `zapc_codegen::modules` (runtime symbols).
  A new module is **parallel-addable**: add the file, add one `MODULES` row, no
  edits to a shared `match`.
  `arch` hangs directly off this seam — the sema facet types
  `arch.x86_64.crc32_u64(...)`, and the lowering facet emits the LLVM intrinsic
  instead of a runtime call.
- **ZIR → LLVM (design/IR.md).** Lowering is nearly 1:1 onto `inkwell` builder
  calls; `call_runtime @zap_*` reaches the runtime ABI.
  The three v1 intrinsics map to LLVM intrinsics the backend already knows, so
  they need a new ZIR producer, not a new instruction category.
- **Generators (#67).** `yield` already lowers to a **stackless state machine**
  — exactly the mechanism the MLP coroutine-interleaving stretch goal needs.
- **No conditional compilation today.** There is no `when`/`cfg`, no
  `--target <triple>` selection (only a `target/` output dir), and no `arch` or
  `cpu` concept anywhere in `SPEC.md`, the grammar, or the CLI.
  So cfg is genuinely new work, and it gates everything arch-specific.
- **Platform matrix (D-0076, `docs/STABILITY.md` §7.3).** Linux x86-64 and
  Windows x86-64 are tier-1/Stable; macOS x86-64 is tier-2 (implemented in
  PR#259 but **parked** pending a macOS CI runner); macOS arm64 and
  aarch64-Linux are **experimental** (#260 JIT-unwinder gap; #223 Immix GC race,
  parked).
  **macOS/Apple-Silicon hardware has no CI runner.**

### Why an escape hatch, and why now

Pro developers from C/C++/Rust expect to be *able* to drop to intrinsics when
the portable path is not enough.
The guarantee "you can always reach the hardware, safely and portably-gated" is
a confidence multiplier across DB/storage, HFT, codec, and HPC audiences.
The goal is **exposure as an option**, not adopting intrinsics in the language's
own implementations.
It is mostly plumbing plus a safety/signature table on an existing seam, not new
language fundamentals — which is exactly why a small, honest v1 is worth
shipping early and growing additively.

## Proposal

### 1. Scope — the v1 cut line

**v1 ships the intrinsics that are memory-safe, backend-known, and useful on
their own**, and nothing else:

| Intrinsic | Lowers to | x86-64 | aarch64 | `unsafe`? |
|---|---|---|---|---|
| `crc32c_u8/u16/u32/u64` | `@llvm.x86.sse42.crc32.*` / ARM `__crc32c*` | SSE4.2 | CRC ext | no |
| `popcount_u32/u64` | `@llvm.ctpop.i{32,64}` | always | always | no |
| `prefetch(ptr, hint)` | `@llvm.prefetch` | always | PRFM | no |

Plus **one friendly wrapper**, `std.hash.crc32c`, over the raw intrinsic with a
portable software fallback (so safe code gets a correct CRC32C everywhere, and
the fast path lights up where the feature is present).

**Deferred past v1** (designed here, scheduled in later phases):

- SSE4.2 string/compare ops (`cmpistri`/`cmpistrm`/`cmpestri`/`cmpestrm` + flag
  accessors, `cmpgt_epi64`) — `PCMPxSTRx` reads a full 16-byte lane and can
  **over-read** a page, so these require `unsafe` and careful docs.
- Non-temporal/streaming stores (`stream_*`) — ordering/fence subtleties.
- Full SSE/AVX intrinsic surface beyond Tier-1 portable SIMD.
- Runtime multiversioning (`@multiversion`/`@target_feature`/ifunc) and
  `cpu.has()` (§4) — the resolution machinery, not the intrinsics.
- Inline asm (Tier 3) — confirmed **deferred-with-direction** (§6).
- MLP coroutine-interleaving helper (§6).

**Why this line.**
The three v1 intrinsics are the only ones that are simultaneously (a) memory-safe
with no `unsafe` needed, (b) already known to the LLVM backend as a single
intrinsic, and (c) individually valuable — `crc32c` is the motivating example,
`popcount` and `prefetch` are universal and cheap to expose.
They need **no** CPUID/runtime-dispatch machinery to be correct via the
`std.hash.crc32c` wrapper's fallback, so v1 proves the seam and the honest
portability contract **without** taking on ifunc, unsafe over-reads, or the full
SIMD surface in one bite.
Each deferred item is additive on top: a new `MODULES` row, a new attribute, or a
new sub-module, never a redesign of v1.

### 2. Namespacing and import surface

Recommendation:

- **`arch.x86_64` and `arch.aarch64`** are the per-architecture modules holding
  the intrinsics under their **C names** (muscle memory transfers).
- **`arch.cpu`** holds feature detection: `arch.cpu.has(.sse4_2)` etc. (§4).
  It lives beside the arch modules, not inside `x86_64`, because feature names
  are per-arch but the `has()` entry point is uniform.
- **A portable `arch` facade is deferred**, not v1.
  When it lands it offers portable aliases (e.g. a `crc32c` that resolves to the
  right per-arch intrinsic) — but v1 ships only the explicit per-arch modules
  plus the `std.hash.crc32c` wrapper, which already *is* the portable entry
  point for the one case that matters.

```zap
import arch.x86_64 as x86        # compiles ONLY when targeting x86-64

h = x86.crc32c_u64(h, word)      # -> @llvm.x86.sse42.crc32.64.64
n = x86.popcount_u64(bits)       # -> @llvm.ctpop.i64
x86.prefetch(ptr, .T0)           # -> @llvm.prefetch, L1 locality hint
```

**Honest portability contract.**
`import arch.x86_64` **simply does not compile** off-target — no silent
fallback, no stub.
The sema facet rejects the import unless the active target arch matches, with a
diagnostic that points at `cfg` (§3) as the way to carry both paths.
This forces the developer to write both arms honestly rather than shipping code
that silently degrades.

### 3. `cfg` / `when target.*` conditional compilation — split out, done first

**Recommendation: split conditional compilation into its own sub-ticket and
land it before the `arch` module.**
It is needed independently of #139 (per-target builds, OS-specific code already
want it), and it is the hard dependency that lets `arch.x86_64` compile into
the x86 build only.

Proposed surface — a `when` block over compile-time **target predicates**:

```zap
when target.arch == .x86_64:
    import arch.x86_64 as a
    fn fast_crc(h: u64, w: u64) -> u64: return a.crc32c_u64(h, w)
elif target.arch == .aarch64:
    import arch.aarch64 as a
    fn fast_crc(h: u64, w: u64) -> u64: return a.crc32c_u64(h, w)
else:
    fn fast_crc(h: u64, w: u64) -> u64: return crc32c_sw(h, w)
```

- **Predicates:** `target.arch` (`.x86_64`/`.aarch64`/…), `target.os`
  (`.linux`/`.windows`/`.macos`/…), and `target.feature(.sse4_2)` for
  *compile-time-known* features (features the build is compiled *for*, distinct
  from runtime `cpu.has` in §4).
- **Semantics:** `when` is resolved at compile time against the active target;
  the non-taken arms are **not type-checked against the current target's
  intrinsics** (so an `arch.x86_64` import inside an x86 arm does not break an
  aarch64 build), but they **are** parsed, so the grammar stays uniform.
  This is the Rust `cfg!`/`#[cfg]` model adapted to Zap's indentation syntax.
- **`--target <triple>`** selects the active target for a build; the host
  triple is the default.
  (Cross-arch is compile-time only — there is no runtime dispatch across
  architectures in an AOT language; fat/universal binaries are a packaging step,
  not a Zap runtime mechanism.)

This is the **first real implementation dependency** and the natural Phase-1
companion to the three intrinsics.
Grammar/`SPEC.md` edits for `when` are **out of scope for this design ticket**
and land with the cfg implementation sub-ticket as Experimental surface.

### 4. CPU feature detection + runtime resolution — deferred to Phase 2, designed now

Two distinct problems, pinned:

- **Cross-architecture (x86-64 vs aarch64): COMPILE-TIME only.**
  Handled entirely by `cfg` (§3) + per-target builds.
  Not runtime.
- **Same-arch, different CPU features (SSE4.2/AVX2 present or not): genuine
  runtime resolution.**
  This is CPUID (x86) / hwcaps (ARM) + function multiversioning.

Proposed model (Phase 2+):

- **Primitive:** `arch.cpu.has(.sse4_2)` → CPUID/hwcaps, a plain runtime query.
- **Manual dispatch:** `let f = if cpu.has(.avx2): impl_avx2 else: impl_sw` —
  one indirect call, built on first-class fn values (D-0067, already shipped).
- **Attribute surface (the maintainer call in §8):**
  - `@target_feature("avx2")` on a fn — the building block; compiles that fn
    with the feature enabled.
  - `@multiversion(.avx2, .sse4_2, .default)` on a fn — the compiler generates
    one variant per listed feature plus an **ifunc resolver**; the dynamic
    linker resolves **once at load time** and patches the call site, so there
    is **zero per-call overhead** after startup (how glibc `memcpy` works).
- **Mandatory `.default` rule:** the compiler **rejects** a `@multiversion`
  that has no portable `.default` variant, so there is never a "no impl for this
  CPU" hole.
- **Safety rule:** `@target_feature` functions are **`unsafe` to call
  directly** — calling an SSE4.2 fn on a CPU without it is a SIGILL.
  They are safe only via a resolved `@multiversion` dispatch or after a matching
  `cpu.has()` check (the same rule as Rust).
- **Separation:** this is **resolve-once-at-load** dispatch and must stay
  conceptually distinct from interface/vtable per-call dispatch (D-0016 fat
  pointers / #84).
  They share no mechanism.

**ifunc sequencing (hardware gate):** ELF/Linux ifunc works today and is the
reference implementation.
**Mach-O ifunc is hardware-gated** behind the parked macOS runner and sequenced
last; where ifunc is unavailable, the fallback is a one-time resolved function
pointer set at startup (a plain indirect call), which is correct everywhere and
only gives up the call-site patch.

### 5. Lowering — how the three v1 intrinsics ride the seam

`arch.x86_64`/`arch.aarch64` are modules on the #121 seam, but unlike `math`
(which lowers to `call_runtime @libm_*`) the arch facet lowers to an **LLVM
intrinsic** directly:

- sema facet (`crates/sema/src/modules/x86_64.rs`, `aarch64.rs`): type the
  method, gated on the active target arch (§2/§3).
- lowering facet (`zapc_ir::lower::modules`): emit a new ZIR producer for the
  intrinsic call — `popcount`→`@llvm.ctpop`, `prefetch`→`@llvm.prefetch`,
  `crc32c`→`@llvm.x86.sse42.crc32.*` / ARM `__crc32c`.
  These are backend-known, so ZIR → LLVM stays nearly 1:1 (design/IR.md); the
  cleanest encoding is a dedicated `intrinsic.<name>` ZIR op rather than
  overloading `call_runtime`.
- codegen facet: map the ZIR intrinsic op to the `inkwell` intrinsic builder.

No RUNTIME-ABI changes for v1 — these are leaf LLVM intrinsics, not runtime
entry points.
(Multiversioning/ifunc in Phase 2 *does* touch the ABI/linker story; that is
part of why it is deferred.)

### 6. Confirmed-deferred stretch items

- **Inline asm (Tier 3):** confirmed **deferred-with-direction**.
  Most "I want inline asm" is really "I want a Tier-2 intrinsic"; it is the
  sharpest tool, hardest to keep safe/portable, and constrains the optimizer.
  Revisit only if Tier 2 proves insufficient; shape when it lands is
  Rust/Zig-style (template + typed operand constraints).
- **MLP coroutine-interleaving helper:** confirmed a **later helper on top of
  #67 generators**, not v1.
  It interleaves a batch of independent lookups to overlap cache misses
  (~2–3× on memory-latency-bound batched queries); offered as an
  "interleaved bulk-lookup" helper in collections once the generator frame is
  provably ~zero-cost (inlined, no per-call heap alloc).

### 7. CRC32C vs ISO CRC32 — a correctness note that must ship with v1

SSE4.2 `crc32` computes **CRC32C (Castagnoli, poly `0x1EDC6F41`)**, **not**
ISO/zlib CRC32 (`0x04C11DB7`).
They are different checksums and are not interchangeable.

- v1 names the intrinsic **`crc32c`** (not `crc32`) precisely to prevent the
  mix-up, and `std.hash.crc32c` is Castagnoli end to end.
- **gzip/PNG/zlib need the ISO variant**, which wants `PCLMULQDQ`
  carry-less-multiply folding — a **separate path**, not this intrinsic.
- This must be documented on both the intrinsic and the wrapper, and
  **coordinated with #106 (zlib/gzip)** so that module never reaches for the
  SSE4.2 `crc32` instruction expecting ISO semantics.

## Phased roadmap

- **Phase 0 — cfg (prerequisite).**
  `when target.*` conditional compilation + `--target` selection.
  Blocks everything arch-specific. **(S0)**
- **Phase 1 — the safe escape hatch (v1).**
  `arch.x86_64` + `arch.aarch64` with `crc32c`/`popcount`/`prefetch`, the
  seam lowering to LLVM intrinsics, `std.hash.crc32c` wrapper, and the CRC32C
  correctness docs.
  Linux/ELF for x86-64 and aarch64. **(S1)**
- **Phase 2 — runtime resolution.**
  `arch.cpu.has`, `@target_feature`, `@multiversion` + mandatory `.default`,
  the unsafe-call rule, ELF ifunc. **(S2)**
- **Phase 3 — the unsafe/SIMD surface.**
  SSE4.2 string ops (behind `unsafe`), non-temporal stores, broader
  SSE/AVX/NEON, the portable `arch` facade. **(S3)**
- **Phase 4 — hardware-gated.**
  Mach-O ifunc + Apple-Silicon verification (behind the macOS runner), then
  the MLP coroutine-interleaving helper. **(S4)**

## Proposed sub-ticket breakdown

Recommendation to the maintainer — **do not file yet; these are proposed, not
opened.**
Cost: S/M/L/XL. ⚠ = hardware-gated (cannot build/verify without the parked
macOS/Apple-Silicon runner).

| # | Sub-ticket | Phase | Cost | Notes |
|---|---|---|---|---|
| A | `cfg` / `when target.*` + `--target` selection | 0 | **L** | Grammar + sema + build driver; the real dependency. |
| B | `arch.x86_64`/`arch.aarch64` seam + `crc32c`/`popcount`/`prefetch` lowering | 1 | **M** | New ZIR `intrinsic.*` op; no ABI change. |
| C | `std.hash.crc32c` wrapper + CRC32C-vs-ISO docs; coordinate #106 | 1 | **S** | Portable fallback; correctness note. |
| D | `arch.cpu.has` (CPUID/hwcaps) | 2 | **M** | Runtime feature query. |
| E | `@target_feature` + unsafe-call rule | 2 | **M** | Building block for F. |
| F | `@multiversion` + mandatory `.default` + ELF ifunc | 2 | **L** | Resolve-once-at-load; linker story. |
| G | SSE4.2 string ops (`unsafe`), non-temporal stores, broader SIMD | 3 | **L** | Over-read safety; `unsafe`-gated. |
| H | Portable `arch` facade | 3 | **S** | Aliases over per-arch intrinsics. |
| I | Mach-O ifunc + Apple-Silicon verification | 4 | **L ⚠** | Blocked on macOS runner. |
| J | MLP coroutine-interleaving bulk-lookup helper | 4 | **M** | On top of #67; needs zero-cost frame. |
| K | Inline asm (Tier 3) | later | **XL** | Deferred-with-direction; revisit only if needed. |

## Rationale

- **Minimal v1 over a big-bang landing.**
  The three safe intrinsics prove the seam, the honest portability contract, and
  the CRC32C story with no `unsafe`, no CPUID, and no ifunc.
  Everything else is strictly additive on top, so a small v1 de-risks the whole
  epic.
- **cfg first.**
  It is the one hard prerequisite and is independently useful; building `arch`
  without it would mean a throwaway gating hack.
- **ELF-first, Mach-O-behind-hardware.**
  Designing Mach-O ifunc now but sequencing it last keeps the plan honest about
  what can actually be verified in CI today (D-0076 parks the macOS runner).
- **Keep multiversioning separate from vtable dispatch.**
  Conflating resolve-once-at-load with per-call interface dispatch (D-0016/#84)
  would muddy both; they share no mechanism and are scheduled apart.
- **`crc32c` not `crc32`.**
  Naming the thing for what it computes is the cheapest possible guard against
  the single most likely correctness bug in this area.

**Alternatives considered.**
*Ship the full Tier-2 set in v1* — rejected: pulls in `unsafe` over-reads,
CPUID, and ifunc at once, with no de-risking.
*Portable `arch` facade as the primary surface in v1* — rejected: the honest
per-arch contract is the point; the one facade case that matters
(`crc32c`) is already covered by `std.hash.crc32c`.
*Gate arch imports with an ad-hoc flag instead of cfg* — rejected: cfg is needed
anyway and the hack would be thrown away.

## Compatibility

Everything proposed is **new, additive, Experimental surface** and **post-1.0**.

- No change to the 1.0 Stable set (D-0076), `SPEC.md`, `docs/GRAMMAR.ebnf`, or
  `docs/STABILITY.md` in this ticket.
- `arch.*`, `cfg`/`when`, and the attributes all ship **Experimental** (shipped,
  not promised) and graduate later at a MINOR once proven (policy §4.2).
- `import arch.x86_64` not compiling off-target is by design, not a
  regression — it is the honest-portability contract (§2).
- Adding aarch64 as a buildable arch for these intrinsics is a backward-
  compatible MINOR (`docs/STABILITY.md` §7.3); the existing experimental tier of
  aarch64-Linux (#223) is unchanged by a design-only doc.

## Implementation

Phase boundaries touched:

- **Phase 0/1:** grammar (`when`), sema module facets, a new `zapc_ir`
  `intrinsic.*` producer, codegen intrinsic mapping, and the build driver
  (`--target`).
  **No RUNTIME-ABI change.**
- **Phase 2:** the ABI/linker story for ifunc (ELF), plus attribute handling in
  sema and codegen.
  This is where RUNTIME-ABI is revisited.
- **Phase 3+:** `unsafe` surface and the portable facade are additive module
  rows; Mach-O ifunc (Phase 4) is hardware-gated.

Parallelism: C can proceed alongside B once B's intrinsic op exists; D/E/F are a
Phase-2 cluster; G/H are independent of the Phase-2 cluster.
A (cfg) blocks B and must land first.

## Load-bearing calls for the maintainer

These are the decisions this design most wants ruled on (see §8 of #139's
spirit):

1. **v1 scope cut line (§1):** confirm v1 = `crc32c` + `popcount` + `prefetch`
   (safe, Linux/ELF, x86-64 + aarch64) + `std.hash.crc32c`, everything else
   deferred.
2. **`@multiversion`/`@target_feature` attribute surface + mandatory-`.default`
   rule + unsafe-call rule (§4).**
3. **Split `cfg` conditional compilation to its own sub-ticket and do it first
   (§3, sub-ticket A).**
4. **ELF-first, Mach-O-behind-hardware sequencing (§4, sub-ticket I).**

## Proposed decision

> **D-0079 (Proposed — pending maintainer ratification).**
> Adopt a phased, opt-in low-level performance escape hatch (#139).
> v1 = three memory-safe, backend-known arch intrinsics (`crc32c`, `popcount`,
> `prefetch`) on Linux/ELF for x86-64 and aarch64, plus a `std.hash.crc32c`
> wrapper, riding the #121 module seam and lowering to LLVM intrinsics.
> `cfg`/`when target.*` conditional compilation is split to its own sub-ticket
> and lands first.
> Runtime resolution (`@multiversion`/`@target_feature`/`cpu.has`/ifunc, with a
> mandatory `.default` and `unsafe`-to-call-directly rule) is Phase 2, ELF-first;
> `unsafe` SSE4.2 string ops, non-temporal stores, broader SIMD, and the
> portable `arch` facade are Phase 3; Mach-O ifunc / Apple-Silicon verification
> and the MLP coroutine-interleaving helper are Phase 4 (hardware-gated); inline
> asm (Tier 3) stays deferred-with-direction.
> All surface is additive, Experimental, and post-1.0; nothing here touches the
> 1.0 Stable set (D-0076).
> This is design-only; no code lands in this ticket.

Not accepted — awaiting the maintainer's sign-off on the four calls above.

## Open issues

- Exact feature-name enum for `cfg` `target.feature(...)` vs runtime
  `cpu.has(...)` — keep them a single shared enum or two?
- Whether `--target` accepts full LLVM triples or a small curated set in v1.
- ifunc fallback ergonomics when the linker/loader lacks ifunc (static musl,
  some BSDs) — the startup-resolved-pointer fallback (§4) needs a concrete
  shape in Phase 2.
- Whether `std.hash.crc32c` should auto-multiversion (Phase 2) or stay a simple
  feature-gated-with-fallback wrapper in v1 (recommended: the latter).
