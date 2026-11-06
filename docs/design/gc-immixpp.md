# Zap GC design — "Immix++" (concurrent + generational Immix with FFI pinning)

## 1. Scope & assumptions
Designed for: 64-bit only; compiler-controlled (precise, not conservative); multicore; coroutine-native (M:N scheduler); mixed workloads caring about both tail latency and throughput; FFI required; heaps MB→tens of GB. These assumptions drive every choice; if any flips (32-bit, embedded, no compiler control), revisit.

## 2. Object model & pointer representation
- Precise: compiler emits stack maps + per-type field maps → exact roots, freedom to move.
- Contiguity is an addressing invariant, not a cache one: field/element access is `base + offset`; an object must be one contiguous byte range or that arithmetic is wrong (not just slow). The GC therefore never splits an object across non-adjacent regions.
- Minimal header (type/size); push mark bits / dirty / forwarding into side tables for mutator cache cleanliness.
- Pointer representation today: canonical raw pointers (keeps FFI trivial). Colored pointers are a Phase-2 option only (§10), not the baseline.
- Decide weak refs / finalizers / ephemerons early — they couple to the mark phase (weak refs cleared post-mark, finalizers can resurrect, ephemerons need a fixpoint). Late design = bugs + surprise pauses.

## 3. Heap structure: regions / blocks / lines
- Heap is a contiguous VA range carved into fixed-size regions (Immix: 32KB blocks; "region" = block or group of blocks), each sub-structured into lines (~128B).
- Regions are the unit of policy/tagging and independent reclamation; lines are the unit of in-place reclamation.
- Regions are contiguous in VA, so a run of adjacent regions is itself contiguous (needed for large objects, §5).

## 4. Region tagging & policy
A region carries a policy tag deciding: (a) is it in the minor collection set? (b) can its objects move? (c) how is it scanned? Tags: young, old, pinned/non-moving (FFI arena), large/humongous, immortal (pretenured).
- Shared fungible free pool (anti-stranding): regions are NOT permanently partitioned into young/old pools. One global free pool; a region is tagged on demand when allocation/promotion claims it, returns to the untagged pool when emptied. Generation sizes float dynamically → never OOM in the "young pool" while the "old pool" has free regions. (G1's rationale; the answer to "OOM while memory is available.")
- Homogeneous vs mixed regions — the sticky consequence:
  - Copying promotion (G1) evacuates survivors into old regions → homogeneous, densely packed, but paid a copy.
  - Sticky/non-copying promotion (our choice) promotes in place by flipping an age/sticky bit → regions become mixed (young + in-place-promoted old interleaved). Generation tracked per object (age/sticky bit), region tag = "dominant age." Mixed regions are normal.
  - You cannot have both: clean per-region generation ⟺ copying. Sticky buys no-copy at the cost of mixed regions + per-object age.
- Tag flipping / lifecycle:
  - young→old (aging): after N surviving minor cycles, flip tag/ages in place (sticky; no move).
  - old→young: NOT live-object demotion (would make a survivor scanned every minor cycle + tangle the remembered-set invariant). Legitimate only at the region-lifecycle level: an emptied/evacuated region returns to the free pool and is reused as young. That region-reuse path is the valid "old→young" and the anti-stranding mechanism.
  - pinned regions can't be re-tagged to a moving policy while pins are live.
  - Tags flip at collection/phase boundaries, not mid-mutator.

## 5. Large objects
- "Large" = a single allocation above a geometry threshold (Immix: > block → Large Object Space; G1: > ½ region → humongous), decided at allocation from requested byte size — NOT the transitive size of a structure. A dict of a million small entries = a million small cells + possibly one large backing array (which alone may be large); entries stay small.
- Threshold reasons: copy cost, placement (doesn't fit bump-pointer fast path), scan cost (large pointer arrays want card/chunked scanning).
- Objects larger than a region → humongous: a contiguous run of adjacent regions for the one object (regions are contiguous VA → stays one flat range, value-struct promise preserved). Humongous = non-moving, freed as a unit. GC never splits a flat object.
- If data exceeds that cleanly: represent it at the type level as an explicitly chunked structure (header + spine of chunk pointers, each chunk a normal object) — a visible representation choice, costs nothing in cache at multi-MB scale, buys GC-friendliness + growth. Relevant to #499 N-D arrays (header + chunked/strided backing, not one flat cell).

## 6. Reclamation: mark-region + opportunistic evacuation
- Tracing is O(live) for both copying and mark-region. Cost asymmetry is in reclamation: classic mark-sweep sweeps whole heap O(heap); copying resets from-space O(1) but copies survivors. Mark-region reclaims by scanning compact per-line mark-bytes and handing back free lines/blocks — copying-like cheap reclamation without copying.
- Per-block copy-vs-mark decision (core adaptive rule): evacuate only sparse, low-survival blocks (copying's O(survivors) cheap, defrag pays); mark-in-place dense blocks (copying wasted). Mortality/scan-count argument applied per block — beats always-copy (G1-young) and never-move (CMS/Go).
- Line-granularity cost: a small live object keeps its whole 128B line (line-level fragmentation) — price of not always copying, bought back by opportunistic evac.

## 7. Generational: Sticky Immix (non-copying)
- Generational needs only: distinguish young/old + collect young without tracing old (→ remembered set + write barrier for old→young). Copying NOT required.
- Sticky mark bits: marks from last collection stick; a minor cycle traces only objects allocated since last GC + remembered-set roots; promotion = the mark/age bit staying set (no move).
- Minor cost scales with the young set (trace O(young-live) + reclaim O(young-lines) + opportunistic evac of sparse young blocks) — copying-nursery cost profile without copying; preserves the reclaim-in-place edge over G1.
- Tiers: frequent minor (young) → occasional major (adds evac of fragmented old regions to consolidate in-place-promoted survivors) → rare full.
- Prior art: sticky mark bits = Demers et al. POPL 1990; Sticky Immix = generational Immix (MMTk/Jikes RVM line; Immix base = Blackburn & McKinley PLDI 2008).

## 8. Barriers (shared linchpin)
- Liveness (concurrent mark): SATB (snapshot-at-the-beginning, pre-write, records old referent). Chosen over Dijkstra/incremental for: simpler termination (no stack re-scan); consistency with Shenandoah (Phase-2 analog) so Phase 1→2 keeps one barrier semantics; floating garbage mopped up by frequent sticky minors.
- Remembered set (generational): dirty-line/card marking (post-write; set dirty bit for the line/card containing the store; scan dirty lines for old→young roots at minor). Cheap, reuses Immix's per-line structure.
- Honest: "one barrier" = two cheap barriers (pre-write SATB + post-write dirty-line), same as G1 (SATB + card table). Both O(1)/store; generational one piggybacks on existing structure.

## 9. Concurrency phases (latency roadmap)
- Phase 0 (today): cooperative STW mark + opportunistic STW evac (zap_safepoint handshake). ≈ parallel-STW Immix.
- Phase 1: concurrent/incremental marking (SATB write barrier; safepoints + precise stack maps exist). Marking is the long pole → most of the latency win; evac stays STW. ≈ G1 on Immix's substrate (cheaper reclaim-in-place). Realistic near-term target.
- Phase 2 (deferred, #713): concurrent evacuation via read/load barrier. ≈ Shenandoah/ZGC. Hard; §10. Pursue only if Phase-1 evac pauses still bite.
- Effort gradient real: Phase 1 achievable; Phase 2 person-decade-class.

## 10. FFI: pinning + arena (across all phases)
- FFI fundamentally demands only: objects C holds must not move while held. Solve with pinning, by generalizing the existing #120 internal pin pass (already pins runtime-C-held buffers / finalizer queue / scheduler-owned G referents) into a scoped user/FFI pin API, plus a non-moving FFI arena (coordinate #539 FFI-11 / #553).
- Invariant across all phases: a pinned object is never evacuated. The Phase-2 load barrier doesn't help C (C doesn't execute it) → pinned objects simply never move, C's raw pointer stays valid in every phase.
- Phase-2 adds: pin must be atomic vs an in-progress concurrent move (no safepoint window): header bit the evacuator checks; a pin racing a move follows forwarding and re-pins. Precedent: ZGC/Shenandoah implement JNI critical sections this way (pin page/region).
- Phase-2 pointer fork (the #713 content): Option A Brooks forwarding word (canonical pointers, FFI-safe, +1 word + indirection) vs Option B colored pointers (~48-bit VA + spare bits + multi-mapping/masking; lowest pause but fights FFI — C can't see tag bits, needs masking at every boundary). Prefer A first.
- FFI and the concurrency roadmap are independent tracks, meeting only at the pin invariant. FFI ships on STW (Phase 0/1) with pinning; not gated on Phase 2, and Phase 2 not gated on FFI.
- Open FFI-epic questions (#539 FFI-4/FFI-6): transitive/deep pin vs shallow-pin-plus-contract vs arena-only; sync (scoped) vs async (global-ref/GCHandle) pin lifetimes.

## 11. OOM robustness
- Copying's trap: needs free to-space to evacuate into; under pressure can OOM during collection ("need memory to free memory").
- Sticky/mixed regions sidestep it: in-place promotion needs no to-space → mark + line reclamation always progresses near OOM.
- Immix's opportunistic (optional) evacuation is a first-class safety valve: evacuation never mandatory → under pressure degrade to pure mark-region (no copy, no to-space) rather than fail. Ladder: full evac → partial evac → mark-only. Reclamation always-possible; defrag best-effort (keep a small emergency region reserve).

## 12. Pacer & telemetry (co-equal subsystem)
- Feedback-driven: measure alloc/mark/survival rates; start the concurrent cycle early enough to finish before heap exhausts (late start = CMS's concurrent-mode failure). Dual control: heap-growth target (GOGC-style) + pause/CPU budget (G1-style) + soft memory-limit backstop (GOMEMLIMIT-style). Adaptive generation sizing + promotion thresholds from measured survival.
- Native pause/throughput/heap telemetry from day one (ties to #568); can't tune/trust what you can't measure, and "measure the workload before assuming the generational hypothesis" requires it first.

## 13. Boehm's role
CI correctness-oracle (independent differential collector; characterized #223 as collector-agnostic) + bring-up/FFI fallback. Explicit `--gc=boehm` only, never implicit. Sunset once pinning/arena prove out and precise rooting is solid on all arches. (Live maintenance cost, e.g. #710 livelock, is what the sunset retires.)

## 14. Build order (interface-first)
1. STW mark-sweep/semispace — nails interfaces (object model, stack maps, safepoints, roots, alloc API, barrier insertion points). 2. Regions + mark-region + opportunistic evac (compaction, STW). 3. Sticky generational + remembered-set barrier. 4. Large Object Space. 5. Concurrent marking (Phase 1; SATB live). 6. FFI pin API + arena. 7. (deferred) Phase 2 concurrent evac. Pacer + telemetry mature alongside. Separate mechanism from policy; nail the mutator/collector interface before the algorithm.

## 15. Prior art & paper-worthiness
- Refs: Immix — Blackburn & McKinley PLDI 2008; sticky mark bits — Demers et al. POPL 1990; LXR (concurrent Immix) — Zuo, Blackburn et al. PLDI 2022; MMTk as eval vehicle/baseline. (Verify exact titles/pages before citing.)
- Synthesis itself not novel (LXR largely occupies concurrent generational Immix). Publishable path = experience/systems paper or workload-driven paper (distinctive handle: GC for agent-authored, coroutine-native, FFI-heavy programs, Zap pillar-1 #178), requiring implementation + measurement vs ZGC/G1/LXR (via MMTk), not design alone.

## 16. Issue / docs mapping
- Epic #714 (north star + sub-issue checklist): each sub-issue pulls detail from the matching section — Sticky generational ← §4+§7, Large Object Space ← §5, barrier foundation ← §8, Phase 1 ← §9, FFI pin+arena ← §10, pacer+telemetry ← §12, Boehm reframe ← §13.
- Deferred Phase 2 #713: correctly narrow (§10 pointer fork + atomic-pin). Add a one-line context pointer, do NOT inline the depth.
- DECISIONS: new D-number, content from the decision proposal + §6 design principle.
