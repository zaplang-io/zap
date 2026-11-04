# Zap C FFI — design (D-0012)

Authoritative design for Zap's **C foreign-function interface**: calling existing
C libraries from Zap, and the ABI/GC/build contract that makes it sound under the
default **precise, moving Immix** collector.

**Decision key:** this is the design backing **D-0012** ("Interop: Python-first,
C later"). D-0012 currently records C FFI as *deferred*. Accepting this design
flips C FFI from deferred to **Experimental** (per `STABILITY.md` §1.5 / §4); the
DECISIONS entry and status change land in the first implementing PR, not here.

**Status:** Proposed (pending review).

Each top-level section (§3–§14) is sized to become one ticket; §15 is the epic +
sub-issue breakdown.

---

## 0. Problem & scope

Zap wants a large ecosystem of official non-core packages (`zaplang-io/zap#333`):
database drivers (`postgres`, `mysql`, `mongodb`, `redis`…), messaging
(`kafka`, `nats`, `rabbitmq`…), `grpc`, `kubernetes`, cloud SDKs, and more. For
most of these, a mature, battle-tested **C library already exists** (libpq,
hiredis, librdkafka, OpenSSL, libxml2, SQLite, …). Re-implementing each wire
protocol in pure Zap is the difference between an **L/XL** ticket and an **S/M**
one, and it forfeits decades of hardening.

A C FFI is the lever: bind the C library once, wrap it idiomatically, ship the
package. This document specifies how that binding works **without breaking the
garbage collector or the scheduler**, which is the entire difficulty.

### Goals

- Call C functions from Zap with near-zero per-call overhead (direct `call`, no
  marshalling interpreter).
- A **sound** contract for passing Zap-managed memory to C under precise, moving
  Immix — no use-after-free, no premature collection, no stale pointers.
- Reuse existing C libraries statically-linked into Zap's self-contained binary.
- A two-layer package convention (`-sys` raw bindings + safe wrapper) that scales
  across the `#333` ecosystem.

### Non-goals (for this design)

- Exposing Zap *as* a C library for other languages to consume (reverse FFI) —
  separate future work.
- C++ interop (name mangling, exceptions, vtables) — out of scope; wrap a C shim.
- A stable Zap↔Zap binary ABI — explicitly rejected by D-0021 / `STABILITY.md`
  §1.5; unchanged here.
- Automatic header → binding generation (`bindgen`-style) — desirable, tracked as
  a later tooling ticket (§15), not required for v1.

---

## 1. Why Zap does **not** copy Python's C-extension model

Python's C extensions (`PyObject*`, refcounts, `PyInit_*`, runtime-loaded `.so`
plugins) exist because CPython is an **interpreter** that dynamically loads
native plugins and hands them interpreter-internal objects. All of that is
interpreter-ABI glue.

Zap compiles **whole-program, ahead-of-time** via LLVM (D-0009, D-0021). The
natural model is therefore **plain C FFI resolved at link time** — the Rust
`extern "C"` / Go cgo / Zig model — not a plugin ABI. Two consequences:

1. **Static linking makes *consuming* C easy, not hard.** The C library is
   archived into the binary; calls resolve to direct symbols. The toolchain
   already does exactly this for `zaprt` (the C runtime) and for the CPython
   embedding (`ZAP_HAVE_CPYTHON`, D-0012 / I#27). FFI generalizes an existing
   capability.
2. The hard part is **not** the call. It is the **GC and scheduler contract** at
   the boundary, which §2 and §7 specify.

---

## 2. The GC contract (the hard core)

> This section is the load-bearing one. Everything else is mechanism around it.

Zap's **default** collector since **I#116** is **precise Immix** with
**opportunistic moving evacuation** (I#120). **Boehm** remains a selectable
alternate backend, untouched by the Immix work. The two have *opposite* FFI
properties, and the contract below is written to the **stricter** one (precise +
moving) so that correct code is automatically correct under Boehm too — never the
reverse.

### 2.1 What precise + moving means for a pointer handed to C

- **Precise.** The collector finds roots *only* through the per-coroutine
  **shadow stack**: codegen gives every pointer-typed SSA value a `gcframe` slot
  and stores it at its def site (I#167); the marker traces object fields via
  per-type layout descriptors. **C memory has no descriptors and no shadow
  slots.** Therefore a Zap pointer that lives *only* inside a C struct or C-held
  location is **invisible** to the collector → it will be freed at the next GC.
  (Boehm, being conservative, would have scanned C memory and kept it alive. The
  default collector will not.)

- **Moving.** Evacuation (I#120) relocates live objects out of sparse blocks and
  **rewrites the shadow slots**; compiled code observes the new address via
  **load-at-use** (reload the slot at every use after a safepoint). A raw address
  handed to C is **not** a shadow slot, so after an evacuation it is **stale** →
  dangling. Today only `zap_desc_atomic` leaf buffers are in the movable
  whitelist, but PRECISE user structs are an anticipated future movable class —
  **FFI code must not assume "my object won't move."**

### 2.2 The three rules for passing Zap memory to C

**Rule 1 — PIN anything whose address escapes into C.**
A pinned object is excluded from evacuation. The evacuator **already has this**:
its "mark-in-place-first" set pins the `G` object, green stacks, `Scope`,
`ZapChan` and their raw buffers (precisely because they are "held by raw C
pointers … that are NOT GC slots and would dangle"), the reflection/signal
tables, LOS, and **any object with a registered finalizer**. The FFI pin
primitive reuses this path.

**Rule 2 — ROOT anything C retains across a safepoint.**
Because the precise collector won't discover C-held pointers, a Zap pointer that
must stay live while only C holds it is registered in the global root table
(`zap_groots`, already a trace root). Pin (Rule 1) keeps it from moving; root
(Rule 2) keeps it from being collected. Together: the **handle** pattern.

**Rule 3 — the C-call boundary is the precise-scan limit.**
The collector can precisely scan a Zap stack (shadow slots) but **not** a C
frame. Entering C is a **safepoint boundary**: the shadow stack up to the last
Zap frame is the root set; C frames are opaque. Anything C transitively needs
must be reachable from pinned+rooted state, never from a value that exists only
in a C register/frame.

### 2.3 Mutation / write-barrier rule

Immix today is stop-the-world mark-sweep-evacuate, so no write barrier does any
work yet. One barrier hook already exists in the ABI: reassigning a **mutable
module-level pointer global** (D-0112, #663) emits `zap_gc_write_barrier(&slot)`
after the store — a linkable no-op today (the collector re-scans every static
root each cycle), reserved for a future generational/incremental mode that would
not. Load-at-use and any such future barrier assume Zap heap pointer slots are
only written by Zap code. Therefore **C code must not store a Zap pointer into a
Zap object's pointer field or into a Zap static root slot** except through a
runtime helper that performs the (current or future) barrier. C may freely read
Zap data and write into *C* or into Zap **atomic/leaf** buffers (no pointer
slots).

### 2.4 Precedent already in tree

`pyinterop` (I#27) ships the pattern: a `ZapPy` object that escapes into CPython
is pinned (finalizer-registered → mark-in-place) and finalized when collected.
The FFI contract is a generalization of this proven case, not a new GC story.

### 2.5 Collector choice as an explicit FFI lever

Because Boehm is still a supported backend (conservative, non-moving → trivial C
interop, at the cost of Immix throughput/locality), an FFI-heavy program *may*
select `gc-boehm`. The ABI contract is written to precise-moving Immix so the
same source is correct under both. "Which collector for FFI-heavy builds" is a
documented, supported choice, not a forced default.

---

## 3. Surface syntax

> Illustrative; exact spelling is owned by the syntax/grammar ticket and must
> conform to `SPEC.md` (`def`, type-first declarations D-0003, indentation
> blocks D-0002) and update `docs/GRAMMAR.ebnf`.

Raw declarations live in an `extern` block; linkage in a `link` block:

```zap
extern "c":
    def sum_of_squares(int64 n) -> int64
    def PQconnectdb(cstr dsn) -> cptr[PGconn]
    def PQstatus(cptr[PGconn] conn) -> int32
    def PQfinish(cptr[PGconn] conn) -> void

link:
    system "pq"          # -lpq from the system
    # or: static "pq"    # link libpq.a
    # or: source "csrc/shim.c"   # compile + link vendored C
```

`cstr`, `cptr[T]`, `cvoid`, and the `c_*` primitive aliases (§4) are the FFI
type vocabulary. A call into an `extern "c"` function emits the boundary
transition (§2.3, §7) around a direct `call`.

C-compatible aggregates use a decorator (consistent with `@derive`,
`decorators-26`):

```zap
@repr(c)
struct Timespec:
    int64 tv_sec
    int64 tv_nsec
```

---

## 4. Type & ABI mapping

| Zap | C | Notes |
|---|---|---|
| `int8/16/32/64`, `uint*` | `int8_t`…`uint64_t` | fixed-width, explicit |
| `c_int`, `c_long`, `c_size`, `c_char` | `int`, `long`, `size_t`, `char` | platform-width aliases |
| `float32/64` | `float`/`double` | |
| `bool` | `_Bool` | 1 byte |
| `cptr[T]` | `T*` | raw, non-GC pointer |
| `cvoid` / `cptr[cvoid]` | `void` / `void*` | opaque |
| `cstr` | `const char*` | NUL-terminated; see below |
| `@repr(c) struct` | `struct` | C layout: field order, padding, alignment |
| C bitfields | — | reuse C-style bitfields (I#173) |
| `cfn[(A,B) -> R]` | function pointer | non-capturing only (§6) |

- **`@repr(c)`** guarantees C field order/padding/alignment (vs Zap's own value
  struct layout D-0051/D-0067, which is unspecified). This is the main new
  layout work; it extends value structs (I#75) and C bitfields (I#173).
- **Strings/bytes.** Zap `str`/`bytes` are GC objects with a header; they are
  **not** C strings. Crossing the boundary either (a) borrows a pinned pointer to
  the payload for the duration of a call (atomic buffer, safe to pin), or (b)
  copies into a C buffer. `cstr` marshalling helpers live in the FFI runtime.
- **Arrays/slices.** Pass `(ptr, len)`; the payload buffer is an atomic leaf
  object — pin for the call's duration.

---

## 5. Memory ownership & lifetimes

Two heaps coexist: the Zap GC heap and C's `malloc` heap. The boundary rules:

- **Zap → C (short).** For a value passed into a call and not retained past it,
  pin for the call duration:

  ```zap
  with pin(buf) as p:
      c_write(fd, p.ptr, p.len)   # buf cannot move or be freed inside the call
  ```

- **Zap → C (retained).** For a pointer C keeps (callback userdata, a stored
  handle), use a rooted+pinned **handle** (Rules 1+2); release explicitly or when
  the owning wrapper is finalized.

- **C → Zap (C owns).** A C resource (`PGconn*`, `FILE*`) is wrapped in a Zap
  object whose **finalizer** calls the C free function (`PQfinish`, `fclose`).
  This is RAII-via-GC and reuses Immix finalizers (I#92) — the same mechanism
  that already pins the object.

  ```zap
  struct Conn:
      cptr[PGconn] raw
  # finalizer registered at construction -> PQfinish(raw) on collection
  ```

- **Atomic buffers.** Memory handed to C that holds **no** Zap pointers is
  allocated as `zap_desc_atomic` (leaf) so the marker never scans it for
  pointers — cheaper and avoids false retention.

---

## 6. Callbacks (C calling into Zap)

A C function pointer is a bare code address; a Zap function value/closure (I#163)
is a fat record. So:

- **v1:** only **non-capturing** functions may be passed as `cfn`, with state
  carried through C's conventional `void* userdata` (itself a rooted handle if it
  references Zap memory). The callback body is a boundary in reverse: it must
  re-enter the Zap world (register its thread with the GC if C spawned it, §7)
  before touching any Zap object.
- **Later:** a trampoline generator that synthesizes a C-ABI thunk per closure
  (tracked, not v1).

---

## 7. Blocking calls & the scheduler

A synchronous C call blocks the OS thread (M) it runs on. Per I#119, Zap is
currently **M:P 1:1 with no P-handoff** ("deferred"). So today a blocking C call
pins its M and **stalls that P's coroutines** with no escape.

Design:

- **Entering C** flips the coroutine/thread to an *in-C* state so the collector
  treats its Zap stack as frozen at the boundary (§2.3) and STW waits correctly.
- **Potentially-blocking** `extern` calls (annotated, or all of them by default
  in v1) must either:
  - **(a)** hand off the P so the scheduler keeps running other coroutines on a
    fresh M — **depends on P-handoff landing** (a prerequisite ticket), or
  - **(b)** for libraries exposing a non-blocking fd (libpq async, hiredis),
    wire the fd into the existing **netpoller** (I#80) and never block an M at
    all — the preferred path for the DB/network packages.
- **Exiting C** rendezvouses at a safepoint (may briefly block if a GC is in
  progress) before resuming Zap execution.

**This is the single biggest runtime dependency and is called out as a blocker
ticket in §15.**

---

## 8. Error handling

C reports failure via return codes / `errno` / out-params. The **safe wrapper**
layer (not the raw `-sys` layer) translates these into Zap's typed exceptions
(exceptions shipped via LLVM unwinder). `errno` is read immediately after the
boundary returns (before any Zap code that could clobber it). No C error code
crosses into user-facing Zap APIs unwrapped.

---

## 9. Allocator strategy

Three options, in order of preference:

1. **Per-library allocator hooks (DEFAULT).** Many important C libraries let the
   caller supply `malloc`/`free`: OpenSSL `CRYPTO_set_mem_functions`, SQLite
   `SQLITE_CONFIG_MALLOC`, zlib `zalloc`/`zfree`, libpng, Lua, curl, Python. For
   these, the `-sys` wrapper registers a shim routing that library's allocations
   to a pinned/atomic Zap allocation or to the C arena (option 3). Opt-in, scoped
   to one library, no global symbol games, works under static linking.

2. **Split heaps + finalizers (FALLBACK).** For libraries with no allocator hook:
   C uses its own `malloc`; Zap wraps C resources with finalizers (§5). The
   default conservative model.

3. **Segregated C arena (EXPERIMENTAL, appendix §A).** A **non-moving,
   conservatively-scanned** region beside the precise Immix heap, into which C
   allocations are routed (optionally via global `malloc` redirect). This is the
   only sound form of "route C memory through the GC" under a precise collector,
   because C memory has no layout descriptor and cannot be precisely traced. It
   is effectively a Boehm-style sub-heap for FFI; see §A for why it is **not** the
   default and its hazard list.

---

## 10. Build & linking

- **Linkage.** `link` directives (§3) emit linker flags / archive members. The
  toolchain already links C (`zaprt`), so this extends an existing path.
- **Package-manager native build step (NEW).** The registry/package manager
  (I#69, tooling I#157) grows a `build.zap`-style hook — the analogue of Cargo's
  `build.rs` + `cc` crate, or cgo's compiler invocation: "this package has C
  sources or a system-lib dependency; compile/locate them, produce a `.a`, feed
  it to the linker." This is the main new infrastructure.
- **Static vs dynamic.** Static link (vendored source or `.a`) preserves Zap's
  self-contained binary (D-0021). Fully-static + some system libs (glibc) is
  hostile; the documented escape hatches are **musl** static builds or vendoring
  the C source. System `.so` linking is supported but trades away full staticness.
- **`-sys` + safe convention.** Each C-backed package in `#333` is two layers:
  `<lib>-sys` (raw `extern` + native build) and `<lib>` (idiomatic, safe). Maps
  directly onto the created repos (`postgres`, `redis`, `grpc`, …).

---

## 11. Safety & the unsafe boundary

- `extern` declarations and `cptr` dereferences are **unsafe**: the compiler
  cannot verify the C signature matches the library, nor that pin/root rules are
  honored. The `-sys` layer is explicitly unsafe; the safe wrapper upholds the
  invariants and exposes only safe Zap types.
- The pin/root/handle rules (§2.2) are a **programmer contract** in v1. A future
  ticket may add static lint ("pointer escapes to C without `pin`") — desirable,
  not required for v1.
- No `cptr` may be dereferenced from safe Zap code.

---

## 12. Stability

Per `STABILITY.md` §1.5 / §4, the C FFI surface ships **Experimental** when it
lands and is governed by the stability policy from then on. The `zaprt`/GC/ABI
internals it rests on remain **Internal** and may change between compiler
versions; FFI bindings are recompiled with the program (whole-program model), so
there is no cross-version binary-ABI promise for Zap itself — only the **C** side
follows the platform C ABI.

---

## 13. Security considerations

- FFI is an unsafe, memory-unsafe boundary by nature; a buggy or hostile C
  library can corrupt the Zap heap. Official `-sys` packages vendor pinned,
  audited C sources and record provenance.
- The package-manager native build step runs a C compiler on dependency source —
  it must run under the same supply-chain controls as the registry (I#236 index
  checks) and should be sandboxed.
- The global-`malloc`-redirect appendix (§A) is gated and off by default for the
  reasons in its hazard list.

---

## 14. Testing strategy

- **Golden tests green under BOTH collectors** (gc-immix and gc-boehm),
  deterministic output, **no exact heap-byte assertions** (GC byte counts are
  non-deterministic across triggers).
- **Move-stress with C escapes:** allocate, hand pinned pointers to a C shim,
  force evacuation, assert the C shim still sees valid data and the program
  result is unchanged (models `immix_evac_pinned`).
- **Leak/UAF:** run the FFI goldens under ASan/Valgrind in CI.
- **Boundary/GC:** a C callback that triggers a Zap allocation + GC while C holds
  a rooted handle; assert no premature collection.
- Per the local-Docker constraint, CI is the sole gate for the full
  multi-collector matrix.

---

## 15. Phasing — epic & sub-issues

Proposed epic: **"C FFI (D-0012)"**. Sub-issues, ordered by dependency:

1. **FFI-0 · P-handoff prerequisite** (runtime) — scheduler P-handoff so a
   blocking C call doesn't stall a P (§7). *Blocks the blocking-call story; the
   netpoller path (§7b) can land independently for async-fd libraries.*
2. **FFI-1 · `extern`/`link` syntax + grammar** (§3) — parser, `GRAMMAR.ebnf`,
   SPEC.
3. **FFI-2 · Type & ABI mapping + `@repr(c)`** (§4) — C layout for structs,
   `c_*` primitives, building on value structs (I#75) / bitfields (I#173).
4. **FFI-3 · Codegen boundary + GC contract** (§2) — boundary safepoint, in-C
   state transition, the pin primitive (reuse evacuator mark-in-place) and the
   `zap_groots` handle API.
5. **FFI-4 · Memory & lifetimes** (§5) — `pin`/handle API, finalizer-backed C
   resource wrappers, atomic buffers, `cstr`/slice marshalling.
6. **FFI-5 · Errors** (§8) — errno/return-code → typed exceptions in wrappers.
7. **FFI-6 · Callbacks** (§6) — non-capturing `cfn` + userdata.
8. **FFI-7 · Package-manager native build step** (§10) — compile/link vendored C
   or system libs; `-sys` convention.
9. **FFI-8 · Allocator hooks** (§9) — per-library hook helpers.
10. **FFI-9 · Testing matrix** (§14) — both-collector goldens, ASan, move-stress.
11. **FFI-10 · (stretch) header→binding generator** — `bindgen`-style tooling.
12. **FFI-11 · (experimental) segregated C arena / global malloc redirect** (§A).

**First C-backed package to validate the stack end-to-end:** a thin `redis` over
**hiredis** (async-fd → netpoller path, modest surface), then `postgres` over
**libpq**.

### Prerequisites & accelerants (sequence first)

Several existing deferred/open decisions materially change FFI's cost or
soundness and should be sequenced ahead of the FFI tickets.

**Prerequisites** (FFI is weaker or dishonest without them):

- **Scheduler P-handoff** (deferred — I#119: "M:P 1:1 … no P-handoff"). Without
  it a blocking C call stalls its P's coroutines; the async-fd→netpoller path
  (§7b) is the only way around it. *Gates FFI-0 for synchronous C libraries.*
- **Unsigned integers** (D-0019, deferred — OPEN-QUESTIONS: "likely reintroduced
  only for C interop"). C APIs are saturated with `uint8/uint16/uint32/size_t`;
  without real unsigned types §4's mapping and `@repr(c)` are casts-and-prayers.
  *Do before FFI-2 / FFI-4.*

**Accelerants** (FFI ships faster / safer / more static):

- **musl / fully-static link mode** (Open; the #287 portability work notes "a
  static/musl link mode" needing musl builds of OpenSSL/zlib/GC). Gives `-sys`
  packages clean self-contained static binaries and makes the Appendix-A
  interposition viable; shares the vendored-C build muscle FFI-7 needs. *Pairs
  with FFI-7.*
- **Arena / region allocation** (deferred — its own gate, "revisit **after**
  precise GC lands," is now satisfied by I#116/I#120). A `scope:`-bound,
  non-moving, escape-checked region is the principled primitive for pinning
  buffers across a C section (§5) and for the segregated C arena (§9.3 /
  Appendix A). *Feeds FFI-4 and FFI-11.*
- **Richer reflection + field/parameter decorators + macro API** (the D-0036
  remainder; D-0087/D-0089 already landed). Makes `@repr(c)` a natural field
  decorator and enables the header→binding generator (FFI-10). *Pairs with
  FFI-2 / FFI-10.*

Explicitly **not** prerequisites: the `set` type and `**kwargs` — no FFI bearing.

### Ecosystem payoff

With FFI in place, many `#333` tickets drop from **L/XL → S/M** (bind + wrap
instead of reimplement the wire protocol): `postgres`, `mysql`, `redis`,
`mongodb`, `cassandra`, `kafka`, `grpc`, `elasticsearch`, and others. This is the
strategic reason to build FFI before most of the `#333` package work.

---

## 16. Alternatives considered

- **Python-extension-style plugin ABI** — rejected (§1); Zap is AOT, not an
  interpreter.
- **`ctypes`-only / pure-Zap `dlopen` wrappers** — kept as a *possible* ergonomic
  layer but insufficient as the base: it forfeits static linking and type
  checking, and still needs the GC contract for retained pointers.
- **Global `malloc`→GC redirect as default** — rejected (§A); unsound/brittle
  under a precise collector and under static linking.
- **Default to Boehm for all FFI** — rejected as a *default* (throughput/locality
  cost on all programs) but retained as a per-program lever (§2.5).

---

## 17. Open questions

- Should **all** `extern` calls default to the boundary-transition cost, or only
  those annotated `blocking`? (Perf vs safety; leaning "all in v1, annotate to
  opt *out* of handoff for known-fast calls.")
- Granularity of `pin`: per-object (current evacuator model) vs per-call-arg
  automatic pinning inserted by codegen.
- Do we expose `cfn` trampolines for capturing closures in v1 or defer? (Deferred
  above.)
- Minimum viable set of `c_*` platform aliases for the target matrix
  (Linux/macOS/Windows × x86_64/aarch64).

---

## Appendix A — Experimental: segregated C arena / global `malloc` redirect

The idea: route C allocations into the GC so the collector tracks them. Feasible
(Boehm ships `--enable-redirect-malloc`), but as a **global default under precise
Immix it is unsound/brittle**:

1. **Precise tracing can't read C memory** — redirected allocations have no
   layout descriptor, so the marker can't trace pointers inside them; they must
   be conservatively scanned and pinned — which a precise collector is not built
   to do.
2. **Conservative scanning of hostile C → UAF/false-retention** — hidden/tagged/
   XOR'd pointers, pointers only in kernel buffers, etc.
3. **Atomic-vs-pointerful is unknowable** without type info — scan everything
   (false retention) or scan nothing (miss live pointers).
4. **Static linking defeats interposition** — glibc calls its own `malloc`
   directly; musl is friendlier.
5. **Early/async `malloc`** — libc/loader call `malloc` before `GC_INIT`; the
   signal-preemption (I#78) and signal-diag (I#77) paths need async-signal-safety
   that `GC_malloc` lacks.
6. **Reentrancy** — a collection fired mid-C-call (holding a non-GC-aware lock)
   risks deadlock.

**Sound form:** a **segregated, non-moving, conservatively-scanned C arena** —
effectively a Boehm-style region bolted beside Immix, which the evacuator's
existing "raw-C-held, mark-in-place" carve-out already resembles. Gated, likely
musl-only, for trusted leaf libraries. Not the default; the bootstrap recursion
(GC growing its own heap) is handled exactly as Boehm does — the GC takes OS
memory via `mmap`/`sbrk`, never the public `malloc`, so redirecting public
`malloc` does not recurse.

---

## References

- **Decisions:** D-0012 (interop), D-0009 (Rust+LLVM, Boehm-first), D-0021
  (whole-program AOT), D-0013 (`spawn`), D-0051/D-0067 (value structs).
- **Issues/PRs:** I#116 (default→Immix), I#120 (evacuation), I#119 (per-P bump /
  no P-handoff), I#167 (def-site rooting), I#92 (Immix finalizers), I#80
  (netpoller), I#27 (CPython interop), I#173 (C bitfields), I#75 (value structs),
  I#77/I#78 (signals), I#236 (registry index checks).
- **Docs:** `STABILITY.md` §1.5/§4, `docs/GRAMMAR.ebnf`, `DECISIONS.md`.
