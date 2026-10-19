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
**Status:** Locked
Composite types are **structs** (data) with **methods**. **Composition only —
no inheritance, no classes.** Go-style. (Method-definition syntax — inline vs
receiver block — TBD; see OPEN-QUESTIONS.)

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
**Status:** Locked (direction)
One official, bundled tool with a **manifest + lockfile + semver** and
reproducible builds (Cargo/Go-modules style), under **Python-like import
syntax**. Explicitly **not** pip/PyPI's model (dependency-hell, bolted-on
isolation). Manifest format (e.g. `zap.toml`) and resolver TBD.

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
**Status:** Locked (direction)
A single canonical formatter enforcing **Python-style** formatting (one true
style, gofmt-spirit). Tooling detail, built later.

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
because Zap's committed features — unwinding exceptions D-0007, goroutines
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
