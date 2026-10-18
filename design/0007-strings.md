# Proposal: The Zap string model (`str`, `char`, `bytes`)

- **Author(s):** Zap team
- **Last updated:** 2026-10-18
- **Status:** Accepted (design) — implemented incrementally per strings tickets
- **Discussion:** github.com/parveshgarg/zap#16

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

This document specifies Zap's text types in implementation detail: the
immutable UTF-8 `str`, its heap layout and ASCII fast path, codepoint-based
indexing and view-based slicing, the `char` (Unicode scalar) and `bytes`
(raw-byte) types, escape processing in the lexer, and the lowering of
f-strings.
It builds on the locked decisions D-0025 (string model), D-0026 (f-strings),
and D-0041 (string literals), and reconciles the layout with
`design/RUNTIME-ABI.md`.
It is intended to be enough to implement T7 (strings codegen) and the string
runtime.

## Background

Zap targets Python developers who want native performance, so the string model
must *feel* like Python's `str` while compiling to efficient native code.
The locked decisions fix the shape:

- **D-0025** — `str` is **immutable, stored as UTF-8**.
  `s[i]` and slicing are **codepoint**-based (Python semantics) with an
  **ASCII fast path** (O(1) when ASCII, O(n) fallback otherwise); iteration
  yields `char` (a Unicode scalar).
  A separate **`bytes`** type holds raw bytes.
  Because strings are immutable, **slices are cheap views** over the shared
  buffer.
- **D-0026** — interpolation is via **f-strings** (`f"{x} items"`).
- **D-0041** — Python lexical conventions: `'...'` and `"..."` are both
  strings; `'''...'''` / `"""..."""` are multiline; prefixes `r"..."` (raw),
  `b"..."` (bytes), `f"..."` (f-string), and the combinations `rb`/`rf`;
  Python escape sequences (`\n`, `\t`, `\\`, `\uXXXX`, …).
  There is **no dedicated char literal**: a `char` comes from `s[0]` or
  `char(...)`.

`design/RUNTIME-ABI.md` already reserves the heap layout for `str` and `bytes`
and the runtime entry points (`zap_str_from_utf8`, `zap_str_concat`, …); this
document fills in the semantics behind that ABI and must stay consistent with
it.

### What the first increment (T7a / #17) already shipped

A minimal end-to-end slice is merged (PR #30): string **literals**, `str + str`
**concatenation**, `str` **parameters and returns**, and **`print(str)`**.
Its runtime (`crates/codegen/src/zaprt.c`) uses a two-word, `malloc`-based
object with **no GC** and **no type-descriptor header** yet:

```c
typedef struct { int64_t len; const char *data; } ZapStr;
void *zap_str_new(const char *data, int64_t len);   // copies `len` bytes
void *zap_str_concat(void *a, void *b);
void  zap_print_str(void *s);                        // fwrite(data, 1, len)
```

This document describes how that minimal object **evolves** into the full model
below (header, ASCII flag) without changing the happy path that already works.

## Proposal

### Types at a glance

| Type    | Element      | Mutable | Indexing unit      | Stored as        |
|---------|--------------|---------|--------------------|------------------|
| `str`   | `char`       | no      | Unicode codepoint  | UTF-8 bytes      |
| `char`  | —            | n/a     | n/a (scalar)       | `i32` scalar     |
| `bytes` | `int` (0–255)| no¹     | byte               | raw `u8` buffer  |

¹ `bytes` is immutable in v1 (mirrors Python's `bytes`, not `bytearray`); a
mutable `bytearray` may be added later — see Open issues.

### `str` representation and heap layout

`str` is a **reference type** (D-0023): a value of type `str` is a pointer to a
heap object, and `None` is the null pointer.
The buffer is **UTF-8** and the object is **immutable** — no in-place mutation
is ever exposed, which is what makes views (below) safe.

The full object matches the layout reserved in `design/RUNTIME-ABI.md`:

```
offset  0:  header   : *const TypeDesc   (8)   — ABI object header
offset  8:  len_bytes: i64               (8)   — length of the UTF-8 data
offset 16:  flags    : i32               (4)   — bit0 = is_ascii
offset 20:  _pad     : i32               (4)
offset 24:  data     : *const u8         (8)   — UTF-8 bytes (not NUL-terminated)
```

Notes:

- `len_bytes` is the **byte** length, not the codepoint count.
  The codepoint count is *not* stored; it is computed on demand (and is equal
  to `len_bytes` on the ASCII fast path — see below).
- `data` points at a byte buffer that is **not** required to be NUL-terminated;
  all runtime entry points carry an explicit length.
  Interop that needs a C string copies into a NUL-terminated buffer.
- The buffer may be **shared** by several `str` objects (slices, and literal
  deduplication); it is never written after construction.
- **String literals** are emitted as constant global byte arrays; their `str`
  objects can be built once (static or on first use) and point directly at the
  constant data, which needs no GC-managed buffer.

#### Evolution from the shipped `{ len, data }` object

The shipped two-word object is the tail of the full layout minus the header and
flags.
Evolution is additive and staged with the runtime tickets:

1. **Add the `flags` word** and set `is_ascii` at construction
   (`zap_str_from_utf8` scans the bytes once; literals precompute it at compile
   time).
   This unlocks indexing/slicing (below) and is independent of GC.
2. **Add the ABI header** (`*const TypeDesc`) when GC/reflection land (#10):
   allocation moves from `malloc` to `zap_alloc` and the descriptor enables the
   collector to keep shared buffers alive.
   Until then the header may be a null placeholder so the offsets are stable.

Field order above is the final one; the shipped object is understood as the
same object with the two leading words not yet present.

### The ASCII fast path

The `is_ascii` flag records whether every byte is `< 0x80`.
When set:

- **codepoint count == `len_bytes`**, and
- the i-th codepoint is the byte at offset `i` — so `s[i]` and codepoint
  offsets for slicing are **O(1)**.

When clear (the string contains multi-byte UTF-8):

- `s[i]` and codepoint→byte-offset mapping are **O(n)**: the runtime walks the
  UTF-8 sequence from the start counting codepoints.
  (A future optimization may memoize offsets or add an index for very long
  non-ASCII strings — see Open issues; v1 is the straight walk.)

The flag is computed once, at construction, by the entry point that builds the
string (`zap_str_from_utf8`), by `zap_str_concat` (`ascii(a) && ascii(b)`), and
at compile time for literals.
Because strings are immutable, the flag never goes stale.

### Indexing: `s[i]` → `char`

`s[i]` yields a **`char`** (a Unicode scalar), *not* a one-character `str`
(this differs from Python, where `s[0]` is a `str`; Zap has a real `char`
type — D-0025).

- `i` is a **codepoint** index, counted from 0.
- Negative indices count from the end (`s[-1]` is the last codepoint), Python
  style.
- An out-of-range index raises `IndexError` (the exception machinery, D-0029 /
  #22; until it lands, the runtime traps).

Runtime surface:

```c
// Returns the codepoint at codepoint-index `i` (handles negatives); faults on
// out-of-range. O(1) when is_ascii, else O(n).
int32_t zap_str_index(void *s, int64_t i);
```

### Slicing: `s[a:b:step]` → `str` view

A slice produces a **new `str` object that shares the parent's buffer** — a
**view**, not a copy (D-0025; contrast `list[T]` slices, which copy — D-0030).
Because `str` is immutable, sharing is always safe.

- Bounds follow Python slice semantics: `a`/`b` default to the ends, negatives
  count from the end, out-of-range bounds clamp, an empty range yields `""`.
- For the common `step == 1` case the view's `data` is `parent.data +
  byte_offset(a)` and its `len_bytes` is `byte_offset(b) - byte_offset(a)`;
  computing the byte offsets is O(1) on the ASCII path, O(n) otherwise.
  `is_ascii` is inherited from the parent (a sub-range of ASCII is ASCII; for a
  non-ASCII parent the slice conservatively keeps the flag clear, or rescans —
  see Open issues).
- A **non-unit or negative `step`** cannot be a view (the bytes are not
  contiguous): it **materializes a new buffer** holding the selected
  codepoints, like Python.

The view keeps the parent buffer alive through the GC: the slice's `data`
points **into** the parent's buffer, and the collector (conservative Boehm in
v1) sees that interior pointer and does not reclaim the buffer.

```c
// step == 1: O(1)/O(n) view over the shared buffer.
void *zap_str_slice(void *s, int64_t a, int64_t b);
// general step: materializes a new string.
void *zap_str_slice_step(void *s, int64_t a, int64_t b, int64_t step);
```

### Length, iteration, comparison

- **`len(s)`** returns the **codepoint** count (Python semantics): O(1) on the
  ASCII path (equals `len_bytes`), O(n) otherwise.
  A separate way to get the **byte** length is `len(bytes(s))` (O(1)).
- **Iteration** (`for ch in s`) yields `char` values in order; it walks the
  UTF-8 buffer once, so it is O(n) overall regardless of the ASCII flag and
  does not pay per-index walks.
- **Comparison** — `==`/`!=` are **byte-wise** on the UTF-8 buffer (valid UTF-8
  compares equal iff byte-equal); ordering `< <= > >=` is **lexicographic by
  codepoint**, which for UTF-8 is identical to byte-wise comparison, so all
  comparisons reduce to `memcmp` + length.
  `==` on `str` is built-in (SPEC §6).

```c
int64_t zap_str_len_codepoints(void *s);           // len(s)
int32_t zap_str_eq(void *a, void *b);              // ==
int32_t zap_str_cmp(void *a, void *b);             // <0 / 0 / >0
```

### The `char` type

`char` is a **Unicode scalar value** (a codepoint in `U+0000..U+10FFFF`
excluding the surrogate range `U+D800..U+DFFF`).
It is a **scalar/value type** represented as `i32` (per `design/RUNTIME-ABI.md`
"Value representation").

- Obtained from `s[i]`, from iteration, or by conversion `char(n)` from an
  integer codepoint (which validates the range and raises `ValueError` on a
  surrogate or out-of-range value) — D-0041 says there is **no char literal**.
- `int(ch)` gives the codepoint number; `str(ch)` gives a one-codepoint `str`
  (its UTF-8 encoding).
- Because it is a scalar, a `char` is passed in a register and needs no heap
  object.

### The `bytes` type

`bytes` is an **immutable sequence of raw bytes** — a reference type with the
same shape as `str` but **no UTF-8 interpretation** and **no `is_ascii`
semantics** (the flag word is unused / zero):

```
[ header(8) | len: i64 | flags: i32 (unused) | _pad | data: *const u8 ]
```

- `b[i]` yields an **`int`** in `0..255` (a byte), and is **O(1)** always
  (indexing is by byte, not codepoint).
- Slicing `b[a:b]` is a **view** over the shared buffer (immutable, like
  `str`).
- Literals: `b"..."` (D-0041); only byte-range escapes are meaningful
  (`\xHH`, `\n`, …); `\u`/`\N` are **not** allowed in a `bytes` literal.

Conversions bridge the two types explicitly (never implicitly):

- `bytes(s)` — the UTF-8 encoding of `s` (O(1): it can share `s`'s buffer as a
  view, since both are immutable).
- `str(b)` — **decodes** `b` as UTF-8 and raises `ValueError` ("invalid UTF-8")
  on malformed input; this is the only place UTF-8 validity is enforced at
  runtime for the `bytes→str` direction.

`str` vs `bytes` in one line: `str` is *text* indexed by codepoint and
guaranteed valid UTF-8; `bytes` is *data* indexed by byte with no encoding
promise.

### Escape processing (lexer)

Escapes are resolved **once, in the lexer**, when a string/bytes literal token
is scanned; the parser and later stages see the already-decoded bytes.
Per D-0041 (Python escapes):

- Simple: `\n \t \r \\ \' \" \0 \a \b \f \v`.
- Byte: `\xHH` (exactly two hex digits).
- Unicode (in `str`/`f`-strings only): `\uXXXX`, `\U00XXXXXX`, and `\N{NAME}`
  (named codepoint); each is validated and **encoded to UTF-8** in the stored
  buffer.
  A `\u`/`\U`/`\N` that names a surrogate or out-of-range scalar is a
  **compile-time error**.
- **Raw** strings (`r"..."`): no escape processing at all — backslashes are
  literal (a raw string still may not end in an odd run of backslashes before
  the closing quote, Python rule).
- **Bytes** literals (`b"..."`): `\x` and the simple escapes are allowed;
  `\u`/`\U`/`\N` are **rejected** (bytes have no codepoints).
- Prefix combinations `rb`/`rf` compose the two behaviors (`rb` = raw bytes; a
  raw f-string keeps backslashes literal in the literal pieces but still parses
  `{...}` fields).

The result is that a `str` literal's stored buffer is final UTF-8 bytes and its
`is_ascii` flag is computed at compile time.

### f-string lowering (D-0026, D-0041)

An f-string `f"...{expr[:spec]}..."` is **syntactic sugar lowered during
parsing / AST construction** into ordinary expressions, so neither the type
checker nor codegen needs special f-string support:

1. The lexer splits the literal into alternating **text pieces** (escape-
   processed as above) and **replacement fields** `{ expr }`, each field
   optionally carrying a **conversion** (`!r`/`!s`/`!a`) and a **format spec**
   after `:` (itself possibly containing nested `{...}` fields).
   `{{` and `}}` are literal braces.
2. Each field `expr` is parsed as a normal Zap expression (full expressions are
   allowed inside the braces).
3. The whole thing lowers to a **concatenation of `str` pieces**, with each
   field converted to `str` via `format(expr, spec)` (or `str(expr)` when there
   is no spec and no conversion):

```python
f"{name} has {count} items"
# lowers to roughly:
str_concat(str_concat(str_concat(name_as_str, " has "),
                      format(count, "")), " items")
```

- The `format(value, spec)` builtin dispatches on the value's type (and later a
  `__format__`-style protocol) to implement Python's format mini-language
  (`:>10`, `:.2f`, `:x`, …); v1 can start with a minimal spec grammar and grow.
- `!r` lowers to `repr(expr)`, `!s` to `str(expr)`, `!a` to `ascii(expr)`
  before formatting.
- An **empty** f-string or one with no fields is just a plain `str` literal.
- Because lowering happens before type-checking, type errors inside `{...}`
  are reported at the field's source location like any other expression.

A later optimization can lower to a single `zap_str_builder`/join call instead
of nested pairwise concatenations to avoid intermediate strings; the semantics
are identical, so this is a codegen-only concern (Open issues).

## Rationale

- **Immutable UTF-8** (D-0025) is the choice that makes everything else cheap:
  slices can be views, comparison is `memcmp`, buffers can be shared and
  deduplicated, and there are no mutation/aliasing hazards.
  It matches Python's `str` immutability while using a compact native encoding.
- **Codepoint indexing with an ASCII fast path** gives Python-compatible
  semantics (`s[i]` is "the i-th character") while keeping the overwhelmingly
  common ASCII case O(1).
  Storing only `len_bytes` plus a one-bit flag — not a codepoint count or an
  offset index — keeps the object small; the count is derivable and free on the
  fast path.
- **A real `char` type** (vs Python returning a length-1 `str`) avoids a heap
  allocation per character and fits Zap's static typing: iteration and indexing
  have a precise element type.
- **`bytes` distinct from `str`** cleanly separates "text" from "data",
  enforces UTF-8 validity exactly once (at `str(b)`), and matches the ABI's
  pointer-free `zap_alloc_atomic` for byte buffers.
- **View slices for `str`** (D-0025) deliberately differ from **copying `list`
  slices** (D-0030): immutability is what justifies the difference, so the two
  rules do not conflict.
- **Lexer-time escapes and parse-time f-string lowering** keep the heavy
  machinery out of codegen: by the time the type checker runs, a literal is
  just bytes and an f-string is just concatenation of typed expressions.

Alternatives considered:

- **UTF-16 or UTF-32 storage** — rejected: UTF-32 wastes 4× space for ASCII-
  heavy text and UTF-16 has its own surrogate complexity; UTF-8 is the native
  interchange encoding and the ASCII fast path recovers O(1) indexing where it
  matters.
- **Byte indexing for `str`** (Go's model, where `s[i]` is a byte) — rejected:
  it breaks the Python mental model D-0025 commits to; Zap gives byte indexing
  via `bytes` instead.
- **Always storing a codepoint count / offset table** — rejected for v1: it
  grows every string object for a cost only non-ASCII indexing pays; left as a
  possible optimization.

## Compatibility

- The model is **backward compatible** with the shipped first increment: the
  literal/concat/print path keeps working; `flags` and the ABI `header` are
  **additive** fields (see "Evolution"), and the final field order is fixed
  here so later tickets do not reshuffle offsets.
- This document **does not change** `design/RUNTIME-ABI.md`; it fills in the
  semantics behind the layout and entry points that doc already reserves.
  Any new runtime entry points named here are additive to the ABI surface.
- New reserved type names `char` and `bytes` are already in SPEC §4 and the
  predeclared-identifier list (SPEC §19), so no grammar change is implied beyond
  what D-0041 already locks.

## Implementation

Maps onto the roadmap's strings work, after the merged first increment:

1. **Object evolution** — add `flags`/`is_ascii` to the string object and set
   it in `zap_str_from_utf8`/`zap_str_concat` and at compile time for literals.
   (Runtime + codegen; no ABI change beyond the reserved layout.)
2. **Indexing & `len`** — `zap_str_index`, `zap_str_len_codepoints`; codegen for
   `s[i]` (→ `char`) and `len(s)`.
3. **Slicing** — `zap_str_slice` (view, step 1) and `zap_str_slice_step`
   (materialize); codegen for slice syntax.
   Depends on GC (#10) for correct lifetime of shared buffers; until then views
   over `malloc` buffers leak, which is acceptable for pre-GC testing.
4. **Comparison** — `zap_str_eq`/`zap_str_cmp`; wire up `== != < <= > >=`.
5. **`char`** — scalar lowering (`i32`), `char(n)`/`int(ch)`/`str(ch)`,
   iteration yielding `char`.
6. **`bytes`** — the type, `b"..."` literals, `b[i]`, view slicing,
   `bytes(s)`/`str(b)` conversions.
7. **f-strings** — lexer field-splitting, parse-time lowering to
   concat/`format`, and a minimal `format(value, spec)` builtin (grown over
   time).
8. **GC integration (#10)** — allocation via `zap_alloc`/`zap_alloc_atomic`,
   the ABI header, interior-pointer keep-alive for views.

Phase boundaries touched: the **runtime ABI** (additive entry points and the
already-reserved header/flags) and **codegen** (indexing, slicing, iteration,
f-string lowering).
Items 2–6 can proceed largely in parallel once item 1 lands; f-strings (7)
depend only on `str` concat (already shipped) plus `format`.

### Status: done vs. remaining

- **Done (#17):** `str` literals, `str + str` concat, `str` params/returns,
  `print(str)` — on a minimal `malloc` `{ len, data }` object, no GC, no header.
- **Remaining:** `is_ascii` flag; `s[i]` indexing; slicing (views + step
  materialization); `len(s)`; comparison/ordering; iteration; the `char` type;
  the `bytes` type and `str`/`bytes` conversions; f-strings and `format`; and
  GC-managed allocation with the ABI header (#10).

## Open issues

- **Non-ASCII indexing acceleration** — whether very long non-ASCII strings get
  a memoized codepoint-offset index or stay O(n) walks; v1 is the straight
  walk.
- **Slice `is_ascii` for non-ASCII parents** — inherit-clear (conservative) vs.
  rescan the sub-range to possibly set it; affects whether an ASCII sub-slice of
  a non-ASCII string gets the fast path.
- **Small-string / inline optimization** — whether to inline short strings into
  the object (SSO) to avoid a separate buffer allocation; deferred.
- **Literal interning / dedup** — whether equal string literals share one
  object and whether runtime interning is offered.
- **`format` mini-language scope** — how much of Python's format spec v1
  supports, and whether a `__format__`-style protocol is exposed to user types.
- **Mutable `bytearray`** — whether a mutable raw-byte type is added alongside
  immutable `bytes`.
- **f-string builder optimization** — lowering to a single join/builder instead
  of pairwise concat (codegen-only; semantics unchanged).
- **NUL-termination for C interop** — exact policy for handing a `str`/`bytes`
  to C (D-0012 Python interop) — copy-to-NUL-terminated vs. length-carrying.
