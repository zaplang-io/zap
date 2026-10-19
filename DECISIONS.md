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

**In-place augmented assignment (`__iadd__`/`__isub__`/… family):** **reserved**
in the grammar/spec but its **implementation is deferred to a separate ticket**.
It is a pure allocation optimization over deriving `+=` from `__add__` and is
gated on value/reference aliasing rules (D-0023); until it lands, `+=` always
derives from the binary operator.

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
