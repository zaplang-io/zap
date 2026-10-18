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
- **GC (D-0009):** conservative **Boehm** GC for v1; precise (Immix-style) later.
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
  type (string/bytes/scalars built-in).
- `array[T]` (#24): `[ header | dtype | ndim | dims: *i64 | strides: *i64 |
  data: *T ]`, contiguous row-major; views share `data`.

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

// Allocation — routes to the GC (conservative Boehm in v1).
void* zap_alloc(size_t size, const TypeDesc* desc);   // may contain pointers
void* zap_alloc_atomic(size_t size);                  // pointer-free (e.g. byte buffers)

// Strings (#17)
void* zap_str_from_utf8(const uint8_t* bytes, int64_t len);
void* zap_str_concat(void* a, void* b);

// Collections (#21): zap_list_new/push/get/len, zap_dict_new/set/get ...

// I/O
void  zap_print_i64(int64_t v);     // or the compiler emits printf directly
void  zap_print_str(void* s);

// Faults / exceptions (#22)
void  zap_panic(void* exc);         // raise / built-in faults; triggers unwinding
```

The Phase 1 slice calls libc `printf` directly; those calls migrate behind
`zap_print_*` as the runtime grows.

### Garbage collection

**v1 — conservative Boehm GC.** `zap_alloc*` wrap `GC_malloc` /
`GC_malloc_atomic`; the collector conservatively scans the stack, registers and
globals, so **no stack maps or safepoints are required**. The dev container
adds `libgc-dev`; the runtime links `-lgc`.

**Later — precise GC (Immix-style).** Needs pointer maps (from type
descriptors), GC safepoints, and LLVM statepoint-based stack maps. The header
layout above is forward-compatible; this is a swap behind the same `zap_alloc*`
surface.

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
