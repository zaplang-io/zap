# Zap — Decision Log (ADR)

A running record of locked language/implementation decisions and *why*. New
decisions get a new numbered entry; superseded ones are marked, never deleted.
This file exists so settled questions are not re-litigated — the single biggest
time sink in language projects.

Status legend: **Locked** · **Proposed** (baked into docs, pending confirmation)
· **Open** · **Superseded**

_Last updated: 2026-10-17_

---

## D-0001 — Language name & file extension
**Status:** Proposed
The language is **Zap**; source files use the **`.zap`** extension. One
authoritative living spec (`SPEC.md`). Confirm or override the name/extension.

## D-0002 — Surface syntax: Python-like
**Status:** Locked
Indentation-based blocks, `def`, `if/elif/else`, `while`, `for x in range(...)`.
Readability of Python is a primary goal (target users: Python devs who need
performance).

## D-0003 — Declarations are type-first
**Status:** Locked
A declaration is *exactly* one of:
- `TYPE name = expr`  (e.g. `int64 count = 0`)
- `auto name = expr`  (inferred, still statically typed)

Reassignment is `name = expr` (no redeclaration). Name-first (`count: int64 = 0`)
was explicitly rejected. Type keywords are reserved and cannot be identifiers.
**Rationale:** C/Go familiarity, simple grammar, no compile-speed difference vs
name-first — the choice is purely grammar/readability.

## D-0004 — Static typing with inference by default
**Status:** Locked
Statically typed underneath; `auto` + local inference make it *feel* dynamic.
Public function signatures should be explicit; inference is local.

## D-0005 — Default integer type & overflow
**Status:** Locked
Default inferred integer is **int64**. Integer overflow **wraps** (two's
complement). **Rationale:** matches hardware and C/Go release behavior; no
runtime check cost. (A checked-arithmetic opt-in may be added later.)

## D-0006 — Mutability: mutable by default
**Status:** Locked
Bindings are reassignable by default; `const` marks immutable. **Rationale:**
matches the type-first/C-Go feel and is simplest for Python users.

## D-0007 — Error handling: unchecked exceptions
**Status:** Locked
Python/C#-style **unchecked** exceptions. `raise` throws; built-in faults
(divide-by-zero, index-out-of-range, nil access) also throw. `try`/`catch`
handles them **optionally** — never forced (no checked exceptions, no `throws`).
Uncaught exceptions unwind to the top and terminate with a stack trace.
**Consequence:** the runtime must support stack unwinding + handlers.
**Open micro-decision:** catch keyword `catch` vs Python's `except`.

## D-0008 — Concurrency: goroutines + channels
**Status:** Locked
CSP-style lightweight green threads + channels + an M:N scheduler (Go model).
**Consequence:** Zap ships its own scheduler and green-thread runtime; this
constrains Go-library interop (see D-0012).

## D-0009 — Implementation stack: Rust + LLVM, Boehm GC first
**Status:** Locked (direction)
Compiler implemented in **Rust**; codegen via **LLVM** (mature SIMD + opt).
**Boehm (conservative) GC** for v1 to ship early; move to a precise Immix-style
GC once the language stabilizes.

## D-0010 — Regex: Perl-style literals, PCRE2 engine
**Status:** Locked
First-class literals `m/.../` (match) and `s/.../.../` (substitute), plus the
`=~` / `!~` bind operators. Engine: **PCRE2**.
- **Lexing ambiguity:** `m`/`s` prefix is *always required*, and `=~`/`!~`
  signal a regex context — so a bare `/` is *always* division. No Perl-style
  context heuristics.
- **`s///` returns a copy** (does not mutate in place). In-place shorthand is
  `subj ~= s/a/b/g`, which desugars to `subj = subj =~ s/a/b/g`.
- **Captures via a returned match object**, bound to a user-named local
  (null/none on no match). **No magic globals** (`$1`, `$~`) — this keeps
  matching coroutine-safe by construction, since there is no shared match
  state. Positional `mt.group(1)` and named `mt["year"]` / `mt.year`
  (from `(?<year>...)`). Inline sugar: `if s =~ m/.../ as mt:`.

See `design/0001-perl-regex.md`.

## D-0011 — Built-in N-D numeric arrays with SIMD
**Status:** Locked (design) / incremental implementation
A built-in dense numeric array type (`array[T]` / tensor) with a **shape**,
distinct from `list[T]` (generic growable). **Full N-D with shape** is the
target; implemented incrementally (1-D → N-D). Element-wise operator
overloading (`a + b`), broadcasting, NumPy-style slicing. SIMD is a
codegen/runtime concern (LLVM vector ops; compile-time `-mcpu` or runtime CPU
feature dispatch) layered on the `vec[T,N]` primitive. No `import` required.

See `design/0002-arrays.md`.

## D-0012 — Interop: Python-first, C later
**Status:** Locked
v-plan does **Python runtime interop only** (embed CPython; `import numpy`,
etc.). **Rationale:** directly serves the target audience and is well-trodden.
**C FFI is deferred**; once added it becomes the general bridge to *all*
languages (including Go via `c-shared`). Direct Go-library `import` is **not a
goal** — Go ships its own GC + scheduler that conflict with Zap's (D-0008);
reachable only later via the C boundary.

See `design/0003-python-interop.md`.
