# Design: Zap runtime ABI

- **Author(s):** Zap team
- **Last updated:** 2026-10-18
- **Status:** Accepted (design) — implemented incrementally per runtime tickets
- **Discussion:** github.com/parveshgarg/zap#13

## Abstract

This document defines the contract between **compiled Zap code** and the **Zap
runtime library**: value representation, heap-object layout, the calling
convention, the runtime entry points (allocation, strings, collections), the
GC strategy, and the exception/unwinding ABI. Freezing it lets the runtime be
built independently of the compiler, and lets the runtime tickets (GC,
collections, exceptions, strings, arrays) proceed against a stable interface.

## Background

Relevant locked decisions:

- **Value vs reference semantics (D-0023):** scalars are values; `list`, `dict`,
  `array`, and `struct` instances are reference types; `str` is immutable.
- **GC (D-0009):** precise **Immix** GC by default (ticket #116); conservative
  Boehm GC is the opt-in fallback (`--features zapc/gc-boehm`).
- **Native target:** the compiler emits a relocatable object (via LLVM) and
  links it with `cc`. The runtime is a library linked into every program.
- **Exceptions (D-0007):** unchecked, with real stack unwinding.

The target platforms are x86_64 and arm64 (Linux in the dev container; macOS
later). We follow the platform C ABI so `cc` linking and future C/Python interop
(D-0012) come for free.

## Proposal

### Value representation

- **Scalars** — `int32`/`int64` → `i32`/`i64`; `float32`/`float64` → `f32`/`f64`;
  `bool` → `i1`/`i8`; `char` → `i32` (Unicode scalar). Passed and stored
  **unboxed, by value**.
- **Reference types** — `str`, `bytes`, `list`, `dict`, `array`, and `struct`
  instances are a **pointer to a heap object**. `None` is the **null pointer**
  for a reference-typed slot; member access on `None` traps (→ exception).
- **Tuples / multiple returns** — returned by value as a small aggregate, or via
  an `sret` pointer when large (see Open issues).

### Heap object header

Every heap object begins with a one-word (8-byte) **header**:

```
offset 0:  type descriptor pointer   (*const TypeDesc)   — 8 bytes
offset 8:  payload ...
```

- The **type descriptor** identifies the object's type for runtime reflection
  (D-0036), method/interface dispatch, and — under precise GC later — pointer
  maps. Under conservative Boehm GC it is informational (the collector scans
  conservatively and does not need maps).
- A future precise GC adds mark/forward bits (either in spare header bits or a
  side table) without changing this base layout.

### Struct layout

Fields are laid out **in declaration order** with natural C alignment; the
object is `header` followed by the fields. Reference-typed fields are pointers.
Struct instances are reference types (D-0023): heap-allocated and passed by
pointer. Methods receive the instance pointer as the first argument (`self`).

```
struct Point { f64 x; f64 y }  ->  [ header(8) | x: f64(8) | y: f64(8) ]  = 24 bytes
```

### Interface values — the `{data, itable}` fat pointer (#84, D-0016)

A value of **interface type** is a two-word **fat pointer**, passed and returned
**by value** (design/0006 §2.4 — "the one reference type wider than a single
word"):

```
offset 0:  data   (*object)  — the underlying concrete instance   — 8 bytes
offset 8:  itable (*ITable)  — the dispatch table for this (struct, interface)
```

- `data` is a heap pointer to the concrete instance (one word per the value
  representation above). The object's own header/type-descriptor word is
  unchanged — the fat pointer is *beside* the object, not in it. A **reference
  struct** is used directly (shared heap identity). A **`value struct`** has no
  stable address — the function body addresses it through a pointer that may be a
  stack slot under the register/sret ABI (a spilled parameter or call result), or
  an lvalue the interface value must not alias — so on conversion it is **boxed**:
  copied to a fresh heap instance that `data` points at (see Construction).
- `itable` points at a **static, immutable** dispatch table: an `[N x ptr]`
  array of function pointers, one per interface method, in **interface
  declaration order** (slot *k* = the implementor's method for interface method
  *k*). The compiler emits one itable per `(concrete struct, interface)` pair
  that occurs, built once and shared.
- **Construction** (concrete → interface, a conversion site: assignment to an
  interface-typed binding, an interface-typed argument, or an interface return)
  materializes `{ data = instance, itable = &itable_for_pair }`. For a **`value
  struct`** the conversion first boxes it (#75 Part 1, D-0051): a field-wise deep
  copy into a fresh heap instance (the same copy that collection-boxing uses —
  nested value-aggregate fields are copied recursively, reference fields shared),
  and that box pointer becomes `data`. The box is an ordinary GC object: it is
  allocated with the struct's type descriptor, so the precise collector traces
  its pointer fields, and it is kept live transitively through the fat pointer's
  root exactly as a reference-struct `data` is. The copy makes the interface
  value independent of the original (value semantics) and gives `data` the stable
  heap address the fat pointer requires.
- **Dispatch** of `s.m(args…)` loads slot *k* from `s.itable` and calls it with
  `s.data` as `self`: a single indirect call, no per-object cost beyond the
  shared header. The callee uses the ordinary calling convention with `self` as
  the leading pointer argument.
- The fat pointer is a **value**: it uses the same by-value aggregate calling
  convention as a flat `value struct` / tuple (register/sret), so there is no
  heap allocation per conversion. Its `data` word is a GC pointer traced via the
  originating instance's root; the `itable` word addresses static data and is
  safely ignored by the collector (it is not a heap object).
- `None` as an interface value would be `{ data = null, itable = null }`;
  interface→interface conversion and interface-typed **struct fields / collection
  elements** are **not yet realized**. Interface-typed **bindings, parameters,
  and returns** are supported over both reference structs and (via the boxing
  above) `value struct`s.

### String & bytes layout (coordinate with #16 / design/0007-strings)

`str` is immutable UTF-8:

```
[ header(8) | len_bytes: i64 | flags: i32 (bit0 = is_ascii) | _pad | data: *const u8 ]
```

- `is_ascii` enables the O(1) codepoint-indexing fast path; otherwise indexing
  is O(n) over UTF-8.
- **Slices are views:** a slice shares the parent's `data` buffer (adjusted
  `data`/`len`) and keeps the parent alive via the GC. Because strings are
  immutable this is always safe.
- `bytes` is similar; mutability per the strings design.

### Collection layout (coordinate with #21)

- `list[T]`: `[ header | len: i64 | cap: i64 | data: *T ]`. Elements are unboxed
  for scalar `T`, pointers for reference `T`. Amortized-doubling growth.
- `dict[K,V]`: an open-addressing hash table — `[ header | len | cap | entries:
  *Entry ]`, `Entry = { hash, key, value }`. Hashing/equality come from the key
  type (string/bytes/scalars built-in). A **user-struct key** (operator
  overloading, #52 / D-0047) is hashed/compared by the type's own
  `__hash__`/`__eq__`: the dict/set carries those two function pointers (set on
  first insert) and the typed entry points gain an `_fn` key variant
  (`zap_dict_set_fn_i64`/`_ptr`, `zap_dict_get_fn_*`, `zap_set_add_fn`,
  `zap_set_contains_fn`) that take the `__hash__`/`__eq__` pointers as trailing
  arguments. A new `KEY_FN` key kind routes the probe loop through them. This is
  the struct-element-hashing unblock deferred in D-0043.
- `array[T]` (#24, #49): `[ header | dtype | ndim | dims: *i64 | strides: *i64 |
  data: *T ]`, contiguous row-major; views share `data`. `dtype` tags the
  element type — `0 = f64`, `1 = i64` (8-byte words), `2 = uint8`, `3 = bool`
  (1-byte words). `zap_array_new(dtype, len)` sizes the buffer by dtype; `uint8`
  /`bool` cross the typed get/set/sum entry points as `i64` words (get
  zero-extends, set truncates/normalizes). Mixed-dtype element-wise ops promote
  per D-0011; the SIMD kernel reads each operand buffer at its own element type.
- `heap[T]` (#76, D-0057): a binary min-heap priority queue — `[ header | len:
  i64 | cap: i64 | data: *T | kind | dir ]`, a complete binary tree over a
  growable element-word array (amortized-doubling growth). `kind` selects the
  element word representation (`0` = `i64`, `1` = `str`), picking the typed
  `zap_heap_{push,pop,peek}_{i64,ptr}` entry point; `dir` selects the ordering
  (`0` = min, `1` = max). Elements are **ordered, not hashed**: `i64` words
  compare numerically, `str` pointer words by `ZapStr` bytes (`zap_str_cmp`).
  `push` sifts up, `pop` removes the root and sifts down, `peek` reads the root
  (all O(log n)); `zap_heap_new(kind, dir)` constructs, `zap_heap_len` is the
  count, and `zap_heap_items` snapshots the elements into a `list[T]` in
  priority (root-first) order for non-destructive iteration. The comparator
  surface is `heap[T](cmp="min"|"max")`; user-struct elements ordered via a
  comparator / `__cmp__` function pointer, float elements, and an arbitrary
  `cmp=` comparator function are clean follow-ups.

### Calling convention

Zap functions use the **platform C calling convention** (System V AMD64 /
AArch64 AAPCS), so object code links with `cc` and interoperates with C (and,
later, CPython). `self` is the first parameter. The program entry is a C
`int main()` the compiler generates, which:

1. calls `zap_runtime_init()` (GC init, etc.),
2. runs the Zap top-level / `main`,
3. returns an exit code.

### Runtime entry points (the ABI surface)

Compiled code calls these C-ABI functions (names illustrative; exact set grows
per ticket):

```c
void  zap_runtime_init(void);

// Allocation — routes to the GC (precise Immix by default; Boehm fallback).
void* zap_alloc(size_t size, const TypeDesc* desc);   // may contain pointers
void* zap_alloc_atomic(size_t size);                  // pointer-free (e.g. byte buffers)

// Strings (#17)
void* zap_str_from_utf8(const uint8_t* bytes, int64_t len);
void* zap_str_concat(void* a, void* b);

// Collections (#21): zap_list_new/push/get/len, zap_dict_new/set/get ...
// Struct keys (#52, D-0047): zap_dict_set_fn_*/get_fn_*, zap_set_add_fn,
//   zap_set_contains_fn (take __hash__/__eq__ fn pointers); and the hashing
//   helpers the @derive(Hash)/@derive(Ord) dunders call:
//   int64_t zap_i64_hash(int64_t), zap_str_hash(void*),
//   zap_hash_combine(int64_t acc, int64_t h), zap_str_cmp(void*, void*).

// I/O
void  zap_print_i64(int64_t v);     // or the compiler emits printf directly
void  zap_print_str(void* s);

// Faults / exceptions (#22)
void  zap_panic(void* exc);         // raise / built-in faults; triggers unwinding
void* zap_eh_begin_catch(void* ue); // landing-pad entry: recover the exc object
void  zap_eh_end_catch(void);       // balance a begin_catch when leaving a handler
void  zap_uncaught(void* exc);      // top-level handler: report + exit non-zero

// Netpoller (#80, design/0005 "Blocking syscalls"): non-blocking I/O readiness
// for the M:N scheduler. A goroutine that would block on a not-yet-ready fd
// registers it and PARKS its G (freeing its M to run other goroutines) instead
// of blocking the OS thread; a poller thread re-queues the G on readiness. These
// are the INTERNAL surface the stdlib calls — no language I/O exists yet
// (sockets #79, signals #77). `mode` is 1=read / 2=write; `wait` returns
// 0=ready / 1=timeout / 2=closed. `deadline_ns` is absolute CLOCK_MONOTONIC
// nanoseconds (0 = none). Edge-triggered: callers drain to EAGAIN.
void*   zap_netpoll_open(int64_t fd);                     // register fd; NULL if unsupported
int64_t zap_netpoll_wait(void* pd, int64_t mode, int64_t deadline_ns); // park until ready/deadline
void    zap_netpoll_wake(void* pd, int64_t mode);         // force-wake a parked waiter (#77 hook)
void    zap_netpoll_close(void* pd);                      // deregister + release

// Signals, runtime diagnostics & fault reporting (#77, D-0058). The mechanism
// (see design/0005 "Signals"): an async-signal-safe handler on a DEDICATED
// signal thread writes the signal number to a self-pipe REGISTERED WITH THE
// NETPOLLER; the signal thread parks on the netpoller and dispatches in normal
// context — fan-out to notify channels (cap-1, coalescing), arm cancel_on
// scopes, or run a default diagnostic handler. `zap_signal_init()` runs at the
// top of `main` and installs the default handlers (SIGQUIT -> goroutine stacks
// + runtime dump; SIGUSR1 -> force GC + memory stats), unless the runtime is
// built -DZAP_NO_DEFAULT_SIGNALS. Catchable signals only; FAULT signals
// (SIGSEGV/SIGFPE/SIGBUS/SIGILL) go to a crash handler (on a sigaltstack) that
// reports the fault + a backtrace and re-raises the default disposition.
void  zap_signal_init(void);                              // main prologue: start + default handlers
void* zap_signal_notify(int64_t signo);                   // -> channel[Signal] (cap-1, coalescing)
void  zap_signal_on(int64_t signo, void* fn);             // @on_signal: detached receiver -> fn()
void  zap_scope_cancel_on(void* scope, int64_t mask, int64_t grace_ms); // scope(cancel_on=,grace=)
void  zap_scope_disarm(void* scope);                      // drop cancel_on (called from scope exit)

// `gc`/`runtime` module + the uniform GC control surface (backend-agnostic:
// both collectors export these, so no per-backend #ifdef). `gc.stats()` builds
// a MemStats from these scalar getters.
void    zap_gc_collect(void);                             // force a full collection
void    zap_gc_stats(int64_t* heap_bytes, int64_t* live_bytes);
int64_t zap_gc_count(void);                               // collections run so far
int64_t zap_gc_heap_bytes(void);
int64_t zap_gc_live_bytes(void);
int64_t zap_gc_free_bytes(void);
void    zap_runtime_goroutine_dump(void);                 // stacks + scheduler summary -> stderr
int64_t zap_runtime_num_goroutines(void);

// Scheduler controls (ticket #172). The parallelism knob is `MAXPROCS` (env var
// + this runtime call; no `ZAP_` prefix). LockOSThread pins the calling
// coroutine to a dedicated OS thread until the balanced unlock (Go analogue).
int64_t zap_runtime_maxprocs(void);                       // effective worker-M count
void    zap_runtime_lock_os_thread(void);                 // pin caller to its own OS thread
void    zap_runtime_unlock_os_thread(void);               // release the pin (nestable)
```

Long CPU-bound coroutines are preempted asynchronously (Go 1.14 model): a monitor
thread signals (`SIGURG`) a coroutine that has monopolized its M past a ~10 ms
slice; it yields at the next GC safepoint under the precise collector, or via a
direct in-handler switch under Boehm. No ABI surface — it is internal to the
scheduler/runtime.

The first exceptions increment (#22) lands `raise`, `try`/`except`/`finally`,
and propagation to an aborting top-level handler on the platform Itanium
unwinder (libstdc++ personality `__gxx_personality_v0`, linked with `-lstdc++`;
the `_Unwind_*` unwinder comes from libgcc). The in-flight exception object is
currently the message string; the typed exception hierarchy (D-0029),
type-directed `except` matching, built-in faults (div-by-zero,
index-out-of-range, `None` access), and stack traces are follow-ups.

The Phase 1 slice calls libc `printf` directly; those calls migrate behind
`zap_print_*` as the runtime grows.

### Decorators & reflection (#16, D-0036)

The built-in `@derive` decorators are expanded to real methods at AST→ZIR
lowering (zero runtime cost); two small runtime helpers back the generated
code:

```c
void*   zap_i64_to_str(int64_t v);      // used by generated @derive(Repr)
int64_t zap_str_eq(void* a, void* b);   // content equality, used by @derive(Eq)
```

Runtime reflection over annotations/decorators is a flat metadata registry,
pay-only-when-used. The compiler emits `zap_reflect_register` calls at the top
of `main` for every struct-level annotation/decorator; queries read the table:

```c
void    zap_reflect_register(void* type_name, void* deco_name);  // startup bootstrap
int64_t zap_reflect_has(void* type_name, void* deco_name);       // 1/0
void*   zap_reflect_names(void* type_name);                      // comma-joined ZapStr*
```

The surface is intentionally **string-keyed** (`reflect_has("User", "Table")`)
in v1. Only `@derive(Eq)` and `@derive(Repr)` are generated so far; the rest of
the locked built-in set (`@cache`, `@default`, `@validate`, `@range`,
`@deprecated`, `@inject`, and `@derive(Ord/Json/Hash)`), field/parameter/
function decorators, and the user-defined compile-time macro API remain
follow-ups. (Note: the `@range` built-in name currently collides with the
`range` keyword and cannot be written until decorator-name parsing allows
keyword-like identifiers.)

### Garbage collection

Two collectors are selectable at build time behind one allocation surface
(ticket #48, D-0009, design/0008-precise-gc.md). As of ticket #116 the **default
is the precise Immix collector** (codegen's `default` feature is `gc-immix`); the
conservative Boehm collector is the opt-in fallback behind
`--features zapc/gc-boehm`.

**Default — precise, non-moving Immix collector.** A plain codegen build
(`--features zapc/llvm`, or the explicit `gc-immix`) links it; codegen defines
`ZAP_GC_IMMIX` for the runtime and drops `-lgc`. It is **non-moving** in v1 (mark-only line marking over a 32 KiB
block / 128 B line heap with bump allocation, line-granularity recycling, and a
separate large-object space); opportunistic evacuation/defragmentation is a v2
follow-up. The precise machinery:

- **Object header behind the pointer.** Every heap object is laid out
  `[ descriptor|mark (8) | payload_size (8) | payload… ]`; the pointer handed to
  Zap/runtime code is the **payload** pointer, so struct field offsets and the
  runtime object layouts above are byte-identical to the Boehm build (no field
  shift). The collector recovers an object's descriptor from `payload[-2]`.
- **Type descriptors & pointer maps.** `zap_alloc_typed(const ZapTypeDesc*)`
  carries a per-type descriptor. Codegen emits one PRECISE descriptor per user
  struct and spawn closure (the pointer fields' byte offsets, computed from the
  LLVM layout); the runtime defines descriptors for its own objects (strings,
  the collection headers, arrays, regex matches, scheduler `G`/`Scope`). Variable
  collection buffers are traced precisely by their owning container using a
  per-instance element-kind flag (set in the `_ptr`/`_str`/`_fn` entry points),
  so there is **no conservative scanning**.
- **Roots via a per-coroutine shadow stack.** Codegen maintains an explicit
  shadow-stack frame per function (linked onto a per-thread `zap_shadow_head`),
  rooting every GC-pointer value. The head is saved into each coroutine's `G`, so
  a parked coroutine's roots are just its saved chain (no stack scanning). Also
  rooted: the main thread's chain, a global-root registry, and a reflection-table
  callback.
- **Safepoints & stop-the-world.** Codegen emits a `zap_safepoint()` poll at the
  top of every basic block (function entry + loop back-edges). A collection runs
  **only** at a safepoint (never inside runtime C, which has no shadow frames);
  allocation merely flags that one is due. Stop-the-world is cooperative: mutator
  threads park at safepoints or in blocking runtime calls, and the collector
  marks + sweeps once all are parked. No write barrier / non-generational in v1.
**Fallback — conservative Boehm GC.** Built with `--features zapc/gc-boehm`.
`zap_alloc*` wrap `GC_malloc` / `GC_malloc_atomic`; the collector conservatively
scans the stack, registers and globals, so **no stack maps or safepoints are
required**. The dev container adds `libgc-dev`; the runtime links `-lgc`. The
runtime object layouts above are byte-identical to the Immix build.

- **Migration.** The default was flipped to Immix in ticket #116 after the full
  golden suite passed **both** ways; the `full` CI check runs it under both
  collectors (Immix primary, Boehm secondary). Boehm stays buildable as the
  documented fallback.

### Exception / unwinding ABI (coordinate with #22)

Zap uses **zero-cost table-based unwinding** (Itanium C++ ABI / `libunwind`)
with a Zap **personality function**:

- `raise e` and built-in faults call `zap_panic(exc)`, which begins unwinding.
- `try` bodies install **landing pads**; `except` clauses match on the
  exception's type descriptor; `finally` and `with` cleanup run via cleanup
  landing pads during unwind.
- The **exception object** is a heap object: `[ header | type | message | payload ]`,
  GC-managed. Unwinding must preserve GC roots; conservative Boehm scanning of
  the (still-live) stack handles this in v1.
- An uncaught exception unwinds to the top frame and aborts with a stack trace.

### Name mangling

Exported/user symbols use a stable mangling scheme (to be specified) that
encodes module path and, for monomorphized generics, type arguments.

## Rationale

- **Platform C ABI** makes `cc` linking and C/Python interop natural and avoids
  inventing a bespoke convention.
- **One-word type-descriptor header** is the minimum that serves reflection and
  dispatch now and precise-GC pointer maps later, without a v1 cost under Boehm.
- **Conservative Boehm first** ships GC in days, not months (D-0009); the ABI is
  designed so the precise collector is a later drop-in.
- **Table-based unwinding** is zero-cost on the happy path — important given
  exceptions are the *normal* error mechanism in Zap (D-0007) — and is exactly
  the capability that justified choosing LLVM (D-0042).

## Compatibility

This is a binary contract: changing object layout, the calling convention, or
the entry-point signatures is a breaking ABI change that requires rebuilding the
runtime and all programs. Additive growth (new entry points, new type
descriptors) is fine.

## Implementation

- Runtime as a library crate (`crates/runtime`, Rust with `extern "C"` exports,
  or a thin C shim) compiled to `libzaprt` and linked into every program.
- Start with `zap_runtime_init`, `zap_alloc*` (Boehm), and the string entry
  points (#17); expand for collections (#21), arrays (#24), and exceptions (#22).
- Dev container (`.devcontainer/Dockerfile`) gains `libgc-dev` (and later
  `libunwind`, PCRE2, libpython) as those tickets land.

## Open issues

- **Tuple / multi-return representation** (by-value aggregate vs `sret`
  threshold).
- **Exact `dict` implementation** (probing scheme, load factor, tombstones).
- **Precise GC transition** details (statepoints, Immix heap structure).
- **macOS vs Linux** linking/unwinding specifics (the dev container is Linux;
  native macOS support is later).
- **Name-mangling scheme** finalization (ties to generics, #15).
