---
title: Stack-promote non-escaping transient heap values — pointer-bearing structs and length-bounded strings
status: Locked
issue: 1081
pr:
date: 2026-11-19
domain: runtime
supersedes:
superseded-by:
---

# Stack-promote non-escaping transient heap values — pointer-bearing structs and length-bounded strings

**Status:** Locked

## Decision

The escape-analysis pass (`crates/ir/src/escape.rs`, built on D-0151 / #850) is
extended to remove the per-creation heap allocation and GC-root churn for two
more classes of provably non-escaping value, with no surface-language change:

1. **Pointer-bearing reference structs.** A non-escaping genuine reference struct
   (`!is_value && !inline`, D-0023) within the size cap is placed in a stack slot
   (`Inst::StackStruct`) even when it has GC-pointer fields — the #850 v1 required
   leaf / pointer-free. Codegen gives such a struct an **aggregate-root
   `(base, desc)` pair** (the #586 by-value-spill mechanism) instead of a direct
   root: its own pointer addresses the stack (which the collector ignores), while
   its interior reference fields are traced — and rewritten in place on an
   evacuating move — through the struct's existing precise `ZAP_TD_PRECISE`
   descriptor. A leaf promoted struct still takes no root at all (#839). No new
   ZIR instruction and no runtime-C change: `zap_mark_shadow_chain` already traces
   aggregate pairs generically over any stack base.

2. **Length-bounded transient strings.** An f-string `StrBuild` whose joined
   length has a compile-time upper bound that provably fits the inline stack
   buffer (`STR_STACK_CAP = 64` payload bytes) is marked `bounded`. A promoted
   (`stack`) `bounded` build can never take the runtime spill path in
   `zap_strbuild_into`, so — like a leaf `StackStruct` — it reserves **no GC
   root**, removing the per-iteration `gc_store_root`. A promoted build that is
   NOT `bounded` (an interpolated string, a float, an enum, any spec'd/padded
   field) may still spill to a heap `ZapStr` and so keeps its ordinary direct
   root. `lower_fstring` computes `bounded` by summing a conservative per-piece
   max width (literal: exact byte length; integer field: ≤ 24; bool: ≤ 8; char:
   ≤ 4; anything else or any format spec: unbounded).

### Shared non-escape condition

Both reuse the single #850/#839 monotone escape fixpoint — there is no second
escape pass. A value is promotable iff it is NOT: stored into a heap object
(`FieldSet` value, a retaining dict/set/list insert, a non-clean/global store),
returned, captured by a closure/first-class-fn (#163), boxed behind an interface
(`IfaceMake`), or passed to a retaining callee (every user function and dynamic
dispatch; only a tight allowlist of pure readers borrows). ZIR has no
field-address / interior-pointer instruction (`FieldGet`/`FieldSet` go through the
struct base), so an interior address cannot leak — escape condition (4) is
structural. Any ambiguity fails safe to the heap.

### Struct byte threshold

Unchanged from #850: `MAX_STACK_ALLOC = 256` bytes per promoted struct (a pointer
field counts as its 8-byte width), under a `MAX_FRAME_BUDGET = 4096`-byte
per-function cap summed over distinct static sites. Deliberately well below Go's
64 KB implicit-stack-allocation cap because Zap coroutines run on fixed OS-thread
stacks, not growable goroutine stacks (discussion #849 §7). Over either bound the
value stays on the heap.

## Rationale

- **Reuse the #586 aggregate-root path, don't invent a new one.** A
  pointer-bearing stack struct and a by-value `value struct`/tuple spill have the
  identical problem — a stack object whose interior GC pointers must be traced and
  move-rewritten in place — and #586 already solved it with `(base, desc)` pairs
  and the same precise descriptor heap instances use. Routing the new case through
  it means zero runtime-C change and inherits the already-proven move-safety. A
  direct root was rejected: the struct's base is a stack address the collector
  ignores, so a direct root would never trace the interior fields.
- **No new ZIR instruction.** Extending `StackStruct` to cover pointer-bearing
  structs (codegen decides leaf-vs-aggregate from the struct layout,
  `struct_has_ptr`) avoids the new-`Inst`-variant hazards (missing `inst_dest`
  rooting, exhaustive-match arms). The whole point is to *remove* heap pointers and
  roots, so adding one would be self-defeating.
- **A runtime spill guard, not a static-only string buffer.** The issue proposed
  "only promote when statically bounded, else heap". We keep #850's more general
  promote-with-runtime-spill (`zap_strbuild_into`), which already elides the heap
  allocation for the common short case, and layer the static bound ON TOP purely
  to decide whether the root can also be dropped. This keeps the heap-alloc win
  for unbounded-but-non-escaping joins while still giving the fully root-free path
  for the motivating `f"k{i}"` transient dict key.
- **Conservative width, over-estimating only.** Soundness here is a potential
  use-after-free (an unrooted heap spill), caught only by the CI GC-stress leg, so
  `bounded` over-estimates every width and drops to "unbounded" on anything
  uncertain. Over-estimating only forgoes the optimization; it can never mark a
  spillable join bounded.

## Consequences

- The motivating `dict_string` lookup loop (`s = s + d[f"k{i}"]`, perf PR #836)
  now builds each transient key in a stack buffer with **zero** heap allocation
  and **zero** GC root — the alloc/root churn (not hashing) that made wyhash #1065
  show ~0% end-to-end. A non-escaping struct built in a loop likewise allocates ~0.
- Move-safety now depends on the aggregate-root registration being correct for
  pointer-bearing stack structs. This is exercised by the
  `stack_struct_ptr_evac` golden under `gc-immix` (a promoted struct's `str` field
  is relocated across evacuating collections and read back) and guarded at the ZIR
  level by the `escape_*_pointer_bearing_*` / `escape_*bounded*` tests.
- Still out of scope (unchanged): `value struct` / tuple by-value aggregates
  (already stack spills traced via #586), interface-boxed structs (escape via
  `IfaceMake`), and any value over the size / frame-budget caps.

<!-- Builds on [runtime D-0151 / #850 escape analysis] (frozen DECISIONS.md) and
the #586 by-value aggregate-root mechanism; see also #839 StackStruct, #838
one-object ZapStr, #837 StrBuild. -->
