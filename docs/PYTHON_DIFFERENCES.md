# Zap vs. Python: differences

Zap is designed to be **Python-readable**: most Python code reads the same in
Zap, and the common constructs — f-strings, comprehensions, `list`/`dict`/`set`
displays, `try`/`except`/`finally`, `match`/`case`, slicing, generators — mean
what a Python programmer expects. Zap is, however, a statically typed,
ahead-of-time compiled language (Rust front end, LLVM back end, a C runtime), so
some of Python's surface is spelled differently and some of it is deliberately
absent.

This document is written from the **final-goal** point of view: it assumes the
whole roadmap is eventually implemented, and describes the language as it is
meant to end up. It has two parts:

- **[Part A — Left out](#part-a--features-intentionally-left-out)**: Python
  features Zap will **not** have, and why.
- **[Part B — Spelled differently](#part-b--features-that-differ-syntactically)**:
  capabilities Zap keeps but writes with different syntax.

A short **[deferred, not omitted](#deferred-not-omitted)** note separates the
two, and a closing **[largely the same](#largely-the-same)** section lists what
needs no translation.

References of the form `#NNN` are tracking issues in
[`zaplang-io/zap`](https://github.com/zaplang-io/zap); `D-00NN` are design
decisions recorded in `design/`, and `§N` are sections of [`SPEC.md`](../SPEC.md).

---

## Part A — Features intentionally left out

These are Python constructs Zap deliberately does **not** provide. Each is a
consequence of a design pillar, not an unfinished corner, so none has an "add
it" tracking issue.

### `global` / `nonlocal`

Zap uses **static, lexical scoping** and does not allow executable statements at
module scope, so there is no mutable module-level state for a `global` rebind to
target, and no enclosing-function cell for `nonlocal` to reach (SPEC §2, §13;
D-0021/D-0090). State is passed explicitly or held in a `struct`; a module
exposes `const` bindings and definitions, not reassignable globals.

```python
# Python
count = 0
def tick():
    global count
    count += 1
```
```python
# Zap — hold state in a value, pass it explicitly
struct Counter:
    int64 count = 0
    def tick(self):
        self.count += 1
```

### Annotated-assignment statements (`x: int = 0`)

Zap has exactly **one** declaration form: type-first (`int64 x = 0`, or
`auto x = 0` with inference), D-0003. Python's annotated-assignment
`x: int = 0` would be a redundant second spelling of the same thing — and an
ambiguous one next to type-first declarations — so it is not part of the
language. (Bare annotations without a value, `x: int`, likewise have no place:
a declaration always binds.)

### `from M import name`, `import *`, relative and lazy imports

Imports are **explicit and qualified** by design. You import a module and then
reach its members through it (`import math` then `math.sqrt(x)`); there is no
`from math import sqrt`, no `from M import *`, no relative `from . import x`, and
no lazy/conditional import statement (GRAMMAR `import_item` note; D-0028/D-0052;
SPEC §13). This keeps every name's origin visible at its use site and keeps
module resolution reproducible and decentralized (MVS). Aliasing is available
(`import a.b as c`) and imports may be grouped (`import (a, b as c)`).

### `async` / `await`, `async for` / `async with`, async comprehensions

Zap's concurrency is **CSP-style coroutines** on an M:N scheduler, with
structured concurrency (`scope:`), `spawn` / `spawn detached`, channels, and
`select` (SPEC §12; D-0031/D-0044). Because every function can suspend on the
scheduler, there are **no function colors** — no split between "sync" and
"async" functions — so `async`/`await` and their `async for` / `async with` /
async-comprehension variants are unnecessary and are not in the language. This
model replaces Python's `threading`/`asyncio`/`multiprocessing` surface
(SPEC §18). See **Part B** for how the concurrency surface is written.

> A much-deferred, purely cosmetic idea — allowing `await` as a **readability
> synonym** for awaiting a spawned handle, with *no* new semantics and *no*
> function coloring — is tracked as **#610**. It is explicitly **not** a
> committed async feature; it would only ever be sugar over the existing
> coroutine model.

### Metaclasses and dynamic class bodies

Even once full object orientation lands (classes + inheritance are deferred, not
omitted — see below), Zap will **not** have **metaclasses** or **dynamically
executed class bodies**. Types are resolved and laid out at compile time; a type
definition is declarative, not a block of code that runs to build a class
object. Compile-time code generation is served by `decorator` / `annotation`
(Part B) and conditional compilation, not by a runtime type-construction
protocol.

---

## Deferred, not omitted

These Python features are **not in Part A**. They are intended or under design —
simply not shipped yet — and must not be read as "won't have":

| Feature | Status | Tracking |
|---|---|---|
| Template strings `t"..."` (PEP 750) | Planned, deferred | #612 |
| `@` matrix-multiplication operator | Planned with full numeric-array support | #613 |
| Classes + inheritance (full OO) | Deferred | #66 / #30 |

---

## Part B — Features that differ syntactically

Same capability as Python, different spelling. Each row is *construct → Python →
Zap → note*.

### Declarations and types

| Construct | Python | Zap | Note |
|---|---|---|---|
| Typed binding | `x: int = 0` | `int64 x = 0` | Type-first is the one declaration form (D-0003); `int` is accepted as a synonym for `int64`. |
| Inferred binding | `x = 0` | `auto x = 0` | `auto` infers the type; the binding is still statically typed. |
| Constant | `X = 0` (by convention) | `const int64 X = 0` | `const` is enforced, not a naming convention. |
| Function-value type | `Callable[[int], int]` | `fn(int64) -> int64` | First-class function types and closures (D-0067). |
| Transparent type alias | `type Row = tuple[int, str]` | `type Row = (int64, str)` | Same PEP 695 `type X = …` spelling and the generic form `type Pair[T] = (T, T)` (D-0105); the tuple *type* is the parenthesized `(…)` form, and the alias is transparent (same identity, not nominal). Union aliases await anonymous unions (#607). |

### Imports and modules

| Construct | Python | Zap | Note |
|---|---|---|---|
| Use a module member | `from math import sqrt` then `sqrt(x)` | `import math` then `math.sqrt(x)` | Always qualified; see Part A. |
| Alias | `import numpy as np` | `import a.b as np` | Last path segment is the default name. |
| CPython interop | *(n/a)* | `import python "numpy" as np` | Explicit interop import (D-0012). |

### Concurrency

Python spreads concurrency across `threading`, `asyncio`, and `multiprocessing`;
Zap has one CSP model (SPEC §12).

| Construct | Python | Zap | Note |
|---|---|---|---|
| Start a task | `asyncio.create_task(f())` / `Thread(target=f).start()` | `spawn f()` | Owned by the enclosing `scope:`. |
| Fire-and-forget | *(detached task)* | `spawn detached f()` | Opts out of scope ownership (D-0044). |
| Structured group | `async with TaskGroup() as g:` | `scope:` block | Joins/cancels every lexical `spawn` at the dedent; `on_error` / `timeout` / `cancel_on` / `grace` kwargs (D-0044/D-0094). |
| Channel send | `q.put(v)` | `ch <- v` | First-class channels (D-0031). |
| Channel receive | `v = q.get()` | `v = <-ch` (or `v, ok = <-ch`) | Comma-ok form reports closure. |
| Multiplex | `asyncio.wait(...)` / `select` | `select:` with `case` / `default` | CSP `select` over channel ops; `default` is the non-blocking arm. |

### Data modeling

Python models data with classes; Zap uses nominal `struct` / `enum` /
`interface` (full OO with inheritance is deferred — Part A note above).

| Construct | Python | Zap | Note |
|---|---|---|---|
| Record | `class P: ...` / `@dataclass` | `struct P:` / `value struct P:` | Reference vs. value semantics (D-0023/D-0051); value structs allow C-style bitfields (D-0068). |
| Abstract type | `class Shape(ABC):` / `Protocol` | `interface Shape:` | Structural, satisfied implicitly, checked at compile time (D-0016). |
| Sum type | `class`/`enum.Enum` + subclasses | data-carrying `enum` + `match` | Tier-1 constants and tier-3 named-payload variants; exhaustive `match` (D-0018/D-0046). |
| Optional | `Optional[T]` / `None` | `Optional[T]` (prelude enum) | `None` is the empty variant (D-0064/D-0065). |

### Functions and decorators

| Construct | Python | Zap | Note |
|---|---|---|---|
| Variadic positional | `def f(*args):` | `def f(int64... xs):` | Homogeneous, type-first (D-0038). |
| Keyword-collector | `def f(**kwargs):` | `def f(int64 **opts):` | Homogeneous `dict[str, V]`, type-first (#601). |
| Keyword-only marker | `def f(*, k):` | `def f(*, int64 k):` | Same `*` separator (I#496). |
| Positional-only marker | `def f(a, /):` | `def f(int64 a, /):` | Same `/` separator; params before `/` can't be passed by name (#602). |
| Starred call args | `f(*args, **kw)` | `f(*args, **kw)` | Same spelling; `*tuple` expands fixed params, `*list`/`**dict` forward into a variadic / collector (#597). |
| User decorator | `def deco(fn): ...` + `@deco` | `decorator Deco(...): ...` + `@Deco` | Declared decorators and `annotation` types, reflectable (D-0036). |
| Behavioral decorator | `def deco(fn):` wrapping at runtime | `decorator traced:` with `before(ctx)` / `after(ctx, result)` hooks | **Compile-time** code generation, not a runtime rebind; the wrapper keeps the decorated function's exact signature (automatic `@wraps`), visible via `zap expand` (D-0108, #652). `ctx.name` is the only context field. |
| General wrapper | `def deco(fn):` + `def inner(*args, **kwargs): return fn(*args, **kwargs)` | `decorator retry(int64 times):` + `def wrap(call):` + `def inner(*args): ... return inner` | **`*args` is a compile-time forwarding template, not a runtime varargs tuple** (D-0108, #653). It may appear only as `call(*args)`, which forwards the decorated function's exact typed parameters; it cannot be read, indexed, iterated, or re-packed, and `**kwargs` capture is not a thing. The generated wrapper has the decorated function's concrete signature (not `(*args, **kwargs)`), so there is no per-call argument boxing. Consequently an `args`-keyed `functools.lru_cache`-style `memoize` is **not** expressible via `wrap` (it needs `args` as a runtime key and persistent cross-call state). A `wrap` with a declared return type whose body may fall through gets an implicit `raise RuntimeError(...)` guard rather than Python's implicit `return None`. |
| Deprecation | `warnings.warn(..., DeprecationWarning)` at runtime, or `@typing.deprecated` (3.13) | `@deprecated` / `@deprecated("use X")` on a `def`/method | **Compile-time use-site warning** (`ZAP5124`), emitted at every call site, not a runtime warning — the caller sees it while building, and it never fires at run time (D-0109, #472). |
| Memoize / DI | `@functools.cache`, framework `@inject` | — | **Not in v1** (D-0109). `@cache` is **deferred**, awaiting module-level persistent state (tracked #658/#661/#666). `@inject` is **declined for core** — dependency injection is done with reflection-driven `Depends(...)` adapters (D-0089/D-0092), not a core decorator; there is no planned core `@inject`. Both names stay reserved and are rejected with `ZAP5055`. |

### Operators and expressions

| Construct | Python | Zap | Note |
|---|---|---|---|
| Regex match | `re.search(p, s)` | `s =~ m/p/` (and `s !~ m/p/`) | First-class `m/.../` literals and bind operators (D-0010). |
| Regex substitute | `s = re.sub(p, r, s)` | `s ~= s/p/r/` | In-place substitution assignment (D-0010). |
| Tuple return | `return a, b` | `return a, b` | Same spelling; first-class in Zap's type system (D-0049). |
| Walrus `:=` | `while chunk := f():` or `(x := e)` | `while (chunk := f()):` — parentheses required | `(name := expr)` evaluates once, binds `name` in the enclosing scope (PEP 572, including comprehension filters), and yields the value. Zap accepts **only** the parenthesized form, and `:=` is never a bare declaration — `a := 10` is rejected (use `auto a = 10`) (#600, D-0106). |
| Identity `is` / `is not` | `a is b`, `x is None` (defined on every object) | `a is b`, `x is None` — **reference types only** | Same spelling and reference-identity meaning, and `x is None` / `is not None` is the idiomatic null/empty check. The divergence: CPython defines `is` on *every* object (so `3 is None`, `"a" is "a"` are valid, if footgun-prone), whereas Zap — which has value/reference semantics (D-0023), not universal object identity — makes `is` on a value type or scalar (`int`, `float`, `bool`, `char`, `str`, `bytes`, a `value struct`, an `enum`, a tuple) a **compile error** (ZAP4073; ZAP4074 for `is None` on a never-None value), directing the user to `==`. `is` / `is not` chain like other comparisons (#611, D-0110). |

### Statements

| Construct | Python | Zap | Note |
|---|---|---|---|
| Bottom-tested loop | *(none — `while True` + `break`)* | `do: ... while cond` | Body runs once before the test (D-0039). |
| Conditional compilation | *(none — runtime checks)* | `when target.os == .linux:` | Compile-time target selection; dead arms are parsed but not checked/lowered (D-0079). |

---

## Largely the same

These read and behave as in Python and need no translation:

- **f-strings** — `f"{x:.2f}"`, including the Python format mini-language and
  PEP 701 nesting (SPEC §9.3; #285/#498).
- **Comprehensions** — list, dict, and set comprehensions, with any number of
  `for` clauses and interleaved `if` filters (#599; SPEC §8).
- **Displays** — list `[...]`, dict `{k: v}`, and set `{...}` literals (empty
  `{}` is a dict; SPEC §4, §8), including `*`/`**` unpacking: `[1, *xs]`,
  `{*a, *b}`, `(x, *rest)`, `{**a, "k": v}` (#598).
- **`match` / `case`** — literals, captures, wildcard `_`, or-patterns, guards,
  sequence/tuple/struct patterns, qualified value patterns such as
  `case Color.RED:`, and `as` sub-pattern captures (`case <pattern> as name:`,
  which binds the whole matched value; SPEC §6; D-0040/D-0111).
- **Exceptions** — `try` / `except E as e` / `finally`, `raise`, `assert`
  (SPEC §10; D-0007/D-0029).
- **Slicing** — `xs[lo:hi:step]` (SPEC §8; D-0030).
- **Generators** — `def` + `yield` returning an iterator (SPEC §7; I#67/D-0048).
- **Generator expressions** — `(x for x in xs if cond)` is a lazy iterator, not
  a list; as a sole call argument it needs no inner parentheses
  (`sum(x*x for x in range(5))`). Loop variables do not leak, matching Python
  (SPEC §8.1; #596/D-0107).

---

## Parity backlog

The ongoing CPython-vs-Zap parity pass is tracked in `zaplang-io/zap` as
**#593–#613**. The items most relevant to this document: qualified value
patterns and set displays are already shipped; `not in` (#593), chained
assignment (#594), element `del` (#595), generator expressions (#596), starred
call args (#597), `*`/`**` unpacking in displays (#598), multiple `for`/`if`
in comprehensions (#599), walrus `:=` (#600), `**kwargs` (#601), positional-only
`/` (#602), `as` sub-patterns (#603), `raise ... from` (#604), type aliases
(#606), and set graduation/constructors (#608/#609) are shipped; `except*`
(#605) and anonymous union types (#607) bring the surface closer to Python. The deferred/planned items (#610–#613, #66/#30) are listed above.
