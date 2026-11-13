# Zap arena / region allocation — scope-bound, non-moving, escape-checked

Normative design for **#553** (arena / region allocation), an accelerant for the
C FFI epic **#539**. This document is the authoritative spec; the design
discussion and rationale live in GitHub Discussion #871. The decision record is
**D-0158** in `DECISIONS.md`.

Builds on, and does not restate: `docs/design/c-ffi.md` (§2 pin/root rules, §5
lifetimes, §9 allocator strategy, Appendix A), `docs/design/gc-immixpp.md`
(§3–§4 regions and the pinned/non-moving region tag, §10 FFI pin+arena), and
**D-0151** (#839/#850, escape analysis).

## 1. What an arena is

An arena is a **bump allocator with deterministic batch free**, bound to a
lexical block: allocations are handed out by advancing a pointer, and the whole
batch is freed as a unit at block exit — never by the collector. Two payoffs:

- **Stable addresses (the FFI motivation).** The arena is *non-moving*, so a
  buffer allocated in it keeps its address for the whole block. A pointer handed
  to C stays valid across an entire C-heavy section without per-call `pin`.
- **GC relief (a bonus).** A batch of short-lived allocations costs one bump each
  and one bulk free, bypassing tracing/reclamation entirely.

The arena draws its backing memory from **the GC's own region pool**
(`gc-immixpp.md` §4), not from system `malloc` — so arena objects remain inside
the traced heap (the collector can still trace *through* them), merely tagged
non-moving. This is what distinguishes the #553 arena from the conservative C
arena of `c-ffi.md` Appendix A / #551, which holds descriptor-less C memory.

## 2. Surface syntax & semantics

### 2.1 Opening an arena

An arena is opened with a `with` statement (reusing the existing context-manager
grammar, #185 — no new grammar); allocation is explicit and handle-targeted.

```zap
with arena() as a:
    buf = a.alloc(bytes, 4096)      # bump-allocated in `a`, non-moving
    n = c_read(fd, buf)             # extern borrows the pointer for the call
    process(buf[:n])
# <- `a` and everything allocated in it is freed here, deterministically
```

```zap
# many short-lived leaf objects in a hot loop, no GC churn
with arena(size=64*1024) as a:
    for line in lines:
        rec = a.new(Record, parse(line))   # lives only this block
        emit(rec)
# one bulk free; nothing survived into the GC heap
```

```zap
# nested arenas: inner is strictly shorter-lived than outer
with arena() as outer:
    hdr = outer.alloc(bytes, 64)
    with arena() as inner:
        tmp = inner.alloc(bytes, 4096)
        # inner -> outer pointer OK (inner dies first); outer -> inner is an error
    # inner freed here
# outer freed here
```

- **Allocation is explicit** (`a.alloc(T, n)` / `a.new(T, …)`), never ambient:
  code lexically inside the block still allocates on the normal GC heap unless it
  is handed `a`. This keeps the escape check tractable and the surface
  unsurprising.
- There is **no manual `alloc`/`free`** API and **no `a.extend`**; growth is
  automatic (§2.3).

### 2.2 `arena()` is well-formed only as a `with` head

A bare, detached `arena()` is a **compile error**. Writing `a = arena()`,
returning an `arena()`, or otherwise constructing one outside a `with` head is
rejected:

```
ZAP#### : an arena must be opened with a `with` block
  a = arena()
      ^^^^^^^ arena() is only valid as `with arena(...) as a:`
  note: arenas are freed deterministically at block exit; a detached
        arena has no bounding scope.
```

Rationale: a detached arena has no lexical block to bound its lifetime, so its
regions would either leak or be reclaimed nondeterministically at GC
finalization — defeating the deterministic-bulk-free guarantee and
reintroducing exactly the leak class arenas exist to remove. For a GC'd
language a leak-by-forgetting is unacceptable, so the guarantee is enforced
*statically at construction*, not left to the escape checker or to documentation.

Mechanism: an arena is a **`@scoped` resource** — a type whose values are
*affine*: each must be **consumed exactly once**, and the only consumers are (a)
a `with` head, or (b) a move into another `@scoped` type's field (ownership
transfer to an owner that is itself `@scoped`). Scopedness is **contagious
through ownership** (a struct owning a `@scoped` field is itself `@scoped`), and
an owner's teardown auto-runs each owned `@scoped` field's `__exit__` in reverse
construction order (Rust `Drop`-glue style), so cleanup cannot be forgotten. A
bare `arena()` is simply the degenerate violation of (a)/(b): it is consumed by
neither.

`@scoped` is a **general language feature** in its own right — frozen as
**D-0159** (#876), spec in `docs/design/scoped.md` — with arena as its first
consumer; it also underlies composite scoped types such as `arena_dict` (#873).
Arena **v1 is not blocked on the full feature**: the base arena owns no `@scoped`
fields, so it needs only the degenerate *`with`-head-only construction check*,
implementable standalone. Now that `@scoped` has landed (D-0159), that degenerate
check is **subsumed** by the general mechanism — the arena is simply a `@scoped`
struct, its with-binding guarantee is the `@scoped` with-head-only rule
(`docs/design/scoped.md` §3.2), and the general affine/contagion/auto-teardown
machinery enables composite `@scoped` types built on the arena. (The same
discipline may later extend to `pin()`.)

### 2.3 Sizing and growth

```zap
with arena(size=64*1024, cap=1<<20) as a: ...
```

- **`size`** (optional) — an initial-reservation hint, rounded **up to the GC
  region granularity** (the 32 KB Immix block, not a power of two, since the
  backing unit is a whole region). Default: one region. Pure performance hint;
  the arena still auto-grows past it.
- **`cap`** (optional) — a hard ceiling in bytes. The first region-claim that
  would push total reservation past `cap` fails and raises `MemoryError`.
  Default: unbounded (grow until heap OOM). `cap` is the bounding knob for
  FFI/embedded use and is the hook the #551 C-malloc model uses (over-cap ⇒
  malloc returns NULL ⇒ the C library's own error path fires).
- **Growth** is automatic and transparent: when the current region fills,
  `a.alloc` claims another region from the pool and chains it (the arena is a
  *set* of regions; individual objects never straddle a region). A single
  allocation larger than one region becomes a **humongous** contiguous run
  (`gc-immixpp.md` §5), claimed as a unit; if it cannot be satisfied, OOM.

## 3. Escape / lifetime rule — the compile-time dual of D-0151

**Rule (static).** A value whose provenance is "allocated in arena `a`" — the
handle `a`, anything returned by `a.alloc`/`a.new`, and anything derived by
field/element/slice access from such a value — is *region-bound to `a`* and **may
not outlive `a`'s block**. It may not be: returned from the enclosing function;
stored into a longer-lived slot (a GC-heap object field, a collection, a module
global, an outer arena); captured by a closure that outlives the block; boxed
behind an escaping interface value; or passed across a `spawn` / `scope:`
boundary.

This reuses **D-0151's escape lattice** (`crates/ir/src/escape.rs`) in structure,
changing two things:

1. **Granularity:** the boundary is the *arena block*, not the *function*.
2. **Consequence is inverted.** In D-0151 an escaping value has a safe fallback —
   it stays on the GC heap. An escaping *arena* value has none (its backing
   memory is gone at block exit), so it is a **hard compile error**, never a
   silent heap promotion. Ambiguity fails safe to *reject* (the dual of D-0151's
   fail-safe-to-heap), exactly as D-0151's monotone-to-escape lattice already
   does.

**Compile error vs runtime.** Escape is always a compile error; there is no
runtime escape check. The only runtime behaviours are the deterministic bulk free
at block exit and arena exhaustion (`cap`/heap OOM → `MemoryError`). A debug build
may poison freed regions to catch a checker bug, but the static rule is the
safety mechanism.

**Reach across calls (v1).** Intra-function only. An arena value may be passed to
a call only if the callee is on the same proven-non-retaining allowlist D-0151
maintains (`zap_str_len`, `zap_dict_get`, etc.); any other Zap user call with an
arena argument is an error. The one carve-out is the FFI boundary (§4).
Threading arena values through user helpers is deferred to a later `@noescape`
parameter annotation (§7).

## 4. FFI tie-in

- **Passing to a C `extern` is a permitted borrow.** The extern borrows the
  pointer for the call's duration; the arena guarantees non-move, so the pointer
  is valid throughout. This is the primary use case and needs no interprocedural
  reasoning.
- **C retaining a pointer past the call/block is NOT statically caught.** The
  arena guarantees *non-move*, not *outlives-the-block-from-C's-view*. A pointer C
  keeps beyond the block is the same programmer contract as `c-ffi.md` §2.2
  (pin/root/handle) — use a rooted handle for retained pointers, not an arena
  value. This is the one documented sharp edge, and it is the existing FFI
  contract, not a new unsafety.
- **Relation to #544 (FFI-4, pin across a section):** #553 **underlies** #544 —
  "pin a buffer across a C section" becomes "allocate it in an arena." The
  per-call `with pin(buf)` of `c-ffi.md` §5 remains for one-shot borrows of
  pre-existing heap objects; the arena is the allocate-fresh, keep-pinned-across-
  many-calls case. Dividing line: *fresh allocation → arena; pre-existing object
  → pin.*
- **Relation to #551 (FFI-11, segregated C arena / Appendix A):** adjacent, not
  subsumed. #553 holds **Zap objects with precise descriptors** (non-moving but
  precisely traced); #551 holds **C-allocated, conservatively-scanned**
  descriptor-less memory. #553 provides the scope-bound region *lifecycle*
  (claim pinned regions, bump, bulk-return); #551 = that lifecycle + a
  conservative scanner + C-`malloc` routing. Build #553 first; #551 becomes a
  smaller follow-on with the no-individual-`free` constraint of a bump arena.

## 5. GC integration (soundness)

The arena maps onto the `gc-immixpp.md` §4 **pinned/non-moving "(FFI arena)"
region tag**, served from the shared fungible free pool:

- **Allocate:** opening the arena claims a region, tags it pinned/non-moving, and
  bump-allocates; growth chains more regions.
- **No evacuation:** the pinned tag places arena objects in the evacuator's
  existing "mark-in-place-first" set (`c-ffi.md` §2.2 Rule 1 / gc-immixpp §10).
  Arena objects never move in any GC phase, so a raw address handed to C is
  stable.
- **Bulk free:** at block exit the arena's regions return to the pool as a unit —
  O(regions), no per-object sweep, no liveness trace. Arena objects are
  live-by-construction for the block; they are freed by scope, not by the
  collector. A collection firing mid-block must **not** reclaim them even if they
  appear unreachable from Zap, because C may hold pointers the collector cannot
  see.

Two pointer directions:

- **heap → arena** (a GC object points at an arena object): a surviving such
  pointer is precisely an *escape* and is forbidden by §3. That static rule is
  what makes the bulk free sound. Transient heap→arena pointers during the block
  (e.g. on the stack) are fine — traced as roots to a non-moving target, gone at
  block exit.
- **arena → heap** (an arena object points into the moving heap): governed by the
  leaf-only rule below. In v1 this edge cannot exist, so the collector never
  traces out of an arena and the §8 generational write-barrier is not involved.

### 5.1 Leaf-only arena objects (v1)

In v1, arena objects must be **leaf / pointer-free** — `bytes`, scalar `@repr(c)`
structs, numeric arrays — the same restriction D-0151 v1 imposes on
stack-promotion. This erases both (a) tracing outgoing pointers from arena
objects and (b) the `arena → young` old-to-young edge that would require the
`gc-immixpp.md` §8 remembered-set/write-barrier (a #714 generational feature). It
covers the entire FFI-buffer use case, and — because arena-internal links can be
expressed as **indices into the arena** (plain scalars), not GC pointers — it
also covers index-linked arena-local data structures without any pointerful
support.

Pointerful arena objects (storing real heap references) are deferred to v2,
sequenced behind #714's generational write-barrier (§7).

## 6. Concurrency

An arena is **coroutine-local**: its bump pointer is unshared, the handle is not
`@shareable` (D-0113), and passing an arena value across a `spawn` or into a
`scope:` child is an **escape** (compile error) — which falls out of §3 for free.
Rationale: a child coroutine could outlive the block or race the bump pointer;
both are unsound. Parallel allocation into one logical arena is a real future
need, but is served by **per-worker sub-arenas** (each child gets its own region
slab, bumps locally with a plain add, all freed at the parent block's exit) —
*not* a shared atomic cursor, which would tax the fast path and drag in the
`@shareable`/`--race` surface. Tracked as a deferred issue (§7).

## 7. Deferred / out of scope

Tracked follow-ups, each gated on need, none in v1:

- **`@noescape` parameter annotation** (threading arena values through Zap
  helpers): an inline parameter annotation (reusing the D-0089 surface) the arena
  checker verifies in the callee and relies on at the call site; viral but local,
  checkable, and *visible* — preferred over inferred interprocedural
  non-retention, because a soundness-critical property belongs in the signature,
  not hidden in how the code happens to be written.
- **Pointerful arena objects** (`arena → heap` edges): requires #714's §8
  generational write-barrier; v2.
- **Per-worker sub-arenas** for parallel allocation under a join (§6).
- **`a.reset()`** mid-block recycle for loop-chunked reuse: v1 uses the
  nested-`with`-block idiom instead; `reset()` gated on a benchmark showing the
  per-iteration pool round-trip costs.
- **Arena-local typed collections** (`arena_dict()` etc.): a separate feature
  (collections parametrized by an allocator); leaf-or-pointerful per element type
  by the same §5.1 rule. Explored separately.

## 8. Sequencing

The precise-GC gate (`revisit after precise GC lands`, OPEN-QUESTIONS) is
satisfied by I#116/I#120. #553 is **not** on the #714 critical path: with the
leaf-only v1, it needs from #714 only a modest region-lifecycle API (tag a region
pinned/non-moving, claim/bulk-return from the fungible pool) — a *subset* of the
region-tag model — and no generational barrier. It can therefore land on today's
Immix (Phase 0, STW: every root rescanned each cycle, non-moving arena never
relocated) as the **first concrete consumer of the #714 region-tag model**,
co-designing that API. Only pointerful arena objects (v2) wait for #714's
generational write-barrier. Implementation order: **#553 leaf-v1 now →
pointerful-v2 after #714 generational**, not "#714 then #553".

## 9. Grammar

No `GRAMMAR.ebnf` change: `with` (#185) and inline parameter annotations (#458 /
D-0089, for the future `@noescape`) already exist. The `with`-head-only
constraint on `arena()` (§2.2) is a sema rule, not a grammar production.

## 10. Implementation status (v1)

The shipped v1 (epic #553: runtime #878, surface/sema #879, codegen #880) realizes
this spec with two refinements worth recording:

- **`alloc` element set.** `a.alloc(bytes, n)` (a non-moving byte buffer, the FFI
  motivation) and `a.new(T, …)` (a single leaf struct) are implemented. Typed
  numeric arena arrays — `a.alloc(numericT, n)` yielding a flat element-typed
  buffer (§5.1) — are deferred (#881): they need a size-parametric element-typed
  buffer representation that `sema_types::Ty` does not yet have. Leaf-only still
  applies to every element type. Allocating a non-`bytes` element in `alloc`, or a
  pointerful `T` in `new`, is a compile error (`ZAP5130`).
- **Exhaustion exception.** `cap` overflow / heap OOM raises a **catchable**
  exception as specified, but v1 raises a `RuntimeError` whose message names it a
  memory error rather than a first-class `MemoryError` — Zap has no `MemoryError`
  type yet. Promoting it to a dedicated `MemoryError` is a small, separable
  language-surface follow-up tracked under the #553 epic.

Enforcement lands in sema (`crates/sema/src/scoped.rs` arena-value escape,
`ZAP5131`; `crates/sema/src/lib.rs` leaf-only, `ZAP5130`), with the runtime region
primitive in `crates/codegen/src/zapgc.inc.c` / `zaprt.c` (`zap_arena_open` /
`zap_arena_alloc_bytes` / `zap_arena_alloc_atomic` / `zap_arena_close`, a pinned
`ZapBlock` tag the evacuator and sweep both honor) and the `with`-exit bulk free +
allocation path in `crates/ir/src/lower.rs` / `crates/codegen/src/lib.rs`.
