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
explicit values, `.name`, iterable (`enum Color: RED, GREEN, BLUE`).
**Tier 3 (data-carrying variants / Rust-style sum types) + `match` destructuring**
are now locked in **D-0046** (the `match` statement itself was locked earlier in
D-0040).

## D-0019 — Numeric types: all signed; bigint, complex, tuples
**Status:** Locked
All integer types are **signed** for v1 (`int32`, `int64`; default `int64`).
Unsigned deferred (interop + array dtypes only — see OPEN-QUESTIONS). Provide
**`bigint`** and **`complex`**. Provide **tuples**. **No type aliases** (they
hurt cross-project readability).

## D-0020 — Numeric conversions: implicit widening, explicit narrowing
**Status:** Locked
Widening conversions are **implicit** (`int32→int64`, `int→float`). Narrowing
conversions that can lose data (`int64→int32`, `float→int`) require an
**explicit** conversion (e.g. `int32(x)`). **Rationale:** silent narrowing is a
classic bug source.

## D-0021 — Execution model: top-level code, else `def main()`
**Status:** Locked
Code outside functions runs **top to bottom**; a program may instead define
`def main()`. Because Zap is AOT-compiled as a whole program, **no
forward-declaration is required** — definitions may follow uses.

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

**Scope options are typed, not stringly:** `scope(timeout=<duration>)` cancels
all children at the deadline; `scope(on_error=OnError.Cancel | OnError.Wait)`
selects the failure policy (`Cancel` is the default — a failing child cancels its
siblings; `Wait` lets siblings finish). `on_error` takes an **enum**, not a
string, so a typo is a compile-time error (consistent with the interface and
decorator "catch typos at compile time" ethos, D-0016/D-0036).

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
