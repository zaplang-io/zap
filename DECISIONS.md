# Zap — Decision Log (ADR)

A running record of locked language/implementation decisions and *why*. New
decisions get a new numbered entry; superseded ones are marked, never deleted.
This file exists so settled questions are not re-litigated — the single biggest
time sink in language projects.

Status legend: **Locked** · **Proposed** (baked into docs, pending confirmation)
· **Open** · **Superseded**

_Last updated: 2026-10-25_

---

## D-0001 — Language name & file extension
**Status:** Locked
The language is **Zap**; source files use the **`.zap`** extension (`.z` may be
accepted as a short alias). One authoritative living spec (`SPEC.md`).

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

## D-0004 — Static typing with inference by default
**Status:** Locked
Statically typed underneath; `auto` + local inference make it *feel* dynamic.
Public function signatures should be explicit; inference is local.

## D-0005 — Default integer type & overflow
**Status:** Locked
Default inferred integer is **int64**. Integer overflow **wraps** (two's
complement). (A checked-arithmetic opt-in may be added later.)

## D-0006 — Mutability: mutable by default
**Status:** Locked
Bindings are reassignable by default; `const` marks immutable.

## D-0007 — Error handling: unchecked exceptions
**Status:** Locked
Python/C#-style **unchecked** exceptions. `raise` throws; built-in faults
(divide-by-zero, index-out-of-range, `None` access) also throw. `try`/`except`
handles them **optionally** — never forced (no checked exceptions, no `throws`).
Uncaught exceptions unwind to the top and terminate with a stack trace.
Keyword is **`except`** (Python-style), with `finally`; see D-0029.

## D-0008 — Concurrency: coroutines + channels
**Status:** Locked
CSP-style lightweight coroutines + channels + an M:N scheduler (Go model).
Zap ships its own scheduler and green-thread runtime; this constrains Go-library
interop (see D-0012). Launch keyword is `spawn` (D-0013). Channels D-0031, sync
D-0032, cancellation D-0033, memory model D-0034.

## D-0009 — Implementation stack: Rust + LLVM, Boehm GC first
**Status:** Locked (direction)
Compiler in **Rust**; codegen via **LLVM**. **Boehm (conservative) GC** for v1;
move to a precise Immix-style GC once the language stabilizes.

## D-0010 — Regex: Perl-style literals, PCRE2 engine
**Status:** Locked
First-class literals `m/.../` and `s/.../.../`, plus `=~`/`!~` and the `~=`
in-place substitution shorthand. `m`/`s` prefix always required (bare `/` is
always division). `s///` returns a copy. Captures via a returned match object
(no magic globals; coroutine-safe). Engine: PCRE2. See
`design/0001-perl-regex.md`.

## D-0011 — Built-in N-D numeric arrays with SIMD
**Status:** Locked (design) / incremental implementation
Built-in dense numeric array type (`array[T]` / tensor) with a **shape**,
distinct from `list[T]`. Full N-D is the target, built incrementally (1-D → N-D).
The supported Experimental subset is specified in SPEC §15 (I#476): flat
literal construction, four dtypes, `+`/`-`/`*`, scalar broadcasting, `len`/`sum`,
and contiguous views. Unsupported forms are diagnosed; unequal lengths raise
`ValueError`. `list[float]` is supported (I#569, D-0100); floats as dict/set
keys or values, or heap/channel elements, remain unsupported, as do float fields
of tuple/struct values and `array[f64]` (which remain supported).
Element-wise operator overloading, broadcasting, NumPy-style slicing (slices are
**views**, D-0030). SIMD via LLVM vector ops layered on `vec[T,N]`. See
`design/0002-arrays.md`.
**Dtype promotion (#49):** mixed-dtype element-wise ops promote on the lattice
`bool < uint8 < i64 < f64` (`i64 + f64 -> f64`); arithmetic never yields `bool`
(`bool + bool -> uint8`, Python `bool`-as-`int`). Scalars are **weak** (NumPy
NEP 50): an int scalar keeps the array dtype (`uint8 + 1` stays `uint8`), only a
float scalar forces `f64`. Dtypes shipped: `f64`, `i64`, `uint8` (image data,
1-byte), `bool` (masks, 1-byte); integer-array arithmetic wraps per D-0005
(`uint8` mod 256). `f32`/`i32` dtypes remain deferred.

## D-0012 — Interop: Python-first, C later
**Status:** Locked
v-plan does **Python runtime interop only** (embed CPython). **C FFI deferred**;
once added it becomes the general bridge to all languages (incl. Go via
`c-shared`). Direct Go-library `import` is not a goal. See
`design/0003-python-interop.md`.

## D-0013 — Coroutine-launch keyword: `spawn`
**Status:** Locked
Coroutines are launched with **`spawn`**: `spawn worker(ch)`. Operand must be a
call. **Fire-and-forget** to start (statement, no handle); coordination via
channels + sync primitives (D-0032) and structured concurrency (D-0033). Chosen
over `go`, `spark`, `launch`, `start`, `fork`, `co`.

## D-0014 — Nullability: `None`
**Status:** Locked
Python-style `None`, applicable to **reference types** (`str`, `list`, `dict`,
`array`, `struct` instances, interfaces). Value scalars (`int64`, `f64`, `bool`,
…) are **not** nullable. Member access / method call on `None` throws (D-0007).

## D-0015 — User-defined types: structs + methods, no inheritance
**Status:** Locked — method syntax locked (D-0050); value structs added (D-0051)
Composite types are **structs** (data) with **methods**. **Composition only —
no inheritance, no classes.** Go-style. Method-definition syntax is locked
**inline** (D-0050). A performance-oriented opt-in **`value struct`** value type
is added by **D-0051** (plain `struct` stays a reference type, D-0023). **Full OO
(classes + inheritance) remains deferred** — tracked, not under active
consideration.

## D-0016 — Interfaces: structural, compile-time checked
**Status:** Locked
Interfaces are **structural** and satisfied **implicitly** by any struct whose
method set matches — but conformance is **checked at compile time**. Using a
struct where an interface is required, when a method is missing or misspelled,
is a **compile error**. **Rationale:** typo-class bugs are the worst to catch at
runtime; compile-time checking eliminates them. (Supersedes the earlier
"missing methods throw NotImplemented at runtime" idea.) An optional explicit
`impl Interface for Struct` declaration may be offered for clearer error
locations.

## D-0017 — Generics: yes, monomorphized, staged
**Status:** Locked (design) / staged implementation
User-defined generics (`def map[T, U](...)`, generic structs) via
**monomorphization** (specialize per type — fits perf goals + LLVM). Built-in
generic containers (`list`/`dict`/`array`) ship first; user generics follow. The
grammar reserves generic syntax from the start so adding user generics later is
additive, not a breaking change.

## D-0018 — Enums: tier 1 now; sum types later
**Status:** Locked (tier 1) — tier 3 now locked by D-0046
**Tier 1 now:** named-constant enums — a distinct type backed by int, optional
explicit values, `.name`, iterable (`enum Color: RED, GREEN, BLUE`). Repeated
values are aliases: the first declaration is canonical for `.name` and iteration
visits each distinct value once in first-declaration order. An enum with `pass`
has no members and iterates zero times.
**Tier 3 (data-carrying variants / Rust-style sum types) + `match` destructuring**
are now locked in **D-0046** (the `match` statement itself was locked earlier in
D-0040).

## D-0019 — Numeric types: all signed; bigint, complex, tuples
**Status:** Locked; bigint and complex are implemented for native codegen
All integer types are **signed** for v1 (`int32`, `int64`; default `int64`).
Unsigned deferred (interop + array dtypes only — see OPEN-QUESTIONS). Provide
**`bigint`** (arbitrary-precision signed integer) and **`complex`** (pair of
IEEE-754 doubles, with imaginary-suffix literals and complex arithmetic).
Provide **tuples**. **No type aliases** (they hurt cross-project readability).

## D-0020 — Numeric conversions: implicit widening, explicit narrowing
**Status:** Locked
Widening conversions are **implicit** (`int32→int64`, `int→float`). Narrowing
conversions that can lose data (`int64→int32`, `float→int`) require an
**explicit** conversion (e.g. `int32(x)`). **Rationale:** silent narrowing is a
classic bug source.

## D-0021 — Execution model: `def main()` entry point
**Status:** Locked; original top-level execution wording superseded by D-0090
A compiled program runs from `def main()`. Executable statements at module
scope are rejected in both entry and imported modules (D-0090); a bare,
non-f-string module docstring is permitted (D-0041). The parser still accepts
statements in module position, but that is not an execution promise. Because
Zap is AOT-compiled as a whole program, **no forward-declaration is required**
— definitions may follow uses.

## D-0022 — Truthiness: Python-falsy and strict bool
**Status:** Locked
Both are allowed: Python-style truthiness (empty/zero/`None` are falsy) in
boolean contexts, *and* strict `bool`.

## D-0023 — Value vs reference semantics
**Status:** Locked
**Scalars** (`int*`, `f*`, `bool`, `char`) pass/assign **by value** (copied).
**`list`, `dict`, `array`, and `struct` instances are reference types** (shared;
passed by reference). `str` is immutable so the distinction is unobservable.
(Note: this is the *opposite* of Go, where structs are value types — a
deliberate choice for Python-like feel.)
An opt-in **`value struct`** (D-0051) provides C-like value semantics for hot
aggregates; a plain `struct` stays reference-typed as above.

## D-0024 — Division and equality
**Status:** Locked
`/` is **true division** (float result), `//` is **floor division**
(Python-style). `==` is built-in for scalars and strings; **collections and
user types implement their own equality** (no automatic deep equality).

## D-0025 — String model: immutable UTF-8
**Status:** Locked
`str` is **immutable, stored as UTF-8**. `s[i]` and slicing are **codepoint**
based (Python semantics) with an **ASCII fast path** (O(1) when ASCII, O(n)
fallback for non-ASCII); iteration yields `char` (a Unicode scalar). A separate
**`bytes`** type holds raw bytes. Because strings are immutable, **string slices
are cheap views** over the shared buffer.

## D-0026 — Ergonomics: f-strings, comprehensions, generators, lambdas
**Status:** Locked
- **f-strings** for interpolation (`f"{x} items"`).
- **Comprehensions** (list/dict; set if a set type is added).
- **Generators** via `yield`.
- **Lambdas**, Python-style (`lambda x: x + 1`).
- **Python-style slicing** (`a[1:3]`, `a[::2]`).

## D-0027 — Resource cleanup: `with` + `try/finally`
**Status:** Locked
Cleanup is done with **context managers (`with`)** and **`try/finally`**
(Python-style). **No Go-style `defer`.** `finally` runs during exception
unwinding (D-0007).

## D-0028 — Modules & visibility
**Status:** Locked
**Python-like modules:** one file = one module; a directory = a package.
**Python-like `import` + aliasing** (`import x as y`). **Visibility:** everything
is public **except** names beginning with `_`, which are private — and, unlike
Python's honor system, this is **enforced** at compile time (external access to
`_name` is an error).

## D-0029 — Error-handling shape
**Status:** Locked
Python-like `try` / `except` / `raise` / `finally`, with a Python-like exception
hierarchy (user-defined exceptions subclass a base). Resolves the D-0007
keyword question → **`except`** (not `catch`).

The built-in hierarchy and typed dispatch are specified in SPEC §10. Exception
objects carry their runtime type identity and message; `except E` matches `E`
and its subclasses in source order. `Cancelled` is a `BaseException` outside
`Exception`, so ordinary `except Exception` handlers do not swallow
cancellation.

## D-0030 — Slicing semantics: list copies, array views
**Status:** Locked
Python-style slice syntax everywhere. **`list[T]` slices are copies** (Python
semantics). **`array[T]`/numeric slices are views** (NumPy semantics — share the
buffer; mutating a view mutates the parent; the parent buffer is kept alive by
the GC). Amends the open issue in `design/0002-arrays.md`.

## D-0031 — Channels: unbuffered default, optional buffer, `select`
**Status:** Locked
`channel[T]()` is **unbuffered** (rendezvous: send blocks until received).
`channel[T](capacity)` is **buffered** (send blocks only when full). Plus
`close(ch)`, comma-ok receive (`v, ok = <-ch`), and a **`select`** statement to
multiplex. Default to unbuffered; use buffered only to decouple producer/consumer.

## D-0032 — Sync primitives
**Status:** Locked
Channels + `select` are the idiom ("share memory by communicating"). Standard
toolbox: **Mutex, RWMutex, WaitGroup, atomics, Once.** `WaitGroup` matters
because `spawn` is fire-and-forget (D-0013) — it is the primary "wait for these
coroutines to finish" primitive; results flow back over channels.

## D-0033 — Cancellation: structured concurrency
**Status:** Locked (model) — surface resolved by D-0044
Child coroutines are **scoped** to a parent; leaving the scope awaits/cancels
children, so no coroutine is leaked and cancellation propagates automatically
(Swift/Kotlin/trio style, chosen over Go's `context`). The surface syntax (the
`scope:` block, detached spawn, and failure policy) is now locked in **D-0044**.

## D-0034 — Memory model: DRF-SC
**Status:** Locked (principle)
Data-race-free programs behave **sequentially consistently**; synchronization is
established by channel ops and sync primitives. **Data races are bugs**, ideally
caught by a race-detector tool (Go/Java/C++11/Rust family). Formal spec later.

## D-0035 — Package/dependency manager: Cargo/Go-modules style
**Status:** Locked (direction) — concrete design in D-0052
One official, bundled tool with a **manifest + lockfile + semver** and
reproducible builds (Cargo/Go-modules style), under **Python-like import
syntax**. Explicitly **not** pip/PyPI's model (dependency-hell, bolted-on
isolation). Concrete design (manifest, resolver, distribution, `zenv`) locked in
**D-0052**.

## D-0036 — Decorators / annotations
**Status:** Locked (design) — v1 scope; user macros staged
Unified `@`-syntax metaprogramming in two kinds, applicable to **functions,
methods, structs, fields, variables, and parameters**:
- **`annotation`** — declarative metadata (declared like a struct), read by
  compile-time codegen or runtime reflection (e.g. `@Column(name="id")`).
- **`decorator`** — behavioral/generative, with a `before`/`after` convenience
  form and a general `wrap`; may take arguments (e.g. `@retry(times=3)`).

**Compile-time-first** — decorators run at compile time and generate real,
type-checked code with **zero runtime cost** (`@derive`, `@validate`,
`@default`, `@cache`); annotations are **also** queryable via optional
**runtime reflection** for dynamic frameworks. Field/variable targets are the
key capability Python lacks (enables pydantic-style declarative structs with no
runtime cost). Multiple decorators stack bottom-up. Generated code is always
inspectable via **`zap expand`** (the antidote to "annotation magic").

**Rationale:** with no inheritance (D-0015), decorators/derive are Zap's primary
cross-cutting-reuse mechanism; compile-time codegen is the only version
consistent with the performance goal and the "catch at compile time" ethos
(D-0016). **v1 ships** the syntax, annotations + reflection, the built-in
decorator set, behavioral decorators, and `zap expand`; **user-defined
compile-time macros** are the hardest piece, targeted for v1 or an immediate
fast-follow. See `design/0004-decorators.md`.

## D-0037 — Canonical formatter
**Status:** Locked
A single canonical formatter (`zap fmt`) enforcing **Python-style** formatting
(one true style, gofmt-spirit); no options. It reuses the parser and comment
trivia (#225) and refuses to format source that does not parse. Guarantees:
**idempotence** (`fmt(fmt(x)) == fmt(x)`), AST-preservation, and comment
preservation.

Style specifics pinned (I#157/A2, #226; full spec in `docs/FORMAT.md`):
- **Indentation:** 4 spaces (per D-0041).
- **Max line width:** 88 columns.
- **Strings:** prefer double quotes; fall back to single only to avoid escaping;
  triple-quote values containing a newline; prefixes lowercased, ordered r/b/f.
- **Blank lines:** collapse to ≤2 between top-level items, ≤1 inside blocks and
  between type members; none at a block's start.
- **Trailing commas:** added when a call/collection/signature is wrapped across
  lines; absent when it fits on one line. Wrapping is **width-triggered**; a
  Black-style magic trailing comma is **not** adopted in this slice (a comment
  inside a collection does force a wrap so it keeps its place).
- **Imports:** one module per `import` line (the comma-list/grouped forms, D-0052,
  normalize to this); import **ordering is not rewritten** in this slice
  (deferred, to protect comment attachment).
- **Comments:** preserved in place; standalone on their own line, trailing glued
  with two spaces before `#`; one space after `#` (except `#!`/`#:`); column
  alignment not preserved.

`zap fmt --check` is the non-writing CI mode (D-0037 fmt-gate, #227).

## D-0038 — Function arguments: defaults, keyword calls, variadics
**Status:** Locked
- **Default arguments:** syntax `TYPE name = expr` in the parameter list (reads
  like a declaration). Required params precede defaulted ones.
- **Keyword arguments:** any parameter may be passed by name; at a call site
  positional args precede keyword args. Parameter names are therefore part of
  the public API (renaming is a breaking change).
- **Default evaluation is per call** — a fresh value each time, so mutable
  defaults (`[]`, `{}`) are safe. This deliberately avoids Python's
  evaluate-once-at-definition-time bug. (Impl: an omitted argument is filled
  with the default expression at the call site; callers pick up a changed
  default on recompile.)
- **Variadics:** homogeneous, Java-style `TYPE... name`, received as a
  `list[TYPE]`.
- **`**kwargs`** (arbitrary named arguments) is **deferred** — it fights static
  typing, and keyword args + defaults cover the common cases. See
  OPEN-QUESTIONS.

## D-0039 — `do`/`while` loop
**Status:** Locked
A bottom-tested loop whose body runs once before the condition is tested
(Python has none):

```python
do:
    line = read()
    process(line)
while line != ""
```

The trailing `while condition` (no colon, no block) closes the `do:` block.

## D-0040 — `match`/`case` structural pattern matching
**Status:** Locked
Python-3.10-style **structural pattern matching** as a statement (not a plain
switch). Supported patterns: literals; capture (a bare name binds); wildcard
`_`; or-patterns (`a | b`); guards (`case p if cond:`); sequence/tuple
(`[a, *rest]`, `(x, y)`); struct (`Point(x=0, y=y)`); mapping (`{"k": v}` — may
be staged). Bare names capture-and-bind (Python semantics) — to match a known
constant use `_`/a qualified name, not a bare name.

**One upgrade over Python:** for `enum`/sum-type scrutinees the compiler performs
**exhaustiveness checking** — a `match` that omits a variant and has no `_` is a
compile error (fits the compile-time-safety ethos, D-0016; ideal for matching
AST nodes). Non-enum scrutinees follow Python (no match = no-op; `_`
recommended). Statement form now; an expression form (`auto x = match ...`) may
come later. Extends naturally to data-carrying enum variants when those land
(tier 3; see OPEN-QUESTIONS).

## D-0041 — Lexical conventions (Python-style)
**Status:** Locked
Adopt Python's lexical conventions throughout:
- **Integer literals:** decimal `42`; hex `0x2A`; octal `0o52`; binary
  `0b101010`; `_` digit grouping (`1_000_000`).
- **Float literals:** `3.14`, `.5`, `1.`, exponent `1e10` / `1.5e-3`; `_`
  grouping allowed.
- **Complex literals:** imaginary suffix `j` (`3 + 4j`, `2j`) — D-0019.
- **String literals:** both `'...'` and `"..."` are strings; triple-quoted
  `'''...'''` / `"""..."""` are multiline; prefixes `r"..."` (raw), `b"..."`
  (bytes), `f"..."` (f-string, D-0026), and the combinations `rb`/`rf`; Python
  escape sequences (`\n`, `\t`, `\\`, `\uXXXX`, …).
- **Char:** **no dedicated char literal** (Python has none). `char` values come
  from string indexing/iteration (`s[0]`) or a `char(...)` conversion. A literal
  can be added later if a real need appears.
- **Line structure:** a newline terminates a statement; `\` is explicit line
  continuation; lines continue implicitly inside `()` / `[]` / `{}`; `;` may
  separate statements on one line.
- **Comments:** `#` to end of line (no block comments). A docstring is a
  triple-quoted string as the first statement of a module/def.
- **Indentation:** spaces; consistent within a block; mixing tabs and spaces for
  indentation is an error (Python 3 rule). Canonical formatter uses 4 spaces
  (D-0037).

**Boolean/null casing:** full **Python casing** — `True`, `False`, `None`
(resolves the earlier lowercase `true`/`false` from D-0002). These are keywords.

## D-0042 — Codegen backend: LLVM via inkwell
**Status:** Locked
Codegen lowers the type-checked AST to native code via **LLVM**, using the
`inkwell` safe wrapper (reaffirms D-0009; chosen over Cranelift and C-emit
because Zap's committed features — unwinding exceptions D-0007, coroutines
D-0008, SIMD arrays D-0011 — all reward LLVM, which the lighter backends do not).
Codegen lives in `crates/codegen` behind a small `Backend` trait so it stays
swappable; for the vertical slice the AST lowers directly to LLVM IR (a separate
typed SSA IR, `design/IR.md`, is deferred until features demand it), and objects
are linked with `cc`.
**Toolchain:** the dev host is Intel macOS, where Homebrew no longer ships LLVM
binaries, so LLVM builds run in a dev container (`.devcontainer/Dockerfile`:
Ubuntu + LLVM 18 + Rust; driven by `make docker-*`). Codegen is gated behind the
driver's `llvm` feature and excluded from the workspace `default-members`, so
host builds of the other crates work without LLVM. A single `Backend` trait keeps
a future Cranelift or native build a drop-in option.

## D-0043 — `set` built-in collection type
**Status:** Locked
`set[T]` joins `list[T]` and `dict[K,V]` as a built-in collection (the original
core types were list/dict/array; `set` is added here). `set` becomes a reserved
type keyword alongside the other container keywords.
**Literal syntax (brace disambiguation):** `{...}` is read as a **set** when it
holds comma-separated elements with no `key: value` pairs (`{1, 2, 3}`), and as a
**dict** when it holds `key: value` pairs (`{k: v}`). Empty `{}` stays a **dict**
(the conventional default; an empty set has no literal form yet — use a typed
declaration). The same rule extends to comprehensions: `{x for x in it}` is a set
comprehension, `{k: v for ...}` a dict comprehension.
**First increment (ticket #21):** literal, membership (`x in s`), `.add(x)`,
`len(s)`, and `for x in s` iteration (unordered). Implemented as a dict without
values — an open-addressing hash table sharing the dict's key hashing/equality
(per `design/RUNTIME-ABI.md`), GC-managed. Deferred, consistent with list/dict:
float elements, set algebra (union/intersection/difference), `.remove`/`.discard`,
and struct-element hashing.

## D-0044 — Structured-concurrency surface syntax
**Status:** Locked — resolves the surface left open by D-0033
The surface for structured concurrency (`design/0005-concurrency.md`,
OPEN-QUESTIONS).

**`scope:` is a block keyword**, not a library nursery object. A `scope:` block
lexically **owns** every coroutine `spawn`ed inside it: the dedent is the join
point (awaits all children), a child's uncaught failure cancels its siblings and
re-raises at the dedent, and an early parent `return`/`break`/exception cancels
and awaits children first. This keeps `spawn` (D-0013) the single launch verb
with no second `n.spawn(...)` spelling and no handle object. `scope` becomes a
reserved word used only as a block introducer.

**Detached coroutines are a v1 feature**, not deferred: `spawn detached <call>`
opts a coroutine out of **all** scope ownership — it is never awaited, and the
program does **not** block on it at exit; when `main` returns, a still-running
detached coroutine is terminated (Go goroutine semantics). This is the escape
hatch for true daemons (loggers, metrics flushers, background pollers). `detached`
is a **contextual modifier** valid only immediately after `spawn` (no new hard
keyword). A *bare* `spawn` with no enclosing `scope:` remains owned by the
implicit **root scope** and **is** joined at program exit — so the default is
leak-free and detachment is always explicit.

**Scope options are typed, not stringly:** `scope(on_error=OnError.Cancel | OnError.Wait)`
selects the failure policy (`Cancel` is the default — a failing child cancels its
siblings; `Wait` lets siblings finish). `on_error` takes an **enum**, not a
string, so a typo is a compile-time error (consistent with the interface and
decorator "catch typos at compile time" ethos, D-0016/D-0036).
`scope(timeout=<ms>, grace=5000)` cancels and drains owned children at a deadline
(see **D-0094** for the finalized model). Timeout values are a nonnegative integer
number of **milliseconds** — no unit suffix (`_` digit separators are fine, e.g.
`timeout=30_000`). Expiry uses the shared cancellation path, including nested
scope ancestry and parked channel wakeup; cleanup runs, then the scope raises
`ScopeTimeout` **to its caller**. Method/lambda spawn capture remains intended and
open in #475.

**Cancellation** is a `Cancelled` exception injected at cancellation points
(channel ops, `select`, sleep/timer/I/O waits, nested `scope:` boundaries,
`checkpoint()`), unwinding through ordinary `finally`/`with` cleanups (D-0027);
`Cancelled` derives from a `BaseException` root **outside** the `Exception`
hierarchy so a broad `except Exception:` does not swallow it.

**Rationale:** block-scoped ownership gives leak-free structured concurrency by
default (chosen over Go's `context`, D-0033) while explicit `spawn detached`
preserves real fire-and-forget coroutines for daemons; typed options and the
single launch verb keep the surface small and typo-safe.

## D-0045 — Concurrent sibling failures: aggregate exception
**Status:** Locked
When several coroutines owned by the same `scope:` fail concurrently, the scope
raises an **aggregate `ExceptionGroup`** carrying all of them (Python 3.11
`ExceptionGroup` / trio MultiError style), rather than re-raising the first and
dropping the rest. A single failure still propagates as that one exception. This
interacts with the exception hierarchy (**D-0029**): `ExceptionGroup` is defined
there, and `Cancelled` (D-0044), sitting outside `Exception`, is grouped
separately from ordinary failures. **Rationale:** losing concurrent failures
hides bugs; the Python-aligned aggregate matches our target users' mental model.

## D-0046 — Data-carrying enum variants (tier-3 sum types)
**Status:** Locked — resolves the tier-3 half left open by D-0018
Rust-style sum types, resolving issue #51 and finalizing `design/0006`.

**Declaration** extends the `enum` block with variants that carry **type-first,
named payload fields**; payload-free variants are allowed, and an enum may be
generic:
```python
enum Shape:
    Circle(f64 r)
    Rect(f64 w, f64 h)
    Unit
```

**Representation is per-enum, value-first (performance).** An enum with any
data-carrying variant is a **tagged value** `[ tag | payload-union ]` sized to
the largest variant, copied **by value** (D-0023) like a scalar — *not*
heap-boxed by default. The compiler inserts a **GC pointer only where needed**:
a field whose type is (mutually) **recursive** (to break the cycle — e.g. an AST
`Add(Expr l, Expr r)`) or whose payload is oversized. An all-bare-constant enum
stays the **tier-1 unboxed integer**. The two representations do **not mix**
within one enum: any payload-carrying variant makes the whole enum a tagged
value. (An unboxed small-enum/niche optimization beyond this is a later
follow-up.)

**Construction is qualified**, accepting named *or* positional args, like struct
construction: `Shape.Circle(r=3.0)` or `Shape.Circle(3.0)`.

**Pattern matching** reuses the D-0040 struct-pattern form on a qualified
variant, binding fields **positionally or by name** at the developer's choice:
`case Shape.Circle(r):` or `case Shape.Circle(r=r):`. Exhaustiveness checking
(D-0040) extends to variant coverage.

**Generics:** generic enums (`enum Option[T]: Some(T v); None`) are **reserved in
the grammar now** and implemented by monomorphization **when user generics land**
(D-0017 staging); non-generic data-carrying enums ship first. Shipping stdlib
`Option[T]`/`Result[T, E]` rides on that.

**Methods on enums** (a `def` with `self` in the enum block, like structs
D-0015) are **allowed** (dispatch via `match` internally), and may be staged
after the core.

**Rationale:** value-first representation serves the performance goal while
pointer-on-recursion keeps the motivating AST/tree types expressible; reusing the
`match` and struct-construction surfaces means no new syntax families, and
qualified variants keep exhaustiveness and name-collision handling clean.

## D-0047 — Operator overloading for user types
**Status:** Locked — resolves issue #52 (the "built-in types only" gap, D-0011)
User types overload operators via **dunder-named methods** resolved through a
fixed set of compiler-known, **structural, compile-time-checked** stdlib
operator interfaces (D-0016): `a + b` desugars to `a.__add__(b)` and compiles
**iff** `a`'s type satisfies `Add`. The dunder names are Python's — familiar to
the target audience, collision-free, and unambiguously "operator"; the operator
interfaces later double as **generic bounds** (D-0017, e.g. `def sum[T: Add]`).

**Overloadable — all operators:** arithmetic (`__add__ __sub__ __mul__
__truediv__ __floordiv__ __mod__ __pow__`), bitwise/shift (`__and__ __or__
__xor__ __lshift__ __rshift__`), unary (`__neg__ __pos__ __invert__`), equality
(`__eq__`), ordering (a single `__cmp__(self, other) -> int`), indexing
(`__getitem__ __setitem__ __delitem__`), membership/size (`__contains__`,
`__len__`), truthiness (`__bool__`), **call** (`__call__`), and repr (`__str__
__repr__`). Iteration (`__iter__`) is out of scope here — it is issue #53.

**Derivation:** `!=` derives from `__eq__`; the four relational operators
(`< <= > >=`) derive from `__cmp__`. Augmented assignment (`+=`, …) desugars
from the binary operator by default.

**`@derive` integration (D-0036):** `@derive(Eq, Ord, Hash, Repr)` generates the
corresponding dunders field-by-field, so the common value-object case needs no
hand-written operator. The compiler **warns** if a type defines/derives `Eq`
(equality) but no `Hash` and is then used as a `dict`/`set` key.

**Hash / struct keys:** `__hash__` (hand-written or `@derive(Hash)`) is what lets
a struct be a `dict`/`set` key — unblocking the struct-element hashing deferred
in D-0043. Invariant: `a == b` ⟹ `hash(a) == hash(b)`; `@derive(Hash)`
guarantees it (same fields feed `__eq__` and `__hash__`), a hand-written
`__hash__` must preserve it.

**In-place augmented assignment (`__iadd__`/`__isub__`/… family):** **implemented
in I#61.** `a OP= b` calls the in-place dunder `a.__iOP__(b)` when the LHS type
defines it (mutating in place and returning the operand, avoiding the fresh
allocation of the binary form); otherwise it falls back to deriving
`a = a.__OP__(b)`. The twelve dunders are `__iadd__`, `__isub__`, `__imul__`,
`__itruediv__`, `__ifloordiv__`, `__imod__`, `__ipow__`, `__iand__`, `__ior__`,
`__ixor__`, `__ilshift__`, `__irshift__`. Soundness per D-0023: a reference
struct shares identity so the in-place mutation is the intended visible effect,
and a value struct copies on assign so operating on the local is fine; built-in
scalar `+=` keeps its direct lowering.

**Rationale:** reusing structural interfaces keeps overloading compile-time
typo-safe (D-0016) and makes operators usable as generic bounds; dunder names
fit the Python audience and sidestep name collisions with ordinary methods.

## D-0048 — Iteration protocol for user types
**Status:** Locked — resolves issue #53
One protocol underlies `for`, comprehensions, generators, and built-in/channel
iteration, expressed as dunder methods on compiler-known **structural**
interfaces (D-0016, consistent with operator overloading D-0047):
- `Iterable` — `__iter__(self) -> Iterator` (produce a fresh iterator).
- `Iterator` — `__iter__(self) -> Iterator` (returns `self`) + `__next__`.

**Native `__next__` is comma-ok:** `__next__(self) -> (T, bool)` returning
`(value, has_more)` — exception-free, symmetric with the channel comma-ok receive
(D-0031), and needing no generics (the compiler infers element type `T` from the
signature).

**Python-compat path (support both):** a `__next__(self) -> T` that raises
`StopIteration` is **also** accepted as a first-class form, so ported Python
iterator classes run unchanged. The compiler selects the desugaring from
`__next__`'s return signature: `(T, bool)` → comma-ok; `T` → the loop catches
`StopIteration`. Comma-ok is the idiomatic/preferred form for new code;
`StopIteration` raises once at exhaustion (not per element), so with zero-cost
table-based EH (D-0007) its overhead is a single throw per loop.

**Desugaring:** `for x in obj` → `it = obj.__iter__(); while (v, ok) =
it.__next__(); ok: ...`. Comprehensions and the `in`-without-`__contains__`
fallback use the same protocol.

**Generators (`yield`, D-0026)** auto-implement `Iterator` via a
compiler-generated state machine and end by falling off / `return` (NOT `raise
StopIteration` — Python itself banned that, PEP 479); they compile to the
comma-ok protocol.

**Built-ins conform to the same protocol:** `list`/`dict`/`set`/`array`, `str`
(yields `char`), and channels (`for x in ch` wraps comma-ok receive until
closed); enums keep their compiler-generated iteration (D-0018). Iterators are
**single-pass reference objects** (D-0023); `__iter__` yields a fresh one.

**Rationale:** one protocol unifies all iteration; comma-ok is exception-free and
reuses the channel pattern; accepting `StopIteration` honors the Python-porting
pitch at negligible cost; structural interfaces keep it compile-time-checked and
let `Iterator` serve as a generic bound once user generics land.

## D-0049 — Tuple syntax & multiple return
**Status:** Locked — resolves issue #54 (the syntax half of D-0019)
Fixes the literal, access, destructuring, and multiple-return surface for tuples
(their existence was locked in D-0019); fully specified in `design/0006` §4.
- **Literals (Python-style):** `(a, b)` is a tuple, `(x,)` a 1-tuple (trailing
  comma required), `()` the empty/unit tuple, `(e)` is just grouping; a bare
  comma-list is a tuple in return/assignment context. Type: `(int64, str)`.
- **Element access `t[k]`** with a **compile-time-constant** `k`; the result is
  that position's static type. A non-constant index is a **compile error** (a
  tuple is heterogeneous, so the type must be static) — the key difference from
  `list[i]`.
- **Destructuring:** positional binding in decl and assignment, **nested**, with
  a single **`*rest`** capturing the remainder as a tuple: `auto (a, b) = t`,
  `auto (first, *rest) = xs`, `(x, y) = (y, x)` (swap).
- **Multiple return IS tuple return** — no separate mechanism (unlike Go):
  `def f(...) -> (A, B): return a, b` with `auto (x, y) = f()`. The comma-ok
  channel receive `v, ok = <-ch` (D-0031) and the iterator `__next__ -> (T, bool)`
  (D-0048) are **this same** destructuring — no special-casing.
- **Semantics:** immutable (no `t[0] = ...`), compared element-wise by `==`; a
  **by-value aggregate** (anonymous-struct layout, returned in registers when
  small / via `sret` when large, per RUNTIME-ABI), not a reference type, no heap
  header unless boxed into a reference slot.

**Rationale:** a Python-familiar surface; one tuple mechanism unifies multiple
return and comma-ok; compile-time-constant indexing preserves static typing of a
heterogeneous aggregate.

## D-0050 — Method-definition syntax: inline
**Status:** Locked — resolves the method-syntax question left open by D-0015
Methods are **inline `def`s** in the type's block, with an explicit,
source-untyped `self` first parameter (Python-style); the call form is
`receiver.method(args)`. There is **no implicit `self`** and **no
method-outside-the-body form**, except the optional `impl Interface for Struct`
block (D-0016, §2.3) for interface-conformance clarity. Enums use the same inline
form (D-0046). **No extension methods** — you cannot add a method to a type you
do not own; structural interfaces (D-0016) provide polymorphism instead. Chosen
over a Go-style receiver block or Rust-style `impl` block; the indentation-based
syntax keeps even large type blocks readable. **Rationale:** one place for a
type's behavior, unambiguous call sites, and functions-first simplicity (no OO
inheritance, D-0015).

## D-0051 — Value structs (opt-in value types)
**Status:** Locked — refines D-0023; resolves the "no value structs" perf gap
A **`value struct`** is a C-like **value type**: copied on assign/pass (value
semantics, no shared identity), laid out **inline** (stack / registers /
field-inline, no heap header), for hot, small aggregates. A plain **`struct`
stays a reference type** (D-0023, Python-object-like shared mutation) and remains
the **default**. Both kinds use the same inline methods (D-0050), structural
interfaces (D-0016), operator overloading (D-0047), and `@derive` (D-0036);
**neither has inheritance**. `value` is a **contextual modifier** before
`struct`.

**Interactions (documented limits):** a value struct stored in a collection or
placed behind an interface value is **boxed** (copied to the heap), so the win is
on locals, fields, arguments, and returns — not when erased behind an interface;
a value struct holding references is still GC-scanned; mutating a value struct
does not escape the copy the caller passed.

This closes the main performance gap flagged against D-0023 (everything
non-scalar being heap + indirection) **without reintroducing OO**. **Full OO
(classes + inheritance) stays deferred** (tracked separately, not under active
consideration). **Rationale:** gives "Python devs suffering from performance" a
zero-indirection aggregate for hot paths while keeping the reference-default
Python feel and the no-inheritance simplicity.

## D-0052 — Package manager & project environment (concrete design)
**Status:** Locked — resolves issue #56 (concretizes D-0035)
- **Manifest & lockfile:** **`zap.toml`** (author-edited: dependencies, versions,
  metadata) + **`zap.lock`** (tool-generated, committed) — TOML, Cargo-style.
- **Resolver: Minimal Version Selection (MVS, Go-style)** — the build uses the
  maximum of the required minimum versions; deterministic and reproducible by
  construction, no SAT solver, no dependency-hell. Versions are **semver**; a
  major version is the compatibility boundary.
- **Distribution: decentralized** — dependencies resolve to **VCS sources** (git
  URL + semver tag); **no mandatory central registry**. Integrity and reliability
  come from a **checksum database** (+ an optional official caching proxy),
  Go-style — the deliberate opposite of pip/PyPI (D-0035).
- **Imports:** source uses clean **logical names**; `zap.toml` maps each to a
  source + version (`jsonlib = { git = "…", version = "1.4" }` → `import jsonlib`).
  Aliasing `import jsonlib as j` (already in the grammar, D-0028); **multi-import**
  `import a, b, c` and grouped `import (a, b, c)` with per-item aliases (grammar
  reserved now; the multi-form's implementation may be staged).
- **`zenv` — project environment:** a project-local, **env-var-activated**
  environment (venv-simple: `source zenv/activate` sets vars, deactivation unsets
  them) that makes the **whole** project reproducible and self-contained. When
  activated it acts on **both** ecosystems:
  - **Zap deps** resolve/build against a **project-local module cache inside
    `zenv/`** (materialized from `zap.lock` via MVS), not a global cache.
  - **`zenv/venv/`** is a standard **Python venv** the embedded CPython (#27) is
    pointed at (`VIRTUAL_ENV` + a `ZAP_PYENV` var), so `import python 'numpy'`
    resolves there and `pip`/`uv` work normally inside it.

  Layout: `zap.toml`, `zap.lock`, `zenv/{activate, pkg (Zap cache), venv (Python)}`.
- **Scope:** `zap.toml` manages Zap packages; the Python side is the nested venv.
  Build/run command names are issue #58.

**Rationale:** MVS + lockfile + decentralized checksum-verified sources give
reproducible, dependency-hell-free builds with no central chokepoint; `zenv`
unifies Zap and Python reproducibility under the env-var simplicity of venv.

Concrete file formats, the MVS algorithm, the `zap1:` content-checksum scheme,
and the `zenv` activate-script are specified in `design/0009-packaging.md`
(issue #69). Two as-built notes: the resolver's module identity is
`(name, major)` so majors coexist; and `path` sources (local, Go-style) join
`git` sources so local multi-module dev and hermetic builds work offline.

## D-0053 — Toolchain & CLI
**Status:** Locked — resolves issue #58
One user-facing tool, **`zap`** (the compiler binary `zapc` becomes internal,
driven by `zap`). Core verbs:
- `zap run <file>` / `zap run` — run a single file (no manifest needed) or the
  project (via `zap.toml`).
- `zap build` — build the project to a native binary (output in `target/`, or
  under an active `zenv`).
- `zap test` — run tests, with a **watch mode** (`zap test --watch`) that stays
  running and **re-runs automatically** when a test file or target source
  changes. Tests use a **`@test` annotation suite** (built on decorators,
  D-0036); its concrete surface is designed with the test framework (issue #59).
- `zap fmt` — canonical formatter (D-0037).
- `zap new <name>` / `zap init` — scaffold a project (writes `zap.toml`).
- `zap add <dep>` / `zap remove <dep>` — manage dependencies (`zap.toml` + `zap.lock`).
- `zap zenv <create|…>` — manage the project environment (D-0052; activation stays
  `source zenv/activate`).
- `zap expand` — show `@derive`/decorator-generated code (D-0036).
- `zap ir <file>` — dump ZIR (diagnostic).
- **`zap mcp`** — start a built-in **MCP server for coding agents**: exposes tools
  to query the language reference/grammar, type-check/compile snippets and return
  diagnostics, run tests, and format — so AI coding agents can understand Zap
  syntax and get structured feedback. Its concrete tool-set is designed in its own
  ticket.
- `zap version` / `zap help`.

Both single-file and project modes are supported. `zap repl` / `zap doc` are
deferred. **Rationale:** one predictable CLI matching modern-language
expectations; watch-mode and a first-class MCP server directly serve fast,
agent-driven development — the way Zap itself is being built.

## D-0054 — Test framework & doc conventions
**Status:** Locked — resolves issue #59
- **`@test` annotation suite** (decorators, D-0036; run by `zap test`, D-0053):
  core set `@test`, `@before`/`@after` (per-test), `@before_all`/`@after_all`
  (per suite), `@parametrize(cases=[…])` (table-driven), `@skip`/`@skip_if`,
  `@expect_raises(ErrType)`. **Deferred:** `@bench`, `@timeout`.
- **Assertions:** a plain **`assert`** statement with **power-assert
  diagnostics** (on failure, print the expression and its operand values); no
  matcher DSL. `with expect_raises(ErrType):` (D-0027) for exceptions. `assert` is
  a language statement, **always evaluated** in v1 (a disable flag may come
  later).
- **Location & discovery:** BOTH inline `@test` functions (compiled only under
  `zap test`, **never in release binaries**) AND `*_test.zap` files / a `tests/`
  directory; `zap test` discovers every `@test` across the project.
- **Imports in tests:** tests resolve imports through the **same module
  resolution as the build** (`zap.toml`/MVS, D-0052) — no separate test path or
  `sys.path` hacks — so Zap avoids Python's chronic "tests crib on imports"
  problem.
- **Runner:** human output (per-test pass/fail, power-assert diffs, summary) plus
  **`zap test --json`** (structured, consumed by `zap mcp`); `--watch` re-runs
  **targeting only changed/affected paths** (incremental).
- **Doc conventions:** docstrings are the triple-quoted first statement (D-0041),
  **Python-familiar** (summary line + `Args:`/`Returns:`/`Raises:`, Google-style).
  `zap doc` generation is deferred (D-0053). **Doctests** (runnable docstring
  examples run by `zap test`) are a noted future direction, deferred.

**Rationale:** decorator-driven, Python-familiar tests; power-assert gives great
DX without a matcher library; unified module resolution removes test-import pain;
structured output + incremental watch feed the agent workflow.

## D-0055 — `zap mcp`: the agent interface (beyond tests)
**Status:** Locked — expands the `zap mcp` role (D-0053); built under issue #72
`zap mcp` is a first-class interface for AI coding agents, not merely a test
runner. It provides:
1. **Language understanding** — serves the language reference, grammar
   (`docs/GRAMMAR.ebnf`), semantics, and canonical examples so an LLM can produce
   correct Zap **even with little open-source Zap in its training data** (directly
   attacks the "new language, no corpus" problem).
2. **Fast checks without a build** — validate produced code via **parse +
   type-check only** (no LLVM build/link), returning structured diagnostics
   quickly, so an agent can iterate in a tight loop.
3. **Code graph / packed memory** — by default build and maintain a **compact
   semantic index** of the current codebase (symbols, types, signatures,
   references) that agents query to understand the project with **far fewer
   tokens** than reading raw files.
4. **Targeted testing** — run tests incrementally (only changed/affected paths),
   returning structured (`--json`) results.

Transport: stdio (and/or socket). **Rationale:** Zap is built by coding agents, so
the language shipping its own agent interface — reference, fast checks, a
token-efficient code map, and targeted tests — turns "no training corpus" from a
liability into a served capability and makes agent-driven development fast and
accurate.

## D-0056 — Standard library: scope & layout (v1)
**Status:** Locked — resolves issue #57
**Three tiers:**
- **Built-ins** (no import): `print`/`len`/`range`/`assert`; the scalar/container
  types including the new **`heap`** (D-0057); enums; structs + `@derive` (which
  replace Python dataclasses).
- **Bundled std** (ships with the toolchain, imported by **flat short name**, no
  `zap.toml` entry): `io`/`fs`, `os`, `sys`, `strings`, `math`, `random`,
  `collections` (deque/bisect/ordered), `sync` (D-0032), `time`/`datetime`,
  `json`, `csv`, `regex`, `logging`, `argparse`, `testing` (D-0054), `net`,
  `http` (server depends on the netpoller — built first), `crypto`
  (hashing/hmac/secrets; TLS rides with `net`/`http`), `process` (external
  programs), `zlib`/`gzip` (so a gzip-enabled http server is trivial), `signal`
  (D-0058), and `gc`/`runtime` (D-0058).
- **Official non-core packages** (we author, but distributed/versioned via the
  package manager D-0052, not bundled): `sqlite3`, `xml`, `email`, and future
  libraries.

**Crypto binary results (#647).** Actual byte buffers from `crypto.digest`,
`crypto.hmac_sha256_raw`, and `crypto.random_bytes` retain `Ty::Bytes` during IR
lowering, matching their semantic types and enabling binary operations on
inferred locals. Hex/text results retain string dispatch; streaming `_new`
results remain opaque context handles. The shared `ZapStr*` runtime ABI is
unchanged. A single crypto runtime-symbol result table classifies buffers,
text, handles, boolean verification and void updates; every call uses shared
emission after argument lowering/count coercion. Unregistered symbols fail
lowering rather than defaulting to `Ptr`. `crypto_byte_results` covers known answers, binary operations,
streaming finalization, and random-buffer boundaries.

**Imports** are whole-module with flat names (`import datetime` → `datetime.now()`);
Zap deliberately has **no `from X import Y`**. The **concurrency model**
(coroutines/channels/`scope`/`sync`) **replaces** Python's
`threading`/`multiprocessing`/`asyncio`/`queue` — no such modules (no GIL, so
coroutines give real parallelism). **Python interop (#27) is a rare relief valve**
for libraries like NumPy/PyTorch, not an everyday-stdlib crutch.

**Deferred:** `stats` (statistics), TLS (late-v1), `tar`/`zip` archives, profiling
(D-0058), and the official `sqlite3`/`xml`/`email` packages. **Rejected:**
`pickle` (RCE-unsafe, Python-specific — use `@derive(Json)`, or a future safe
`@derive(Binary)` codec); `ctypes` (= C FFI, deferred D-0012).

**Philosophy:** a curated **"Go-plus"** standard library — substantial enough that
reaching for Python interop is rare, without being a Python-sized kitchen sink.

## D-0057 — `heap` built-in collection
**Status:** Locked
A first-class **`heap[T]`** priority queue joins `list`/`dict`/`set`/`array` as a
built-in collection — a fundamental structure most languages bury in a library
(Python's `heapq` is functions-on-a-list). **Min-heap by default**; an optional
**comparator** selects max-heap or custom ordering (e.g. `heap[T](cmp=…)`).
Operations: `push`, `pop`, `peek`, `len`, iteration. `heap` becomes a reserved
type keyword; it is a GC-managed reference type. **Rationale:** an ergonomic
built-in heap is rare and genuinely useful — a small differentiator.

## D-0058 — Signals, runtime diagnostics & fault reporting
**Status:** Locked
First-class Unix signal handling, built on the concurrency runtime.
- **Mechanism:** scheduler threads mask signals; a dedicated signal thread's
  async-signal-safe handler writes a **self-pipe registered with the netpoller**,
  and a dispatcher coroutine delivers in normal context. The capability ships in
  every binary; there is **no global channel** — it wires up **lazily per
  subscription** (zero-cost when unused, apart from the default handlers below).
- **Surface:** `signal.notify(SIG…) -> channel[Signal]` (primitive);
  `@on_signal(SIG…)` built-in decorator (sugar); `scope(cancel_on=[SIG…],
  grace=<dur>)` — a signal **cancels the scope tree** for structured graceful
  shutdown, with an optional hard-exit deadline.
- **Delivery semantics:** a **small buffered** channel (default cap 1) that
  **coalesces** (drops on overflow — signals are edges, not a queue);
  **fan-out** to all subscribers; `notify` **takes over disposition** (suppresses
  the OS default; **no auto-exit** — the program decides when to exit after
  cleanup); **asynchronous** delivery; **catchable signals only** (a `Signal`
  enum; `SIGKILL`/`SIGSTOP` excluded).
- **Fault signals** (`SIGSEGV`/`SIGFPE`/`SIGBUS`/`SIGILL`): **not** via `notify` —
  a runtime crash handler prints diagnostics (faulting address/location,
  backtrace, offending coroutine) before terminating, for debugging.
- **Default diagnostic handlers** (ON by default, **opt-out at build time**,
  **user-overridable** — a user `notify`/`@on_signal` for that signal wins):
  `SIGQUIT` → dump coroutine stacks + scheduler/runtime summary; `SIGUSR1` →
  force GC + print memory stats (heap/live/free bytes, GC count, #coroutines) via
  Boehm `GC_gcollect`/`GC_get_*`; `SIGUSR2` → toggle the CPU profiler
  (**implemented**, ticket #78, `design/0014-profiling.md`).
- **Programmatic access:** a `gc`/`runtime` stdlib module — `gc.collect()`,
  `gc.stats() -> MemStats`, `runtime.coroutine_dump()` — the signals are just the
  zero-code operator path to these.
- **Diagnostic utility:** ship a small tool (a `zap` subcommand, e.g. `zap
  inspect <pid>`) that triggers the stack-dump / GC-stats / (later) profiling of a
  **running process by PID**, so operators need not hand-run `kill -QUIT`.

**Rationale:** graceful shutdown and production observability as first-class,
idiomatic features — critical for serverless/containers and usually an
afterthought elsewhere.

## D-0059 — `json`/`csv` design: three modes, `@derive(Json)`, substrate-first
**Status:** Locked — resolves the open design forks of issue #100 (scope set by
D-0056). Full spec: `design/0010-json.md`.

Native (no-CPython) data-format support, built **substrate-first**: a shared,
Zap-level, derive-driven **serde substrate** (phase 1, #131), then `json` on top
(phase 2, #100), then `csv` on top (phase 3, #132). The recursive-descent scanner
stays **per-format**; only the format-neutral **codec protocol** (the visitor/sink
+ derive-codegen for field-walk, key↔field mapping, defaults/optional, enum
tagging, error-path) and an **incremental byte-reader** are shared.

- **Three access modes, one scanner core per format:** (1) dynamic `json.Value`
  DOM with `loads`/`dumps`; (2) a compile-time **typed codec** via a new
  **compiler built-in `@derive(Json)`**; (3) a public pull `json.scan`/`Scanner`
  (an `Iterator[Token]`, D-0048). All three ship in json v1.
- **`@derive(Json)` is a compiler built-in** (like `Eq`/`Ord`, D-0036), generating
  `T.from_json(s)` / `self.to_json()` + internal `__from_json_value__` /
  `__to_json_value__` dunders — **no user generics, no user macros**. This
  **un-defers** the `@derive(Json)` noted deferred in `design/0006`. A generic
  `json.decode[T]`/`encode` sugar is a fast-follow once user generics (D-0017)
  land. `@derive(Csv)` is the phase-3 mirror.
- **`json.Value`:** a non-generic data-carrying enum (D-0046) with `Int(i64)` /
  `Float(f64)` **split** (not JS-style f64-only); out-of-`i64` integers decode to
  `Float` (lossy beyond 2^53); `BigInt`/`RawNumber` deferred.
- **Parser:** recursive-descent for v1 (SIMD stage-1 a later fast-follow behind the
  same token API). Typed decode is `Value`→struct DOM-map for v1 (token-direct
  path deferred).
- **Validation:** strict typed, lenient only on unknown keys
  (`@json(deny_unknown=true)` opts into strict); errors raise exceptions carrying
  byte offset + JSON path (typed `json.DecodeError` when typed exceptions land).
- **Seam type-hosting:** `json.Value`/`json.Token` register as **compiler-known
  enums** contributed by the module, extending the #121 seam — not via the
  deferred `PRELUDE_SRC` injection.
- **No grammar change:** the whole surface uses existing EBNF
  (`decorators`/`enum_def`/`field`; D-0036/D-0046).

**Rationale:** two fully-specced consumers (json, csv) up front justify designing
the shared substrate once rather than extract-on-second-use; a compiler built-in
derive delivers the high-value typed path (gates AWS #89, web #107) without waiting
on staged generics/macros; the `Int`/`Float` split honors Zap's static-typing
pitch; recursive-descent keeps v1 small and correct with a clean SIMD upgrade path.

## D-0060 — `zlib`/`gzip` compression: link system zlib on the #121 seam
**Status:** Locked — resolves issue #106 (listed under D-0056's bundled std)
**Surface** (flat modules on the #121 seam; `import zlib`/`import gzip` accepted,
not required, like `crypto`/`signal`):
- `zlib.compress(data: str|bytes, level?: int) -> bytes` — zlib wrapper (RFC 1950,
  Adler-32 trailer). `zlib.decompress(data) -> bytes`.
- `gzip.compress(data: str|bytes, level?: int) -> bytes` — gzip wrapper (RFC 1952,
  **ISO CRC32**, poly 0x04C11DB7). `gzip.decompress(data) -> bytes`.
- `zlib.crc32(data) -> int` / `zlib.adler32(data) -> int` — the running checksums
  (unsigned 32-bit widened to `int`), the natural known-vector surface and useful
  on their own. `level` defaults to `Z_DEFAULT_COMPRESSION` (-1).

**Implementation:**
- **Link the system zlib (`-lz`)** rather than vendoring a DEFLATE — the dev
  container / CI already install `zlib1g-dev` and it ships on macOS, so this adds
  a one-line link flag and a `#include <zlib.h>`, not a new package. zlib is the
  reference DEFLATE and gets both wrappers and both checksums provably correct
  (the gzip CRC32 is ISO CRC32, **not** the SSE4.2 CRC32C/Castagnoli).
- Thin `zap_zlib_*` / `zap_gzip_*` wrappers in one new `zaprt.c` banner section:
  single input buffer, output grown in ≤64 KiB steps so zlib's 32-bit `avail_out`
  never overflows; framing selected by `windowBits` (15 = zlib, 31 = gzip).
  Malformed/truncated input **raises a Zap exception** (`zap_panic`) instead of
  aborting. `str`/`bytes` are the same binary-safe `ZapStr`; results are `bytes`.
- **No grammar change** (plain module calls); **no new type** (reuses `bytes`).

**Deferred (noted in the PR):** streaming/incremental compression (a `compressobj`
/ `decompressobj`-style feed+flush handle), a raw-DEFLATE (headerless) mode,
dictionary presets, and inputs larger than ~4 GiB (zlib's 32-bit `avail_in`).
`tar`/`zip` archives stay deferred per D-0056.

**Rationale:** linking the ubiquitous, battle-tested zlib is the lowest-risk path
to correct DEFLATE + both framings + both checksums, keeps the runtime small, and
makes a gzip-enabled `http` server (D-0056) trivial; a hand-rolled DEFLATE would
be a large correctness/perf liability for no benefit when the library is already
present in every target environment.

## D-0061 — `net`/`http`: plaintext TCP + HTTP/1.1 on the netpoller
**Status:** Locked — resolves issue #104 (scope set by D-0056). Completes the
core stdlib (epic #79) and unblocks AWS #89, the web framework #107, and the
ecosystem clients #109. On the #121 module seam (per-crate `modules/{net,http}.rs`
+ one registry line each + a `zaprt.c` banner section).

- **Netpoller-first I/O.** Every socket operation is non-blocking and, on a
  would-block, PARKS the coroutine via `zap_netpoll_wait` (#80) — freeing its
  worker M — instead of blocking an OS thread or busy-polling. A descriptor is
  registered once per socket (edge-triggered R+W) at create time. `getaddrinfo`
  (DNS) and the no-netpoller fallback `poll()` are bracketed by
  `zap_enter/leave_blocking`. No GC pointer is held across a park: input payloads
  are copied to `malloc` scratch and results built as GC objects only at the end
  (the precise collector traces a parked G by its shadow chain, not runtime C
  locals) — the same rule the `process`/`zlib` sections follow.
- **Opaque handles.** Sockets (`ZapSock`) and parsed HTTP messages
  (`ZapHttpResp`/`ZapHttpReq`) are opaque GC objects typed as `bytes` in sema
  (the `crypto`/`sync` handle pattern) — no new `Ty`, no prelude struct. Sockets
  are ATOMIC (fd + non-GC poller desc); message handles are PRECISE (trace their
  `ZapStr` fields).
- **Surface.** `net`: `listen`/`accept`/`connect`/`recv`/`send`/`port`/
  `close_conn`/`close_listener` (socket close is not `close` — that token is the
  reserved channel-close). `http` client: `get`/`post`/`request` →
  `status`/`body`/`header`; `url(host,port,path)` builds a URL (there is no
  int→str conversion yet). `http` server: `read_request` →
  `method`/`path`/`req_header`/`req_body`, and `respond(conn,status,body)`. The
  server's accept loop and per-connection coroutines are written in Zap
  (`spawn handle(conn)`; `bytes` is a valid parameter type), so concurrent
  connections ride the scheduler.
- **No grammar change:** the whole surface is generic module calls (existing EBNF).

**Deferred (follow-ups):** TLS/`https` (needs a separate un-defer decision; gates
#109 — rejected at runtime for now); UDP; HTTP keep-alive (client sends
`Connection: close` and reads to EOF); chunked transfer-encoding; custom client
request headers / content types; and server graceful-shutdown/SIGTERM semantics
(belong to the web framework #107). **Rationale:** a correct, concurrency-safe
plaintext core on the existing netpoller is the smallest piece that unblocks the
ecosystem epics; TLS is the one genuinely large follow-up and is best decided on
its own.

## D-0062 — `tls`: link system OpenSSL for an `https` client on the netpoller
**Status:** Locked — resolves issue #147 (un-defers the TLS/`https` follow-up
left open by D-0061). Unblocks `http` `https://`, the ecosystem clients #109
(Postgres/Redis/… reuse raw TLS), and TLS serving in the web framework #107. On
the #121 module seam (per-crate `modules/tls.rs` + one registry line each + a
`zaprt.c` banner section); no grammar change (all generic module calls).

- **Backend: link system OpenSSL/LibreSSL via `-lssl -lcrypto`** (chosen over
  rustls-ffi and platform-native). It mirrors the existing C-runtime precedent —
  the regex (`-lpcre2-8`) and zlib/gzip (`-lz`, D-0060) sections already link a
  system C lib with pure-C glue in `zaprt.c` — so it is the smallest integration
  and composes directly with the netpoller via a non-blocking socket BIO. The CI
  image (Ubuntu 24.04, the `make docker-test` container) ships **OpenSSL 3.0.13**;
  `libssl-dev` is added to `.devcontainer/Dockerfile` next to `zlib1g-dev`.
  rustls-ffi (memory-safe, but a bundled static Rust lib in every Zap binary +
  heavier build wiring) stays the fallback if OpenSSL ever proves too fragile.
- **OpenSSL 1.1 vs 3.x drift: avoided.** The client uses only the unified
  post-1.1.0 API — `TLS_client_method`, `SSL_CTX_set_min_proto_version`
  (floor TLS 1.2), automatic library init, `SSL_set1_host` + `SSL_VERIFY_PEER`
  for built-in chain **and** hostname verification, `SSL_set_tlsext_host_name`
  for SNI — which is identical across OpenSSL 1.1.1 and 3.x (and LibreSSL ≥ 2.7),
  so no version `#ifdef` is needed. (The test-only self-signed server cert is
  generated with the 3.0 `EVP_RSA_gen`/`X509_*` API — the CI target — not shipped.)
- **Netpoller-first, like D-0061.** The fd is non-blocking; `SSL_connect`/
  `SSL_read`/`SSL_write` returning `WANT_READ`/`WANT_WRITE` PARK the coroutine via
  `zap_sock_park` (`zap_netpoll_wait`), freeing the M — never a blocked OS thread
  or busy-poll. Same GC discipline: fd/pd/`SSL*` are extracted to C locals (the
  `SSL*`/`SSL_CTX*` are OpenSSL-heap, not GC), payloads copied to `malloc` scratch,
  GC results built only after the last park.
- **Security (on by default, non-negotiable).** `tls.connect` verifies the
  certificate chain against the system trust store (`SSL_CTX_set_default_verify_paths`)
  AND the hostname (`SSL_set1_host`); expired/invalid/mismatched/self-signed certs
  are rejected (the handshake raises a catchable Zap exception). Skipping
  verification is a separate, loudly-named entry point, `tls.connect_insecure`,
  never the default.
- **Surface.** `tls`: `connect(host,port)` (verify on) / `connect_insecure(host,port)`
  (verify off) → a TLS conn handle (opaque GC object typed as `bytes`, ATOMIC, the
  `net`/`crypto` handle pattern); `send(conn,data)->int64`; `recv(conn,n)->str`;
  `close_conn(conn)` (`close` is the reserved channel-close token, so sockets spell
  it `close_conn`, as in `net`). An internal `_test_server()` starts a background
  self-signed TLS echo server and returns its port — a test scaffold for the
  deferred server, not stable API. `http` gains `https://` (default port 443) by
  routing the client transport through `tls` with verification on; `http://` is
  unchanged.

**Deferred (follow-ups, noted in the PR):** TLS **server**/accept as public API
(web-fw #107); ALPN / HTTP/2; client certificates (mTLS); custom CA-bundle
configuration; session resumption / 0-RTT; OCSP stapling. **Rationale:** a
verified `https` **client** is the one piece that gates the whole ecosystem epic
#109; the server and the advanced-TLS knobs are independent follow-ups.
## D-0063 — `yaml`: subset YAML serde on the shared JsonValue DOM / derive core
**Status:** Locked — resolves issue #149. Promotes `yaml` out of the ecosystem
epic (#109) into core, the third serde format on the shared substrate
(design/0010-json.md §4b) after json (D-0059) and csv. On the #121 module seam
(`modules/yaml.rs` per crate + one registry line each + a `zaprt.c` banner
section). D-0062 is the sibling TLS decision (#147).

- **Reuse `JsonValue`, do not invent `YamlValue`.** YAML 1.2 is a JSON superset,
  so the common data model (null/bool/int/float/string, sequences, mappings)
  maps onto the existing compiler-known `JsonValue` prelude enum. `yaml.load(str)
  -> JsonValue` and `yaml.dump(JsonValue) -> str`. A YAML-only `YamlValue` is
  revisited only if a YAML-specific type ever forces it.
- **Backend: hand-written YAML→JSON transpiler in C, NOT libyaml.** `yaml.load`
  is `__yaml_loads(s) = __json_loads(yaml._to_json(s))`: a small subset scanner
  (`zap_yaml_to_json` in `zaprt.c`) rewrites the supported YAML surface to a JSON
  string, which the existing json tokenizer + prelude tree builder parse into
  `JsonValue`. This keeps all DOM-layout knowledge out of the runtime (the json
  precedent) and keeps core dependency-free. Linking system libyaml (`-lyaml`,
  the zlib `-lz` precedent) was the documented fallback; it proved unnecessary —
  a correct pragmatic subset was cheap on top of the json DOM. `yaml.dump` is a
  Zap prelude block-style writer (`__yaml_dumps`) over `JsonValue`.
- **`@derive(Yaml)`** mirrors `@derive(Json)`: `to_yaml(self) -> str` (a block
  mapping; scalars reuse the json inline formatters, nested structs recurse and
  are re-indented by `zap_yaml_indent`) + a `from_yaml(str) -> Self` associated
  function that parses to the DOM and reuses the SAME `{Struct}__from_json_value`
  decode core as `@derive(Json)`/`@derive(Csv)` (the shared `DeriveFieldKind`
  walk). A struct deriving both Json and Yaml emits that shared core once (deduped
  by name). Flat + nested structs; collections/enums remain the follow-up.
- **Supported v1 subset:** block mappings + block sequences (incl. `- key: val`
  sequences of mappings), JSON-compatible flow `{...}`/`[...]`, plain / single- /
  double-quoted scalars, `#` comments, a single leading `---`/`...` document
  marker, and YAML 1.2 core scalar resolution (null/~, true/false, int, float,
  else string). Indentation is spaces only — a tab in indentation is rejected
  (`zap_panic`).
- **No grammar change:** the whole surface is generic module calls plus the
  existing `@derive(...)` syntax (existing EBNF).

**Deferred (follow-ups, noted in the PR):** anchors & aliases (`&`/`*`),
custom/explicit tags (`!!`), merge keys (`<<`), multi-document streams (beyond a
single `---`), block scalars (`|` literal and `>` folded — deferred together
because the line-oriented comment-strip/trim preprocessor is lossy for their
significant whitespace), complex/non-string mapping keys, and directives. These
are the long tail of the spec; v1 targets the 95% config/data-file use.

## D-0064 — User-defined generics (first vertical) + `Option[T]`
**Status:** Locked (ratified by the user; resolves the four open calls of
`design/0012-generics.md` — two of them *overriding* the design's
recommendation, see the product-calls section). Implements epic I#154 slices
0–3. Builds on D-0017 (generics: yes, monomorphized, staged).

Lands the first end-to-end vertical:

- **M — monomorphization**, as a ZIR-stage AST→AST pass between sema and codegen
  (`crates/ir/src/mono.rs`): each used `(generic, concrete-type-args)` becomes a
  specialized concrete `struct`/`enum` with a mangled name, deduped by that name;
  the result is a generics-free module the existing lowering and codegen handle
  unchanged. **M1 — generic bodies are checked at *definition*** (Rust model),
  type parameters opaque (unconstrained ⇒ universal operations only).
- **GC metadata — no new mechanism (verified).** A monomorphized instance is a
  plain concrete `StructLayout`, so codegen's existing per-struct `ZapTypeDesc`
  emission (`emit_struct_desc`: pointer-field byte-offsets from the real LLVM
  layout, `t.is_ptr()`) produces a correct precise-GC pointer map per instance
  with zero new code. Generic enums ride the tier-3 tagged-struct path. Confirmed
  against the real codegen and exercised by `generics_gc_stress` under both
  collectors. GC correctness for generics is a theorem. (The niche `Option[ref]`
  layout below preserves this: a niche optional is a single nullable pointer
  slot, traced by the same pointer-map mechanism, with `None`/null a non-root.)
- **S — subscript syntax**, plus the two additive grammar extensions (optional
  interface bound on a type parameter; call-site `type_args`). A postfix `[...]`
  parses as a uniform node (a multi-element list becomes a tuple index) and is
  classified as type-arguments vs `__getitem__` by name resolution.
- **I — inference** = argument-driven unification + an expected-type sink at
  `let`/`return` (and variant/struct fields); explicit type args otherwise. The
  bare `None` literal resolves to `Option.None` of the sink's instance.
- **O — `Option[T]`** = a prelude generic enum `Some(T)`/`None`, with
  `is_some`/`is_none`/`unwrap` (raises a string message on `None` until typed
  exceptions, I#12) / `unwrap_or`, consumed by methods + `match` (no `?` operator
  — Zap is exception-based). This lifts the D-0046 enum-method deferral for
  tier-3 enums. **Representation (ratified end state): the niche layout —
  `Option[T]` for a reference/pointer `T` is a single nullable pointer (`None` =
  null, `Some(p)` = `p`), one word, no tag; a value-type `T` with no available
  niche falls back to the tagged tier-3 form.** So Option is niche-where-possible,
  tagged-otherwise. **Implementation staging:** the first vertical (PR #156)
  shipped the **tagged** representation everywhere; the **niche** layout for
  reference `T` landed in the immediate follow-up PR as a lowering-side change
  (two new ZIR ops `NullConst`/`IsNull`; construction, `match`, and binding
  special-cased — `Some(p)` = `p`, `None` = null, one word, no tag), observable
  behaviour unchanged. Niche-eligible = an `Option`-shaped tier-3 enum (two
  variants, one payload-free, one single field) whose payload is a non-nullable
  pointer (a reference type that is neither a value struct nor another tier-3
  enum — the latter would let a null niche collide with the outer `None`). The GC
  property is preserved with no collector change: a niche optional is one pointer
  slot, null is skipped by `zap_mark_push`/`zap_is_heap_ptr`, and a live value is
  traced by its own object-header descriptor via the existing per-struct pointer
  map.
- **X — built-in collections stay special-cased** (`list`/`dict`/`set`/`array`/
  `heap` are not rewritten as user generics) **for now, and are re-skinned as
  user-generic *surface* types in a scheduled post-core-generics slice** (methods
  lowering to the same tuned runtime entry points — a source-compatibility change,
  not a runtime rewrite). Not "special-cased permanently."
- **First end-to-end vertical = slices 0–3** (generic *types* + `Option[T]`).

**Open product calls — resolved (two overriding the design's recommendation):**
(1) interface bounds deferred to slice 5 — *as recommended* (the flagship needs
none; grammar + schema carry the optional bound now); (2) check-at-definition —
*as recommended*; (3) **OVERRIDE** — built-in collections are re-skinned as user
generics in a scheduled post-core-generics slice (the design recommended leaving
them special-cased permanently); (4) **OVERRIDE** — `Option[T]` uses the niche
layout (nullable-pointer niche for reference `T`, tagged fallback otherwise) as
the end state (the design recommended staying tagged indefinitely).

**Deferred (follow-ups, filed as later slices):** the **`Option` niche layout**
(the immediate next PR for this epic); generic *functions*/methods with their own
type parameters (`def map[T, U]`, `Option.map[U]`); interface-bound enforcement
(reuse the I#84 conformance checker, static dispatch); `json` dropping
`@default`-as-optionality via `Option[T]` (#100/#133); AWS SDK optional modeling
(fork O-a, #89); `Result[T, E]`; the built-in-collection re-skin as user-generic
surface; code-size layout-sharing; typed `UnwrapError` (rides I#12).

**Update — slice 4 (I#161): generic functions & methods.** User-defined generic
*free functions* and *methods with their own type parameters* are implemented,
extending the same `crates/ir/src/mono.rs` erasure pass (slices 1–3 handled
generic *types*). A generic free function is emitted on demand per call site as
a mangled concrete function (`identity$int64`), deduped by mangled name; a
generic method is specialized per call and **attached to its owner's concrete
declaration** (`Box$int64.repack$str`, `Option$int64.and_opt$str`), dispatching
through the ordinary struct-method path — so the GC-metadata-free theorem still
holds (every instance is a concrete struct/function; no collector or `zaprt.c`
change). Call-site inference follows Fork I: argument-driven unification (now
also over list-literal element types and function types) plus the expected-type
sink on the return type at `let`/`return`/argument positions; explicit call-site
type arguments (`identity[int64](...)`, `recv.m[U](...)`) are the fallback and
are classified in sema/mono by name resolution. Bodies are still checked
unconstrained at definition (M1); interface bounds remain slice 5. `Option[T]`
gains one callback-free generic method in the prelude, `and_opt[U]` (Rust's
`Option::and`). **Deferred — higher-order generic functions** (`map[T,U]`,
`Option.map[U]`/`and_then` taking a `fn(T)->U`): these need first-class function
*values*, which Zap does not yet have — `parse_type` does not parse the
`fn(...)->...` type (grammar reserves it as `func_type`), `resolve_ty` rejects
`Type::Func`, and there is no function-address / indirect-call lowering. That is
a separate feature (parser + codegen + calling convention; GC is already safe
because a code pointer fails `zap_is_heap_ptr`) filed as a follow-up, not part
of the monomorphization epic. Generics machinery is otherwise complete for
callback-free generic functions and methods.

**Update — slice 5 (I#164): interface bounds on type parameters.** A type
parameter may carry a single interface bound (`def announce[T: Display](...)`,
`struct Box[T: Display]`); the bound syntax already parsed (slice 0) and is now
enforced. Enforcement is split along the pipeline (sema checks; `ir` re-derives
types and does not consume sema output, so it cannot call sema's conformance
checker): **sema** does the check-at-definition (Fork M1) — inside a bounded body
a method call on a value of `T: Iface` resolves through `Iface`, typed by the
method's return and rejecting any method the bound does not declare, before any
instantiation; an unbounded `T` stays opaque as in slices 1–4. **ir/mono**
enforces *satisfaction* at each concrete instantiation using the same I#84
structural-conformance rule (re-derived at the AST level), emitting a clear
compile error that names the type, bound, and parameter when the argument does
not conform. Because the body is monomorphized, a bound-method call substitutes
the concrete type and lowers to a **direct call** of that type's method — **static
dispatch, no fat pointer / vtable** (interface *values* keep the dynamic
fat-pointer path from I#84; this is Rust's `impl Trait` vs `dyn Trait` split). The
GC-metadata-free theorem is unchanged: every instance is a concrete struct/enum
with its existing per-struct descriptor, so no collector or `zaprt.c` change.
Operator-dunder interfaces (D-0047) are valid bounds for free (`def sum[T: Add]`)
since they are ordinary interfaces. Tested under both collectors
(`tests/golden/generics_bounds*.zap` + `mono`/`sema` unit tests). **Deferred
(follow-ups):** multiple bounds per parameter (`T: A + B` — grammar carries a
single bound), where-clauses, bounds referencing other type params, and
satisfying an operator bound with a *primitive* argument (only user structs/enums
carry the methods a bound checks today).

**Update — slice 6 (I#165), the final generics slice:** the prelude optional
enum is **renamed `Option[T]` → `Optional[T]`** (Python `typing.Optional`;
see D-0065) and `json` is migrated off `@default`-as-optionality onto
`Optional[T]`. This closes the core generics epic's planned slices; the two
remaining items — the built-in-collection re-skin as user-generic surface, and
higher-order generic functions (which need first-class function values, I#163) —
are tracked separately, not part of I#154. Everywhere this document says
`Option[T]`/`Option.Some`/`Option.None` above, read `Optional[T]` etc.; the old
name has no code left behind it (clean rename, no alias).

**Update — post-core slice (I#170): built-in-collection re-skin DECLINED; they
stay special-cased (resolves Fork X / override #3).** Fork X and open-call
override #3 above scheduled the built-in collections (`list`/`dict`/`set`/`heap`)
to be *re-skinned* as user-generic *types* flowing through monomorphization,
reducing their special-casing. On investigation this was **declined**: a genuine
re-skin cannot be done without a **performance regression** and would **increase**,
not reduce, special-casing. Three facts decide it:
- The collections are **type-erased**: one hand-tuned C-runtime implementation per
  (collection × machine-word kind `Elem = i64|ptr|struct`), shared across all
  element types (`design/RUNTIME-ABI.md` §Collection layout). Monomorphization is
  the opposite — **type-reifying**: a distinct concrete struct per full type-arg
  tuple. Forcing collections through it would wrap the runtime handle in a struct,
  adding a heap object + pointer indirection to every collection = a regression;
  the alternative (register them as generics so mono mangles `list[int64]` →
  `list$int64`, then un-mangle back to `Ty::List(Elem::I64)` in `resolve_ty`) is
  *more* special-casing, not less.
- Their operations are **compiler intrinsics** lowered to `zap_list_*`/`zap_dict_*`
  runtime calls; there is no Zap surface for raw GC-word-buffer push/get/hash, so a
  prelude `struct list[T]` (modelled on `Optional[T]`) cannot carry real method
  bodies. Adding such surface would itself be a user-facing change — forbidden by
  I#170's overriding zero-surface-change constraint.
- **Interop is already achieved** without a re-skin: to the `mono` pass a
  collection is an ordinary `Type::Named` node, so `subst_with`/`unify`/
  `apply_subst` already substitute, unify, and infer `list[T]`/`dict[K,V]`/`set[T]`
  uniformly with user generics (`rename_type` keeps the head and rewrites only the
  args — the one deliberate built-in line). Verified end-to-end:
  `firstOf[T](list[T]) -> Optional[T]`, `list[Pair[int64,str]]`,
  `Box[list[int64]]`, `Optional[list[int64]]` all compile and run.

**Shipped instead (the only safe, zero-regression change):** the one residual
non-uniformity — a `dict`/`set` *literal* passed directly to a generic function
did not drive type-parameter inference, though a `list` literal did — is fixed by
mirroring the list-literal inference for `Expr::Set`/`Expr::Dict` in
`crates/ir/src/mono.rs`. This is purely additive (the case previously errored with
"cannot infer type argument"); typed-variable and explicit-`[K,V]` forms already
worked. No surface, behavior, or performance change to any existing program; all
existing collection + generics goldens pass unchanged under both collectors. New
golden `tests/golden/generics_collections_interop.zap` exercises the interop.
Collections remain special-cased by design — Python-surface fidelity and the tuned
runtime both win over internal uniformity (I#170's own guidance). See
`design/0012-generics.md` §"Built-in collections".

## D-0065 — Optional type spelling (`Optional[T]`) + `json` optionality via `Optional[T]`
**Status:** Locked (ratified by the user 2026-10-20). Implements epic I#154
slice 6 (the final slice); builds on D-0064 (generics + the optional enum) and
D-0059 (`json`/`@derive(Json)`). Tickets: I#165 (parent I#154); touches
`design/0012-generics.md` and `design/0010-json.md`.

- **Name — `Optional[T]`, not `Option[T]`.** The prelude optional enum is spelled
  `Optional[T]`, matching Python's `typing.Optional`, which Zap's Python-literate
  audience already knows. This is a **clean rename** of the shipped `Option[T]`
  (D-0064): no deprecated alias, no `Option` left in the compiler, prelude,
  stdlib, goldens, or grammar (pre-public, so no compatibility surface to keep).
  **Unchanged:** the `Some`/`None` variant names, the tagged + niche
  representations (D-0064 #4), `match` ergonomics, the bare-`None` inference sink,
  and every method (`is_some`/`is_none`/`unwrap`/`unwrap_or`/`and_opt`). Only the
  type identifier changes.
- **Deferred (fast-follow, separate):** `T | None` union sugar for a fully
  Pythonic optional spelling — out of scope here; `Optional[T]` is the canonical
  form it would desugar to.

- **`json` optionality — `Optional[T]` fields, not `@default`.** `@derive(Json)`
  now recognizes an `Optional[T]` field as *the* way to say "nullable / may be
  absent": on **decode**, an absent key *or* an explicit JSON `null` yields
  `Optional.None`, and a present non-null value yields `Optional.Some(decode(v))`;
  on **encode**, `Optional.None` **omits the key entirely** (chosen over emitting
  `null`) and `Optional.Some(v)` emits the inner value. Omit-on-`None` is the
  serde/pydantic default idiom (`skip_serializing_if`/`exclude_none`) and makes
  the absent↔`None` and present↔`Some(v)` round-trips clean and symmetric. The
  inner `T` may be any type the derive already supports (scalars, `str`, nested
  `@derive(Json)` struct). This supersedes design/0010-json.md's original
  "a `@default` field is optional" language (that approach predated the optional
  type).
- **`@default` stays, as an honest fallback.** The `=` literal field default
  (D-0038) keeps its plain meaning — *absent → this fallback value* — matching
  Python dataclass / pydantic defaults. Only the **abuse** of `@default` to encode
  absent-vs-present optionality is removed; `Optional[T]` now carries that.
- **`csv`/`yaml` (D-0063):** their `@derive` surfaces never implemented
  `@default`-as-optionality either (the derive was lenient for v1), so no
  migration is forced; an `Optional[T]` field in a `csv`/`yaml` derive is a
  natural follow-up on the same shared derive core, not part of this slice.

## D-0066 — `Atomic[T]` lock-free atomics (I#176)

The `sync` module gains `Atomic[T]`, the atomic cell deferred by I#103. Zap is
truly parallel (no GIL), so it needs real atomics; the model follows Go
(`sync/atomic`, `atomic.Int64`) / Rust (`AtomicUsize`) / C++ (`std::atomic<T>`),
not Python (which has none).

- **A built-in generic struct, not a special ZIR type.** `Atomic[T]` ships in
  the parser prelude as `struct Atomic[T]: T value` — one machine-word cell — and
  rides the merged user-generics (I#154) monomorphizer unchanged. `Atomic[int64]`
  specializes to a concrete `Atomic$int64 { int64 value }`, `Atomic[Node]` to
  `Atomic$Node { Node value }`, etc. This was chosen over a dedicated
  `Ty::Atomic(..)` (the `Ty::Chan` route) specifically because it makes the
  **GC claim fall out for free**: the pointer variant is an ordinary struct whose
  single field is a reference, so codegen's existing per-struct `ZapTypeDesc`
  emission marks that word as a traced pointer and the precise Immix collector
  scans the atomic slot as a normal GC root — **zero new GC/runtime-C code** (the
  issue's explicit "reuse the ZapTypeDesc machinery" requirement). A scalar cell
  gets a pointer-free descriptor, exactly like `Mutex`/`WaitGroup`/`Once`.

- **Methods are compiler intrinsics, lowered inline — not real functions.**
  `load()`, `store(v)`, `add(n)`/`sub(n)` (integer cells only), `swap(v)`, and
  `compare_and_swap(old, new) -> bool`. A method call on an `Atomic$…` receiver is
  intercepted in lowering (by mangled-name prefix, the same trick the niche
  `Optional` layout uses) and emitted as one of four new ZIR nodes — `AtomicLoad`,
  `AtomicStore`, `AtomicRmw{Add,Sub,Xchg}`, `AtomicCas` — which codegen turns into
  LLVM `load atomic` / `store atomic` / `atomicrmw` / `cmpxchg` on the cell. The
  prelude's placeholder method bodies are never emitted (skipped in lowering); they
  exist only so the generic surface type-checks and monomorphizes like any other
  generic struct. `add`/`sub` yield the **new** value (Go's `Add` semantics);
  `swap` yields the old; a failed CAS yields `False`.

- **Access width.** Each op uses an integer access type derived from the cell:
  `int64`→`i64`, `int32`→`i32`, `bool`→`i8` (LLVM atomics need a byte-sized,
  power-of-two integer, so the `i1` cell is accessed as a byte), a reference
  payload→pointer-wide word via `ptrtoint`/`inttoptr`. `add`/`sub` are rejected at
  lowering for a `bool` or reference cell. Zap has no scalar `uint32`/`uint64`
  today, so the surface is `int64`/`int32`/`bool` + a reference variant; because
  the integer atomic ops are sign-agnostic, unsigned scalars (when added) will flow
  through the same `i32`/`i64` paths with no new work.

- **Memory ordering: sequentially consistent by default, not cornered.** Every
  operation is `seq_cst`. The ordering is carried as an explicit `AtomicOrd` field
  on each ZIR node (only `SeqCst` today), and `cmpxchg` carries separate
  success/failure orderings, so relaxed / acquire-release can be added later (a
  `load_relaxed()`-style method or an ordering argument) without changing the node
  shapes or breaking any existing caller.

- **Unblocks** lock-free / CAS collections (deferred until this landed), lock-free
  fast paths in the runtime, and general concurrent counters/flags.

## D-0067 — First-class function values (`fn(...)->...` + closures, I#163)

Functions become values: a `fn(T, U) -> R` type can be a parameter, field, local,
return value, or generic type argument, and `lambda`s and bare function names are
first-class values. This unblocks higher-order generics (`map`/`filter`/`and_then`
over a callback), the web-framework handler model (I#107), and functional
ergonomics generally. The grammar already reserved `func_type` and `lambda`
(D-0026); this wires them end-to-end.

- **Surface.** The type is `fn(T, U) -> R` (grammar `func_type`); `->` and the
  return type are omitted for a `void` result. `fn` is a **soft keyword**,
  recognized only in type position (immediately followed by `(`), so it stays a
  usable identifier everywhere else — no lexer keyword, no churn to existing code.
  Closure literals are Python's untyped `lambda a, b: expr` (D-0026); a lambda's
  parameter types are **inferred from the expected function type** at its use site
  (a `fn(...)` parameter, an annotated binding, or a `fn(...)`-returning `return`),
  so no new annotated-lambda syntax is introduced. A lambda with no such context is
  a compile error (its parameter types are unknowable).

- **Representation: a single-pointer closure record.** A function value is ONE
  pointer to a GC-heap record `{code_ptr, captures…}` — the code pointer at offset
  0, the captured free variables after it. This was chosen over a two-word
  `{code, env}` fat value (the interface route, D-0016) precisely because a single
  pointer **stores without dangling**: in a struct field, a list element, a return
  slot, or another closure's environment, exactly like any reference. The calling
  convention is uniform: the code pointer has signature `R (ptr env, args…)` and
  the record passes itself as `env`.
  - A **bare top-level function** used as a value gets a compiler-generated wrapper
    `f$fnval(env, args…)` that ignores `env` and calls `f(args…)`; its captureless
    record is `{wrapper_ptr}`.
  - A **lambda** is lifted to a top-level function `$closure$N(env, params…)` that
    reloads its captured free variables from `env` (fields 1..); the record is
    `{lifted_ptr, cap0, cap1, …}`. Free variables are found by a walk of the body
    (minus the lambda's own parameters and any inner binders); only names actually
    bound in the enclosing scope are captured (globals / top-level functions /
    builtins resolve directly in the lifted body).
  - Calling a value (`f(x)`, `fns[i](x)`, `op.f` then call) loads the code pointer
    from the record and does an indirect call (an `invoke` on an exception edge),
    passing the record as `env`.

- **GC claim falls out — zero new runtime C.** The environment is an ordinary typed
  heap object allocated through the existing `zap_alloc_typed` with a precise
  `ZapTypeDesc` emitted exactly like a struct's: the code-pointer word is marked
  non-pointer (never traced) and each captured word by its ZIR pointer-kind, so the
  precise Immix collector scans captured pointers and ignores captured scalars. A
  captureless record's static-looking code pointer and any captured top-level/static
  references are non-heap and skipped by block-registry validation — the same
  property interfaces (D-0016) and value structs (D-0051) already rely on. This
  means **no changes to `zaprt.c` / the collector** (it reuses the per-struct
  descriptor machinery, FORK 4a).

- **ZIR.** `Ty::Func(idx)` (physically a pointer; `idx` interns the signature in
  `Module::fn_sigs`) carries the call signature lowering needs; `Inst::ClosureMake`
  builds a record and `Inst::CallClosure` is the indirect call. The environment is a
  forced-non-inline reference struct (shared identity, outlives its creating frame).

- **sema stays permissive.** Function types and lambdas type as `Unknown`
  (assignable both ways), and a call through a function-typed local is accepted; the
  lowerer owns the real signatures (via `Ty::Func`). Full bidirectional checking of
  function values is a clean follow-up.

- **Scoped follow-ups (noted, not built).** Wiring `Optional.map`/`and_then` and a
  list `map[T,U]` combinator onto this; generic inference of a type argument that
  appears only under a function-value parameter (today the callback's result type
  `U` is given explicitly, `apply[int64, int64](...)`); a value-struct captured
  **by value** into an environment (scalars and references work); a function value
  retrieved from a collection re-attaching its signature without a `fn(...)`
  annotation (collections store a uniform reference word).

## D-0068 — C-style bitfields on value structs (I#173)
**Status:** Locked — refines D-0051; resolves issue #173

A `value struct` field may carry an explicit **bit width**, so several small
fields pack into one machine word (C bitfields), e.g. a 3-bit mode next to a
5-bit level in a single byte. This is deliberately the *easy* half of the old
D-style proposal: **references are excluded**, so there is never a packed pointer
and the precise Immix collector (D-0009/#48) stays entirely out of it — a
bitfield value struct is just a non-traced scalar blob with masked accessors.

- **Surface.** A width annotation `: N` after the field name, type-first and
  C-recognizable (grammar `field ::= … type IDENT [ ":" INT ] [ "=" expr ]`). The
  colon is unambiguous: a field line never opens a block.

  ```
  value struct Pixel:
      int64 r : 5
      int64 g : 6
      int64 b : 5     # 16 bits total -> one u16 backing word
  ```

- **Where.** `value struct` only — a bitfield on a plain (reference) `struct`
  (D-0023) or on an `annotation` is a compile error. **All-or-nothing in v1:** if
  any field declares a width, **every** field must, and the whole struct becomes
  a single packed scalar. (Mixing bitfields with full-width fields — which would
  reintroduce pointer fields and GC scanning — is a documented follow-up.)

- **Field types.** A fixed-width signed integer (`int32`/`int64`, incl. `i32`/
  `i64`), `bool`, or a **tier-1** (unboxed-integer) `enum`. Never a
  pointer/reference (plain struct, `str`, a collection, a nested value struct, an
  interface, a function value), float, `char`, `bigint`, or a tagged (tier-3)
  enum.

- **Widths & limits.** `1 <= N <= type_bits` (`int32`→32, `int64`→64, `bool`→1; a
  tier-1 enum's width must hold every discriminant — a negative discriminant is
  rejected, since the field is unsigned). The widths of one struct sum to **≤ 64**
  (one machine word).

- **Layout & endianness.** Fields pack **least-significant-first in declaration
  order** into the smallest unsigned backing integer (`u8`/`u16`/`u32`/`u64`) that
  holds the total width — which is therefore `sizeof(struct)`. Bit order within
  the word is fixed (LSB-first); the word itself is an ordinary integer, so its
  byte order is the platform's. Values round-trip within a program; a
  cross-platform binary layout is **not** guaranteed in v1.

- **Overflow / sign.** A signed integer bitfield stores the low `N` bits and reads
  back **sign-extended** (round-tripping `[-2^(N-1), 2^(N-1)-1]`); `bool` and enum
  read back zero-extended. A write keeps the low `N` bits (C truncation); a
  literal default that does not fit is a compile error.

- **Lowering (ZIR/codegen).** `StructLayout.packed` carries the backing `Ty` and a
  per-field `{offset, width, signed}` slice. Codegen emits the LLVM type as a
  one-element `{ iN }` aggregate (so it stays pointer-addressed like any struct —
  `StructNew`, value-copy, array element, generic type arg all work unchanged);
  `FieldGet`/`FieldSet` become shift+mask on the backing integer instead of a GEP.
  Because the aggregate holds no pointers, its GC type descriptor has zero pointer
  offsets — **no `zaprt.c` / collector changes**, reusing the per-struct
  descriptor machinery exactly as D-0067 and D-0051 do.

**Rationale:** gives the "Python devs who need performance" audience C-style
compact layouts for flags/headers/packed records on the zero-indirection value
struct, while the no-pointer restriction keeps the precise GC and the language's
simplicity untouched. Mixed bitfield/full-width layout and a guaranteed
cross-platform binary encoding are the scoped follow-ups.

## D-0069 — Context-manager protocol & `with` (I#185)
**Status:** Locked — implements the general `with`; refines D-0027. Design:
`design/0015-context-managers.md`.

The `with` statement drives a Python-faithful context-manager protocol. A
manager is any struct defining `__enter__`/`__exit__`:

- `__enter__(self) -> T` runs on entry; `with M as x:` binds `x` to its result.
- `__exit__(self)` runs on **every** exit path: normal fall-through, early
  `return`/`break`/`continue`, and panic/unwind.

```
with lock:              # acquire on entry, release on every exit
    critical_section()

with open(path) as f:   # f closed on every path
    use(f)
```

- **Desugaring.** `with M as x:` ≡ `x = M.__enter__()` then a finally-equivalent
  running `M.__exit__()`. Lowered by `lower_with_items` (ir) with a per-`with`
  landing pad for the unwind path (re-raises, chaining to any enclosing manager)
  and a **cleanup stack** walked by `return`/`break`/`continue` for the
  structured early exits. The *same* cleanup stack now also runs a
  `try ... finally`'s `finally` on early exit — closing the gap I#22 deferred, so
  `finally` is Python-faithful (always runs) and `with lock:` cannot leak the
  lock on an early `return`.
- **Multiple managers.** `with a, b:` nests left-to-right (`b.__exit__` before
  `a.__exit__`); supported in v1.
- **Std guard.** Prelude `Lock`/`RLock`/`Semaphore`/`Condition` implement the
  protocol (`__enter__` acquires & returns self, `__exit__` releases), so
  `with lock:` is the idiomatic mutex guard; `as NAME` is optional there.
- **v1 divergence from CPython (deferred).** `__exit__(self)` takes no
  `(exc_type, exc_value, tb)` triple and **cannot suppress** an in-flight
  exception. The 4-arg suppressing form is a fast-follow gated on typed-exception
  introspection (I#12). The surface (`with M as x:`, `__enter__`/`__exit__`)
  stays Python-faithful; only `__exit__`'s arity is reduced for v1.

**Rationale:** resource cleanup that is correct on *all* exit paths is the whole
point of `with`; the lock guard is the motivating case and demands that early
`return` release the lock, which forced the shared cleanup-stack fix for
`try/finally` too. The reduced `__exit__` arity keeps the lowering a plain
re-raise until typed exceptions land.

## D-0070 — Versioning & stability policy: the 1.0 contract (I#205)
**Status:** Locked (policy). Full text: `docs/STABILITY.md`. Epic I#158, Area D2
(`design/0016-platform-1.0.md`). Resolves the "compatibility / versioning
promise" open question. Blocks the spec-freeze audit (#206) and release
process (#207).

Zap is distributed as **one artifact** (the `zap` toolchain) carrying **one
version** — the `Cargo.toml` `[workspace.package] version` — interpreted as
**SemVer 2.0.0**. That single version covers several surfaces (language, stdlib
API, CLI, package format, runtime/ABI); **a release's bump is the maximum change
class over all surfaces**.

- **MAJOR** = a backward-incompatible change to any *Stable* surface; **MINOR** =
  a backward-compatible addition (new syntax/API/flag, additive package-format
  change, **graduating an Experimental surface to Stable**, **deprecating** a
  Stable surface, new target platform, MSRV bump); **PATCH** = bug fixes with no
  surface change and no behavior change to a *conforming* program.
- **Whole-program AOT (D-0021) ⇒ no public runtime ABI.** There is no dynamic
  linking of separately-compiled Zap code, so Zap promises **no** stable binary
  ABI between units or compiler versions, no value-layout stability (D-0051/
  D-0067/D-0046), and no `zaprt` ABI. The runtime/ABI tier is **Internal**; the
  eventual C FFI (D-0012) ships **Experimental** and is governed from then on.
- **0.x regime (now):** no compatibility promise; `0.MINOR` may break anything,
  `0.x.PATCH` is fixes-only. The `SPEC.md` `Version:` header (0.2) is an
  editorial doc revision; the `Cargo.toml` version is authoritative and is
  reconciled with the spec at 1.0.
- **1.0 promise (Go-1 style):** a program using only **Stable** surfaces that
  compiles and runs under 1.0 keeps compiling and behaving across all of 1.x.
  **Does not cover** Experimental/Internal surfaces, bug-for-bug behavior
  (fixable at PATCH/MINOR), performance/GC/scheduler timing, unspecified
  iteration order, value layout, diagnostic text, `zap ir`/`zap expand` output,
  or security/soundness fixes (narrow carve-out allowed at MINOR/PATCH with
  sign-off).
- **Three tiers** (Stable / Experimental / Internal). Experimental is opt-in and
  signposted (target: `[experimental]` allow-list in `zap.toml` + compiler warn);
  Experimental→Stable graduates at a MINOR with a DECISIONS entry. The
  authoritative 1.0 Stable set is produced by the **#206 audit** — `docs/STABILITY.md`
  §4.4 provides only a provisional seed (e.g. `threading`, Python interop,
  arrays/SIMD, C FFI, WASM provisionally Experimental).
- **Deprecation:** a Stable surface is deprecated (warning) at a MINOR, stays
  functional for the rest of the MAJOR, and is removed only at the next MAJOR.
  **Breaking changes** require a design doc + DECISIONS entry + migration guide,
  with a `zap fix`-style migrator as a goal.
- **Editions: rejected for 1.0.** Whole-program AOT removes the cross-edition
  linking problem editions solve; Zap uses the Go model (evolve within a MAJOR,
  batch breaks into the next) plus a lightweight `language = "<major>.<minor>"`
  pin in `zap.toml` instead.
- **Windows for support window:** the §3 promise holds for all of 1.x; the prior
  MAJOR gets security/critical fixes for ≥12 months after the next MAJOR.
  Platform matrix: adding a platform is MINOR, dropping one is MAJOR. MSRV/LLVM
  build-dep bumps are MINOR (not a Zap-program surface).

**Rationale:** "make it public / call it 1.0" is a *commitment*, not a feature —
it needs a precise, enforceable definition of what stable means, what can still
change, and how surfaces graduate, before any installer is shipped or anything
is labeled 1.0. The single-version/max-bump scheme keeps the promise simple for
a single-artifact toolchain; the whole-program AOT model lets us refuse an ABI
promise honestly; the Go-1 compatibility model + language pin gives long-lived
stability without the complexity of editions.

## D-0071 — Stable error-code scheme: `ZAPnnnn` (I#157/F1, #239)
**Status:** Locked (scheme). Full text: `docs/STABILITY.md` §3.4. Issue #239,
epic I#157 (`design/0021-tooling.md`, Area F). Extends the 1.0 contract (D-0070).

Every diagnostic the toolchain emits carries a stable **error code** `ZAPnnnn`
(`ZAP` + four digits). The code — **not** the message prose — is the
machine-parseable identifier tools and coding agents key off; this is the
pillar-1 (agent-authorship) multiplier that lowers iterations-to-green (the
metric I#178 charts).

- **Format:** `ZAP` + four digits. Severity (error/warning) is **orthogonal** —
  carried by the diagnostic, not encoded in the code. The leading digit is the
  category/phase: `1` lex, `2` parse, `3` name/resolution, `4` type, `5` semantic/
  declaration validation, `6` back end (lowering/IR/codegen); `0` reserved for
  meta (ICE, not-yet-implemented), `7`–`9` reserved for future phases.
- **Single source of truth:** a registry in the `zapc_diag` leaf crate generates
  both the typed constants used at emission sites (so an unregistered code cannot
  be emitted) and the data behind `zap explain <ZAPnnnn>`. A code identifies a
  *condition*, so one condition reuses one code across sites.
- **Surfaced** on the CLI (`error[ZAPnnnn]: …`), on the `zap mcp` / `--json`
  structured diagnostic surface (additive `code` field), and via `zap explain`.
- **Stability (see STABILITY.md §3.4):** a Stable code's documented *meaning* is
  stable within a MAJOR; adding a code is MINOR; renumbering/removing/repurposing
  is breaking (MAJOR, after deprecation); **message text is never stable**;
  broadening is MINOR, narrowing is breaking for the old code; severity relax is
  MINOR, tighten is a language-level change.
- **Tier:** codes ship **Experimental** and may be **renumbered freely pre-1.0**;
  the **#206** spec-freeze audit ratifies the Stable set at 1.0.

**Rationale:** stable, phase-categorised codes give agents and tooling (LSP, MCP,
fmt) a durable handle on each diagnostic that survives the message-wording churn
the stability policy explicitly disclaims — turning "parse the prose" into "look
up the code", which is both more reliable for machines and cheaper to document.

## D-0072 — Queryable frontend analysis API (I#157/B1, #228)
**Status:** Locked (API shape). Full text: `design/0022-analysis-api.md`. Issue
#228, epic I#157 (`design/0021-tooling.md`, Area B). The LSP's foundation,
consumed by #229/#230.

The frontend gains a **position-keyed analysis surface** so tooling (LSP first)
can ask "what symbol / type / definition is at this offset?" — the state the
resolver already computes but the compile path discards.

- **New leaf crate `zapc_analysis`** over `syntax`/`sema`/`sema_types`/`diag`
  (no LLVM), not a `sema` module: keeps the type checker's surface to the two
  gated hooks below and gives #229 one in-process dependency. It exposes
  `Analysis::analyze(src)` with `diagnostics`, `type_at`, `symbol_at`,
  `definition_of`, `references`, `document_symbols`, and a `LineIndex`
  (byte-offset ⇆ 1-based line/col; the LSP UTF-16 layer is #229's, layered on
  top). Keyed by byte offset ([`Pos`]).
- **Resolver-state retention is additive and opt-in.** `zapc_sema::Checker`
  gains an `Option<AnalysisFacts>` sink, set only by the new
  `analyze_queryable` entry point; `check`/`analyze` leave it `None`, so the
  **compile/codegen path records nothing and is byte-identical** (proven by
  unchanged goldens + a parity test). The facts are three span-keyed tables
  (expr types, local use→def resolutions, binding def sites); globals are
  AST-derived in `zapc_analysis`. The only always-on change is `declare` taking
  a `Copy` def span it drops when not analysing.
- **Incremental:** recompute-per-document now (the #129 crate split makes it
  cheap); the immutable-per-snapshot, offset-keyed handle is the boundary a
  future salsa-style engine memoizes behind, so #229/#230 are insulated.
  Cross-file go-to-def widens the def target to `(file, span)` in a later
  multi-module ticket (D-0052/#190) without changing query names.
- **Def-span fidelity:** name-precise for name uses / `for` targets / match
  captures; construct-granular for `let`/params/comprehension vars (no name-only
  sub-span on those declarations yet) — tightens for free when A1 (#225) lands.
- **Tier:** a new public tooling surface, shipped **Experimental** under the
  stability policy (D-0070), like the LSP/`zap doc` surfaces.

**Rationale:** the diagnostics half of an LSP was already free (`check_source`);
the interactive half needs the resolver's decisions kept, not recomputed. Making
retention an opt-in side-table — rather than restructuring the checker — buys
symbol/type/def-at-position without touching compile behaviour or perf, and the
leaf-crate boundary matches #129's intent (frontend reused by tooling without
LLVM).

## D-0073 — Interactive REPL via in-process LLVM JIT (I#159)

**Status:** Accepted. Full text: `design/0025-repl.md`. Issue #159, epic I#157
(`design/0021-tooling.md`). The engine behind runnable doc examples (#234) and
the web playground (#238).

`zap repl` executes typed Zap incrementally by reusing the real
`AST → ZIR → LLVM` backend through an **in-process MCJIT**, so REPL code has
**identical semantics to compiled code** (same monomorphization, value-struct
layout, exceptions, Immix GC). A second (tree-walking) interpreter was rejected
as a divergent second semantics — fatal for a "prototype == ship" language. ORCv2
was the stated target, but inkwell 0.5 exposes no safe ORCv2, so MCJIT is used
(functionally equivalent for the REPL: multi-module engine + process-symbol
resolution, both validated).

- **One process, one session, one thread.** The C runtime is compiled once and
  loaded `RTLD_GLOBAL` (its GC constructor runs; MCJIT resolves `zap_*` by
  `dlsym`); the runtime's one "main mutator" means a session runs on one thread.
- **Session state = runtime slots.** The ZIR has no module-level globals, so a
  `let` value persists in a fixed table of 64-bit runtime slots (stable
  addresses; a pointer slot is GC-rooted once via the existing `zap_gc_add_root`,
  evacuation-safe, and conservatively scanned under Boehm). Each entry re-declares
  prior bindings from their slots in a generated `__repl_entry` prelude and writes
  new/updated values back on success; a failed entry commits nothing. Shadowing
  assigns a fresh slot. The only new frontend surface is the
  `__repl_load_*`/`__repl_store_*` lowering intrinsics (sema builtins → runtime
  slot calls) and the `__repl_entry*` top-level landing pad.
- **Errors.** A compile error is reported with its `ZAPnnnn` code and the entry
  discarded; an uncaught runtime exception is caught by `zap_repl_call` (a
  non-exiting boundary; `zap_uncaught` unwinds back to the loop) and reported.
  The session survives both.
- **MCJIT + dynamic TLS.** MCJIT cannot emit a correct access to the `__thread`
  `zap_shadow_head` in the dlopen'd runtime, so codegen gains a JIT mode that
  fetches the shadow-head slot via a `zap_shadow_head_addr()` runtime helper; the
  AOT object path is unchanged.
- **First cut.** Persists `int*/bool/char/str/bytes/list/dict/set` and
  explicit-typed bindings (incl. `fn(...)` closures); defers float bindings,
  cross-entry top-level `def`/type persistence, and readline history. Host-only
  (needs LLVM), gated on the `codegen` feature like `zap run`. New dep: `libc`
  (dlopen/dlsym) in codegen.

## D-0074 — Doctests: runnable docstring examples (`zap test --doc`, I#234)

**Status:** Accepted. Issue #234, epic I#157 (`design/0021-tooling.md`); builds on
the doc model (#232, D-0041), the Markdown renderers (#233), and the REPL
`Session` engine (#159, [[D-0073]]).

A *doctest* is a runnable example embedded in a docstring. `zap test --doc [path]`
extracts the examples from every docstring under `path`, runs each through the
REPL `Session` (so a documented example has **identical semantics to compiled
code** — no second executor), and checks it against its stated behaviour; exit is
non-zero on any failure.

- **Example format.** A fenced ```` ```zap ```` block in a docstring is one
  example, run as a single `Session::eval` in a fresh (reset) session — so
  examples are self-contained and isolated. Other fences (```` ```text ````,
  bare) are ignored. Expectations are `#` comment directives, keeping the block
  plain, Python-readable, agent-writable Zap:
  - `# => <text>` — one expected **stdout** line (repeat, in order; a trailing
    bare expression auto-prints, so `x` + `# => 42` asserts the value);
  - `# raises: <substr>` — must raise an uncaught runtime exception whose message
    contains `<substr>`;
  - `# error: ZAPnnnn` (or `# error[ZAPnnnn]`) — must fail to compile with that
    stable code ([[D-0071]]);
  - no directive — must compile and run clean (stdout unchecked).
  `# =>` and `# raises:` combine; `# error:` is exclusive. A block that produces
  diagnostics with no `# error:` directive is a **failure** (a broken example).
- **Self-contained first cut.** The session does not load the enclosing module's
  declarations (cross-entry/module `def`/type persistence is deferred, I#252), so
  an example cannot yet call the item it documents by name — it uses literals,
  builtins, stdlib imports, and inline logic. Auto-loading the module into the
  doctest session is the obvious follow-up.
- **Slots into `zap test`.** Doctests ride the test verb (cf. `pytest
  --doctest-modules`): `zap test --doc` discovers **all** `.zap` under the path
  (docstrings live in ordinary source), reuses the `@test` framework's report
  model (human + `--json`), and is codegen-gated like `zap run`/`zap repl`. The
  `@test` path, its report API, and all goldens are unchanged.
- **Stdout capture.** The landed `EvalOutcome` surfaces `diagnostics` +
  `runtime_error` only, and auto-print is emitted as `print(...)`, so the runner
  captures each snippet's output at the **file-descriptor** level (redirect fd 1,
  `fflush`, restore) rather than changing the shared engine or the REPL loop. New
  dep: `libc` on the driver (codegen-gated). No grammar change — example syntax is
  docstring *content*, not Zap grammar.

## D-0075 — Web playground over the shared `Session` engine (`zap playground`, I#238)

**Status:** Accepted. Issue #238, epic I#157 (`design/0021-tooling.md`, Area E);
builds on the REPL `Session` engine (#159, [[D-0073]]) and shares its single
executor with doctests ([[D-0074]]). See `design/0026-playground.md`.

A *playground* is a local web UI — a code editor and a **Run** button — that
compiles and runs a Zap snippet and shows its stdout, auto-printed value, and
`ZAPnnnn` diagnostics. `zap playground [--host H] [--port N] [--open]` serves it;
it is codegen-gated like `zap run`/`zap repl`.

- **Server-side native execution, not WASM.** The WASM target is post-1.0
  (B-track, deferred), so a snippet cannot run in the browser. It runs
  server-side through the same REPL `Session` (`Session::eval` → `EvalOutcome`),
  so a snippet behaves exactly as it will when compiled — **one semantics**, no
  second executor. Rendering of stdout/value/diagnostics is therefore identical
  to `zap repl`/doctests.
- **The server runs untrusted native code, so the sandbox is the design.** Each
  `POST /run` executes in **its own short-lived worker process**
  (`zap playground-worker`), never a shared in-process `Session`: the engine is
  in-process MCJIT with a process-global GC/scheduler and hard faults abort the
  process, so a shared session would leak state across requests and a single
  segfault would take the server down. One process per request gives isolation
  (no shared heap/GC/scheduler) and crash containment (a segfault/abort kills
  only the worker; the parent reports `crashed` and stays up) by construction.
- **What the first cut enforces.** In the worker: `RLIMIT_CPU`, `RLIMIT_AS`,
  `RLIMIT_FSIZE`, `RLIMIT_CORE=0`, and an `alarm` armed *after* startup so the
  per-snippet time budget does not include compiling the runtime / JIT init. In
  the parent: a wall-clock backstop (snippet budget + startup grace), an output
  byte cap (truncate runaway prints), a cap on concurrent workers (HTTP 429 on
  excess), and process-group kill on timeout. Terminating signals map to stable
  statuses (`timeout`/`output_limit`/`crashed`). Binds `127.0.0.1` by default; a
  non-loopback bind warns.
- **What it defers (deployment hardening).** True filesystem/network/syscall
  **denial** needs an OS sandbox (seccomp / namespaces / a container), which is
  platform-specific and depends on the deployment target; it is a **gating
  follow-up before any exposure beyond loopback**. The residual risk (a dev-host
  worker can still read the filesystem / reach the network) is accepted only
  because the deliverable is a not-yet-deployed, loopback-only local server.
- **Minimal surface, no new crates.** A hand-written `std::net` HTTP/1.1 server
  (four routes) over the in-house `json` codec — no web framework — mirroring the
  LSP/MCP decision to take no transport dependency; the sandbox uses `libc`
  (already a codegen-gated driver dep) for `setrlimit`/`alarm`/`setpgid`/`kill`;
  the editor is one embedded HTML page with vanilla JS (no bundler/framework).
  Share links are stateless — the snippet is encoded in the URL fragment
  (`#code=`), so nothing is stored server-side. No grammar change (a tool, not
  language). Deferred follow-ups: OS-sandbox hardening, incremental output
  streaming (SSE over the worker pipe), a cached precompiled-runtime image for
  faster per-request startup, and a server-side share/paste store.

## D-0076 — 1.0 spec-freeze: the ratified Stable set (I#206)
**Status:** Locked (ratification). Full audit: `design/0029-spec-freeze.md`.
Epic I#158, Area D1. Ratifies the Stable set that D-0070 (`docs/STABILITY.md`
§4.4) and D-0071 (§3.4) explicitly deferred to this audit; recorded in
`docs/STABILITY.md` §4.4 (surfaces) and §7.3 (platforms), with `SPEC.md`
Appendix B carrying the matching per-section tier.

The #206 audit surveyed the implemented tree (not just the docs) and fixes the
authoritative 1.0 Stable / Experimental / Internal classification. Posture is
**conservative**: promising stability is a one-way door (frozen for all of 1.x),
so a young/under-specified/platform-fragile surface ships **Experimental** and
graduates later at a MINOR (§4.2) — cheap and additive — rather than being
frozen prematurely.

- **Language Stable:** declarations/scalars/strings/collections/tuples, structs
  (+`value struct`)/interfaces/enums (tier-1 **and** tier-3 sum types), operators
  + overloading + `@derive(Eq,Ord,Hash,Repr)`, control flow + iteration protocol,
  functions/lambdas/generators + default/keyword/variadic args, **first-class fn
  values + closures** (D-0067), comprehensions/slicing, exceptions, `with`/context
  managers (D-0069), modules/imports/visibility, regex, decorators core +
  `@derive(Json)`, **generics incl. bounds** (D-0064), coroutines/channels/
  `select`/`scope:`/`spawn detached`, sync primitives + **`Atomic[T]`** (D-0066),
  `Optional[T]` (D-0065). **Experimental:** numeric arrays/SIMD, `set`, C-style
  bitfields (D-0068), Python interop, `@derive(Yaml)`/`@derive(Csv)`;
  `__iadd__`-family in-place ops remain **not-yet**. **Internal:** value/memory
  layout, representation, GC/scheduler (§1.5).
- **Stdlib (per module) Stable:** built-ins (+`heap`), `io`/`fs`/`os`/`sys`/
  `process`, `math`/`random`, `time`/`datetime`, `collections` extras (`deque`/
  `bisect`/`oset`), `sync`, `json`, `logging`/`argparse`, and the `signal`/`gc`/
  `runtime` **API** (their dump/stats *format* stays Internal). **Experimental:**
  `crypto`, `net`/`http`/`tls`, `zlib`/`gzip`, `yaml`, `threading` (migration-
  only). `strings`/`collections`/`csv`/`testing` are **not** importable modules
  (str methods / built-in containers + the extras / `@derive(Csv)` / the `@test`
  framework) — SPEC §18 corrected.
- **CLI Stable:** `run`/`build`/`test`/`fmt`/`new`/`init`/`add`/`remove`/`zenv`/
  `explain`/`doc`/`version`/`help` (verbs + flags + exit codes). `mcp`/`lsp`/
  `repl`/`playground` verbs Stable, protocols Experimental; `ir`/`expand`/
  `inspect` output Internal; all `--json` schemas Experimental until stabilized;
  `playground-worker` Internal. (SPEC §17 gained the shipped `doc` and `lsp`.)
- **Package format Stable:** `zap.toml`/`zap.lock`/MVS resolver/`git`+`path`
  sources/checksum DB (D-0052). **Experimental:** the registry read path (D-0027,
  read-only, no publish path).
- **Diagnostics:** the `ZAPnnnn` scheme + `zap explain` + the documented
  **meaning** of every non-meta code (117 of the 119 registered) are **Stable**;
  the meta band (`ZAP0001` ICE, `ZAP0002` not-yet-implemented) is excluded;
  message **text** is never stable (§3.4).
- **`zap fmt`:** the tool + its parse-preserving/idempotence contract + exit
  codes are **Stable**; the **exact byte-for-byte rendering is not promised** (the
  Go/`gofmt` stance — it may be refined within 1.x; a reformat is never a semantic
  change).
- **Platform matrix (§7.3):** Linux x86-64 and Windows x86-64 **tier-1 (Stable,
  CI-gated)**; macOS x86-64 **tier-2 (Supported — not yet CI-gated, NOT under the
  §3 promise)** pending a macOS CI runner + PR#259, graduating at a MINOR; macOS
  arm64 **experimental** (#260 JIT unwinder); aarch64-Linux **experimental**
  (#223 multicore GC race); WASM post-1.0. "Stable" for a platform means the §3
  promise is continuously CI-verified there.
- **Freeze mechanism:** recorded as the `docs/STABILITY.md` §4.4 Stable-set table
  + §7.3 matrix and `SPEC.md` Appendix B per-section tiers + "frozen for 1.0"
  markers; post-freeze additions stay additive (MINOR, §1.2/§4.2). The editorial
  `SPEC.md` `Version:` is bumped to `0.3` (freeze revision) but **not** stamped
  `1.0` — that reconciliation happens at the 1.0.0 **release** (#207), per §2;
  the toolchain version in `Cargo.toml` stays authoritative.

**Rationale:** "call it 1.0" is a commitment, not a feature (D-0070). The audit
makes the commitment concrete and honest: it freezes the mature core, keeps the
young/fragile surfaces Experimental so they can still evolve, refuses to promise
what CI does not continuously verify (macOS), and follows gofmt in not freezing
exact formatter output — all one-way doors chosen deliberately and reversibly
only in the additive direction.

## D-0078 — `concurrent` lock-free collections (I#184)
**Status:** Accepted (ratified 2026-10-27). Design: `design/0031-concurrent-collections.md`.
Builds on D-0066 (`Atomic[T]`) and D-0008/D-0009/I#120 (precise moving Immix GC);
governed by D-0076 (1.0 freeze) as an additive **Experimental** surface.

Zap ships a small `concurrent` stdlib module of CAS-based collections built on
`Atomic[T]`. **Phase 1** (this ticket) is exactly a lock-free **`Counter`** (over
`Atomic[int64]`) and an unbounded **MPSC `Queue[T]`** (Vyukov: a multi-producer
atomic tail-swap + publish, a single-consumer `pop`). The concurrent map is
**deferred** (a later sharded-`Mutex`+`dict` ticket, not a CAS map); MPMC is a
later additive phase.

- **Reclamation: lean on the GC.** Nodes are ordinary heap structs; reclamation
  is the tracing moving collector — **no** hazard pointers, epochs, write/read
  barriers, or pinning, and **no new `zaprt.c`/`zapgc`/runtime-C code**. This is
  sound because evacuation is stop-the-world, mutators observe moves load-at-use,
  and tracing reclaims only the unreachable (design §4).
- **Binding invariant (§4.4):** every stored pointer lives in a **genuine
  reference slot** (an `Atomic[T]` cell or a struct field) — never packed, tagged
  or integer-cast — so precise tracing sees and (under the moving collector)
  forwards it. ABA is moot because a referenced node is never reclaimed, so
  version-counter pointer-packing is neither needed nor allowed.
- **Memory ordering:** `seq_cst` throughout (the only `Atomic[T]` order today).
  Non-pointer instantiations (`Queue[int64]`, handles) are correct on every
  platform; a **reference-payload** `Queue[T]` additionally relies on the moving
  collector relocating pointer graphs and therefore inherits the open
  aarch64-Linux evacuation race **I#223**, so it is trustworthy on that platform
  only once I#223 is fixed (§4.6).
- **Surface:** concurrency lives in the **type name**, with **no new syntax** and
  no change to any existing type. `concurrent.Counter()` is a module constructor
  (non-generic, the `threading.Thread()` pattern); the generic `Queue[T]` is a
  bare prelude type built with `new_queue[T]()` — the language already uses bare
  generics (`Atomic[T]`, `Optional[T]`) and reserves module-qualification for
  non-generic types, because a module-qualified *generic type path* is not in the
  grammar and adding it was out of scope. No `len()`/iteration (a lock-free queue
  has no consistent snapshot); `pop()` is comma-ok (`v, ok = q.pop()`), returning
  `(Optional[T], bool)`.

**Rationale:** lock-free earns its complexity only for a counter and a
contended producer/consumer queue; a map is better served by sharded mutexes.
Leaning on the existing GC (rather than building a reclamation subsystem) is both
correct here and an order of magnitude less code (design §4/§5), and keeps the
whole feature additive and runtime-free.

## D-0080 — REPL v1 polish: cross-entry declarations, floats, readline, user-type render (I#252)

**Status:** Accepted. Follow-up to [[D-0073]] (I#159); full text in
`design/0025-repl.md`. Finishes the four items the REPL MVP deferred, all within
the accepted MCJIT + runtime-slot model (no architectural extension).

- **Cross-entry top-level `def`/`type`/`enum`/interface persistence** — the
  session accumulates top-level declarations by name (newest wins, position
  preserved) exactly as it already accumulates imports, and re-emits them at
  module scope in every later entry so each entry type-checks and links against
  them. To keep those re-emitted definitions from colliding as duplicate symbols
  across the MCJIT's accumulated modules, codegen's JIT mode gives every function
  **internal linkage** except the symbols the engine looks up (`main`,
  `__repl_entry_N`); each entry module then carries its own private copy. The AOT
  path is unchanged. *Limitation:* a persisted top-level `def` cannot reference a
  session `let`-binding (bindings are thunk-locals, not module globals), and a
  struct/enum **instance** still does not persist (only the type definition does).
- **Float session bindings** — `float64`/`float32` bindings persist in the same
  64-bit slot cell, reinterpreted as a `double` by new `zap_repl_slot_{get,set}_f64`
  runtime entry points (a float is never a GC pointer, so the slot is unrooted;
  `float32` round-trips exactly via widen-on-store / narrow-on-load). New
  `__repl_load_f64`/`__repl_store_f64` lowering intrinsics mirror the i64/ptr pair.
- **Readline + persistent history** — an interactive terminal gets line editing
  and `~/.zap_history` via `rustyline` (`default-features = false` +
  `with-file-history`, codegen-gated to the REPL). A piped/non-interactive stdin
  (the scripted-stdin test harness) falls back to the plain line reader, so test
  input stays byte-for-byte predictable; the line editor is never engaged there.
- **User-type rendering (minimal).** A bare user value auto-prints through
  `print(...)` only when its type carries a renderer (`__str__`/`__repr__` or
  `@derive(Repr)`); otherwise it falls back to a `<TypeName>` placeholder instead
  of a compile error. The **general `repr`/Display protocol is explicitly NOT
  designed here** — it is shared with `print()` and is language-wide (see the
  follow-up on I#252); the `@ 0x…` address form is deferred with it (no address
  intrinsic exists yet).

New dep: `rustyline` (codegen feature). Still host-only (needs LLVM), POSIX-only
(the JIT does not yet support Windows, so `$HOME` locates the history file).

## D-0081 — Zap-to-Zap module & package imports (I#283)

**Status:** Accepted — full text in `design/0034-modules.md`. **Stage A**
(intra-project local modules) and **Stage B** (cross-package dependency imports
resolved through `zenv`, D-0052) are both implemented. Concretizes [[D-0028]]
(module/visibility model) and
[[D-0052]] (logical dependency names via `zenv`) into the compiler front end so
user code can span files and a library package can be imported. Surfaced by
dogfooding the AWS SDK (D-0011 §8); governed by D-0076 (modules/imports/
visibility is already Stable).

- **A module is a file; a package is a directory with a `zap.toml`** (D-0028).
  Members are accessed **qualified** through the imported name (`a.helper()`),
  as stdlib is today; there is **no** `from x import y` form (I#70).
- **Path resolution.** Within a search root, `x.y.z` → `<root>/src/x/y/z.zap`
  and bare `x` → `<root>/src/x.zap`. The first segment picks the root, in fixed
  precedence: (1) compiler-known **stdlib** module (D-0056, unchanged), (2) a
  **dependency** logical name → that package under `$ZAP_ZENV/pkg/<name>` with
  the remaining segments mapped within it (D-0052), (3) otherwise a **local**
  module within the importing file's own package `src/`. A local file colliding
  with a stdlib/dependency name is shadowed (warning); higher precedence wins
  deterministically because those names are 1.0-stable surface.
- **Visibility** (D-0028, enforced): public = top-level name not starting with
  `_`; a qualified access to another module's `_name` is a compile error; no
  export marker, no re-export in v1.
- **Entry & graph.** Entry stays `src/main.zap` / the explicit file; the compiler
  discovers the import graph transitively and compiles the closure. **Non-entry
  modules are declaration-only** (a top-level executable statement is an error) —
  so importing has no import-time side effects or init order, which makes the
  compilation strategy sound and cycles a pure naming question. v1 **rejects**
  import cycles with a diagnostic (open: allow-now, since declaration-only makes
  it sound).
- **Compilation strategy (internal, not observable).** Merge the import graph
  into **one** AST module before lowering, with module-qualified internal
  symbols, and reuse the existing single-module `lower_module` →
  `build_executable` path. AOT stays a single LLVM module, so the [[D-0080]]
  JIT internal-linkage behavior (REPL MCJIT only) is not involved and
  monomorphized generics keep working. Separate-object linking is rejected for
  v1 (would commit a cross-object symbol ABI).
- **Staging.** Stage A = intra-project local modules (closes the #283 repro);
  Stage B = cross-package dependency imports via `zenv` (reuses Stage A; most
  coupled to D-0052/D-0027). No IR-schema or runtime-ABI boundary is touched; no
  new external deps.
- **Open:** cycle policy (reject vs allow), package root-module convention
  (`src/<name>.zap` vs `src/lib.zap`), re-export/aggregation, shadowing
  (warning vs error).

## D-0082 — Portable link profile for AL2023 / AWS Lambda (I#287)

**Status:** Accepted — implemented. A `zap build --portable` / `zap run
--portable` flag (env `ZAP_PORTABLE=1`) that produces a **self-contained static
binary** that runs on minimal / older-glibc distros — specifically **Amazon
Linux 2023** (glibc 2.34), the AWS `provided.al2023` Lambda base. Surfaced by the
aws-lambda-runtime Docker image (I#89a): the prebuilt Ubuntu-24.04 toolchain
image ([[D-0081]] companion, #277) emits a binary dynamically linked against
glibc 2.39, which exits 127 on AL2023 because glibc symbol versions are fixed at
link time. Governed by D-0076 (`run`/`build` are Stable; this is a new, additive,
**Experimental** flag — STABILITY §12.5).

- **Why static over the alternatives.** Three options unblock the AWS base image:
  (a) an AL2023-based toolchain image variant, (b) a static/musl link mode, (c) a
  CPython-off build. We take **(b)+(c)**: a full static (`-static`) link removes
  the glibc symbol-version floor **entirely**, so one bootstrap runs on AL2023
  *and* every other Linux — broader than an AL2023-only image, and built from the
  **existing** image with no new base. A glibc-version-floor-via-old-base build
  was rejected as narrower (binary stays dynamic; needs a dedicated old-glibc
  build host).
- **CPython is dropped in `--portable` only** (`-DZAP_NO_CPYTHON`; no `pkg-config
  python3-embed`; no `-rdynamic`). libpython3.12 is absent on AL2023 and static
  libpython is impractical, and CPython embedding (D-0012, #27) is fundamentally
  at odds with a self-contained static binary. This **reuses the existing
  `ZAP_HAVE_CPYTHON` guard + stub path** that Windows v1 ([[D-0012]] context,
  I#198) already ships: `import python` lowers and links but raises a *catchable*
  Zap exception at runtime. **The default build is unchanged** — CPython interop,
  `-rdynamic` profiler symbolication and dynamic linking all stay on without the
  flag.
- **Documented trade-offs (accepted for a deploy bootstrap).** Static glibc means
  NSS-backed `getaddrinfo` cannot `dlopen` nsswitch plugins (standard DNS
  resolution is unaffected — fine for Lambda's resolver); OpenSSL engine `dlopen`
  is unavailable (unused by the TLS client); the binary is larger; `import
  python` is unavailable at runtime.
- **Proof.** `scripts/docker-smoke.sh` builds a program with `--portable` in the
  toolchain image and runs the *same* binary inside stock
  `public.ecr.aws/amazonlinux/amazonlinux:2023` (exit 0) — the decisive check a
  default dynamic build fails. Runs in the `docker.yml` PR job.
- **Follow-up (not this change).** Switch `zaplang-io/aws-lambda-runtime`'s Lambda
  image onto the AWS `provided.al2023` base now that `--portable` exists.
- **Open:** a musl variant for truly libc-independent binaries (needs a musl
  toolchain + musl builds of PCRE2/OpenSSL/zlib/GC in the image); a dynamically
  `dlopen`-ed libpython so portable binaries can still opt into Python at runtime;
  cross-build (`--target`) once codegen emits for non-host triples (#203).

## D-0084 — `encoding/url` percent-encoding module (I#335)

**Status:** Accepted — implemented. A built-in **`url`** stdlib module (the
`crates/sema|ir|codegen/.../modules/` seam, D-0056; pure, dependency-free C in
`zaprt.c`, no external library — "pure" in the same sense as `json`/`yaml`),
providing RFC-3986 percent-encoding/decoding and the AWS SigV4 query
canonicalization variant required by every REST and query AWS protocol.

- **Why a built-in, not a pure-Zap source module.** There is no pure-Zap stdlib
  search root (D-0056: stdlib resolves to compiler-known builtins only; D-0081
  Zap-to-Zap imports resolve local-project files or `zenv` dep packages, never
  stdlib), and the aws-core signer is compiled as a single bundled file, so only
  a built-in is universally `import`-able by it today. Matches all existing
  stdlib modules. "Pure-Zap" in I#335 is read as "self-contained, no third-party
  C dependency", which a from-scratch percent codec satisfies.
- **Surface (idiomatic, Python-flavoured names).** `quote`/`unquote` (+
  `unquote_plus`) for the byte codec, `quote_path` for the keep-`/` path form,
  and `encode_query`/`canonical_query` for building a query string from a
  `dict[str,str]`.
- **Unreserved set** is RFC-3986 §2.3 (`A-Z a-z 0-9 - . _ ~`); every other byte
  → `%XX` **UPPERCASE**. Encoding is **byte-by-byte over the UTF-8 encoding**, so
  multibyte codepoints are correct. Space → `%20` (never `+`) — this is exactly
  the SigV4 URI-encode, so `quote` is the signer's primitive for query
  keys/values and single path segments. `unquote` is lenient (a malformed `%`
  escape and a bare `+` pass through).
- **SigV4 path double-encoding** is expressed through `quote_path` (encodeSlash
  off): the CanonicalURI is `url.quote_path(path)` for **S3** and the
  double-encoded `url.quote_path(url.quote_path(path))` for every other service.
- **`canonical_query`** is the SigV4 CanonicalQueryString: each key/value
  `quote`-encoded, pairs sorted by encoded key then value, joined with `&`.
  `encode_query` is the unsorted sibling (dict iteration order is unspecified —
  open-addressing hash order — so callers who need determinism use
  `canonical_query`).
- **Follow-up (not this change).** A `list[(str,str)]` overload of
  `encode_query` for order-preserving / repeated-key query strings (needs
  tuple-list iteration in the runtime); percent-decoding straight into a `bytes`
  value once a distinct `bytes` type lands (I#284 follow-up).

## D-0083 — Stable list sort: `xs.sort()` / `sorted(xs)` (I#336)

**Status:** Accepted. Stable in-place `list.sort()` and a copying `sorted(xs)`
with optional `key=` / `reverse=`, needed for canonical SigV4 header/query
ordering and deterministic JSON/XML serialization (D-0011). ABI in
`design/RUNTIME-ABI.md` §"Collection layout".

- **Surface (Python-readable).** `xs.sort(key=f, reverse=True)` sorts in place;
  `sorted(xs, key=f, reverse=True)` returns a new list and leaves `xs` untouched.
  `key` / `reverse` are keyword-only, matching Python. No positional comparator:
  a `key=` projection covers the ordering need without a second ABI shape.
- **Stable by construction.** A bottom-up merge sort over an index permutation;
  equal keys keep input order, and `reverse=True` keeps that stability (it flips
  only the key comparison, as Python does), which is what canonical ordering and
  reproducible output require.
- **One core entry point, alias-safe.** `zap_list_sort_by(dst, keys, cmp_kind,
  reverse)` orders `dst` by a parallel `keys` list; the no-key case passes
  `keys == dst` (compare elements by their own word). The key array is fully read
  to order the indices before any element moves, so the alias is safe. `cmp_kind`
  is `0` (signed `i64`: int/bool) or `1` (byte-wise `ZapStr`: str).
- **`key=` runs in compiled code.** The projection (a first-class function value,
  [[D-0067]]) is applied per element to build `keys` in generated ZIR, not in the
  C runtime: the growing list must stay a GC root across each call's safepoint,
  which a C loop calling back into Zap cannot guarantee. `zap_list_sort_by` then
  makes no call into Zap and allocates only non-GC scratch, so it is a GC leaf.
  The key's **return type** selects `cmp_kind`.
- **No new runtime GC work.** `zap_list_copy` (for `sorted`) reuses the list
  descriptor's precise tracing; the sort touches no collector internals.
- **Limits (clean follow-ups).** Element types without an ordering and no `key=`
  are a compile error; a `list[float]` (I#569) is not directly orderable and is
  sorted via a `key=` projection; an inline `lambda` key needs a function-typed
  context it does not get here, so
  `key=` wants a named function or a `fn(T)->R` binding in v1; a user-struct
  `__cmp__`/`Ord`-driven ordering is deferred.

## D-0085 — core `xml` module: DOM parse + deterministic serialize (I#351)

**Status:** Accepted — implemented. A core `xml` stdlib module (design/0033-xml.md)
for the AWS **rest-xml** (S3) and **query** (STS/SNS/IAM/EC2) protocols, part of
the broad-coverage program (epic #366). It provides the XML **primitives**; the
aws-codegen layer maps shape traits onto them.

- **DOM shape.** Two compiler-known prelude reference structs (like `JsonValue`,
  [[D-0059]]): `XmlElement` (`name`, resolved `namespace` URI, `text`,
  `list[XmlAttr] attrs`, `list[XmlElement] children`) and `XmlAttr` (`name`,
  `value`). A **data model**, not a full infoset: each element has one `text`
  field plus child elements. True mixed content (text interleaved between child
  elements, order significant) is flattened — the one documented v1 limitation; it
  never arises in AWS payloads. Lists (not dicts) back `attrs`/`children` so
  **document order is preserved** and output is deterministic with no sort pass —
  required for signing canonicalization and golden tests — and repeated siblings
  (flattened lists) and duplicate attributes survive.
- **Surface.** `xml.parse(str|bytes) -> XmlElement` (the root), `xml.dumps(
  XmlElement) -> str` (deterministic), `xml.element(str) -> XmlElement` (builder
  entry). Navigation methods: `find`/`find_all` (first vs **all** matching
  children — `find_all` is what makes flattened lists navigable),
  `get_attr`/`get_text`/`local_name`. Builder methods (each returns `self`):
  `set_text`/`set_attr`/`add_child`.
- **Namespaces.** A declaration is an ordinary attribute (`xmlns` / `xmlns:ns`),
  so it round-trips through `attrs` with no special serialize handling; a prefixed
  name is just a name with a colon. On parse, `namespace` is the **resolved URI**,
  with the in-scope default + prefix bindings threaded down the tree so children
  inherit (e.g. `<Error>` under `<ErrorResponse xmlns="…">`); `local_name()`
  strips the prefix.
- **AWS features expressed** (not hard-coded): `xmlAttribute` via
  `get_attr`/`set_attr`; `locationName` by choosing the tag name; wrapped lists
  via `find` + `find_all`; flattened lists via `find_all` on the parent; error
  docs via plain navigation of both `<ErrorResponse><Error><Code>` and the bare
  `<Error><Code>` forms.
- **Implementation (module seam, [[D-0056]]).** Mirrors `json` ([[D-0059]]): a
  flat C pull-tokenizer in `zaprt.c` (`zap_xml_lex_*`) that skips the XML
  declaration / PIs / comments / DOCTYPE, emits CDATA as literal text, and decodes
  entities (`&lt; &gt; &amp; &quot; &apos;` + numeric `&#DD;`/`&#xHH;` → UTF-8);
  the tree build (`__xml_parse`) and serialize walk (`__xml_dumps`) are Zap prelude
  functions, so the runtime never depends on the DOM layout. Escaping helpers
  `zap_xml_esc_text` (`& < >`) / `zap_xml_esc_attr` (`& < "`, quoted); the fragment
  join reuses `zap_json_join`. Keeping the C side a dumb, reusable tokenizer
  (no tree, no scope stack) is deliberate — the later streaming pull-parser
  follow-up reuses it, and namespace policy stays readable in Zap.
- **Out of scope** (separate codec tickets; the module carries text only):
  timestamp formatting, base64 blob decoding, percent-encoding. A streaming
  pull-parser for huge S3 listings is a later follow-up — a DOM parse suffices for
  v1. No grammar change (a library, not syntax).

## D-0086 — `@derive(Json)` AWS codec hooks: timestamps, blobs, open enums (I#346)

**Status:** Accepted — implemented. Three opt-in extensions to the compiler
built-in `@derive(Json)` codec ([[D-0059]]) so model-driven AWS shape structs
round-trip, part of the broad-coverage program (epic #366). The later
aws-codegen layer ([[I#347]]) emits Zap shape types that use these; plain
(non-AWS) JSON derive is unchanged — every hook is opt-in per field or type.

- **Timestamps.** A `DateTime` field (the prelude value struct, [[D-0056]])
  (de)serializes in a wire form chosen per field with a **field decorator**
  `@json(timestamp="epoch")` or `@json(timestamp="iso8601")`. `epoch` is a JSON
  **number**, Unix seconds UTC (via `zap_datetime_to_unix` / the epoch getters);
  `iso8601` is a JSON **string** (`zap_datetime_format_iso` / `zap_datetime_parse_iso`,
  `2023-11-14T22:13:20`). **Default (no decorator) is `epoch`** — the AWS awsJson
  protocols' default `timestampFormat`. This is the first wired *field* decorator
  (field decorators were deferred before; others stay deferred).
- **Blobs.** A `bytes` field ([[D-0025]], ticket #337) is **type-driven**: it
  (de)serializes as a base64 (standard alphabet, padded) JSON string via
  encoding/base64 (`zap_base64_encode` / `zap_base64_decode`), round-tripping the
  exact bytes — including non-UTF-8 binary. No annotation; using `bytes` *is* the
  opt-in. `bytes` and `str` share the `ZapStr` layout but are distinct ZIR types
  (`Ty::Bytes` vs `Ty::Ptr`), so the derive routes them apart with no ambiguity.
- **Open enums.** `@derive(Json)` + the **enum decorator** `@json(open)` makes an
  enum a forward-compatible string enum. It must be a tier-3 ([[D-0046]]) enum
  whose known variants are **payload-free** and which carries exactly one fallback
  variant `Unknown(str value)`; at least two known variants are required (a lone
  known variant beside `Unknown(str)` would collapse into the `Optional` niche,
  [[D-0064]], leaving no tag to switch on). Encode: a known variant → its name as
  a JSON string, `Unknown(s)` → the carried string. Decode: a string matching a
  known variant name → that variant, **any other string → `Unknown(raw)`** instead
  of erroring, so AWS adding an enum value never breaks a client. The derive
  generates `to_json` / `from_json` / `from_json_value` on the enum (the first
  enum JSON derive — derive was struct-only before), so an open enum nests as a
  struct field and round-trips there too. A **closed** string enum (no `@json(open)`)
  is a documented follow-up; `@derive(Json)` on an enum without `@json(open)` is a
  clear compile error.
- **Scope / validation.** Strict-typing posture is unchanged ([[D-0059]] fork E):
  the open-enum `Unknown` fallback is the one deliberate leniency, mirroring
  "ignore unknown keys". Not in scope (follow-ups): `@json(name=...)` field rename,
  struct-level `@json(tag=...)`/`@json(deny_unknown=...)`, closed string-enum
  derive, and timestamp formats beyond epoch/iso8601 (e.g. AWS `http-date`).
- **No grammar change.** `@json(...)` and `@derive(Json)` reuse the existing
  `decorators` production on fields and enums (D-0036/[[D-0046]]); `docs/GRAMMAR.ebnf`
  needs no new rule. See `design/0010-json.md` §"AWS codec hooks".

## D-0087 — Function decorators: metadata + registering targets (I#457)
**Status:** Locked (capability A) — supersedes the "deferred" stub for function
decorators left by [[D-0036]]/#26.

#26 shipped `@derive` + struct/field/enum reflection but left **function**
decorators unimplemented: any `@deco` on a `def` errored `ZAP6001` ("function
decorators are not implemented yet (deferred)", `crates/ir/lower.rs`), and a
dotted decorator name like `@app.get("/x")` was rejected at `ZAP5032` ("unknown
decorator or annotation") because a bound method on a runtime instance is not a
declared decorator/annotation/built-in. Both gates blocked the idiomatic
FastAPI-style handler surface that fastzap (zap#312, fastzap#8) wants. This
decision defines function-decorator semantics and lifts both gates for
**capability A** (a uniform `fn(Request)->Response` handler; per-parameter typed
injection is capability B, function-parameter reflection, tracked as zap#458 and
explicitly **not** built here).

Two kinds, split by the decorator-name shape, both resolved at **compile time**
(consistent with D-0036's compile-time-first rule). In **neither** case is the
decorated function rebound or wrapped — it keeps its name, signature and
callability. There is no Python-style runtime decorator application; a decorator
is either *metadata on* the function or *a registration of* the function.

### (1) Bare name → declared annotation/decorator → reflection metadata
`@route(path="/x")` where `route` is a declared `annotation`/`decorator` (a bare,
in-module name) attaches **reflection metadata keyed by the function's name**,
exactly mirroring the struct-level model (D-0036): lowering emits
`zap_reflect_register("<fn>", "<deco>")` at the top of `main`, so
`reflect_has("h","route")` / `reflect_names("h")` answer for functions just as
they do for types. The function body lowers unchanged. This clears ZAP6001 for
declared decorators. **Behavioral `wrap`/`before`/`after` code generation stays
deferred** (it is the user-macro piece, D-0036 "staged"); only metadata is
attached in v1. Deferred built-in function decorators (`@cache`, `@validate`,
`@inject`, …) keep their clean "not implemented yet" error.

### (2) Dotted name → registering callable target → registration call
`@app.get("/x")` on `def h(...)` desugars to the **registration call**
`app.get("/x", h)` — the decorated function is passed as a trailing first-class
function value (D-0067) to the receiver's method. This is *exactly* the
method-call registration surface fastzap v1 already compiles and runs
(`app.get(path, handler)`); `@app.get` is pure sugar over it. The desugar runs
as a whole-program AST pass **before sema** (`crates/driver`), so:
- ZAP5032 is resolved **not** by loosening sema's decorator check but by removing
  the dotted decorator and letting sema **type-check the resulting call** — an
  unknown receiver, a missing method, or a wrong arg shape now produces a precise
  error at the call instead of a blanket "unknown decorator".
- The rule is syntactic and safe: **every** existing decorator (built-ins,
  declared annotations/decorators) is a *bare* name, so a **dotted** decorator
  name is unambiguously a registering target. (Cross-module/bare user decorators
  that wrap remain the deferred user-macro API.)

**Placement / ordering.** The registration call is inserted **immediately after
the receiver's binding** (`auto app = …`) in the function that declares it
(`main` in practice), preserving decorated-def source order among routes. This is
#459-independent: it needs no module-level execution. The receiver is a local, the
handler is a top-level `def` referenced by name — the precise pattern that already
type-checks and codegens today. If a future design adds module-scope execution,
the registration semantics for a module-level `app` will need to be specified
alongside its initialization order; D-0090 currently rejects that form.

**Scope boundary for #458.** The registering method just takes a `fn(Request)->
Response` value; per-parameter typed injection (`@app.get(id: int, q: str=…)`,
`Depends`) needs function-parameter reflection (zap#458) and composes later as a
reflection-driven adapter over this same seam — no change to this desugar.

**No grammar change.** `@ dotted_name ( args )` already parses (`docs/GRAMMAR.ebnf`
`decorators`, D-0036); this is sema/driver/lowering only.

**Test note.** The registering golden (`tests/golden/fn_decorator_register.zap`)
is reduced to a **field-free** `App` to avoid **zap#461** — struct field defaults
are dropped at construction when the field is omitted, so a `list[Route] routes =
[]` field left null SIGSEGVs on `.append`. That is a separate codegen bug, not in
the decorator path: the `@app.get` desugar lowers correctly either way (verified
in ZIR). Restore the `list[Route]` router form in the golden once zap#461 lands.

## D-0088 — Struct field defaults are materialized for omitted fields (I#461)
**Status:** Locked — fixes a codegen gap in [[D-0038]] (literal field defaults).

D-0038 gives a struct field a literal default (`list[int64] xs = []`), evaluated
**per construction** so mutable defaults are safe. But struct-construction
lowering (`FnLower::construct`, `crates/ir/src/lower.rs`) only emitted a
`FieldSet` for fields the **caller supplied**: an omitted defaulted field got no
store, leaving it whatever `StructNew` left there — null for a pointer field.
`Box(xs=[])` worked; `Box()` left `xs` null, so `.append` SIGSEGV'd (and other
pointer-typed defaults — `str`, `dict`, nested `struct` — read null silently).
This was severe enough that the #457 decorator golden had to drop its
`list[Route]` field to a field-free `App` to dodge it (D-0087 test note).

**Fix.** `construct` now tracks which field slots the caller sets, then for every
**omitted** field with a declared default it lowers that default expression and
emits the `FieldSet`, in **declaration order**, in the constructor's scope — so
`Box()` is identical to `Box(xs=[])`. The default reuses the explicit-argument
lowering path exactly (same `coerce` + value-struct copy, D-0051), which keeps
the D-0038 **fresh-per-construction** guarantee: each `Box()` gets its own `[]`.
Defaults are stored on the struct layout (`StructCtx::field_defaults`, parallel
to `fields`) at the same pass that records field types. Scope covers all
defaulted field kinds — scalar, `str`, heap (`list`/`dict`), and nested struct.

**No grammar change** (the `TYPE name = expr` field-default syntax is unchanged,
D-0038/D-0015); this is lowering only. Golden: `tests/golden/struct_field_defaults`
covers the repro plus mixed scalar/str/list/nested-struct defaults under fully-
and partially-omitted construction, asserting parity with the explicit-arg form
and mutable-default independence. Follow-up: restore the `list[Route]` router
form in `fn_decorator_register` (D-0087), now unblocked.

## D-0089 — Function/callable signature reflection (I#458)
**Status:** Locked — extends [[D-0036]] (struct/type reflection) and [[D-0087]]
(function-level reflection) to **signatures**.

D-0036 shipped struct/type annotation reflection (`reflect_has`/`reflect_names`)
and D-0087 keyed it to functions; both expose only decorator/annotation *names*.
There was no way to introspect a callable's *shape* — parameter names, declared
types, defaults, or per-parameter annotations — which is the substrate
FastAPI-style typed per-parameter injection and `Depends(...)` dependency
injection need (the fastzap web framework, I#312 / fastzap#7, "capability B";
pairs with D-0087's `@app.get` lowering for the fully-typed `@app.get(id: int,
q: str = ...)` form).

**Surface.** Five string-keyed query builtins, mirroring the existing reflection
shape. The `owner` is a function name (top-level `def`) or `"Type.method"` (struct
or tier-3 enum method); the implicit `self` receiver is excluded:
- `reflect_params(owner) -> str` — comma-joined parameter names, declaration order
  (parallels `reflect_names`); `""` if none/unknown.
- `reflect_sig(owner) -> str` — a readable one-line signature built at compile time,
  e.g. `(id: int, q: str = ...) -> str`; a defaulted param is marked `= ...` and an
  annotated one is `@anno`-prefixed. `""` for an unknown owner.
- `reflect_param_type(owner, param) -> str` — rendered declared type (`list[str]`,
  `fn(Request) -> Response`); `""` if untyped/not found.
- `reflect_param_has_default(owner, param) -> bool` (parallels `reflect_has`).
- `reflect_param_annotations(owner, param) -> str` — comma-joined per-param
  annotation names; `""` if none.

**Per-parameter annotations.** A *declared* `annotation`/`decorator` on a parameter
(`def h(@Inject Svc dep)`) is now recorded as reflectable metadata instead of the
old blanket "parameter decorators are not implemented yet (deferred)" IR error —
the same model D-0087 established for function-level annotations. Behavioral and
unknown names stay deferred (unknowns already rejected by sema's ZAP5032). This is
a **small grammar change**: the parser now accepts inline `@name(args)` prefixes in
a parameter list (the `Param.decorators` AST field already existed; `GRAMMAR.ebnf`
`param` loosened from `[ … ]` to `{ … }`), and `zap fmt` round-trips them.

**Mechanism.** `DecoratorPlan::collect` (`crates/ir/src/lower.rs`) gathers one
`FnReflect` per top-level function and per struct/enum method in the user module;
each holds the per-parameter metadata and a precomputed signature string. These are
emitted once at the top of `main`, beside the D-0036 struct-registry bootstrap, via
`zap_reflect_param_register` / `zap_reflect_sig_register` into two static runtime
tables (`crates/codegen/src/zaprt.c`), enumerated by the existing Immix
`zap_reflect_mark_roots` custom GC root. Reflection metadata is present for **any**
named function (not only decorated ones), since route handlers are plain `def`s.

**First-class values.** Opaque first-class function values / closures (D-0067,
I#163) are not reflectable *by value* — you reflect the named `def`; a parameter
*typed* `fn(...)->...` is reflected by its rendered type (statically known).

Golden `tests/golden/reflect_params` (CI full-LLVM + arm64) exercises a plain fn, a
defaulted/annotated fn, a struct method, and unknown owner/param lookups; an IR unit
test asserts the registration + query lowering host-side.

## D-0090 — Top-level executable statements are a diagnostic, not an ICE (I#459)
**Status:** Locked — robustness fix over [[D-0081]] (module linking) and [[D-0021]]
(bare statements parse at top level); surfaced building fastzap (zap#312).

**The bug.** `auto app = App(0)` (or any bare statement) at entry-module scope hit
`unreachable!("entry statements handled via merge list")` in the module linker's
rewrite (`crates/driver/src/modlink/rewrite.rs`). The claimed "merge list" never
existed: `Loader::merge` feeds **every** entry item — `Item::Stmt` included — to the
per-module `Rewriter`, so the panic fired on the first top-level statement. Latent
since D-0081 (every program now routes through the linker), but only the #457
decorator work put a top-level statement in front of a `def` in anger. The ticket's
repro (binding before an `@app.get` def) is just the minimal trigger; a lone
`print(1)` panicked too.

**Why a diagnostic, not "make it run".** A compiled Zap program runs from
`def main()`. Top-level executable statements have **never** executed: pre-D-0081
AST→ZIR lowering dropped them via its item catch-all, and there is no module-global
storage, no init-order rule, and no `main`-interaction design for them. Making them
"compile and run" is a new language feature with its own design decision, not a
robustness fix — and silently dropping executable code is a footgun. So the honest
behaviour is to **reject** them.

**Rule.** `Loader::check_toplevel_statements` rejects a top-level `Item::Stmt` in
**every** module: `ZAP3016` in the entry module (new code — "would never run, move
it into `def main()`"), `ZAP3014` in imported modules (declaration-only, D-0081).
The one exception is a **module docstring** (D-0041): a bare, non-f-string string
literal carries no behaviour, so `is_doc_stmt` exempts it at module scope (this also
lifts the prior over-broad ZAP3014 that rejected docstrings in imported modules). An
f-string is *not* exempt — its interpolations are executable. The rewrite's
`Item::Stmt` arm no longer panics: it rewrites the statement normally (only a
docstring can still reach it, and a string literal has nothing to rewrite), keeping
the arm total.

**Fastzap impact:** none. The supported `@app.get` surface (D-0087) already lives
inside `def main()` — the receiver is a `main` local and the decorated handlers are
module-level `def`s — so `fn_decorator_register` is unchanged. ZAP3016 now points a
user who wrote the registration at module scope to that working form.

**No grammar change** (`item ::= … | statement` still parses, D-0021; the
restriction is semantic). Coverage: five `modlink` unit tests — the exact #459 repro
(asserts ZAP3016, not a panic), a lone top-level statement, a statement between two
`def`s, and docstrings allowed in both entry and imported modules.

## D-0091 — Reject structurally incompatible `fn(...)` coercions in lowering (I#465)
**Status:** Locked — soundness fix over [[D-0067]] (first-class function values);
surfaced building fastzap's `@app.get` routing seam (zap#312, fastzap#8).

**The bug.** A named function (or any `fn(...)` value) coerced to a structurally
**incompatible** `fn(...)` type was accepted with **no diagnostic**. First-class
function values are monomorphic (D-0067): a bare top-level function used as a value
lowers to a `<fn>$fnval` thunk emitted to the function's **own** signature, while the
indirect call site (`CallClosure`) uses the **expected** `fn`-type. When the two
disagree on arity, a parameter type, or the return type, they miscompile under codegen
— a dropped argument, a `Req` read as a `str`, or a scalar `i64` field-accessed as a
`Resp` struct. `zap ir` exited `0` for all three mismatch kinds.

**Why lowering, not sema.** Sema is permissive about function types: a `fn(...)`
annotation resolves to `Ty::Unknown` (sema has no function-type model — see
`sema_types::Ty`), which is universally assignable, so sema cannot compare signatures.
The real interned signatures (`Ty::Func(idx)` over `Module::fn_sigs`) exist only in
AST→ZIR lowering — the same stage that already validates lambda arity and rejects
`None` bound to a value struct. So the structural check lives there, at the single
coercion funnel `expr_coerced` (argument, `let`/field assignment, and `return`
positions) plus the bare-name reassignment path.

**Rule.** A **function value whose signature is known** (`Ty::Func`) coerced into a
`fn(...)` slot must have the **exact** interned signature of that slot — equal arity,
each parameter type equal, and the return type equal. Because signatures are
deduplicated, a compatible `Ty::Func` is literally the same index, and any other
`Ty::Func` is a genuine structural mismatch, rejected with a precise type error naming
both signatures and the first discrepancy, keyed to the position: `ZAP4021` (argument),
`ZAP4001` (assignment), `ZAP4003` (return) — the same codes a direct call / assignment /
return with a wrong type already uses.

**Only a known-signature source is checked.** The check fires solely when the source is
itself a `Ty::Func`. A function value that has already lost its signature — a collection
element or other uniform reference word read back as a plain `ptr` (D-0067 stores fn
values as single reference words) — carries no contradicting signature, so re-attaching
the expected call signature at a `fn(...)`-annotated binding is the legitimate round-trip
(`fnval_store`), left to `coerce`. Over-rejecting that bare-`ptr` case was the first
cut's bug, now covered by a regression test.

**No fn-subtyping.** This deliberately rejects *all* non-identical signatures, including
ones that might seem assignable (e.g. a function with defaulted trailing parameters
coerced to a shorter `fn` type). Any such widening needs an explicit decision **and** a
coercing adapter thunk — today the default is silently dropped and the arity still
mismatches — tracked with the reflection-driven `Depends` injection work (zap#466). A
`lambda` is unaffected: it takes its parameter types from the expected signature, so it
matches by construction.

**Thunk/call-site agreement.** With incompatible coercions rejected, a coercion into a
`fn(...)` slot only succeeds when the source function's signature equals the expected
one, so the `<fn>$fnval` thunk is always emitted to a signature identical to the call
site's — no residual mismatch. The thunk emission path is unchanged.

**No grammar change** (a sema/lowering type-compatibility fix). Coverage: eight host
`fnval_coercion` tests (the three repro mismatches assert code + message; compatible
handler, lambda, `container_roundtrip` (bare-`ptr` element re-typed to `fn(...)` — the
over-rejection guard), incompatible `let`, and incompatible `return` cases) and a
`fnval_struct_handler` golden that dispatches struct-typed handlers through a
`fn(Req) -> Resp` parameter end-to-end, so CI's full-LLVM + arm64 jobs confirm the
legitimate D-0067 path still runs.

## D-0092 — Reflection-driven handler adapters: `Depends(...)` injection (I#466)
**Status:** Locked (capability C) — composes [[D-0087]] (`@app.get` registering
decorators), [[D-0089]] (signature reflection), and [[D-0091]] (fn-value coercion
soundness) into the FastAPI-style dependency-injection surface. Unblocks fastzap#7.

D-0089 shipped signature **reflection** as the stated substrate for FastAPI-style
per-parameter injection and `Depends(...)`. Building fastzap#7 on it showed
reflection is necessary but **not sufficient**: it can *describe* a signature but
cannot *drive* injection. The idiomatic form is a handler that declares a
dependency as a parameter:

```zap
@app.get("/items")
def list_items(Request req, DB db = Depends(get_db)) -> Response:
    return Response(db.conn)
```

Per request the framework must resolve `db` (run `get_db`, recursively resolve
*its* deps, cache once per request) and then call `list_items(req, <DB>)`. Three
walls blocked that (I#466): (1) the `@app.get` seam holds an opaque
`fn(Request)->Response` **value**, not a name, and values are not reflectable
(D-0089); (2) `reflect_param_annotations` returns annotation **names** only, so a
`Depends(get_db)` provider is unrecoverable; (3) there is no way to invoke a
handler with reflectively-built args — a `fn(Request,DB)->Response` handler does
not match the `fn(Request)->Response` seam, now correctly **rejected** (D-0091),
having previously been silently miscompiled with the default dropped.

### Decision — compile-time per-handler adapter synthesis
Resolve all three walls **at the decoration site, at compile time**, not with a
runtime dynamic-invoke primitive. The `@app.get` desugar (`crates/driver/
decorator_desugar.rs`) already runs as a whole-program AST pass **before sema**,
where the compiler statically holds the handler's full signature and every
provider's `def`. For a decorated handler that declares one or more
`Depends(provider)` parameters, it synthesizes a **statically-typed adapter**

```zap
def list_items$adapter(Request req) -> Response:
    DB get_db$dep = get_db()              # resolve deps, deps-first, once each
    return list_items(req, get_db$dep)
```

and registers **that** (`app.get("/items", list_items$adapter)`) on the uniform
seam. The adapter's parameters are the handler's non-`Depends` parameters, so its
type **is** the seam's `fn(Request)->Response` — checked by sema/D-0091 like any
registration. Walls 1 and 2 dissolve (no value-reflection, no runtime annotation
recovery: the name and the provider are in the AST); wall 3 is a plain typed call.
No new runtime, no new IR, no grammar change. A handler with no `Depends`
parameter is registered directly — unchanged, zero overhead.

### `Depends` is a compiler-recognized marker (not a library value)
A parameter default whose callee is the bare name `Depends` with a single
positional provider-name argument is recognized by the desugar and **consumed** —
it is never a runtime value and never lowered. A library `Depends` value cannot
work: recovering its provider at the seam is exactly wall 2. Outside a handler
parameter default, `Depends(...)` remains an ordinary unknown call (ZAP3003).

### Per-request caching
The adapter body is the per-request scope (one adapter call per request). Each
distinct provider is resolved into exactly one local (`<provider>$dep`) and
reused; nested `Depends` in a provider's own signature resolve recursively,
deps-first. This yields FastAPI's default `use_cache=True` (resolve-once-per-
request, shared across sub-dependencies) with no cache data structure and full
static typing. App-scoped / cross-request singletons are a separate future knob.

### Scope boundary — what the compiler binds
v1 binds **only** `Depends(provider)` injection plus positional pass-through of
the handler's non-`Depends` parameters (the Request). Path/query/body extraction
stays **framework-side** — the handler reads typed values off `req` (as fastzap v1
does) — because binding them would require a compiler-known Request extraction
protocol, coupling the language to one framework's API. Consequently a `Depends`
provider's own parameters must each be `Depends(...)` (or it takes none): a
request-derived provider parameter is the path/query/body case and is rejected
(ZAP5048). A future per-param source annotation (`@query`/`@path`, framework-
declared extraction) is a distinct capability, not this issue.

### No runtime invoke-by-name
Deliberately rejected. The compile-time adapter is statically typed and
consistent with D-0091; a runtime invoke-by-name would reopen exactly the
unsoundness D-0091 closed and commit the language to a large dynamic surface.

### Errors (ZAP5048 "invalid dependency injection")
Raised by the desugar, reported as sema-style errors before type-checking:
an unknown provider (not a top-level function), a malformed `Depends(...)` (wrong
arity / non-name argument), a provider parameter that is not itself `Depends(...)`
(request-derived, unsupported in v1), and a `Depends` dependency **cycle**.

### Tests
Ten host unit tests in `decorator_desugar.rs` (adapter shape, deps-first order,
resolve-once dedup, no-`Depends` direct registration, and each ZAP5048 path).
Three CI full-LLVM + arm64 goldens: `depends_inject` (injection produces the
right Response), `depends_nested` (nested `Depends`, deps-first), `depends_cache`
(shared provider resolved once per request). Docker is down locally, so runtime
behavior is CI-gated.

### Downstream — fastzap#7 migration
Replace the `src/fastzap/di.zap` prototype (D-FZ-0009: `Cell[T]`/`resolve` memo +
a hand-built per-request context struct) with handler parameters
`T x = Depends(provider)`; providers become plain `def provider(...) -> T` with
their own deps as `Depends` parameters. Drop the manual per-request context — the
adapter resolves once per request. Request-derived values (headers, path, query,
body) stay read off `req` inside providers/handlers (scope boundary above).
## D-0093 — `tls`: public TLS **server** surface (un-defer the #147 server half)
**Status:** Locked — resolves issue #488, un-defers the TLS **server** that
[[D-0062]] (`tls` client) explicitly left as a follow-up. Directly unblocks
fastzap#5 (HTTPS / TLS termination): fastzap's accept loop wraps `net.accept`
with a server-side handshake and relies on listener-close unblocking accept. No
grammar change (all generic module calls); same OpenSSL backend and netpoller
discipline as D-0062 — this is **surfacing + cert/key-from-disk + an accept-loop
shape**, not new crypto. The only server path before this was the internal
`tls._test_server()` scaffold (hardwired ephemeral self-signed cert, one-shot
echo); its OpenSSL machinery (server `SSL_CTX`, `SSL_accept` over the netpoller,
server-side read/write parking) is generalized here, and `_test_server` keeps
working for the `tls_echo`/`tls_verify_reject` goldens (the handshake loop is now
a shared helper).

- **Surface.** `tls`: `listen(host, port, cert_path, key_path)` → an opaque TLS
  **listener** handle (`bytes`, a new ATOMIC `ZapTlsListener` = fd + poller desc
  + a server `SSL_CTX` built from the cert+key **PEM files on disk**);
  `accept(listener)` → a TLS **conn** handle that runs the SERVER-side handshake
  over the netpoller — **the SAME `ZapTls` handle the client produces**, so
  `send`/`recv`/`close_conn` (and `http.read_request`/`_write` layered over them)
  are shared verbatim; `port(listener)` → `int64` bound port (resolves an
  ephemeral `0`, as `net.port`); `close_listener(listener)` closes it.
- **Handle unification + ctx lifetime.** `accept` returns the client's `ZapTls`,
  so one opaque conn type flows through `http` unchanged. The server `SSL_CTX` is
  shared and **reference-counted**: the listener holds one ref (released by
  `close_listener`); each accepted conn takes its own via `SSL_CTX_up_ref`,
  released by the existing `SSL_CTX_free` in `close_conn` — so a conn safely
  outlives its listener and no new close verb is needed.
- **Close-raises-accept (non-negotiable, mirrors `net`).** `close_listener`
  deregisters the listening fd from the netpoller, waking any coroutine parked in
  `accept` with `CLOSED`, which **raises** a catchable exception — exactly as
  `net.close_listener` unblocks `net.accept`. This is the contract fastzap's
  staged graceful-shutdown depends on to drain its accept loop.
- **Robust accept loop.** `accept` is caller-driven and loopable (one wrapped
  conn per accepted socket, never one-shot). A client whose handshake fails is
  torn down and the loop continues — one bad peer never kills the server — so
  `accept` returns only on a completed handshake or raises only on a closed
  listener. Netpoller-first like the client: `SSL_accept`/`SSL_read`/`SSL_write`
  WANT_READ/WANT_WRITE PARK the coroutine; fd/pd/`SSL_CTX*` are pulled to C
  locals before any park and the GC `ZapTls` is built only after the last park.
- **Same unified post-1.1.0 OpenSSL API** as D-0062 (`TLS_server_method`,
  `SSL_CTX_set_min_proto_version` floor TLS 1.2, `SSL_CTX_use_certificate_chain_file`
  + `SSL_CTX_use_PrivateKey_file` + `SSL_CTX_check_private_key`), so no version
  `#ifdef`; CI is OpenSSL 3.0.13. A client↔server round-trip golden
  (`tls_server_echo`) drives `listen`→`accept`→server `recv`/`send` against
  `connect_insecure` with a self-signed cert+key written to a temp dir on disk.

**Still deferred (noted in D-0062, unchanged):** ALPN / HTTP/2; client
certificates (mTLS); custom CA-bundle configuration; session resumption / 0-RTT;
OCSP stapling.
## D-0094 — Scope deadlines: `timeout=<ms>`, `ScopeTimeout`, and the pending set (I#475)
**Status:** Locked — finalizes the deadline half of structured concurrency (D-0044).

`scope:` stays a pure **join barrier**: it owns the coroutines spawned inside it
and joins them at the dedent. Every failure — a child's, or the whole scope's — is
reported through an **exception**, the single un-suppressible outcome channel. No
scope option ever silences a failure.

**Trigger.** `scope(timeout=<ms>)` arms a relative deadline. The value is a
**nonnegative integer number of milliseconds** — no unit suffix, no `Duration`
type (`_` digit separators are fine: `timeout=30_000`). `timeout=0` cancels
immediately; a negative duration raises `ValueError`. On expiry the scope tree is
cancelled (running `finally`/`with`/cleanup), then the scope raises `ScopeTimeout`.
There is deliberately **no `.seconds`/`.milliseconds` surface**: an earlier draft
added a lexer rule making `30.seconds` lex as member access; that global change was
reverted (it perturbs float/imaginary literals for one option's sugar).

**Two exception tiers.**
- *Child-level:* a spawned call that raises propagates as its own exception, or an
  aggregate `ExceptionGroup` when several fail (D-0045).
- *Block-level:* a whole-scope failure that is no single child — today just
  `ScopeTimeout`; the tier is open for more later.

**Exception hierarchy.**
- `Cancelled` is what *children receive*, and the generic external cancel. It stays
  **outside `Exception`** (D-0044) so a child's `except Exception:` cannot swallow
  its own cancellation.
- `ScopeTimeout` is **new** and is raised *to the caller* on deadline expiry. The
  caller is outside the scope and is not itself cancelled, so `ScopeTimeout` is an
  ordinary catchable exception **under `Exception`**. An inner scope's uncaught
  `ScopeTimeout` therefore naturally becomes a child-failure of an enclosing scope.

**`ScopeTimeout` reports the COMPLETE pending set.** Reporting one coroutine, or a
sample, is wrong — it would imply the unreported ones succeeded. At the instant the
deadline fires (before cancellation drains anyone) the runtime snapshots **every**
coroutine still outstanding in the scope tree and renders them — each by name and
by what it is parked on — together with the deadline, into the `ScopeTimeout`
message. This aligns with the D-0058 `coroutine_dump`.

*Realized surface (and its limits).* A caught exception in Zap binds to its
**message string** (there is no attribute access on caught exceptions), so the
pending set is surfaced **through the message**, exactly as `ExceptionGroup`
surfaces its sub-exceptions (D-0045) — not as programmatic `.pending`/`.deadline_ms`
attributes, which would need a new exception-object/attribute language feature.
Each entry reports the **spawn target name** (a static C-string carried on the G)
and its **parked-on reason** (channel / nested scope / io, else the scheduler
g-state). The **`spawn`-site `file:line`** is *not* reported in v1: the IR/codegen
carry no source-location table (spans are raw byte offsets, no debug info), so it
needs separate source-location plumbing — tracked as a follow-up. The same report
enriches the `coroutine_dump`. A signal-driven `Cancelled` is a shared singleton
and cannot carry per-scope identity, so the pending report is `ScopeTimeout`-only.

**Policy knobs are orthogonal — all still raise.**
- `on_error = OnError.Cancel | OnError.Wait` reacts **only to a child failure**
  (cancel siblings now vs. let healthy ones finish, then raise). It is **fully
  independent of `timeout`; neither references the other** — there is no "timeout
  caps Wait" coupling. If both a child failure and a deadline occur, each mechanism
  acts on its own.
- `cancel_on = [Signal.…]` stays **strictly opt-in and signal-only**: a bare
  `scope:` or `scope(timeout=…)` installs no signal handling. It composes through
  the existing signal fan-out (notify channels + `cancel_on` + `@on_signal` all
  receive the signal). Note: the first subscription to a signal anywhere takes over
  its disposition **process-wide**, so **applications own shutdown; libraries should
  not grab signals**. Generalizing `cancel_on` to channel/token triggers is deferred
  (`zaplang-io/zap#492`).
- `grace = <ms>` is the existing hard-exit backstop for an uncooperative drain,
  reusing the signal grace timer.

**Precedence (reporting only).** A recorded child failure outranks a block-level
`ScopeTimeout`/`Cancelled`: it decides *which* exception surfaces, not the trigger
behavior. When *this* scope's own deadline cancelled its body, the caller sees
`ScopeTimeout` rather than the raw `Cancelled`; a `Cancelled` injected by an
*enclosing* scope, or a genuine error from the body, propagates unchanged.

**Spawn-call forms** are unchanged by this decision: method/lambda/function-value
capture and full keyword/default handling remain the honest implementation gap
tracked by #475 (`ZAP5076`); this PR does not close #475.

**Rationale.** One trigger unit (ms) with no sugar keeps the surface tiny and the
lexer unperturbed; exceptions as the sole outcome channel keep failures
un-suppressible; the complete pending set turns a timeout into an actionable
diagnostic instead of a bare "timed out".

## D-0095 — Keyword-only parameters via the bare `*` separator (I#496)
**Status:** Locked — extends the parameter model (D-0038) with Python's `*` marker.

A bare `*` in a parameter list makes every parameter after it **keyword-only**:
it may only be supplied by name at a call site, never positionally. This is
exactly Python's `def f(a, *, b)` separator, adopted unchanged so signatures read
the same across the two languages.

```python
def open(str path, *, bool create = False, int64 mode) -> File:
    ...
open("a", mode=0)                    # create defaults to False
open("a", create=True, mode=0)       # keyword-only params, in any order
# open("a", True, 0)                 # error: create/mode are keyword-only
# open("a")                          # error: required keyword-only `mode` omitted
```

**Surface.** The `*` is a **separator, not a parameter** — it carries no name,
type or default and does not appear in the function's parameter list. It may
appear **at most once** and **must be followed by at least one parameter** (a
bare `*` with nothing after it has nothing to make keyword-only, so it is a parse
error, matching Python). Parameters before `*` stay positional-or-keyword as
before; parameters after it may be required or carry a default (D-0038 defaults
compose unchanged, #298 keyword passing composes unchanged).

**Enforcement is front-end.** The parser records a `keyword_only` flag per
parameter; sema enforces it at the call site. Supplying a keyword-only parameter
positionally is `ZAP4029`; omitting a required keyword-only parameter is the
ordinary missing-argument error `ZAP4020`; a misplaced or duplicated `*` is
`ZAP2018`. Because keyword-only is purely a **call-site admissibility** rule and
every supplied keyword-only argument is an ordinary keyword/default argument by
the time it reaches lowering, **nothing in the IR, calling convention or codegen
changes** — the restriction never reaches the backend.

**Scope.** Enforcement targets free-function calls, where arity/keyword binding is
already checked. User-method calls are still typed loosely (return type only), so
keyword-only is not yet enforced on methods; that waits on the method default-arg
work (`zaplang-io/zap#481`). `**kwargs`-style arbitrary named parameters remain
deferred (D-0038).

**Rationale.** Optional parameters could previously only be trailing defaulted
positionals, so an API could not *force* call-site keyword use the way
Python/boto3-style signatures do — the gap surfaced writing the AWS SDK
credential chain. Reusing Python's exact `*` marker closes it with no new
concept and no backend cost.

## D-0096 — Socket read deadline: `net.set_read_timeout` + `TimeoutError` (I#485)
**Status:** Locked — adds a Zap-visible read timeout to the `net`/`http` server surface (D-0056).

`net.recv` and `http.read_request` previously parked on the netpoller with the
deadline hard-coded to `0` (park forever), so a server built on the stdlib
`net`/`http` (e.g. zaplang-io/fastzap) could not bound how long a socket read
blocks — an idle keep-alive connection, or an abandoned/slowloris client, held a
coroutine + fd until the peer closed. The netpoller already took an absolute
`deadline_ns`; this decision plumbs it to a Zap API.

**Surface.** `net.set_read_timeout(conn, ms)` stores a per-read deadline on the
socket handle. `ms == 0` is the default (no timeout, park forever, behaviourally
identical to before); a negative `ms` raises `ValueError`. The timeout is a
**property on the connection**, not a `recv_timeout` call variant, so it composes
uniformly with `http.read_request`'s internal recv loop and with the future TLS
transport seam (I#556) without threading a timeout argument through every read.

**Semantics.** The timeout bounds a **single wait-for-readiness** (SO_RCVTIMEO
style), recomputed into an absolute monotonic deadline on each park. It therefore
caps each idle wait rather than a whole multi-chunk operation — which directly
solves the stated case (a connection idle-parked between keep-alive requests times
out after `ms`). Bounding a whole slow-dribble (mid-request slowloris) request
would need a total-request deadline; that is a tracked follow-up, not this change.

**Outcome.** When the deadline elapses the read raises **`TimeoutError`** (new
built-in exception, derives from `Exception`), *not* a bare empty string — an
empty `recv` already means clean EOF, so a distinct type is what lets a server
tell "idle timeout" from "client closed." `recv`/`read_request` already lower as
throwing calls (`invoke`), and the timeout is raised through the same
`zap_panic_typed`/`__cxa_throw` path as every other runtime error, so it is caught
by `try` rather than aborting.

**Default path unchanged.** With no timeout set (the default), every read passes
`0` to the park primitive exactly as before; `connect`/`accept`/`send` always pass
`0` (no write/accept deadline). The no-timeout path is byte-for-byte identical.
## D-0097 — `http.read_request` / `http.respond` over TLS via a transport seam (I#556)
**Status:** Locked — extends the `http` server surface (D-0056) to TLS connections (D-0062/D-0093), unblocking HTTPS termination.

`http.read_request` and `http.respond` previously cast the connection straight to
the plaintext `ZapSock` and read/wrote with raw `recv`/`send`. A `tls.accept`
connection (`ZapTls`, an OpenSSL session) could not be read: the bytes on the fd
are encrypted TLS records, so the plaintext parser saw garbage. A server built on
the stdlib `http` (e.g. zaplang-io/fastzap) therefore could not parse a single
request off a TLS conn, blocking HTTPS termination outright — even though the
server-side TLS primitives (`tls.listen`/`accept`/`port`/`close_listener`, #488)
already worked.

**Either transport, one parser.** At the Zap type level a connection is the one
opaque handle (`bytes`), so `http.read_request(conn)` / `http.respond(conn, ...)`
already accept a `tls.accept` conn with no front-end change — the same mechanism
`net`/`tls`/`crypto` already use for their handles. The discrimination is at
runtime: both `ZapSock` and `ZapTls` lead with an `int kind` tag (a C
common-initial-sequence, read through a tiny `ZapConnHdr`), and the server
request/response path drives the connection through **one** read seam and **one**
write seam that dispatch on that tag — plaintext via `recv`/`send` + the
netpoller park, TLS via `SSL_read`/`SSL_write` + the WANT_READ/WANT_WRITE park.
There is a single HTTP request parser and a single response writer; the TLS path
does **not** duplicate parsing (rejected: a second parser drifts, and the
language-first rule forbids papering over the gap in the library).

**Read deadline is uniform.** The #485 read timeout (`net.set_read_timeout` +
`TimeoutError`) applies over TLS too: `ZapTls` carries the same
`read_timeout_ms`, `net.set_read_timeout` dispatches on the tag to set it, and the
TLS fill threads the deadline into its park. The timeout is raised through the
same `zap_panic_typed`/`__cxa_throw` path, so it is caught by `try` over a TLS
conn exactly as over a plaintext one.

**Plaintext unchanged.** A plaintext `http.read_request`/`respond` takes the same
`recv`/`send` path as before; the TLS fill with a `0` timeout (the client's HTTPS
transport) is byte-for-byte identical to the prior behavior.
## D-0098 — Raw `tls.recv` read deadline + whole-operation read deadline (I#566, I#567)
**Status:** Locked — closes the two read-deadline gaps left open by D-0096/D-0097 on the `net`/`tls`/`http` read path (D-0056/D-0062).

Two follow-ups to the read deadline, both on the read path, landed together
because they share the park primitive and the #556 transport seam.

**I#566 — raw `tls.recv` honors the per-conn deadline.** `tls.recv` parked via
`zap_tls_want`, which hard-coded an unbounded wait, so `net.set_read_timeout` on a
`tls.accept`/`tls.connect` conn was ignored on the *raw* TLS read surface (only
the `http` server fill path honored it over TLS). The recv park now reads the
handle's `read_timeout_ms` and bounds the wait, inlining the WANT_READ/WANT_WRITE
handling (rather than routing through `zap_tls_want`, whose `-2` return already
means "protocol error") so a timed-out park is told apart from a protocol error
and turned into the same catchable `TimeoutError`. `tls.recv` already lowers as a
throwing call (`invoke`), so it is caught by `try` exactly as plaintext
`net.recv`. **Scope: read only** — no `tls.send` write deadline (that overlaps
#559, deliberately out of scope).

**I#567 — whole-operation read deadline.** The #485 timeout is **per-park**
(SO_RCVTIMEO style, recomputed each park), so a slow-dribble peer that sends a
byte just under the per-park timeout forever (a slowloris) keeps resetting the
clock and the request never completes. A new **`net.set_read_deadline(conn, ms)`**
records a whole-operation budget on the handle (`ms == 0` = none, default;
negative = `ValueError` — mirrors `set_read_timeout`). At the start of a read
(`recv`, or a whole `http.read_request` spanning headers + body) the budget is
turned into **one** absolute monotonic deadline; each park then waits the
**nearer** of the per-park deadline and this whole-op deadline, so the entire
operation is bounded regardless of byte pacing. A separate API (not a flag on
`set_read_timeout`) was chosen so the two deadlines **compose** rather than one
overwriting the other: a server can set a modest per-park idle bound *and* a
larger total-request bound at once.

**Both transports, one surface.** The whole-op deadline is threaded through the
same #556 server seam as the per-park one — `net.set_read_deadline` dispatches on
the `ZapConnHdr` tag to store `read_deadline_ms` on either `ZapSock` or `ZapTls`,
and `http.read_request` computes the single absolute deadline once and passes it
down the fill seam — so it fires uniformly over plaintext **and** TLS. The same
`TimeoutError` is reused for both the idle (per-park) and slowloris (whole-op)
cases; both are read timeouts, so no new exception type was added.

**Defaults unchanged.** With neither deadline set, every read passes `0/0` to the
park primitive exactly as before (unbounded). With only the per-park timeout set,
the whole-op deadline is `0` and the effective deadline is exactly the per-park
one — byte-for-byte today's behavior. `connect`/`accept`/`send` still pass `0/0`
(no write/connect deadline — #559).

## D-0099 — Cancellation-aware socket parks and absolute I/O deadlines (#558, #559)

**Build on #574.** Preserve its per-park read timeout and per-read-operation
budget. Add `net.set_deadline_ns(conn, absolute_monotonic_ns)` as a separate,
persistent budget for BOTH directions and across multiple calls. Zero clears
it; negative input is ValueError. Expired deadlines raise TimeoutError even
when a nonblocking syscall could immediately succeed. Relative millisecond
conversions saturate rather than overflowing. Set/clear before starting I/O;
clearing a deadline does not revoke one already captured by a parked wait.
The new setter requires an open netpoller-backed TCP/TLS connection.

**No DNS timeout claim.** `net.connect_ip_deadline(ip, port, deadline_ns)` accepts
only numeric IPv4/IPv6 addresses. `tls.connect_resolved(hostname, ip, port,
deadline_ns)` uses the numeric address for TCP and the hostname for SNI and
normal chain/hostname verification; one budget covers TCP plus handshake.
Zero means unbounded, negative is invalid. DNS resolution remains a separate
prerequisite; existing named connect remains synchronous and is not advertised
as bounded while getaddrinfo is running. Cancellation is checked before/after
resolution, with returned address lists freed before propagating it.

**Wake without premature unwind.** PollDesc borrows each parked G's owning
scope. Cancellation walks live descriptors under registry→descriptor lock
order, removes matching waiters exactly once, clears their deadline/scope,
and wakes them with a distinct cancellation status. Registration checks
cancellation under the same descriptor lock, closing the lost-wakeup window.
Readiness, timeout, close, and cancellation compete for one waiter slot. A
resumed wait never dereferences the descriptor, which may already be recycled.
The precise collector pins borrowed scope pointers through a root callback.

Native callers receive a status, release scratch/partial socket/OpenSSL state,
then raise Cancelled. Established handles remain caller-owned: put close in
finally; a cancelled operation is not transactional and must not be replayed.
The no-netpoller fallback checks cancellation through bounded poll intervals
without abandoning workers or sockets. HTTP fill/send seams preserve status
and release their own I/O buffers before propagation.

Regressions cover sibling-triggered reads/writes, descriptor reuse, one-worker
backpressure, immediate/expired bounds and clearing, loopback backlog saturation,
TLS read/handshake/write cancellation, a verified resolved TLS connection, and
stalled TLS handshake/write deadlines. Native behavior is gated by the full
both-collector and ARM CI jobs, not frontend-only checks.

## D-0100 — scalar `float` lowered as `f64`; `list[float]` supported (I#569)

The scalar `float` type is lowered as a 64-bit IEEE double, the Python-facing
name for `f64`/`float64` (they are exact aliases in every type position: a
binding, a function parameter/return, an `array` dtype, and a `list` element).
Before this, `float` resolved to no type and a `float`-typed binding failed with
a backend error (ZAP6001), while any `list[float]` was rejected in the checker
(ZAP4027). Scalar `float` reuses the existing numeric tower (D-0019/D-0020,
I#19): arithmetic computes at `f64`, `/` is true division, and an `int` value
widens into a `float` implicitly (`SIToFP`); narrowing still needs an explicit
`int64(...)`.

`list[float]` reuses the uniform 8-byte collection word: a `double` occupies one
word, so the list stores it through `_f64` runtime entry points
(`zap_list_push_f64` / `zap_list_get_f64` / `zap_list_set_f64`) that reinterpret
the word bits (as the REPL float slot does, I#252) around the same storage — no
new list object, no change to `len`/iteration/precise GC (float elements are not
pointers). The list element kind gains an `Elem::F64` variant used only on the
`list[T]` paths; floats as dict/set keys or values, heap or channel elements,
and direct float ordering in `sort` (sort a `list[float]` via `key=`) stay
unsupported, as does the `f32` list element (ticket #49). Verified by the
`scalar_float` and `list_float` goldens under the full-codegen and ARM CI jobs.

## D-0101 — built-in field decorators `@default` / `@validate` / `@range` (I#472)

Completes the field-decorator group of the locked decorator set (D-0036,
design/0004) that the decorator slice (I#16) left deferred. These are
compile-time-first: they generate real, type-checked code with no runtime
reflection cost, and `zap expand` shows it.

- **`@default(expr)`** on a field supplies a construction-time default evaluated
  **per instance** (consistent with per-call default evaluation, D-0038). It
  reuses the omitted-field default-fill path added for literal defaults (#461),
  so a mutable default is a fresh object each construction. A field may carry a
  literal default (`T x = e`) *or* `@default(e)`, never both (ZAP5038).
- **`@validate(max_len=n, min_len=n)`** constrains the byte length of a
  `str`/`bytes` field; **`@range(min=v, max=v)`** constrains an integer/float
  field's value. Both lower to a generated `{Struct}____validate__(self)` guard
  — ordinary ZIR, so it is `zap expand`-visible and type-checked like any method
  — that the constructor invokes after it fills every field. A failing
  constraint raises a **catchable `ValueError`** (D-0007), the same mechanism as
  the index/divide faults (#383); inside a `try` the constructor's validator
  call is an `invoke`, so `except ValueError` catches a bad construction.
- Diagnostics are stable and coded, with spans, in sema: `@default` shape/double
  default (ZAP5038); `@range` non-numeric field / non-literal bound / inverted
  bounds (ZAP5039); `@validate` unknown constraint / non-str field / bad length
  literal (ZAP5040). Lowering trusts sema and only re-checks defensively.
- The `range` keyword is accepted as a decorator name (`decorator_name` in the
  parser) so `@range` parses; every other keyword stays reserved. GRAMMAR.ebnf
  gains the `decorator_name` production.

Verified by the `field_decorators` (end-to-end run, incl. caught `ValueError`)
and `field_range_reject` (coded compile error) goldens under the full-codegen CI
jobs, plus sema/ir unit tests on host.

**Deferred to a follow-up (still clean "not implemented yet" errors, #472):**
`@validate(matches=m/.../)` regex validation; and the function/variable group
`@cache` (memoization), `@deprecated` (use-site diagnostic), and `@inject` (DI,
cf. zap#312/fastzap#7). The locked spec surface and stability table are
unchanged; this decision records the realization of part of it.

## D-0102 — `crypto` RSA PKCS#1 v1.5 signing (I#628)

The `crypto` module (D-0056) shipped hashing, HMAC and a CSPRNG but no RSA, so
there was no way to load an RSA private key or produce an RSA signature. This
blocked the AWS SDK's CloudFront signed-URL/cookie helper (zaplang-io/aws#25,
#440), which signs a policy document with `RSA-SHA1(key-pair-private-key, …)`
(not SigV4). Per the language-first policy the helper did not work around the
gap — it takes the signature as an injected `fn(bytes) -> bytes` seam — and the
primitive lands here instead.

- **Surface.** `crypto.rsa_sha1_sign(pem_key, data) -> bytes` and
  `crypto.rsa_sha256_sign(pem_key, data) -> bytes` return the RAW PKCS#1 v1.5
  signature over the message digest (binary, not hex/base64); the caller applies
  any base64 / CloudFront variant. `crypto.rsa_sha256_verify(pem_pubkey, data,
  sig) -> bool` verifies a signature, giving a self-checking test oracle. Key
  and data accept `str` or `bytes` (the shared `ZapStr` layout), via the module's
  existing `expect_data` check; argument-count errors are the usual ZAP4025.
- **PEM.** The private-key PEM may be PKCS#1 (`BEGIN RSA PRIVATE KEY`) or PKCS#8
  (`BEGIN PRIVATE KEY`); `PEM_read_bio_PrivateKey` handles both transparently.
  A malformed key or a signing failure raises a catchable `ValueError` (D-0007);
  a bad verify key or a mismatched signature is a plain `false`, never a panic.
- **Algorithm binding (#642).** Only ordinary RSA keys (`EVP_PKEY_RSA`) are
  accepted; EC, DSA, Edwards, X25519, and RSA-PSS-only keys are rejected. Both
  signing and verification explicitly select `RSA_PKCS1_PADDING`. Wrong-family
  private keys raise `ValueError`; wrong-family public keys return `false`.
  This prevents the generic EVP dispatch from silently selecting another
  signature scheme. Sign calls use throwing-call lowering so a local `try`
  catches parsing, family, and signing errors. No key-size policy or additional
  algorithm is introduced.
- **Result lowering (#644).** Both RSA sign APIs retain `Ty::Bytes` in ZIR,
  matching sema. Inferred locals support binary indexing, slicing, concatenation
  and equality. The runtime ABI remains the shared `ZapStr` pointer layout.
- **Diagnostics.** Verification still returns `false` for malformed keys,
  foreign families, initialization failures, and mismatches. Configuration
  failures emit fixed, non-sensitive messages through `logging` at DEBUG;
  valid signatures and ordinary mismatches emit none. The default WARNING
  threshold suppresses these messages. Shared helpers enforce key family and
  PKCS#1 v1.5 initialization for both signing and verification.
- **Backend.** These are the only OpenSSL-backed entries in `crypto` — the
  hashing/HMAC/CSPRNG section stays a self-contained FIPS/RFC reference
  implementation, but RSA (bignum modexp + ASN.1 key parsing) would be a large,
  security-sensitive reimplementation, and OpenSSL is already linked
  (`-lssl -lcrypto`) for `tls` (D-0062). The runtime (`zaprt.c`, crypto-RSA
  banner below the OpenSSL includes) uses `EVP_DigestSign`/`EVP_DigestVerify`
  over `EVP_sha1()`/`EVP_sha256()`, so no new dependency. GC discipline mirrors
  the hash one-shots: no safepoint runs across the EVP calls, the OpenSSL
  objects are non-GC heap freed before return, and the GC `bytes` result is the
  final allocation.

Verified by the `crypto_rsa` golden (a fixed 2048-bit test key pair in both
PKCS#1 and PKCS#8 PEM, asserting both encodings sign identically, pinning the
deterministic RSA-SHA1 and RSA-SHA256 signature fingerprints, and checking
verify accepts the genuine signature and rejects a tampered message) under the
full-codegen CI jobs, plus the sema/ir unit suites on host. The
`crypto_rsa_key_binding` golden adds 2048/3072-bit known answers, empty/binary
messages, foreign-family signatures, ordinary-RSA PSS signatures, malformed
keys, damaged signatures, and repeated rejection followed by success (#642).
Damage tests mutate actual SHA-256 signer results for each key/message case,
as well as independent OpenSSL signatures, through the bytes test helper.

## D-0103 — `raise ... from ...` explicit cause chaining (I#604)

Part of the CPython grammar-parity pass (epic #614, decision D12). `raise`
gained an optional `from <cause>` clause for explicit exception chaining, the
counterpart to Python's `raise E from cause`.

- **Grammar.** `simple_stmt` grows `"raise" [ expr [ "from" expr ] ]`. `from`
  is a *contextual* word (like `pass`/`del`), not a reserved keyword: it opens
  the cause clause only directly after a raised value, and stays an ordinary
  identifier everywhere else, so no existing program that used `from` as a name
  breaks.
- **Exception object.** `ZapException` gains a `cause` slot (NULL for an
  ordinary raise), traced by the precise descriptor so the chain stays live
  until the exception is reported. `raise X("m") from c` calls
  `zap_exception_set_cause` before unwinding. When `c` is a caught exception its
  real object is reused as the cause (preserving its type id and its own chain);
  a `str` cause is wrapped in a base `Exception`.
- **Inspection.** A caught handler stashes the in-flight exception *object*
  beside the ordinary `except … as e` message-string binding, so `e.cause`
  reads the cause's message (`zap_exception_cause_message`), an empty str when
  there is no explicit cause. The message-string binding itself is unchanged, so
  all existing `print(e)` handlers keep their behavior. This explicit cause is
  distinct from any implicit during-handling context.
- **Traceback.** An uncaught chained exception renders a two-section trace: the
  cause's own `Uncaught exception: …` section first (deepest cause at the top),
  then the separator `The above exception was the direct cause of the following
  exception:`, then the raised exception's section. A plain (causeless) raise is
  exactly one section, byte-for-byte as before.

Verified by the `raise_from_cause` golden (chained uncaught two-section stderr
+ exit 1) under the full-codegen CI jobs, plus `raise_from_cause_type_checks`
(sema), `lower_raise_from_sets_cause` and `lower_caught_exception_cause_is_readable`
(ir) on host.

## D-0104 — Positional-only parameters via the bare `/` separator (I#602)
**Status:** Locked — extends the parameter model (D-0038) with Python's `/` marker; the mirror of the keyword-only `*` separator (D-0095).

Part of the CPython grammar-parity pass (epic #614, decision D10). A bare `/` in
a parameter list makes every parameter **before** it **positional-only**: it may
only be supplied positionally at a call site, never by name. This is exactly
Python's `def f(a, b, /, c)` separator, adopted unchanged so signatures read the
same across the two languages. The motivating use is FFI (I#542): C-mapped
functions need positional-only parameters for ABI/signature mapping.

```python
def f(int64 x, int64 y, /, int64 z) -> int64:
    return x + y + z
f(1, 2, 3)                           # all positional
f(1, 2, z=3)                         # z (after `/`) may be passed by name
# f(x=1, y=2, z=3)                   # error: x/y are positional-only
```

**Surface.** The `/` is a **separator, not a parameter** — it carries no name,
type or default and does not appear in the function's parameter list. It may
appear **at most once**, **must follow at least one parameter** (a bare `/` with
nothing before it has nothing to make positional-only, so it is a parse error,
matching Python), and **must come before any `*`** keyword-only separator.
Parameters before `/` become positional-only; parameters after it (and before
any `*`) stay positional-or-keyword as before. The two separators compose:
`def f(a, /, b, *, c)` is `a` positional-only, `b` positional-or-keyword, `c`
keyword-only.

**Enforcement is front-end.** The parser records a `positional_only` flag per
parameter; sema enforces it at the call site. Supplying a positional-only
parameter by name is `ZAP4034`; a misplaced, duplicated, or `*`-preceding `/` is
`ZAP2019`. Matching Python, when the callee also has a `**` keyword-collector a
positional-only name is collected into it rather than rejected. Because
positional-only is purely a **call-site admissibility** rule and every argument
is positional by the time it reaches lowering, **nothing in the IR, calling
convention or codegen changes** — the restriction never reaches the backend.

**Scope.** Enforcement targets free-function calls, where arity/keyword binding
is already checked. User-method calls are still typed loosely (return type
only), so positional-only — like keyword-only (D-0095) — is not yet enforced on
methods.

Verified by the `posonly_params` golden (working `/` call under the full-codegen
CI jobs), plus `positional_only_params_enforced_at_call_site` (sema), parser
accept/reject tests, the grammar-conformance corpus, and a fmt round-trip test
on host.

## D-0105 — Transparent type aliases `type X = …` (+ generic form) (I#606)
**Status:** Locked — adds a transparent alias to the type grammar; fills the gap `struct`/`enum` (nominal) do not cover.

Part of the CPython grammar-parity pass (epic #614, decision D14). Zap gains a
PEP 695-style type alias: `type Name = <type>`, plus the generic form
`type Name[T] = <type>`. The alias is **transparent** — it is *not* a new
nominal type (struct/enum already cover nominal types). It resolves to its
target everywhere a type is resolved, with the **same identity**, so the alias
and the spelled-out type are fully interchangeable (assignable both ways, equal
in signatures, equal as a collection element `list[Row]` / `dict[str, Row]`).

```python
type Count = int64
type Row   = (int64, str)          # Zap's tuple type is the parenthesized form
type Pair[T] = (T, T)              # generic alias (C++ `using`)
type Boxed[T] = Box[T]             # alias to a user generic struct

def double(Count n) -> Count:      # Count === int64
    return n + n
def mk() -> Pair[int64]:           # Pair[int64] === (int64, int64)
    return (1, 2)
```

(The issue spells the first example `tuple[int64, str]`; that is shorthand —
Zap's tuple **type** is the parenthesized `(int64, str)` form, `tuple_type` in
the grammar. No `tuple[...]` type spelling is introduced here.)

**Surface.** `type` is a **soft keyword**: it is not reserved today, so it is
recognized as the alias introducer only at item start and only when immediately
followed by an identifier (the alias name). `type ident ident` is not a valid
statement, so an ordinary use of `type` as an identifier (`type = 5`,
`type(x)`, `type.foo`) is never shadowed. The target is any `type`
(named/primitive, tuple, function, collection), and a generic alias's target may
reference its type parameters.

**Resolution is transparent.** Sema expands an alias to its target — with the
generic parameters substituted — wherever [`resolve_type`] runs, so an alias
carries no identity of its own: returning a `str` from a `Count`-typed (`=
int64`) function is the *same* `ZAP4003` as returning a `str` where `int64` is
spelled out. Because the alias is not a nominal type, it is never added to the
user-type set; it leaves no entry in the struct/enum registries.

**Generic aliases ride monomorphization.** The ZIR monomorphization pass
(D-0017/D-0064) expands every alias head structurally before generic renaming:
`Pair[int64]` becomes the concrete tuple `(int64, int64)`, and an alias whose
target names a user generic (`Boxed[int64]` → `Box[int64]`) is rewritten to that
instance, which is then requested and mangled exactly as a direct `Box[int64]`
would be. An alias therefore emits **no declaration of its own** — expansion is
purely substitution — and nothing in lowering or codegen sees an alias name.

**Cycles are rejected, not looped.** An alias that expands, directly or through
other aliases, back to itself (`type A = B; type B = A`, or `type A = A`) is
detected by a dependency-graph walk in sema and reported once as `ZAP4090`;
resolving a cyclic alias yields `Unknown` instead of recursing. Monomorphization
independently marks cyclic aliases and leaves them unexpanded so the back end
cannot loop either. A nominal recursive type remains a `struct`/`enum`.

**Out of scope.** Union aliases (`type J = A | B`) depend on anonymous union
types (a separate decision, #607/#25) and are not part of this change.

Verified by the `type_alias` golden (plain + generic tuple + generic-struct
alias, exercised end-to-end under the full-codegen CI jobs), sema positive tests
(transparent resolution, interchangeability, instantiation, alias chains), sema
negative tests (`ZAP4090` for direct, indirect, and self cycles), mono tests
(erasure + expansion + generic-struct monomorphization), parser accept/soft-
keyword tests, and a fmt round-trip test on host.

## D-0106 — Walrus assignment-expression `(name := expr)` (I#600)
**Status:** Locked — adds a bind-and-yield expression; deliberately not a declaration shorthand.

Part of the CPython grammar-parity pass (epic #614, decision D8). Zap gains the
Python (PEP 572) walrus assignment-expression: `(name := expr)` evaluates `expr`
exactly once, binds `name` by inference (like `auto`) in the lexically
**enclosing** scope, and itself evaluates to the value. It is the one capability
`auto` cannot express — a binding introduced in a position where a statement
cannot go — so it unlocks the idioms `auto` could not:

```python
while (line := read()) != "":     # read() once per iteration; `line` live after
    use(line)
if (n := len(xs)) > 10:           # `n` usable in the body and after the `if`
    print(n)
kept = [y for x in xs if (y := f(x)) > 0]   # `y` binds in the enclosing scope
```

**Surface — parenthesized only.** `:=` is lexed as a new `ColonEq` token and the
walrus is parsed **only** inside a parenthesized group, `"(" IDENT ":=" expr ")"`
(grammar `named_expression`). That one form is what reaches every required
position — an `if`/`while` condition and a comprehension `if` filter are all
expression positions — and it sidesteps Python's surprising `:=` precedence
(`(line := read()) != ""` binds `line` to the line, not to the comparison). The
operand is a full expression, bounded by the closing `)`.

**Not a declaration (the non-goal).** A bare statement `a := 10` is rejected at
parse time with **ZAP2020**, whose message steers to `auto a = 10`. Making `:=`
a general declaration shorthand would duplicate `auto` and fight the single
type-first/`auto` declaration form (D-0003); the walrus is strictly the
bind-and-yield expression, nothing more.

**Scope — enclosing, not local.** In an `if`/`while` condition the binding lands
in the current (enclosing) block scope, because the condition is checked and
lowered before the body's own scope is entered, so the name outlives the
construct with no special handling. In a comprehension the target binds in the
scope containing the **outermost** comprehension (PEP 572), not in the
comprehension's per-clause iteration scope: sema declares it below the
comprehension scopes, and lowering allocates the slot in the enclosing frame
(the alloca is hoisted to the entry block, #91, so it dominates both the
comprehension body and later reads). The loop variable `x` stays local to the
comprehension; only the walrus target leaks, matching Python.

**Evaluated once.** Each textual walrus is lowered exactly once (an `if`/`while`
condition block and a comprehension loop body are each emitted once and
re-executed at runtime), so a loop reuses the single slot created for the name —
`read()`/`f(x)` run once per evaluation, never twice.

Verified by the `walrus` golden (all three positions end-to-end under the
full-codegen CI jobs, with an eval-count proof of once-per-iteration), a
`tests/diff` CPython differential case, a `tests/conformance` case (claim
WAL-1), parser accept tests and a parser reject test (ZAP2020 → `auto`), sema
positive tests (visible-after in all three positions, value-type flow) and a
sema negative test, and a fmt round-trip test on host.

## D-0107 — Generator expressions `(x for x in xs)` (I#596)
**Status:** Locked — a bare generator expression is a lazy `Iterator`, never a materialized collection.

Part of the CPython grammar-parity pass (epic #614, decision D4). A bare
generator expression `(elem for CLAUSES)` uses the same clause grammar as a
comprehension (D-0026, multi-clause #599) but is **lazy**: it evaluates to an
`Iterator` (D-0048) that yields each `elem` on demand, so it builds no
intermediate list and can be consumed once.

```python
auto g     = (x*x for x in xs if x > 0)   # a lazy Iterator, not a list
auto total = sum(x*x for x in range(5))   # summed with no intermediate list
for y in (f(x) for x in xs):              # iterated one element at a time
    ...
```

**Surface.** The `atom` grammar gains `"(" expr comp_clauses ")"`, distinct from
a grouping parenthesis and a tuple (neither has a `for`). As the **sole**
argument of a call it needs no parentheses of its own (`sum(x for x in xs)`);
with any other argument it must be parenthesized (`f((x for x in xs), y)`). The
loop variables stay inside the generator — they do not leak into the enclosing
scope, exactly like a comprehension — while a walrus (D-0106) in a filter still
binds the enclosing scope.

**Lowering — reuse the generator machinery (I#67).** A genexpr desugars to an
anonymous generator function whose parameters are its captured free variables
(the enclosing-scope names it references, their types taken from the enclosing
scope at construction) and whose body is the clause list wrapping an innermost
`yield elem`:

```python
def $genexpr$N(cap0, cap1, ...):
    for v0 in iter0:
        if filt0: ... for vk in iterk: ... yield elem
```

This function is fed through the same `discover_generator` /
`lower_generator_next` that free generators use, so the genexpr's value is a
generator **state struct** satisfying the `Iterator` protocol with no new
runtime or GC support. Because the captured types are only known while lowering
the enclosing body (not in the module pre-pass), the state struct is built on
demand: it is a tuple-interned, non-inline (heap, GC-traced) struct rather than
a named one, and a side table routes the D-0048 iteration hooks to it so it is
its own iterator. Constructing it stores the captured values into the parameter
fields with `$state == 0`, exactly like a generator call.

**Deferred — nested generators.** A genexpr nested directly inside another
(`(v for v in (w for w in xs))`) makes one generator drive a second across its
own `yield`, which is the "nested generators" case I#67 deferred; it is not
supported yet (iterate the inner genexpr through an intermediate `for` or a
materialized collection). All other genexpr positions are supported.

**`sum` over an iterator.** `sum(...)` now accepts any iterable — a lazy
generator / generator expression or a `list` — draining it once through the
D-0048 protocol into a running total (the numeric `array` fast path is
unchanged), so `sum(x*x for x in range(5))` never materializes a list.

Verified by a `genexpr_lazy` golden (laziness proven by a side-effecting
generator consumed with an early `break`, plus `sum` over a genexpr, under the
full-codegen CI jobs), a `tests/diff` CPython differential case, a
`tests/conformance` case (claim COMP-4), parser accept tests (bare, multi-clause,
sole-call-argument) and parser tests pinning that a grouping paren and a tuple
are **not** genexprs, sema positive tests (iterator binding, `sum`, `for`-header)
and a sema negative test (loop variable does not leak), and a fmt round-trip
test on host.
## D-0108 — User-defined behavioral decorators: `before`/`after`/`wrap` engine (epic #651)
**Status:** `before`/`after` convenience form **Locked + implemented** (#652); `wrap(call)` general form **Locked + implemented** (#653).

Completes the "deferred user-macro piece" of D-0036: a `decorator` body may now
declare behavioral hooks whose code is **generated around the decorated target
at compile time** (not a runtime rebinding), rather than only attaching
reflection metadata (D-0087) or registering via a dotted call.

**Two forms.** A behavioral `decorator` declares either the convenience pair
`before(ctx)` / `after(ctx, result)`, or the general `wrap(call)` returning a
replacement callable:

```python
decorator traced:
    def before(ctx):
        print("enter", ctx.name)
    def after(ctx, result):
        print("exit", ctx.name, result)

@traced
def add(int64 a, int64 b) -> int64:
    return a + b
```

**Expansion model (compile-time, before sema).** Like the registering-decorator
desugar (D-0087), a whole-program AST pass runs **before type-checking**, so the
generated code is checked and `zap expand`-visible like hand-written code. The
`before`/`after` form renames the decorated function to `<name>$orig` and emits
a wrapper carrying the **exact original signature** that runs `before`, calls
`<name>$orig`, runs `after` on the captured return value, and returns it:

```python
def add$orig(int64 a, int64 b) -> int64: ...
def add(int64 a, int64 b) -> int64:        # identical signature
    print("enter", "add")                  # before; ctx.name -> literal
    int64 $result = add$orig(a, b)          # arguments thread through
    print("exit", "add", $result)          # after; result -> captured value
    return $result
```

Synthetic `$`-suffixed names cannot collide with source identifiers (the lexer
admits no `$`), the same device D-0087/D-0092 use.

**Signature preservation ("`@wraps` is automatic").** Because the wrapper is
emitted with the decorated function's own parameter list and return type,
callers, overload resolution and tooling see an unchanged function — there is no
hand-copying of name/doc and no runtime signature loss. This is the chosen
answer to D-0036's open "how does `*args` `inner` interact with the static
signature": **the `wrap(call)` `inner(*args)` is a typed forwarding template,
not a runtime varargs value** (#653). `inner` is specialized to the decorated
function's concrete signature; `call(*args)` expands to a typed call of
`<name>$orig` passing exactly the decorated function's parameters (a variadic
collector re-splats, a `**kwargs` collector re-maps, a keyword-only parameter is
passed by name). The generated wrapper again carries the decorated function's
exact signature, so the automatic-`@wraps` contract holds for the general form
too.

**The `wrap(call)` form, concretely (#653).** A `wrap` hook is
`def wrap(call):` whose body is a single nested `def inner(...)` followed by
`return inner`. The nested `def` is confined to the decorator-body parser — it
is **not** a general nested-`def` language feature. Its body is inlined into the
generated wrapper (there is no second real function and no closure), with:

```python
decorator retry(int64 times):
    def wrap(call):
        def inner(*args):
            for i in range(times):
                try:
                    return call(*args)
                except Exception as e:
                    if i == times - 1:
                        raise e
        return inner

@retry(times=3)
def fetch(str url) -> str: ...
```

expanding to a wrapper with `fetch`'s exact signature in which the decorator
argument is bound (`int64 times$arg = 3`), `call(*args)` becomes the forwarding
call `fetch$orig(url)`, and a fall-through guard is appended (see below):

```python
def fetch(str url) -> str:
    int64 times$arg = 3
    for i in range(times$arg):
        try:
            return fetch$orig(url)
        except Exception as e:
            if i == times$arg - 1:
                raise e
    raise RuntimeError("decorator `@retry` wrapper for `fetch` returned no value")
```

Decided consequences of the typed-forwarding model:

- **`*args` is a forwarding token, never a value.** `call(*args)` (or `call()`
  for a no-arg forwarder) is the only legal use; reading, indexing, iterating,
  or passing `args` around is ZAP5052, and so is using `call` other than as a
  call. This is what lets the wrapper keep a concrete typed signature rather
  than a runtime varargs list. A consequence is that the classic
  `args`-keyed `memoize` (and the built-in `@cache`) is **not expressible by
  `wrap`**, because it needs `args` as a runtime hashable value *and* persistent
  cross-call state; persistent module-level state is a separate, still-absent
  capability (the same gap that keeps `@cache` a deferred built-in). This is an
  accepted limit, not a workaround target.
- **Fall-through guard.** The generated wrapper obeys the same all-paths-return
  rule as hand-written code. Where the inlined `inner` body cannot be *proven*
  to return on every path (e.g. `retry`'s loop, whose last iteration always
  returns or re-raises but which the type system cannot see), expansion appends
  an implicit `raise RuntimeError(...)` so the wrapper type-checks and has
  defined behavior if control ever reaches the end — rather than lowering to an
  `unreachable` trap. A void wrapper, or one whose body provably terminates,
  gets no guard.
- **Mutual exclusion.** A decorator declares either `before`/`after` **or**
  `wrap`, not both (a `wrap` owns the whole wrapper body); mixing is ZAP5052.
- **Stacking** is uniform: a `wrap` wrapper chains into the next inner function
  exactly like a `before`/`after` wrapper, so the two forms stack together
  bottom-up.

**Context.** `ctx` exposes exactly one field, `ctx.name` — the decorated
function's name, substituted to a string literal. Any other use of `ctx` is a
stable error (ZAP5053).

**Stacking.** Decorators apply **bottom-up** (Python's `a(b(f))`): the chain is
built `f -> f$deco1 -> f$orig`, so hooks fire `a.before`, `b.before`, body,
`b.after`, `a.after`.

**Arguments.** A decorator's own parameters (`decorator retry(int64 times)`) are
bound, at the decoration site, to synthetic locals the hook bodies reference,
matching supplied `@retry(times=3)` arguments positionally then by keyword.

**Declared-only vs behavioral.** A `decorator` with a `pass` body stays a
declared metadata-only decorator (reflectable, D-0087) and is left untouched.
Only a decorator that declares hooks is expanded.

**Diagnostics (stable).** ZAP5049 unknown hook (not `before`/`after`/`wrap`);
ZAP5050 behavioral decorator on an unsupported target (only free functions in
this slice); ZAP5051 hook wrong arity; ZAP5052 unsupported `wrap` body shape
(mixing `wrap` with `before`/`after`, an `inner` that is not a single `*args`
forwarder, `call` used other than as `call(*args)`, or `*args` read as a value);
ZAP5053 invalid application (missing/extra decorator argument, `after`'s
`result` on a void function, or `ctx` used other than `ctx.name`); ZAP5054 a
`wrap` hook that does not end with `return <inner>`.

**Limits (accepted, follow-ups).** As with the registering desugar (D-0087), the
expansion runs only on the compile path, so LSP/MCP `check_source` do not yet
surface the behavioral diagnostics; and only free top-level functions are
supported (methods/fields/structs/enums/params are ZAP5050). `args`-keyed
`memoize` / `@cache` await persistent module-level state (a separate gap). The
canonical re-raise inside a `wrap` is the bound form `raise e`; the bare `raise`
re-raise is a separate unimplemented runtime feature (ZAP6001), independent of
this engine.

Verified by driver unit tests (`before`/`after` and `wrap` expansion shape,
`ctx.name` substitution, bottom-up stacking incl. `wrap`-under-`before`/`after`,
decorator arguments, `wrap` forwarding + fall-through guard + void no-guard,
`*args`/`call`-as-value rejection, missing-`return inner`, `wrap`+`before`
mixing, unknown-hook, unsupported-target, declared-only untouched), parser tests
pinning the `wrap` AST shape, `before`/`after`/stacking/arguments/exception and
`wrap` retry/threading/stacking/void/negative goldens (end-to-end under the
full-codegen CI jobs), and `zap fmt` round-trips of both decorator surfaces. See
`design/0004-decorators.md`.

## D-0109 — built-in decorators completion: `@deprecated` implemented; `@cache` deferred, `@inject` declined for core (I#472)

Closes the remaining built-in-decorator work of epic #469 / I#472, completing
D-0036's locked set. Of the three function/variable-group built-ins the field
group (D-0101) left deferred, one is now implemented and two are
reclassified/deferred out of the advertised v1 set, with clean coded
diagnostics. The issue's "Done when" is satisfied per built-in: each is either
implemented-with-coverage or reclassified in the spec + stability table.

**`@deprecated(msg?)` — implemented (function/method decorator).** It generates
no code; the decorated body lowers unchanged. Instead it emits a **non-fatal
use-site warning (ZAP5124)** at **every call site** of the decorated function or
method, carrying an optional message (`@deprecated("use X")`). This is exactly
the mechanism STABILITY.md §5 already names for retiring a Stable surface with
notice before a MAJOR removal.

- Target is restricted to a function/method (sema `require_target`, ZAP5047);
  deprecating a type/field/parameter has no well-defined v1 use site and is
  deferred. The message must be a single positional string literal (ZAP5042).
- Sema records `@deprecated` callables (free function by name, method by
  `"Type.method"`) in `collect_sigs`, and `check_call` raises ZAP5124 at each
  resolved call site — once per site, so N calls give N warnings.
- **Warnings are now surfaced by the build pipeline.** `front_end_program`
  previously called `zapc_sema::check` (errors only) and silently dropped all
  warnings; it now calls `analyze` and prints warnings through the shared
  renderer at a new `warning[ZAPnnnn]` severity (diag `render_diagnostic_sev`),
  non-fatally (exit unchanged). This also surfaces, for the first time at the
  CLI, the pre-existing `Eq`-without-`Hash` dict/set-key warning (D-0047).
- `zap expand`: nothing to show (no generated code); `zap` build/run still
  compiles and runs the program, warning on stderr.

**`@cache` — reclassified/deferred.** Memoization by argument tuple needs
**module-level persistent (static) state**, which is **not yet a language
feature** (module-level executable statements are rejected, ZAP3014/ZAP3016; no
globals/statics). A `zap expand`-visible, type-checked implementation is
therefore impossible without that feature; a hidden codegen-level static store
would be a language-bypassing workaround (against the language-first policy) and
would not be inspectable type-checked Zap code. The SPEC already notes an
`args`-keyed `@cache`/`memoize` is **not expressible** via the user-level `wrap`
form either (it is precisely the "separate capability" `wrap` lacks). `@cache`
is removed from the advertised v1 built-in set, marked **Deferred** in
STABILITY, and rejected cleanly at sema with the dedicated **ZAP5055** ("built-in
decorator not available in this version"); the name stays reserved so the
dedicated message fires rather than "unknown decorator". A future `@cache` is
gated on first adding module-level persistent state as a language feature (filed
as prerequisite language issue **#658**).

> **Superseded for `@cache` by D-0115 (I#666).** Epic #661 added module-level
> persistent state (`const` #658, mutable globals #662, precise-Immix roots
> #663, the `@shareable` sharing surface #664), so `@cache` is now implementable
> exactly as this decision required — real, `zap expand`-visible, type-checked
> Zap over a module-level dict, no hidden store — and ships in D-0115. The
> `@inject` decision below is unchanged.

**`@inject` — declined for core (permanent).** `@inject` is a DI-framework hook;
core Zap has **no DI container/registry**, and the concrete FastAPI-style
`Depends(...)` per-parameter-injection need was already met **at the framework
level** by the reflection-driven handler adapter (D-0092, I#466), composing
signature reflection (D-0089). An implicit field decorator that fills a value
from a global container is exactly the "annotation magic / Spring problem" that
the decorator design (D-0036) sets out to avoid. So `@inject` is **not deferred
pending work — it is declined as a core decorator**: dependency injection is a
framework concern, served by `Depends(...)`. The name stays reserved only as a
**redirect**: applying `@inject` is rejected at sema with ZAP5055 pointing at the
`Depends(...)` mechanism, rather than being silently an unknown decorator. (If
whole-program compile-time DI is ever wanted, it returns as a new, explicitly
designed feature — Dagger-style, its own epic — not by resurrecting this
built-in.)

**Net spec surface.** The advertised v1 built-in set becomes `@derive(...)`,
`@default`, `@validate`, `@range`, `@deprecated`. `@cache` is **deferred** (its
prerequisite, module-level persistent state, is tracked in I#658/I#661; `@cache`
itself in I#666). `@inject` is **declined for core** (framework-provided via
`Depends(...)`). `BUILTIN_DECORATORS` keeps all three names reserved (so
applications get a dedicated diagnostic, not a typo error); `DEFERRED_BUILTINS`
in lowering now holds only the field group (`default`/`validate`/`range`) for
their non-field-target guard, and `@deprecated` lowers as a recognized no-op.

Verified by sema unit tests (free-function and method call-site warning,
message variant, one-warning-per-site, non-deprecated silence, stacking with a
reflection annotation, wrong-target and bad-message rejection, `@cache`/`@inject`
ZAP5055 reclassification), an ir lowering test (`@deprecated` lowers with no
wrapper), a diag render test (warning severity header), a driver `error_codes`
test (ZAP5055 / ZAP5047), and the `builtin_deprecated` end-to-end golden (the
program builds and runs; warnings on stderr) under the full-codegen CI jobs. See
`design/0004-decorators.md`, SPEC.md and STABILITY.md §5.

## D-0110 — `is` / `is not` reference-identity operators (I#611)

Part of the CPython grammar-parity pass (epic #614, decision D18). Zap gains the
identity operators `is` and `is not`, reserving `is` as a keyword (as in
Python). They are added to `comp_op` at the same precedence as the other
comparison operators and — like them, and unlike `not in` (D-0093/#593) — they
**chain**: `a is b == c` is one Python-style chained comparison, desugaring to
`(a is b) and (b == c)` with each operand evaluated once (I#386). The two-token
`is not` is lexed as `is` followed by `not` and parsed as a single operator
(`BinOp::IsNot`); `not` and `is` keep their ordinary meaning everywhere else.

**Semantics — reference identity, not value equality.** `is` tests whether two
operands are the *same allocation*, which is meaningful only for reference
types. This is deliberately distinct from `==` (value equality / a struct's
`__eq__`): two distinct reference-struct allocations with equal fields are
`==`-equal but **not** `is`-equal.

- **`x is None` / `x is not None`** — the common case, the empty-variant / null
  check. It resolves to the same test as `.is_none()` and `match case
  Optional.None`: an `Optional[T]` niche layout and a plain reference `struct`
  use the null-pointer check (`IsNull`); a tagged `Optional[T]` uses the
  discriminant check. The bare literal `None` has no standalone lowering, so the
  check is emitted from the *other* operand's static type (either operand order
  works); `None is None` folds to a constant. This gives reference structs a
  clean null test that `== None` cannot express today (that requires `__eq__`).
- **`a is b` on reference types** — same-allocation pointer identity: the two
  reference pointers are compared directly (`ICmp`, `ptrtoint` in codegen).
  Eligible operands are a plain reference `struct` (D-0023), the GC collections
  `list`/`dict`/`set`/`heap`, a `channel`, and `Optional[T]`. `Unknown`/`Py`
  stay permissive.

**Value types are a compile error (the type-honest choice).** Scalars (`int`,
`float`, `bool`, `char`, `complex`, `bigint`), `str`, `bytes`, a `value struct`,
an `enum`, a tuple, and `range` have value semantics and no distinct identity,
so `is` on them would be a silently-surprising alias for `==`. Rather than
define `is` as `==` there, Zap **rejects** it:

- **ZAP4073** (`` `is` needs reference operands ``) — `a is b` where either
  operand is a value type. The message points the user at `==`.
- **ZAP4074** (value can never be None) — `x is None` where `x` is a
  non-nullable value type, so the test is always constant.

This diverges from CPython, where `is` is defined on every object (e.g.
`3 is None` is a valid `False`); see `PYTHON_DIFFERENCES.md`. The rationale is
Zap's value/reference split (D-0023): where CPython has object identity for
everything, Zap has it only for reference types, so an identity test on a value
is a type error rather than a footgun. Mixed *reference* operand types (two
different reference types) are permitted and well-typed (`bool`, always
`False`), consistent with how `==` does not constrain operand types.

Verified by parser unit tests (lone `is`, the two-token `is not`, identity
chains, mixing with `==`, comparison-level precedence), sema unit tests
(positive: `is None`/`is not None` on `Optional[T]` and reference structs,
identity on structs/collections, chains, `Unknown`/`Py` permissiveness;
negative: ZAP4073 on every value type and the reference-vs-value mix, ZAP4074 on
`is None` over never-None values), a `zap fmt` round-trip, and the
`is_identity` end-to-end golden (same-vs-distinct allocation, the `is`/`==`
distinction via a struct with `__eq__`, tagged and niche `Optional[T]`, `is
not`, `None is None`, and identity chains) under the full-codegen CI jobs.

## D-0111 — `as` sub-pattern capture in `match` (`case P as name:`) (I#603)

Part of the CPython grammar-parity pass (epic #614, decision D11). A `match`
pattern gained an optional trailing **`as` capture**, the counterpart to
Python's `case <pattern> as name:` (PEP 634 `as_pattern`). It matches the inner
pattern and, on success, binds `name` to the *whole* matched value.

- **Grammar.** `pattern ::= or_pattern [ "as" IDENT ]`. The `as` binds the
  entire or-pattern, so `case 1 | 2 | 3 as n:` is `(1 | 2 | 3) as n`, not
  `1 | 2 | (3 as n)`. The suffix is available at every sub-pattern position, so
  nested captures parse (`case [x as a, y]:`). `as` is the same reserved keyword
  already used by `import`/`with`/`except`, so this adds no new token.
- **Binding is a wrapper, not new destructuring.** A new `Pattern::As { pattern,
  name }` AST node wraps an existing pattern. The inner pattern still matches and
  destructures exactly as before, so its own captures bind alongside the `as`
  name: `case [x, y] as pair:` binds `pair`, `x`, and `y`; `case Point(x=0) as
  origin:` binds `origin`. The capture reuses the ordinary match-binding
  machinery — it is lowered as a bare capture of the scrutinee value, after the
  inner pattern's own bindings.
- **Type and scope.** `name` is typed as the scrutinee for that arm (not
  `Unknown`), so `n` in `case 1 | 2 as n:` is usable in arithmetic and
  `origin.x` resolves on the captured struct. The capture is in scope in the arm
  body **and** in the arm's guard (`case [x, y] as pair if len(pair) == 2:`),
  because both are checked/lowered in the scope the pattern populates.
- **Refutability / exhaustiveness.** The `as` wrapper is transparent to coverage:
  it covers exactly what its inner pattern covers and stays an irrefutable
  catch-all iff the inner pattern is one (`_ as n`, `x as n`). It does not by
  itself make a `match` exhaustive.
- **Diagnostics.** `name` must be a single plain identifier. `as _` (not a
  binding — drop the `as` to discard), a dotted target (`as a.b`), and any
  non-identifier token after `as` are rejected at parse time with the dedicated
  **ZAP2021** ("malformed `as` pattern capture").

No divergence from CPython's `as_pattern` surface, so PYTHON_DIFFERENCES.md is
unchanged. Verified by the `match_as_pattern` end-to-end golden (sequence +
struct + or-pattern captures, a guard reading the capture) under the
full-codegen CI jobs, plus parser unit tests (`as_pattern_capture_parses`,
`nested_as_pattern_parses`, `as_pattern_rejects_bad_capture_name`), sema unit
tests (`as_pattern_binds_capture_and_inner_names`,
`as_capture_is_scrutinee_typed_and_visible_in_guard`), and a driver
`error_codes` test (ZAP2021) on host. See docs/GRAMMAR.ebnf and SPEC.md.

## D-0112 — Module-level persistent state: `const` (Phase 1, I#658) + mutable globals (Phase 2, I#662) (epic #661)

Until now Zap had **no module-level persistent state**: a top-level executable
statement or binding at module scope was rejected (ZAP3014 in an imported
module, ZAP3016 in the entry module) because a compiled program runs from `def
main()` and a bare statement never lowered to code (D-0081/D-0021, #459). Epic
**#661** adds module-level persistent state behind a small, Python/Go-flavoured
model. This decision records the **full locked design** of #661; **Phase 1
(I#658) — module-level `const` only — is implemented now**, and Phases 2–3 are
deferred (tracked under #661).

### The full locked design (#661)

1. **Mechanism & scope.** Module-level state is introduced by ordinary
   *declarations with an initializer* at module top level, Python-style and
   positional: `const T NAME = expr` (immutable; Phase 1) and later
   `T NAME = expr` (mutable; Phase 2). There is **no new storage keyword**, **no
   struct-level statics**, and **no `init()` function**. Loose executable
   statements at module scope stay rejected (the ZAP3014/ZAP3016 intent is
   kept for anything that is not a declaration-with-initializer).
2. **Initialization.** Module state is initialized **eagerly, once, before
   `main`**, in **import-topological order** (reusing the acyclic module graph,
   D-0081) with **source order within a module**. A cross-module initialization
   cycle that would force use-before-init is a compile error — and is already
   impossible, since import cycles are themselves rejected (ZAP3011), so a
   `const` in module A reading a `const` in module B requires A→B with no B→A.
3. **Concurrency model (Phase 2+).** A light Go/Python model: mutable globals are
   permitted, **unsynchronized, and never implicitly locked** (no hidden
   blocking). `const` is the safe default; `Atomic[T]`/`Mutex`/concurrent
   collections are the recommended tools for shared mutable state.
4. **Safety is a diagnostic, never a dialect (Phase 2).** Sharing safety is a
   **static lint** (warnings, reusing the #659 warning pipeline), **not** a
   legality-changing flag — there is no `--concurrency-safe` mode that changes
   what compiles. Strictness is a project *warnings-as-errors* policy.
5. **`@shareable` marker (Phase 2).** A nominal annotation a type author applies
   to assert a type is safe to share across coroutines. The sharing lint
   suppresses when a mutable global's type carries it; the built-in `Atomic`/
   `Mutex`/concurrent types carry it too, under **one uniform rule** (no
   hardcoded allowlist). It is a trusted assertion with the runtime `-race`
   detector as the backstop, is queryable via reflection, and has a per-site
   `# noqa`-style suppression for one-offs.
6. **Runtime `-race` detector (Phase 2).** The dynamic backstop for the light
   model. Settled in **D-0114 (#665)**: a **Zap-native happens-before /
   vector-clock** detector (not LLVM/Clang ThreadSanitizer), a test-time
   diagnostic behind `zap run/test --race`, off by default and never
   legality-changing. Sliced — see D-0114 for what v1 lands and what is
   deferred.
7. **GC.** Module-global slots are **permanent GC roots**. Under the
   conservative Boehm collector (today) the static data/BSS segment is scanned,
   so a `const` holding a heap pointer is a root with no registration. Under the
   precise Immix collector (D-0009) each pointer slot is emitted into the
   static root table so the collector marks and relocates through it; Phase 1
   already registers every pointer-typed slot via `zap_gc_add_root` so **both
   collectors are correct** with one code path. **#663** completes the precise
   side — the static root table and the write barrier below.
8. **`@cache` (#666, supersedes the deferred D-0109 reclassification of #472).**
   Out of scope here; it is unblocked by this state mechanism and tracked
   separately.

### Phase 1 (I#658) — what ships now

A **module-level `const` declaration** is admitted at module top level:

```python
const int64 LIMIT = 100
const list[int64] XS = [1, 2, 3]
const dict[str, int64] TABLE = {"a": 1}
const auto PI = 3.14            # `auto` infers from the initializer
```

- **Scalars and reference/heap types** are both allowed (a reference `const`
  such as a `list`/`dict` is a GC root).
- It is **evaluated once at program start**, before `main`, in
  import-topological + source order. A later `const`'s initializer may read an
  earlier `const` (or call a function); a forward reference to a not-yet-declared
  `const` is an unknown-name error, matching the use-before-init rule.
- It is **immutable**: reassignment (`=`, `+=`, chained, tuple-target) is a
  compile error (ZAP3017). This closes a latent gap — `const` immutability is
  now enforced for **local** `const`s too (D-0003), which was previously
  unchecked.
- It is a **module member**: visible to the module's own code, mangled with the
  module prefix, and reachable across modules as `alias.NAME` unless `_`-private
  (D-0028/D-0081).
- A **module-level mutable** declaration (`T NAME = expr`, or any
  tuple-destructuring declaration, at module scope) is the Phase 2 surface and
  is rejected now with **ZAP3018** (pointing the author at #658 Phase 2), rather
  than the blanket executable-statement ZAP3016.

### Lowering (Phase 1)

The module linker (D-0081) emits merged items in import-topological order, so a
module `const` appears in init order. AST→ZIR lowering gives each module `const`
a **global slot** (`ir_core::Module::globals`) and synthesizes
`@__zap_init_globals`, a void function that evaluates each initializer in order
and stores it into its slot (reusing the ordinary `let` lowering: declared-type
coercion or `auto` inference, value-struct copy); a pointer-typed slot is
registered with `zap_gc_add_root`. A reference to a `const` lowers to a
`GlobalAddr` + `Load`, exactly as a stack local lowers to its `alloca` + load.
`main` calls `@__zap_init_globals` once at its top, under the top-level landing
pad, so an initializer that raises unwinds like any other code. Codegen emits
one zero/null-initialized internal LLVM global per slot; under Boehm
`zap_gc_add_root` is a real exported no-op (the symbol must link), under Immix it
records the slot.

### Diagnostics

- **ZAP3017** (`assignment to a constant binding`) — reassigning a `const`
  (module-level or local), plain/augmented/chained/tuple-target.
- **ZAP3018** (`module-level tuple-destructuring declaration not supported`) — a
  module-level tuple-destructuring declaration (`T a, b = expr`). Single-name
  module-level bindings — `const` (#658) and mutable (#662) — are admitted;
  tuple destructuring at module scope is not (declare each on its own line).
  (In #658 this code originally covered the not-yet-supported mutable form; #662
  narrows it to the one declaration-with-initializer form still rejected.)

### Verification

Parser unit tests already cover the module-level `const` parse (it was always
parseable). Sema unit tests cover module `const` visibility across functions
(including a function declared *before* the `const`), scalar and reference
types, `auto` inference, an initializer reading an earlier `const`, the
type-mismatch rejection (ZAP4001), and reassignment rejection (ZAP3017) for both
module and local `const`s (plain and augmented and chained), plus that mutable
bindings stay freely reassignable and a `const` may shadow a mutable binding.
Module-linker tests cover a module `const` linking in the entry and in an
imported module, cross-module access with prefix mangling and dependency-first
order, and the ZAP3018 mutable-global rejection. IR tests assert the global
slot, the `@__zap_init_globals` store, the GC-root call for reference types,
source-order init, and that a program with no `const` emits no init function.
End-to-end goldens (`module_const`, `module_const_eval_once`,
`module_const_reassign`, and the `module_const_across_modules` project) run
under the full-codegen CI jobs.

### Phase 2 (I#662) — module-level mutable globals (what ships now)

A **module-level mutable binding** is admitted at module top level, alongside the
`const` form:

```python
int64 counter = 0
list[int64] log = []
auto total = 0              # `auto` infers the type; mutable
```

- It is the **same machinery** as the `const` form (#658), minus immutability:
  the module linker admits any *single-name* module binding (const or mutable)
  and keeps rejecting loose executable statements; sema declares it up front in
  the module scope (mutable, so reassignable) and type-checks its initializer in
  source order; lowering gives it a module global slot in `@__zap_init_globals`
  (eager, import-topological + source order), roots a pointer-typed slot, and
  reads it via `GlobalAddr` + `Load`. The **one addition** is the write path: a
  reassignment or augmented/chained/tuple-leaf assignment to a name that resolves
  to a module global lowers to `GlobalAddr` + `Store` through the slot (a new
  `name_lvalue` helper resolves a name to a local alloca else the global slot;
  sema has already rejected storing to a `const`, ZAP3017).
- **No `global` keyword** (D-0021/D-0090): a bare assignment to a name that
  resolves to a module binding reassigns it in place (static lexical scoping). A
  function-local that shadows a module binding is introduced by *declaring* it
  (`auto x = ...`). Mutating container contents (`xs.append(...)`, `d[k] = v`)
  goes through the ordinary read path (the global's pointer) and needs no write
  path at all.
- **GC.** A reference-typed mutable global is a permanent GC root exactly as a
  `const` one is: `@__zap_init_globals` registers the slot's **address** with
  `zap_gc_add_root`. Because the collector re-scans each registered slot address
  on every (stop-the-world) collection — and relocates through it under precise
  Immix — reassigning the slot or growing the container it holds is correct under
  **both** collectors today with no write barrier. The precise-Immix write
  barriers that a future generational/concurrent collector would need are tracked
  under **#663** and are explicitly out of scope here.
- **Scope.** Only single-name bindings; a module-level tuple-destructuring
  declaration stays rejected (ZAP3018). The concurrency-sharing lint and the
  `@shareable` marker — locked-design points 4–5 above — **now ship in D-0113
  (#664)**; the runtime `-race` detector (point 6) remains a later #661 phase.

**Verification (Phase 2).** Sema unit tests cover a mutable global's
reassignability across functions (plain/chained/augmented), reference-type
content mutation and whole reassignment, `auto` inference, the type-mismatch
rejection on declaration *and* on reassignment (ZAP4001), an initializer reading
an earlier module binding, and local-declaration shadowing. Module-linker tests
cover a mutable global linking (entry and imported), cross-module access with
prefix mangling and dependency-first order, mutable bindings placed between
`def`s, and the ZAP3018 tuple-destructuring rejection (with the executable-
statement ZAP3016 still firing for a bare statement). IR tests assert the global
slot, that reassignment and `+=` store *through* the slot (`GlobalAddr` +
`Store`, read-modify-write), the GC-root call for a reference-typed mutable
global, and const/mutable coexistence in one slot table. End-to-end goldens
(`module_mutable_global` — read/reassign/mutate/persist a scalar + list;
`module_mutable_mutation` — dict contents + whole reassignment;
`module_mutable_init_order`; `module_mutable_gc_root` — a growing global survives
heavy garbage, asserting length+sum, never heap bytes; `module_tuple_decl_rejected`;
and the `module_mutable_across_modules` project) run under the full-codegen CI
jobs (both collectors).

### Precise-Immix static roots + write barrier (I#663)

Point 7 above requires module-global pointer slots to be correct under the
precise Immix collector (D-0009), not only conservative Boehm. #663 settles the
precise-side rule in two parts:

- **Static root table (no new code — a property we now rely on and test).** Every
  pointer-typed module-global slot is registered with `zap_gc_add_root` by
  `@__zap_init_globals` (Phase 1/2 already do this). Under Immix that call appends
  the slot's **address** to the runtime's static root table (`zap_groots`); the
  collector walks the table at each GC and processes every entry **move-aware**
  (`zap_process_slot`): it marks the referent and, if the referent was evacuated,
  rewrites the slot in place. So a structure held only via a module global is
  never collected and is **never left with a stale pointer after evacuation** —
  the slot is updated to the relocated address. Scalar/non-pointer globals get no
  entry (nothing to trace). Boehm needs no table: it scans the data/BSS segment
  conservatively, so `zap_gc_add_root` is an exported no-op there.
- **Write barrier (the hook #663 adds).** Reassigning a **mutable** module-global
  *pointer* (`NAME = expr`, plus augmented / chained / tuple-leaf) overwrites a
  permanent-root slot, installing a new old→new edge out of an immortal root.
  Each such store is followed by a `zap_gc_write_barrier(&slot)` call. An
  immutable reference `const` needs the root-table entry but **no** barrier — it
  is stored once at init and never reassigned (ZAP3017). The one-time initializer
  store in `@__zap_init_globals` is likewise **unbarriered**: it runs before
  `main`, so there is no prior value and nothing to remember (startup
  consistency). A local pointer reassignment gets no barrier either (it is a
  shadow-stack root, re-scanned precisely each GC, not a permanent root).

  The barrier is a **post-write card-mark** shape (it takes the slot address, so
  a collector can read the new referent). Today's Immix is stop-the-world
  mark/evacuate and re-scans every static root on every collection, so there is
  nothing to remember and the barrier is a **linkable runtime no-op** — real
  functions, not macros, because codegen emits a `call zap_gc_write_barrier` at
  each reassignment site (and it is an exported no-op under Boehm). It is the
  single named record point a future **generational or incremental** Immix mode
  fills in: such a mode does not re-scan every immortal root each minor cycle, so
  the old→new edge must be remembered there. Keeping the call site and symbol in
  place now makes that evolution a change to one runtime function, with no new
  codegen or ABI. (See `docs/design/c-ffi.md` §2.3 for the matching FFI rule.)

**Verification (#663).** IR tests assert that a mutable pointer-global
reassignment emits exactly one `zap_gc_write_barrier` on the slot **after** the
store, that an augmented reassignment of a pointer global fires it too, and that
a scalar global, a *local* pointer reassignment, an immutable reference `const`,
and the `@__zap_init_globals` init store all emit **none**. End-to-end goldens
run under **both** collectors with contents/identity assertions only (never heap
bytes): `module_global_reassign_across_gc` reassigns a `list[str]` global across
forced collections under garbage pressure and reads the relocated elements back;
`module_const_global_evac_survives` keeps a nested `const list[list[str]]` intact
across eight collections. An Immix-only reclamation test
(`immix_module_global_reassignment_roots_only_current_value`, under a heap cap)
proves reassignment drops the prior value so it is reclaimed — the root tracks
only the live slot contents, with no stale rooting.

## D-0113 — Concurrency-safety surface: `@shareable` marker + static sharing lint (I#664, epic #661)

This **implements locked-design points 4–5 of D-0112 (#661)**: the advisory
concurrency-safety surface for the light model, shipped as a diagnostic — never
a dialect. Module-level mutable globals (D-0112 Phase 2, #662) are legal and
unsynchronized; this adds a way to *mark* a type safe to share and a way to
*warn* about careless shared mutable state, without ever changing what the
language accepts.

### `@shareable` marker

A nominal, metadata-only annotation (D-0036) a type author applies to a
`struct`:

```python
@shareable
struct Registry:
    Mutex guard
    dict[str, int64] entries
```

- It **asserts** — as a *trusted, unverified* claim — that the type is safe
  under concurrent shared access. The compiler does **not** attempt to prove it;
  there is no analysis of the struct's internals. The author owns the claim, and
  the runtime `-race` detector (#661 point 6, a follow-up) is the backstop.
- It takes **no arguments** and applies to a **`struct`** only: `@shareable` on a
  function/field/enum is ZAP5047, and `@shareable(...)` with arguments is
  ZAP5041.
- It is **reflectable**: `reflect_has(T, "shareable")` is true for a marked
  struct (recorded in the runtime reflection registry like any annotation) and
  for the built-in shareable types below.

### Built-in shareable types — one uniform rule, no allowlist in the lint

The concurrency primitives and concurrent collections carry the marker
**implicitly**: `Atomic` (prelude `Atomic[T]`, D-0066), `Mutex`/`WaitGroup`/
`Once` (`sync.*`, D-0063), and `Counter`/`Queue` (`concurrent.*`, D-0078). The
single shared list lives at `zapc_syntax::BUILTIN_SHAREABLE_TYPES`. The lint
does **not** special-case these types: it seeds its *one* shareability predicate
(`name_is_shareable`) with that list, so a user `@shareable` struct and a
built-in primitive are decided by the same membership test. Shareability is
decided on a type's **nominal base name**, so the generic spelling (`Atomic[int64]`,
`Queue[str]`) is covered by its base. For reflection these built-ins have no
registry entry, so `reflect_has("Atomic", "shareable")` (string-literal
arguments) is **folded to a compile-time `true`** in lowering — which also keeps
the `main` prologue of reflection-free programs byte-identical (golden-stable).

### Sharing lint — ZAP5125 (a warning)

A static analysis in sema emits **ZAP5125** (`shared mutable global is not
@shareable`, the ZAP5xxx semantic band) at each **read or write** of a module
global when, and only when:

- the global is **mutable** (a `const` global can never race → **never warns**),
  **and**
- its type is **not `@shareable`** (a mutable global of a `@shareable` type, or
  of a built-in `Atomic`/`Mutex`/concurrent type, is asserted safe → silent).

The warning fires only for accesses **inside a function/method body**: the
single-threaded, ordered module-init pre-pass is exempt (an initializer reading
an earlier global is not a concurrent access). A name that resolves to a
*shadowing local* is not a shared-state access and does not warn. An augmented
assignment (`g += 1`) is one read-modify-write access → one warning. It reuses
the existing `warning[ZAPnnnn]` pipeline (#659); it is **non-fatal** and does
not change what compiles.

### Suppression — a per-site `# noqa` pragma

A trailing line comment suppresses the lint at one site, matching the Python
convention the issue references:

- `# noqa` — suppress every warning on that line.
- `# noqa: ZAP5125[, ZAP5126, …]` — suppress only the listed codes.

Suppression is **line-based** (keyed on the warning's start line) and
**warning-only** — errors are never suppressible. It is a single pure,
source-driven filter (`zapc_sema::apply_suppressions`) shared by the in-process
test path and the `zap` driver, so a sema test observes exactly what a build
produces. (A new, minimal mechanism: Zap had no prior suppression surface.)

### Policy — `--warnings-as-errors` (`-Werror`), off by default

A general build knob on `run`/`build`/`ir`: when set, any warning that survives
suppression is promoted to a **fatal** error (non-zero exit), letting a team make
the sharing lint — and every other warning — fatal in their own CI. It is **off
by default** and **never changes what the language considers legal**: the same
program compiles with or without it; only the build's exit policy differs.

### Diagnostics

- **ZAP5125** (`shared mutable global is not @shareable`) — a non-fatal warning
  at a read/write of a mutable, non-`@shareable` module global.
- **ZAP5047 / ZAP5041** — reused for a misapplied `@shareable` (wrong target /
  arguments).

### Verification

Sema unit tests cover every fires/silent case: a bare mutable global's read,
write and augmented-assign; reference-typed and `auto`-typed globals; a `const`
global (silent); a `@shareable` user struct vs the same struct unmarked; every
built-in shareable type; a shadowing local and plain locals (silent); the
init-pre-pass exemption; the marker's wrong-target/argument rejections; and the
`# noqa` suppression (bare, code-specific, unrelated-code, and per-line
isolation), plus a standalone test of the `apply_suppressions` filter. IR tests
assert a `@shareable` struct reaches the reflection registry and that
`reflect_has("<builtin>", "shareable")` folds to a constant with no runtime call.
Driver integration tests (host-safe, via `zap ir`) assert the lint warns but the
build succeeds by default, that `--warnings-as-errors`/`-Werror` makes it fatal,
that a `const` global and a `# noqa`-suppressed site stay non-fatal even under
`-Werror`. An end-to-end golden (`shareable_reflect`) checks runtime
`reflect_has` for a marked struct, an unmarked struct and the built-ins.

## D-0114 — Runtime data-race detector (`--race`): native happens-before, sliced (I#665, epic #661)

This settles **locked-design point 6 of D-0112 (#661)** — the runtime `-race`
detector, the *dynamic* backstop for the light concurrency model. The light
model (D-0112) deliberately does **not** synchronize shared mutable module
globals; D-0113 (#664) added the *static* sharing lint (ZAP5125) that flags the
obvious unsynchronized accesses at compile time. `--race` is the *dynamic*
complement: at test time it observes the actual interleavings a run exhibits and
reports a true data race — two accesses to the same shared location, at least
one a write, with no happens-before edge between them — with a source location.
It is in the spirit of Go's `-race` and Clang/LLVM ThreadSanitizer (TSan).

Like the lint, `--race` is a **diagnostic, never a dialect** (D-0112 point 4):
off by default, enabled per-invocation on `zap run/test/build/ir --race` (alias
`-race`), and it **never changes what the language accepts** — only whether
accesses are instrumented and checked at run time.

### Decision 1 — a Zap-native happens-before detector, NOT LLVM/Clang TSan

The open question in #665 was whether to build on `-fsanitize=thread` (TSan) or a
Zap-native vector-clock detector. **We build native.** TSan is rejected for this
runtime, for reasons that are structural rather than incidental:

- **TSan assumes pthreads; Zap concurrency is user-space coroutines.** The
  scheduler (D-0, #47) is an M:P:G work-stealing runtime whose context switches
  are `swapcontext` on GC-allocated green stacks (`switch_to_g` in `zaprt.c`).
  To TSan, two coroutines time-sliced onto **one** OS thread are one thread, so
  a race between them is invisible unless **every** switch is annotated with
  `__tsan_switch_to_fiber` / `__tsan_create_fiber` / `__tsan_destroy_fiber`.
  That is a deep, fragile coupling to TSan's fiber ABI at the hottest point of
  the scheduler, and it has to be exactly right or the detector silently misses
  races (worse than no detector).
- **TSan assumes libc `malloc`; Zap objects live on the GC heap.** TSan tracks
  shadow memory for `malloc`'d regions and intercepts the allocator. Zap values
  are `GC_malloc`'d (conservative Boehm) or Immix-allocated (precise, D-0009),
  not visible to TSan's interceptors, and TSan vs. a moving/scanning collector
  (bdwgc's own stop-the-world, Immix evacuation) is a known-bad interaction —
  two runtimes each wanting to stop threads and walk stacks.
- **Whole-program rebuild + overhead profile.** TSan requires compiling the
  program *and* the entire C runtime with `-fsanitize=thread`, a fixed shadow VM
  mapping, large binaries, and a 5–15× slowdown — and after all that it would
  still be blind to coroutine switches without the fiber annotations above.
- **Go's own precedent points native.** Go did not simply pass `-race` to the C
  compiler; it ported TSan's race *algorithm* into the Go runtime and has the
  compiler emit explicit `raceread`/`racewrite`/`raceacquire`/`racerelease`
  hooks tied to Go's scheduler and sync primitives. Zap is in exactly that
  position, and already has the two things that make it tractable: the scheduler
  funnels **every** context switch through `switch_to_g`, and (post #662/#663)
  **every** shared-global access funnels through a single pair of lowering
  helpers (`name_lvalue` read / `store_name` write).

The native detector is a **vector-clock happens-before** engine (Lamport/
Fidge–Mattern, the Go/TSan model): one logical clock per goroutine `G`; a
per-synchronization-object clock; **release** on a "send"-side sync op and
**acquire** on the matching "receive"-side op propagate clocks; each instrumented
memory location keeps shadow state (last writer `G` + its clock, last readers);
an access races iff it is not ordered after every conflicting prior access.

### Decision 2 — instrumentation scope

Instrument exactly what the light model calls "shared", widening later:

- **v-first: module globals.** The only shared mutable state the model admits
  today (D-0112). Scope is **exactly the ZAP5125 set** — a mutable, non-const,
  non-`@shareable` module global (scalar *or* reference). A `const` global never
  races; a `@shareable`-typed global (user `@shareable` struct or a built-in
  `Atomic`/`Mutex`/`WaitGroup`/`Once`/… from `BUILTIN_SHAREABLE_TYPES`) is an
  asserted-safe cell and is **not** instrumented — so an `Atomic`-guarded
  counter is clean *by construction* (its accesses are atomic ops on a
  shareable-typed cell), exactly mirroring the lint.
- **later: shared heap objects** reachable across coroutines (object headers get
  shadow state; the instrumented access points move from the global helpers to
  field/element load/store). Out of scope until the global case is proven.

**Happens-before edges** come from the sync primitives, which all already funnel
through runtime C: coroutine **spawn** (parent→child), **channel** send/recv
(D-0, #47), **`Mutex`** lock/unlock and **`WaitGroup`**/`Once` (#103), and
**`Atomic`** ops (#176, each a release+acquire on its own cell). The engine
serializes its own shadow-state updates under a single detector lock: a
test-time diagnostic optimizes for **obvious correctness over speed**, which
also sidesteps the hardest native-detector bug (racing on the detector's own
metadata under real `MAXPROCS>1` parallelism).

### Decision 3 — overhead budget

`--race` is a **test-time** tool, never a production build mode. Budget: a
single-digit-to-~10× slowdown on an instrumented run is acceptable (Go-class),
because it runs under `zap test`/`zap run`, not in shipped binaries. A default
(no-`--race`) build is **byte-identical** to today — no instrumentation is
emitted and the `main` prologue is unchanged (golden-safe), the same discipline
#664 used for the reflection fold.

### Scope — this is sliced; #665 is recommended for promotion to its own epic

Per #665 ("a larger, likely multi-slice effort — it may be promoted to its own
epic once scoped") and the workspace rule that a multi-issue feature gets an
Epic with trackable sub-issues, the full detector does **not** land as one
reviewable PR: the vector-clock engine + happens-before hooks across the
~17k-line scheduler-coupled runtime, validatable only in CI, is precisely the
case that should be reviewed in focused pieces. The slicing is **recommended to
the maintainer** (not unilaterally created).

**v1 (this PR, I#665) — the design + the flag + the harness.** Lands:
- this decision (D-0114);
- the `--race` / `-race` flag plumbed end-to-end on `run`/`build`/`ir`/`test`
  (off by default; help text; parse tests). Until the engine lands the flag is
  **accepted but inert**, surfaced with an explicit one-line notice rather than
  silently ignored, so default builds and all existing goldens are unaffected;
- the racy-vs-clean fixture pair as the walled-garden spec for the detector
  (`tests/race/`), with the expected report documented for the engine slice.

**Deferred (recommended sub-issues under #665-as-epic):**
1. **Access instrumentation. ✅ Landed — I#677 (slice 1).** Behind `--race`,
   emit `__zap_race_read` / `__zap_race_write(slot, name_id, line, col)` calls at
   the ZAP5125 access sites in lowering (the `expr` global read / `store_name`
   write), to no-op runtime stubs; ZIR unit tests assert emitted/not-emitted ==
   lint scope (host-testable). See the slice-1 note below.
2. **Happens-before engine. ✅ Landed — I#678 (slice 2).** The per-`G` vector
   clock (`G.race_gid` + a gid-indexed clock table), per-sync clocks, and the
   single-lock shadow-state check + report formatting + non-zero exit, in
   `zaprace.inc.c`; HB hooks at spawn and `Mutex` lock/unlock (enough for the
   #665 "Done when"); racy fixtures → reported with location, mutex/atomic-guarded
   → clean. CI-gated golden under both collectors. See the slice-2 note below.
3. **Breadth. ✅ Landed (edges + hardening) — I#679 (slice 3).** Channel
   (rendezvous + buffered), `WaitGroup` (`done`→`wait`) and `Once`
   (`once_end`→every waiter) HB edges, all wired in the runtime; `MAXPROCS>1`
   validation of the single-lock engine across the whole fixture set; a
   deterministic report enrichment (each racing access also names its coroutine
   function). **Shared heap-object instrumentation is split out** into its own
   slice-4 issue (#684; shadow state on heap objects + field/element lowering is a
   materially larger, GC-coupled change — "the big one" this issue flagged); see
   the slice-3 note below. Full multi-frame green-stack traces depend on the
   I#475 green-stack unwinder and are deferred with it.
4. **Shared heap objects. ✅ Landed — I#684 (slice 4).** Extends instrumentation
   from the ZAP5125 module-global set to **shared heap objects**: user-source
   struct-field and list-element load/store now emit the access hooks under
   `--race`. Shadow state is keyed on the **stable object-header identity +
   sub-index** (not the raw, relocatable field/element address), making it
   survive precise-Immix evacuation (#120) by construction, with a
   `zap_race_on_move` evacuator hook as the general relocation path; the slice-3
   happens-before engine and single lock are unchanged. A shared-heap race
   (list element or struct field) is now reported with a location and exits
   non-zero; a `Mutex`/`concurrent`-collection-guarded one stays clean. See the
   slice-4 note below.

### Done-when (v1, satisfied by this PR)

The `--race` surface exists and is plumbed on every relevant subcommand, is off
by default, never changes legality, and the design (native, scope, budget,
slicing) is recorded; the racy/clean fixtures are checked in as the detector's
walled garden. The *detection* behaviour (racy → reported with location;
`Mutex`/`Atomic`-guarded → clean) is specified here and realized by deferred
slices 1–2.

### Verification (v1)

Host-only (Docker/LLVM down locally; CI is the runtime gate): driver CLI parse
tests cover `--race`/`-race` on run/build/ir/test, composition with other flags,
off-by-default, and the `test --doc --race` rejection; `cargo build`/`test`,
`cargo fmt --all`, and clippy are clean. No grammar/SPEC-legality change (CLI +
runtime only). No core defects were uncovered.

### Slice 1 landed — I#677 (access instrumentation)

Behind `--race`, AST→ZIR lowering now emits a race access hook at exactly the
ZAP5125 set, calling runtime functions that are **no-op stubs** for now (the
happens-before engine is slice 2, #678). The flag, inert in v1, is now wired
through lowering; the accepted-but-inert notice is removed.

- **Emission predicate = lint scope.** A `__zap_race_read` is emitted at the
  `expr` global-read site, and a `__zap_race_write` inside `store_name` (the four
  name-target store sites: `=`, `+=`, chained, tuple-leaf), **iff** the name is a
  **mutable, non-`@shareable` module global** (scalar *or* reference), is **not**
  shadowed by a local, and the access is **not** in the single-threaded
  module-init pre-pass (`@__zap_init_globals`). This mirrors sema's ZAP5125
  predicate exactly (`share_lint_globals` ∧ `resolves_to_module_global` ∧
  `scopes.len() > 1`), recorded per-global at slot registration as a `racy` bit
  (`!const ∧ !shareable`). Crucially the gate is **broader than #663's write
  barrier**: it fires for a scalar global too (the canonical `int64` counter
  race), not only the pointer case `ty.is_ptr()` gates.
- **ABI.** `__zap_race_{read,write}(void *slot, i64 name_id, i64 line, i64 col)`:
  `slot` is the global's data-segment slot (the conflict location), `name_id` the
  global's slot index (a stable id), and `line`/`col` the access span's byte
  range for the engine to resolve to a source location. Emitted via the same
  `Inst::Call` mechanism #663 uses for `zap_gc_write_barrier`; codegen declares
  the externs next to it, **only** when the ZIR actually contains such a call.
- **Runtime.** A new `zaprace.inc.c` (`#include`d unconditionally by `zaprt.c`)
  defines both hooks as exported no-op stubs, collector-independent; slice 2
  fills in the vector-clock engine there.
- **Golden-safe invariant preserved.** With `--race` **off** lowering emits zero
  hooks and codegen declares no externs, so the ZIR and the `main` prologue are
  byte-identical to today (asserted by a unit test that compares the race-off ZIR
  to the default `lower_module` output). A `--race` build differs only by the
  added no-op calls.
- **Tests.** Exhaustive host-safe ir unit tests (`crates/ir/src/lib.rs`) over
  scalar/ref global, `const`, user `@shareable` struct, shadowing local,
  init-prepass, and read vs each write form (plain/aug/chained/tuple); a
  CI-gated golden (`crates/driver/tests/golden.rs`) that a `--race` build of each
  `tests/race/*.zap` links and runs identically to a non-`--race` run (the stubs
  are inert). Built-in-`@shareable` exclusion shares the `name_is_shareable` path
  and is covered end-to-end by the `clean_atomic.zap` golden.

### Slice 2 landed — I#678 (happens-before engine + reporting)

The slice-1 no-op stubs are replaced by a real **Zap-native happens-before /
vector-clock** engine in `zaprace.inc.c`, so the #665 v1 "Done when" holds: a
racy program is **reported with a source location** and exits non-zero, while a
`Mutex`/`Atomic`-guarded version is clean.

- **Engine.** Each coroutine `G` carries a vector clock, indexed by a dense
  goroutine id (`G.race_gid`, assigned at spawn and grown as coroutines are
  created). Each synchronization object (a `Mutex`) carries a clock for
  release/acquire. Each instrumented slot keeps shadow state — a per-coroutine
  **write** clock and **read** clock, plus the most recent writer/reader's source
  span for the report. An access **races** iff it is not ordered-after every
  conflicting prior access (write–write or read–write) under happens-before
  (the DJIT+ full-vector-clock check).
- **Happens-before edges (minimal v1 set).** **spawn** — the parent's clock is
  copied into the child at creation (`zap_race_on_spawn`, from
  `zap_spawn`/`zap_spawn_detached`); **`Mutex`** — `unlock` = release (publish
  the holder's clock into the lock), `lock` = acquire (join the lock's clock into
  the locker), hooked in `zap_mutex_lock`/`zap_mutex_unlock`; **`Atomic`** —
  clean by exemption (a `@shareable` cell is not instrumented at all). Channel /
  `WaitGroup` / `Once` edges, and tracking the **main/native thread's** own
  accesses, are slice 3 (#679) — which is why the `WaitGroup`-joined `main` read
  in the `clean_*` fixtures stays clean without a join edge: v1 tracks races
  **between spawned coroutines**, and an access from an untracked context (no
  `G`) is skipped.
- **Single detector lock.** All engine state (every vector clock, slot shadow
  state, the gid/name tables) is serialized under one `race_mu`, taken via the
  preemption-guarded `zap_np_lock` so a signal-preempted holder cannot deadlock a
  switched-in sibling on the same M. Obvious correctness over speed; safe under
  `MAXPROCS>1`. The release/acquire **order** comes from the real `Mutex` and the
  scheduler's direct handoff, not from `race_mu`.
- **Reporting.** On the first racing access the engine prints a diagnostic
  naming the global (resolved from `name_id` via `__zap_race_register_global`,
  emitted per-global in `main` under `--race`) and both accesses' source byte
  spans (the slice-1 `line`/`col` ABI — byte offsets, not true line/column),
  then `_exit(66)` (Go's race-exit code). No heap address, pointer, or timing
  value is printed, so the report is a stable golden. The verdict is
  **schedule-independent** (it reflects the absence of an ordering edge, not a
  particular interleaving). This is diagnostic output (STABILITY: not stabilized).
- **Codegen.** Two more generated-code entry points join slice 1's read/write
  hooks, emitted in `main` under `--race`: `__zap_race_enable()` (arm the engine
  before any spawn) and `__zap_race_register_global(idx, name)`. All carry the
  `__zap_race_` prefix, so the "race ZIR minus the race hooks == off ZIR"
  invariant and the byte-identical non-`--race` build are preserved; the spawn
  and `Mutex` edges are wired entirely in the runtime (no LLVM declaration).
- **Tests.** The host-safe ir unit tests gain bootstrap coverage (enable +
  per-global registration emitted in `main`, only under `--race`). The CI-gated
  golden (`race_detector_reports_races_and_clears_clean_programs`,
  `crates/driver/tests/golden.rs`, both collectors) now asserts the real
  verdicts: `racy_counter` / `racy_read_write` / `racy_write_write` report a race
  on `counter` (location-tolerant, no nondeterministic value) and exit non-zero;
  `clean_mutex` (100000), `clean_atomic` (100000), `clean_single` (50000) run
  clean and print their exact total.

### Slice 3 landed — I#679 (detector breadth: channel / WaitGroup / Once edges, hardening)

Widens the slice-2 engine from the minimal spawn + `Mutex` edge set to the rest
of the light model's synchronization, plus hardening. All new edges are wired
**entirely in the runtime** (the scheduler and `sync.*` primitives call the
engine directly), so codegen emits nothing new and a non-`--race` build stays
byte-identical; the hooks are inert until an access hook / register-global arms
the engine, exactly like the slice-2 spawn/`Mutex` edges.

- **Channels (scheduler, #47).** A send happens-before the matching receive.
  - *Rendezvous* (unbuffered, or the direct sender↔waiting-receiver hand-off a
    buffered channel also takes) transfers the sender coroutine's clock into the
    receiver's at the hand-off (`zap_race_chan_xfer`).
  - *Buffered* splits the edge across the physical ring-buffer slot: the producer
    stamps the slot with its clock (`zap_race_chan_buf_store`, an **overwrite** so
    a later reuse of the same slot fabricates no edge), the consumer joins it on
    drain (`zap_race_chan_buf_load`). This models send-of-value-k → receive-of-k.
    The weaker back-pressure edge (receive-k → send-of-k+capacity) is deliberately
    **not** modeled: omitting a real edge only makes the detector more eager to
    report, never hides a race — the safe direction for a test-time tool. (The
    async-signal `chan_offer_word` delivery path is **not** hooked: taking
    `race_mu` in a signal context is unsafe, and signal-delivered values are out
    of scope.)
- **`WaitGroup` (#103).** Every `done()` (the decrement) publishes the caller's
  clock into the group (`zap_race_wg_release`); every `wait()` that returns
  observes it (`zap_race_wg_acquire`, on both the fast path and the woken path).
  So all `done()`s happen-before a `wait()` returns. An incrementing `add(+n)`
  carries no edge.
- **`Once` (#103).** The winner's `once_end` publishes its clock
  (`zap_race_once_release`, before any loser is woken); every loser's
  `once_begin` that returns 0 — whether it parked until completion or arrived
  after — observes it (`zap_race_once_acquire`). So the initializer happens-before
  every `once_begin` return past it. The winning caller needs no edge (it ran the
  initializer itself).
- **Parked-coroutine reach.** The slice-2 lazy-gid logic is factored into
  `race_vc_of_g(G*)` so a hand-off edge can reach a *parked* counterpart (a
  channel peer blocked in `copark`, a `wait()`er) that is not the running
  coroutine. The **main/native thread stays untracked** (no `G`): this slice does
  not add main-thread tracking, so a genuine main-vs-coroutine race remains the
  documented v1 limitation, and the pre-slice-3 `clean_*` fixtures (whose `main`
  reads after a join) stay clean for the same reason. The new `clean_*` fixtures
  therefore put **both** the synchronizing write and read in spawned coroutines,
  so each new edge is genuinely exercised rather than masked by an untracked
  `main`.
- **Hardening — `MAXPROCS>1`.** The single global `race_mu` is validated under
  true OS-thread parallelism across the whole fixture set (a second golden runs
  every fixture with `ZAP_MAXPROCS=4`). The verdict is schedule-independent, so a
  parallel run must match the serial one exactly; the single lock is left as is
  (correct and simplest — only a perf trade-off, acceptable for a test-time
  tool).
- **Hardening — reports.** The report now also names each racing access's
  coroutine function (`G.name`, a deterministic address-free identifier) beside
  its id and byte span. Full per-access **multi-frame stack traces** need the
  green-stack unwinder tracked by I#475 and are deferred with it.
- **Shared heap objects — SPLIT OUT.** Extending instrumentation beyond module
  globals to heap objects (shadow state on object headers + instrumenting
  field/element load/store in lowering) is a materially larger, GC-coupled change
  — shadow keyed by heap address interacts with Immix evacuation (#120), and the
  lowering touches many sites. Per this issue's own guidance ("the big one … may
  itself be sub-sliced"), it is filed as its own **slice-4** issue of epic #665 (#684)
  (one-issue-one-PR), where the "a shared-heap race is reported with a location"
  done-when belongs. Nothing here works around its absence.
- **Tests.** New `tests/race/` fixtures: `clean_chan_rendezvous` (43),
  `clean_chan_buffered` (1098, cap-8 ring reused ~125×), `clean_waitgroup` (77,
  a coroutine joiner), `clean_once` (246, guarded init read by two coroutines),
  and `racy_chan` — a **soundness guard** that a channel present in a program does
  not fabricate an edge between two coroutines that race on a global (the race
  must still be reported). A second CI golden
  (`race_detector_correct_under_parallelism`) runs the whole suite under
  `ZAP_MAXPROCS=4`. A host-safe guard (`race_flag_is_honored_by_zapc_binary`)
  asserts `zapc ir --race` emits a `__zap_race_write` hook and a plain `zapc ir`
  emits none — a loud regression tripwire for the #678 dropped-flag silent no-op.

### Slice 4 landed — I#684 (shared heap-object instrumentation)

Widens *what* is instrumented from the ZAP5125 module-global set to **shared heap
objects**, without touching the slice-3 happens-before model. The hard part was
not the lowering but keeping shadow state correct across a **moving GC**: the
slice-2/3 engine keys shadow by a stable data-segment slot address, but a heap
location's address is **not** stable — precise-Immix evacuation (#120) relocates
objects, so an address-keyed shadow map would alias or lose state across a GC.

- **Shadow key = stable object identity, not raw address.** The `RaceSlot`
  conflict key becomes the pair **(`slot`, `sub`)**. For a heap access `slot` is
  the **object header** pointer and `sub` the field/element index; for a module
  global it stays the data-segment address with `sub = 0` (unchanged). Keying on
  the header's identity is relocation-safe **by construction**: the v1 evacuator
  moves only `zap_desc_atomic` **leaf buffers** (a container's backing buffer,
  string / numeric-array data) — never the precise, marked-in-place header
  object (user structs get a `ZAP_TD_PRECISE` descriptor; a `list`/`dict`/`set`
  header has a pointer child, so it too is marked in place). So a `list`'s
  backing buffer can be evacuated freely and `(list header, index)` is invariant
  across the move — side-stepping the alias/loss hazard with **no object-header
  surgery**.
- **Why not a per-object header shadow word.** The alternative the issue floats —
  a shadow word on the object header that the evacuator copies — was **rejected**:
  the 16-byte object header is fully occupied (`desc|mark` in word 0, payload
  `size` / forwarding-target in word 1), so adding a word is a core-GC **layout**
  change (header size, 16-byte payload alignment, the Boehm layout, the
  allocator, and line-marking). That is core-collector work beyond a tooling
  slice; per the AGENTS.md golden rule we do **not** hand-roll header surgery the
  GC is unaware of. Stable-identity keying needs none.
- **Relocation path (`zap_race_on_move`), collector-agnostic.** As the general
  mechanism (and the issue's requested "evacuator updates the side table on the
  copy path"), the precise-Immix evacuator (`zap_evac_copy`) calls
  `zap_race_on_move(old, new, size)` for **every** object it moves, before it
  installs forwarding; the engine re-keys any shadow entry whose base lies in the
  moved payload range by the move delta. Because the keyed bases are precise
  headers / data-segment globals that v1 never moves, no entry is ever in a moved
  range today, so it is a **no-op migration** in v1 — but it keeps the table
  correct should the movable set ever widen to header objects, and it means the
  detector *participates in every evacuation* while shadowed objects are live.
  Boehm never moves, so never calls it: one definition, both collectors.
- **Hook ABI (additive, off-path byte-identical).** New
  `__zap_race_read_field` / `__zap_race_write_field(void *base, i64 sub,
  ZapStr *name, i64 line, i64 col)`; `name` is a static string literal naming the
  location (`Point.x`, `list element`) for the report. The module-global
  `__zap_race_{read,write}` 4-arg hooks are unchanged, so the existing global
  goldens' report text ("module global `…`") is untouched; heap reports read
  "shared heap location `…`". Every new call still carries the `__zap_race_`
  prefix, so the "race ZIR minus the race hooks == off ZIR" byte-identity
  invariant holds and a non-`--race` build is unchanged.
- **Instrumentation scope decision — INSTRUMENT-ALL, list + struct.** Lowering
  emits the heap hooks at every **user-source** reference-struct field and
  list-element load/store under `--race` (a *value* struct is inline/stack —
  unshareable by reference — and is skipped; compiler-generated struct methods
  like `__eq__`/display are not user accesses and are not instrumented). We chose
  **instrument-all** over cross-coroutine escape analysis because `--race` is an
  off-by-default, test-time diagnostic with a Go-class slowdown budget: soundness
  over speed, and escape analysis is a separate, larger effort that risks **false
  negatives** (missed races), which is worse than slow. An object only one
  coroutine touches simply never conflicts (single-coroutine shadow) → no false
  positive. **dict/set element** (key-based identity + rehash moving entries),
  **numeric-array element**, and **interface/boxed payloads** are a recorded
  **follow-up** (their stable key is the hash key, not a slot index — a distinct
  design question), out of scope here; the Done-when surface (list element +
  struct field) is covered.
- **Granularity caveat.** A list element is keyed by its raw (pre-normalization)
  index, so `xs[-1]` and `xs[len-1]` are distinct shadow keys for the one element
  — a documented v1 limit (it can only *miss* a race via one odd mixed-index
  pattern, never fabricate one).
- **Tests.** New `tests/race/` fixtures: `racy_heap_list` and `racy_heap_struct`
  (a shared list element / struct field raced by two coroutines → **reported**
  with a location, non-zero exit), `clean_heap_mutex` (same list element behind a
  shared `Mutex` → clean, 100000), `clean_heap_concurrent` (a shared
  `concurrent.Counter`, a `@shareable` type → exempt from instrumentation → clean,
  100000), and `clean_heap_evac` — the **relocation fixture**: a `Mutex`-guarded
  shared list mutated while garbage churn + `gc.collect()` drive evacuation out
  from under the running coroutines, so a shadowed object is live across a moving
  GC; clean and deterministic (40000) under both collectors. All run under the
  existing dual-collector and `ZAP_MAXPROCS=4` goldens. A shared-heap race being
  reported with a location is the slice-3 "Done when" line that was deferred to
  here. No core defects were uncovered.

## D-0115 — `@cache` built-in: memoization as `zap expand`-visible Zap over module-level state (I#666, epic #661; supersedes the D-0109 `@cache` deferral)

**Status:** Accepted. Closes the language work of epic #661 (module-level state)
by delivering its motivating consumer, the built-in **`@cache`** decorator.
Supersedes the `@cache` deferral in D-0109.

**Context.** D-0109 deferred `@cache` because memoization needs module-level
persistent state and that was not a language feature: a `zap expand`-visible,
type-checked implementation was impossible, and a hidden codegen-level store
would be a language-bypassing workaround (against the language-first policy).
Epic #661 then added exactly that state — module `const` (#658/D-0112), mutable
module globals (#662), precise-Immix static roots + write barrier (#663), and
the `@shareable` marker + ZAP5125 sharing lint (#664/D-0113). `@cache` is now
buildable honestly.

**Decision.** `@cache` is a **free-function decorator** that memoizes by the
decorated function's typed argument tuple. It is **generative** (D-0036):
applied to `def f(...)->R`, it is expanded **before sema** (in the driver's
behavioral-decorator pass, the same fold as user `before`/`after`/`wrap`, so it
stacks bottom-up with them) into ordinary, type-checked, `zap expand`-visible
Zap — **never a hidden compiler store** (a hard project rule):

- `@derive(Hash, Eq) struct f$Key { <one field per parameter> }` — the typed
  argument tuple, as a *named* struct. A named key-struct (with derived
  structural hash/eq) is used rather than a raw tuple key, because a bare tuple
  dict key currently ICEs (reported as **#680**, an independent core defect);
  the named-struct dict-key path is fully supported. This is a design choice,
  not a workaround for #680 — it does not touch the broken path.
- `@shareable struct f$Cache { bytes lock; dict[f$Key, R] data }` — the store. A
  bare mutable module dict would trip the #664 **ZAP5125** sharing lint; wrapping
  it in a `@shareable` struct (guarded by a `Mutex`) is the sanctioned
  shared-mutable form (D-0113).
- `f$Cache f$cache = f$Cache(sync.mutex(), {})` — an eagerly-initialized module
  global (#662).
- a wrapper with `f`'s exact signature: build the key, lock, return a hit;
  otherwise unlock, compute `f$orig(...)`, lock, insert, unlock, return.

**Key (value-hashability).** The key is the argument tuple, and it must be
**value-hashable**. In v1 every parameter must be an **integer, `bool`, or
`str`** — the exact set the generated key-struct's `@derive(Hash, Eq)` hashes by
*value* (`str` is a reference but is hashed/compared by content). A **reference
type** — a user `struct`/`enum`, or a collection — is **rejected** (not keyed by
identity), with the dedicated **ZAP5056** diagnostic. This is deliberate: a
`struct`/`enum` key would otherwise be hashed by **identity**, so two
equal-but-distinct arguments would never hit and the cache would *silently give
no benefit while looking correct* — the exact footgun called out in review — and
in fact `@derive(Hash, Eq)` cannot hash a nested struct field by value today, so
an accepted reference key would surface a confusing error about the internal
`<fn>$Key` struct instead. `float`/`bytes`/`char`/`complex`/`bigint`, a bare
tuple, an optional, and a function value are likewise not v1 keys; so are a
variadic/`**kwargs` collector, a zero-parameter/no-return/generic function, and
any `@cache` argument — all **ZAP5056**, never an ICE. The check is an
**allowlist** (only the value-hashable primitives pass), because a user
struct/enum name is an arbitrary identifier that a denylist could not catch;
value-hashable types not yet listed (e.g. a tier-1 `enum`) are a safe, clear
false-negative, and widening the set — or adding structural struct/enum keys
once `@derive(Hash)` hashes nested structs by value — is a future extension.

**Eviction (v1).** **Unbounded for the process lifetime.** The global is an
immortal GC root (#661/#663 note), so every entry stays reachable until the
process exits — a deliberate, documented **memory cost**, not a leak. This is
the simplest correct v1; a bounded/LRU variant (`@cache(maxsize=...)`) is a
possible future extension and is why v1 rejects arguments rather than silently
ignoring them.

**Concurrency.** The store is shared across coroutines, so it is guarded by a
`Mutex` (the `@shareable` assertion is honest because every access holds the
lock). The miss result is computed **outside** the lock: holding it across the
user call would deadlock a recursive `@cache` function (the recursive call
re-enters the wrapper and re-takes the lock) and would serialize unrelated
computations. The lock therefore guards only the dict bookkeeping — the same
choice as CPython's `functools.lru_cache`. Consequence: under contention two
coroutines may both miss and both compute a given key (the second insert wins);
for a pure function this is a benign recompute, not a correctness bug. The
single-coroutine goldens observe exactly-once computation.

**GC.** Entries are reachable from the immortal global, so they live until the
process exits (or an eviction policy removes them, if one is ever added).

**Alternatives rejected.** (a) A hidden compiler-level memo table — violates the
language-first / `zap expand`-visible rule (the whole reason D-0109 deferred).
(b) A raw tuple key — ICEs today (#680) and, once fixed, is equivalent to the
named key-struct; the key-struct additionally gives a stable name in
`zap expand` and reuses the ordinary `@derive(Hash, Eq)` path. (c) A string key
built from the arguments — lossy/ambiguous and a workaround for #680; rejected.

**Tests.** Driver unit tests (store synthesis, one key field per parameter,
bottom-up stacking with a behavioral decorator, every ZAP5056 rejection, method
rejection, no-op when absent); runtime goldens (memoize-once observable effect
count, distinct/order-sensitive argument entries, bottom-up stacking,
unhashable-parameter compile-fail). Observability is by effect count, never
timing or heap bytes.

**Docs.** SPEC §16 (moved `@cache` into the v1 built-in set), STABILITY (moved
out of the deferred set), PYTHON_DIFFERENCES (the `functools.cache` row),
CHANGELOG, and this decision. Diagnostic **ZAP5056** added; **ZAP5055** keeps
`@inject` only.
