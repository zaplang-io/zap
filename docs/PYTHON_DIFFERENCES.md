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
two, a **[largely the same](#largely-the-same)** section lists what needs no
translation, and a short **[standard-library behavior
differences](#standard-library-behavior-differences)** section notes where a
stdlib function deviates from its closest CPython equivalent.

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
module scope, but it **does** have module-level state — both immutable `const`
and mutable bindings, initialized once before `main` (SPEC §3.1, D-0112, epic
#661). What it has no need for is the `global` and `nonlocal` *keywords*. Because
scoping is static and lexical, a bare assignment to a name that resolves to a
module-level binding reassigns that binding in place — there is no Python-style
"assignment inside a function implicitly creates a local" rule to opt out of, so
no `global` declaration is needed. (To introduce a function-local that shadows a
module binding, you *declare* it: `auto x = ...`.) There is likewise no
enclosing-function cell for `nonlocal` to reach (SPEC §13; D-0021/D-0090):
closures capture by value, and shared state is a module binding or an explicit
object.

```python
# Python — `global` opts the function out of creating a local
count = 0
def tick():
    global count
    count += 1
```
```python
# Zap — a module-level mutable binding; no `global` keyword needed
int64 count = 0
def tick():
    count += 1            # reassigns the module binding (static lexical scope)
```

One consequence has no Python analogue: Python coroutines/threads serialize
through the GIL, so a shared mutable global is at least atomic per bytecode.
Zap's coroutines are **genuinely concurrent and unsynchronized** (the light
model, #661), so a shared mutable global can race. Zap therefore ships an
**advisory sharing lint** (`ZAP5125`, D-0113/#664): a read or write of a mutable
module global whose type is not `@shareable` warns — the example above warns on
`count += 1`. A `const` global never warns; mark a type `@shareable` (a trusted
assertion, like asserting a type is `Sync` in Rust — but *unverified*), or use an
`Atomic`/`Mutex`/concurrent collection, to silence it. Suppress one site with a
`# noqa` comment (closest to flake8's `# noqa`, though here it is a *compiler*
pragma, not a third-party linter's). A team can make the lint — and every other
warning — fatal in CI with `zap build --warnings-as-errors` (off by default);
unlike Python's `-W error`, this is a build-exit policy only and never changes
what the language accepts.

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
| Share-safety marker | no equivalent (GIL serializes; `typing` has no `Sync`) | `@shareable` on a `struct` | A no-argument **trusted, unverified** assertion that the type is safe under concurrent shared access (akin to Rust's `Sync`, but unchecked), reflectable via `reflect_has(T, "shareable")`; drives the sharing lint `ZAP5125` (D-0113, #664). |
| Memoize | `@functools.cache` / `@functools.lru_cache` | `@cache` | **`@cache`** memoizes a free function by its argument tuple (#666, D-0115), expanding — visibly, via `zap expand` — to real Zap over a module-level dict: a `@derive(Hash, Eq)` key-struct, a `@shareable` `Mutex`+`dict` store, and a wrapper. Like `lru_cache`, the lock guards only the table (the body runs outside it, so recursion is safe and under contention a key may be computed more than once). **Differences from CPython:** the key must be **value-hashable** — in v1 an **integer, `bool`, or `str`**. A **reference type** (a `struct`/`enum` or a collection) is a **compile error** (`ZAP5056`), *not* silently keyed by identity the way CPython hashes an arbitrary object by `id()`: identity keying would miss on equal-but-distinct arguments and give no speedup while looking correct, so Zap rejects it up front rather than let it mislead. `float`/`bytes` are also not keys yet. The function must **return a value** (else `ZAP5056`, not a runtime `TypeError`). v1 is **unbounded** — no `maxsize`/LRU yet, so it is `functools.cache`, not a bounded `lru_cache`, and `@cache` takes **no arguments**; it is a **free-function** decorator (not methods in v1); no `cache_clear()`/`cache_info()` introspection yet. |
| Dependency injection | framework `@inject` | — | **Declined for core** (D-0109). Dependency injection is done with reflection-driven `Depends(...)` adapters (D-0089/D-0092), not a core decorator; there is no planned core `@inject`. The name stays reserved and is rejected with `ZAP5055`, a redirect to `Depends(...)`. |

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
| dict / set iteration **order** | `dict` preserves **insertion order**; `set` is unordered | `dict` **and** `set` iterate in an **unspecified** order | A map guarantees no ordering, so Zap does **not** preserve insertion order — matching Go (`map`, randomized), Rust `HashMap`, and Java `HashMap`; Python's ordered `dict` is the outlier. For str/bytes keys the order is **seed-dependent across processes** (the hash is seeded per run, #1064), stable within a run. So `print`/`repr`/`str` of a str-keyed `dict`/`set` is **not** order-stable across runs — sort the keys, or use the insertion-ordered `oset` (the `OrderedDict`/`LinkedHashMap` analog, #983), for deterministic output. Decided against insertion-ordered dict in #1066 / D-0235. |
| dict / set iteration under mutation | `for k in d: del d[k]` → `RuntimeError` | `for k in d: del d[k]` → runs over a key snapshot, no error | `for ... in <dict>`/`<set>` iterates a snapshot of the keys taken at loop entry (hash order, D-0056), so inserting or deleting during the loop never raises — it simply does not change what the current pass sees. Deliberate, memory-safe divergence from CPython's "changed size during iteration" `RuntimeError` (epic #374; D-0130). |

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
- **Exceptions** — `try` / `except E as e` / `finally`, `raise` (incl. bare
  `raise` re-raise and `raise ... from`), `except*` group handling, `assert`
  (SPEC §10; D-0007/D-0029/D-0116/D-0129). A bare `raise` re-raises the handler's
  active exception with its type/identity preserved. **Divergence:** an
  out-of-context bare `raise` (no active exception — in a `try` body, a
  non-handling `else`/`finally`, at top level, or in a function merely *called*
  from a handler) is a **compile-time error (ZAP6002)**, not Python's runtime
  `RuntimeError: No active exception to re-raise`; Zap tracks the active
  exception lexically and so does not implement Python's dynamic,
  cross-call-frame re-raise (#657, D-0116).
- **`except*`** (SPEC §10.1; #605/D-0129) matches Python 3.11 group handling —
  all-`except`-or-all-`except*`, every matching clause runs, each binds a
  sub-`ExceptionGroup`. **Divergence:** the auto-re-raised remainder follows
  Zap's bare-for-single rule (D-0045) — a lone surviving sub-exception re-raises
  **bare** (so a plain enclosing `except E` catches it), where Python always
  re-wraps it in an `ExceptionGroup`; two or more remaining still re-raise as a
  group.
- **Slicing** — `xs[lo:hi:step]` (SPEC §8; D-0030).
- **Generators** — `def` + `yield` returning an iterator (SPEC §7; I#67/D-0048).
- **Generator expressions** — `(x for x in xs if cond)` is a lazy iterator, not
  a list; as a sole call argument it needs no inner parentheses
  (`sum(x*x for x in range(5))`). Loop variables do not leak, matching Python
  (SPEC §8.1; #596/D-0107).

---

## Standard-library behavior differences

Beyond the language itself, a few standard-library functions deviate from their
closest CPython equivalent. This section is new and deliberately partial — a
fuller stdlib-parity triage is planned, which may move or prune smaller entries.

### `crypto.b64url_encode` / `crypto.b64url_decode` (#771, D-0133)

Zap's URL-safe base64 helpers follow the **JOSE/JWT convention** (unpadded),
which differs from Python's `base64.urlsafe_b64encode` / `urlsafe_b64decode`:

- **Encode emits no `=` padding.** Python's `base64.urlsafe_b64encode` always
  pads; the unpadded form in Python comes from `secrets.token_urlsafe` or a
  manual `.rstrip(b"=")`.
- **Decode accepts input with *or* without padding.** Python's
  `base64.urlsafe_b64decode` requires correct padding and raises on a missing
  pad; Zap's is lenient.

This matches `crypto.token_urlsafe` (and Python's `secrets.token_urlsafe`), which
is likewise unpadded. The padded, standard variants remain available through the
`base64` module (whose `urlsafe` / `pad` options reproduce Python's
`base64.urlsafe_b64*` exactly); `crypto.b64url_*` is the unpadded convenience for
callers holding a digest or key as `bytes`.

**Faithful (no divergence):** `crypto.hex` / `crypto.unhex` match Python's
lowercase `bytes.hex()` / `binascii.hexlify` and the strict `bytes.fromhex`
decode (odd length or non-hex → `ValueError`); `crypto.token_hex` and
`crypto.token_urlsafe` match `secrets.token_hex` / `secrets.token_urlsafe`. All
of these live under the Zap-only `crypto` namespace (Python has no `crypto`
module), so they are additive surface, not behavioral changes.

### `decimal.Decimal` (#981, #981)

Zap's exact base-10 `Decimal` matches CPython's `decimal` for parsing,
`str()` formatting (the General Decimal Arithmetic *to-scientific-string* rules),
exact `+ - *`, division rounded to a precision, `quantize`, comparisons and the
eight rounding modes (verified against CPython across a large battery). It
deliberately differs in a few places:

- **No global/thread-local context.** CPython keeps a mutable `getcontext()`
  precision + rounding that `+ - *` *and* `/` consult. Zap has none: `+ - *` are
  **always exact** (so `Decimal("1e30") + Decimal("1")` keeps every digit, where
  CPython rounds to the context's 28), and the only operations that round —
  `/` / `div` and `quantize` — take the precision/rounding **per call**
  (`a.div(b, prec=28, rounding="ROUND_HALF_EVEN")`; the `/` operator uses those
  same defaults). This removes hidden, concurrency-unsafe state.
- **Rounding modes are string tags**, e.g. `rounding="ROUND_HALF_EVEN"`. The tag
  *values* equal CPython's `decimal.ROUND_*` constant values, so the call reads
  almost identically; Zap does not (yet) export named `ROUND_*` constants.
- **No special values.** `Infinity` / `NaN` (and `sNaN`) are not modelled —
  parsing one raises `ValueError`. Zap `Decimal` is finite-only.
- **No signed zero.** A zero result is always unsigned: `Decimal("0") *
  Decimal("-1")` is `0`, where CPython yields `-0`.
- **Flat exception model.** CPython's `decimal.InvalidOperation` (malformed
  literal, unknown rounding tag) and `decimal.DivisionByZero` both surface as
  Zap's `ValueError` (these modules map errors onto the built-in hierarchy
  rather than defining their own `@exception` family — the `statistics` /
  `StatisticsError` precedent). Zap *does* support package-defined exception
  families since #1278 (`@exception` / `@exception(Parent)`, a catch-family edge
  only, not general subclassing); `decimal` deliberately stays flat.
- **`quantize` never raises on precision.** CPython raises `InvalidOperation` if
  the quantized result would exceed the context precision; with no context, Zap's
  `quantize` always succeeds.
- **`from_float` is shortest-round-trip, not exact-binary.** CPython's
  `Decimal.from_float(0.1)` expands the full binary value
  (`0.1000000000000000055…`); Zap's `decimal.from_float(0.1)` is `Decimal("0.1")`
  (the float's shortest decimal string, matching Go's shopspring `NewFromFloat`).
  Constructing a `Decimal` *directly* from a float is a compile error either way
  — `from_float` is the explicit opt-in.
- **No decimal literal.** There is no `1.50d` syntax; construct via
  `decimal.Decimal("1.50")`. (A literal is a possible later grammar addition.)

### `fractions.Fraction` (#982)

Zap's exact rational `Fraction` matches CPython's `fractions.Fraction` for
reduction to lowest terms, sign normalization (denominator always `> 0`, zero as
`0/1`), exact `+ - * /`, comparisons, `limit_denominator`, and `str()` rendering
(verified against CPython across a battery of construction, arithmetic,
comparison and `limit_denominator` cases). It deliberately differs in a few
places:

- **No implicit `int → Fraction` promotion for operators.** CPython evaluates
  `Fraction(1, 2) + 1` (the `int` is promoted). Zap's operator dispatch is
  type-first — it dispatches on the left operand and does not coerce the right —
  so mix an `int` in by constructing a `Fraction`: `Fraction(1, 2) +
  Fraction(1)`. (The same model as `Decimal`.)
- **Construction from a `float` is a compile error.** CPython's `Fraction(0.1)`
  adopts the exact binary expansion (`3602879701896397/36028797018963968`). Zap
  rejects a float argument (`ZAP4024`) and steers to the `str` form, so
  `Fraction("0.1")` is exactly `1/10` — the value the author meant. There is no
  `from_float` classmethod; a decimal `str` is the exact path.
- **A signed denominator in a ratio literal is rejected.** Like CPython's own
  grammar (the numerator may carry a sign, the denominator is digits only),
  `Fraction("1/-2")` raises `ValueError`; write `Fraction("-1/2")`.
- **`numerator` / `denominator` are methods, not properties**, and return
  `bigint` (arbitrary precision), not Python `int` — Zap has no properties.
- **Flat exception model.** CPython's `ZeroDivisionError` (a zero denominator, or
  dividing by a zero `Fraction`) and the `ValueError` of a malformed string both
  surface as Zap's `ValueError` (Zap has no exception subclassing — the
  `statistics` / `decimal` precedent).
- **No `Fraction` literal.** There is no rational literal syntax; construct via
  `fractions.Fraction(...)`.

### `collections` (#972)

Zap's `collections` conveniences — `Counter`, `defaultdict`, `OrderedDict` — are
generic prelude structs over the built-in `dict`/`list`, and match CPython for
the common operations: `Counter[k]` reads `0` for a missing key, `most_common`
orders by count descending, `Counter.subtract` keeps non-positive counts while
the `+`/`-` operators drop them, `defaultdict` autovivifies a missing key from
its factory on `d[k]` (so `d[key].append(x)` works) while `k in d` and `get` do
not create it, and `OrderedDict` iterates in insertion order. It deliberately
differs in a few places:

- **`namedtuple` is not provided.** Zap's `value struct` is a strict superset —
  named, immutable, stack-resident, with `@derive(Eq, Ord, Repr)` and structural
  hashing (so it is a dict/set key). Write `value struct Point: int64 x; int64 y`
  instead of `namedtuple('Point', 'x y')`; for the anonymous positional case use
  a tuple (`(a, b)` with `t[0]` access).
- **`most_common` tie order is unspecified.** CPython breaks ties among equal
  counts by first-insertion order. Zap's backing `dict` has no defined iteration
  order (unspecified and per-run seeded, D-0235), so the order among **equal**
  counts is unspecified; the count-descending order for distinct counts is
  deterministic. Use `OrderedDict`, or sort the result, when a stable tie order
  matters.
- **`Counter` / `defaultdict` key iteration order is unspecified** (D-0235), as
  for any `dict`. Use `OrderedDict`, `Counter.most_common`, or `sorted(...)` for
  a deterministic order; `OrderedDict.keys()/values()/items()` are insertion
  order.
- **`Counter` is built from an iterable, not a mapping or `**kwargs`.**
  `collections.Counter(items)` tallies an iterable (or `collections.Counter()`
  empty, then `update`/`add`); there is no `Counter({'a': 2})` mapping form and
  no `Counter(a=2)` keyword form (Zap is statically typed over a single element
  type `K`).
- **`defaultdict`'s factory is an explicit `fn() -> V`.** CPython takes a
  callable or default class (`defaultdict(list)`, `defaultdict(int)`); Zap takes
  a zero-argument function value returning `V` — e.g. a `def mk() -> list[int64]`
  for grouping, or a `def zero() -> int64` for counting.
- **A missing `OrderedDict[k]` read yields the dict's zero word**, not a
  `KeyError` — a flat consequence of the built-in `dict` read pending a
  language-wide `KeyError` (#22); use `get(k, fallback)` for an explicit default.
- **The Python name `Counter` is served by two modules.**
  `collections.Counter` is this multiset; `concurrent.Counter` is the pre-existing
  lock-free integer counter (D-0078). They are distinct types (backed by distinct
  prelude structs) and do not collide.

### `net` UDP sockets (#991, decisions/stdlib/991-udp-sockets.md)

- **Datagram payloads are `str`, not `bytes`.** CPython's `sendto` takes `bytes`
  and `recvfrom` returns `bytes`. Zap's `net.sendto` takes a `str` and
  `net.recvfrom` returns the payload as `str`, exactly as the existing
  `net.send` / `net.recv` already do — UDP stays consistent with the module it
  joins. (A `str`'s bytes are its UTF-8 encoding, so arbitrary binary content
  still round-trips byte-for-byte.)
- **The address family is chosen by the constructor, not an `AF_*` constant.**
  CPython writes `socket.socket(socket.AF_INET, socket.SOCK_DGRAM)` /
  `AF_INET6`. Zap exposes no `AF_*` constants: `net.udp_socket()` is the IPv4
  socket and `net.udp_socket6()` the IPv6 one. A bind/send/connect whose target
  address family does not match the socket's raises, rather than silently
  cross-wiring.
- **`recvfrom` returns a flat `(data, host, port)` triple.** CPython returns
  `(bytes, address)` where `address` is itself a `(host, port)` (and a 4-tuple
  for IPv6). Zap flattens it to `(str, str, int64)`; `host` is the numeric IP
  string.
- **The socket is the first argument.** Zap `net` functions are module functions,
  not methods on a socket object, so e.g. `net.sendto(sock, data, host, port)` /
  `net.recvfrom(sock, n)` take the socket explicitly (as every other `net`
  function does).

### `net` DNS resolution (#992, decisions/stdlib/992-dns-resolution.md)

- **Resolution returns plain `list[str]`.** CPython's `socket.getaddrinfo`
  returns a list of `(family, type, proto, canonname, sockaddr)` 5-tuples and
  `gethostbyaddr` returns a `(hostname, aliaslist, addrlist)` triple. Zap's
  `net.lookup_host` / `lookup_ipv4` / `lookup_ipv6` return a list of numeric IP
  strings, and `net.lookup_addr` a list of hostname strings — the shape Go's
  `net.LookupHost` / `LookupAddr` use. The strings feed `net.connect(ip, port)`
  directly.
- **Address family is selected by the function, not an `AF_*` constant.** CPython
  passes `socket.AF_INET` / `AF_INET6` to `getaddrinfo`; Zap exposes no such
  constants, so `lookup_host` (both), `lookup_ipv4`, and `lookup_ipv6` are
  distinct functions.
- **A missing reverse record is an empty list, not an exception.** CPython's
  `gethostbyaddr` raises `socket.herror` when an address has no PTR record;
  `net.lookup_addr` returns `[]`. A non-numeric `ip` argument raises `ValueError`.
- **No service-name resolution.** There is no `getaddrinfo(host, "http")`
  service→port form; ports are plain integers.

### `net` IP presentation ↔ wire conversion (#1261, decisions/stdlib/1261-ip-presentation-wire.md)

- **Address family is a portable string, not an `AF_*` integer.** CPython's
  `socket.inet_pton(family, s)` / `inet_ntop(family, packed)` take the
  platform-specific integer constants `socket.AF_INET` / `AF_INET6` (`AF_INET6`
  is `10` on Linux, `30` on macOS, `23` on Windows). Zap exposes no `AF_*`
  constants, so `net.inet_pton(family, text)` / `net.inet_ntop(family, packed)`
  take the string `"ipv4"` or `"ipv6"` (exactly those two spellings;
  case-sensitive), matching the explicit-family style of `net.lookup_ipv4` /
  `lookup_ipv6` and `net.udp_socket6`.
- **One typed error: `ValueError`.** CPython raises `OSError` for malformed
  address text and `ValueError` for a bad family. Zap raises a catchable
  `ValueError` for every rejection — unknown family, malformed text, a family /
  text mismatch (IPv6 text with `"ipv4"` and vice-versa), and an `inet_ntop`
  `packed` whose width is not 4 (`"ipv4"`) or 16 (`"ipv6"`).
- **Canonical RFC 5952 IPv6 output, with a narrow IPv4-mapped rule.** `inet_ntop`
  always emits the canonical form — lowercase, no leading zeros, the longest
  zero run (≥ 2 groups, leftmost on a tie) compressed to `::`, a single zero
  group never shortened. Only the IPv4-mapped prefix (`::ffff:a.b.c.d`) renders
  with an embedded dotted-decimal tail; the deprecated IPv4-compatible form
  (`::a.b.c.d`) renders as pure hex groups. (CPython delegates to the platform
  `inet_ntop`, whose compression and v4-mapped behaviour varies; Zap's formatter
  is hand-written for byte-identical output across platforms.)
- **`inet_pton` returns `bytes`, `inet_ntop` takes `bytes`.** As in CPython, the
  packed form is a `bytes` value (4 octets for IPv4, 16 for IPv6) in network byte
  order; no `netaddr`/`ipaddress`-style object is involved.

### `net` Unix domain sockets (#993, decisions/stdlib/993-unix-domain-sockets.md)

- **Two-step `socket()` + `bind`/`connect` is folded into combined
  constructors.** CPython writes `socket(AF_UNIX, SOCK_STREAM); s.bind(path);
  s.listen()`. Zap's `net.listen_unix(path)` does socket+bind+listen in one call
  (and `net.connect_unix(path)` socket+connect), mirroring the TCP side; the
  socket is the first argument everywhere else, as for all `net` functions.
- **Pathname sockets only.** Linux abstract-namespace addresses (a leading-NUL
  `sun_path`, which CPython spells as a leading `\0` in the path) are rejected as
  an invalid path.
- **`recvfrom_unix` returns `(data, path)`.** CPython's AF_UNIX `recvfrom`
  returns `(bytes, address)` where `address` is the path string; Zap flattens to
  a `(str, str)` tuple, and the payload is `str` (as elsewhere in `net`), not
  `bytes`.
- **No stale-path auto-unlink.** `net.listen_unix` fails if the path already
  exists, like CPython `bind`; remove a stale socket file first.
- **POSIX-only.** CPython exposes `AF_UNIX` only where the platform has it; Zap
  likewise supports it on POSIX and raises a catchable "not supported" error on
  Windows. Peer-credential access is `net.peer_cred(sock) -> (uid, gid, pid)`
  (#1143): CPython spells this `sock.getsockopt(SOL_SOCKET, SO_PEERCRED, ...)` and
  unpacks the `struct ucred` by hand; Zap returns the flat `(int64, int64, int64)`
  tuple directly, with no `SO_*` constant. It is Linux-only for now (macOS/BSD and
  Windows raise "not supported"); a non-Unix socket raises `ValueError`.

### `net` socket options (#994, decisions/stdlib/994-socket-options.md)

- **Named options, not `setsockopt(level, optname, value)`.** CPython takes raw
  integer `level`/`optname` constants (`socket.SOL_SOCKET`, `socket.TCP_NODELAY`,
  …). Zap exposes no such constants: each option is its own named function
  (`net.set_nodelay`, `net.set_keepalive`, `net.set_reuseaddr`,
  `net.set_reuseport`, `net.set_recv_buffer`, `net.set_send_buffer`, and the
  `get_*` twins), with `bool` or `int64` values.
- **`SO_REUSEPORT` raises where the platform lacks it**, rather than failing to
  resolve a constant.
- **Keepalive on/off only; no interval tuning yet.** `set_keepalive` toggles
  `SO_KEEPALIVE`; the per-platform idle/interval/count knobs
  (`TCP_KEEPIDLE`/`TCP_KEEPINTVL`/`TCP_KEEPCNT` on Linux, `TCP_KEEPALIVE` on
  macOS) and multicast membership are not yet exposed.

### `os` / `fs` / `io` errors (#1311, #1320; decisions/lang/1311-filesystem-notfound-error.md, decisions/lang/1320-os-fs-typed-errors.md)

- **`FileNotFoundError` ⊂ `OSError`, matching CPython — but a narrow subtree.**
  Across the `os` / `fs` / `io` surface an errno-driven failure raises `OSError`,
  and a missing path raises `FileNotFoundError` (`os.listdir`, `fs.read_file`/
  `read_bytes`/`size`/`stat`/`rename`/`move`/`copy`/`scandir`/`walk`/`makedirs`/
  `rmtree`/`write_file`/`write_bytes`/`append`, `io.open`/`read`/`write`,
  `os.chdir`/`remove`/`rmdir`/`mkdir`/`path_abspath`/`realpath`). Both descend
  from `Exception`, so `except Exception` catches every failure, exactly as
  before. The hierarchy edge is the same as CPython (`FileNotFoundError` ⊂
  `OSError` ⊂ `Exception`), so Python code reads across unchanged. A missing
  executable passed to `process.run` / `process.Popen` likewise raises
  `FileNotFoundError` (as CPython's `subprocess` does).
- **`MemoryError` for allocation failures, `ValueError` for bad input.** An
  out-of-memory condition raises `MemoryError`; an invalid `io.open` mode or an
  empty path raises `ValueError`. `process.run` / `check_run` raise `RuntimeError`
  for a non-zero child exit (a process result, not an OS error); `input()` raises
  `EOFError`; a `process` timeout raises `TimeoutError`.
- **No `IOError` spelling in Zap source.** CPython keeps `IOError` as a legacy
  alias of `OSError`; Zap source spells it `OSError`. (A Python-interop
  exception named `IOError` still maps to Zap's `OSError`.)
- **The finer `OSError` errno subtree is not exposed yet.** CPython has
  `PermissionError`, `IsADirectoryError`, `FileExistsError`, `NotADirectoryError`,
  and the rest; Zap currently distinguishes only the missing-path case
  (`FileNotFoundError`) from everything else (`OSError`). Match on `OSError` for
  the non-missing failures. Platform "not supported" failures also raise
  `OSError` (CPython uses `OSError` / `NotImplementedError`; Zap has no
  `NotImplementedError` type yet).

---

## Parity backlog

The ongoing CPython-vs-Zap parity pass is tracked in `zaplang-io/zap` as
**#593–#613**. The items most relevant to this document: qualified value
patterns and set displays are already shipped; `not in` (#593), chained
assignment (#594), element `del` (#595), generator expressions (#596), starred
call args (#597), `*`/`**` unpacking in displays (#598), multiple `for`/`if`
in comprehensions (#599), walrus `:=` (#600), `**kwargs` (#601), positional-only
`/` (#602), `as` sub-patterns (#603), `raise ... from` (#604), bare `raise`
re-raise (#657), `except*` group handling (#605), type aliases
(#606), and set graduation/constructors (#608/#609) are shipped; anonymous union
types (#607) brings the surface closer to Python. The deferred/planned items (#610–#613, #66/#30) are listed above.
