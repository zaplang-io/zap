# Zap Language Specification

**Version:** 0.3 (1.0 Stable set frozen — #206 / D-0076)
**Last updated:** 2026-10-27

This is the authoritative, examples-driven description of the Zap language.
Where this document and any implementation disagree, this document wins (until
amended). Significant changes go through a design doc in `design/` and are
recorded in `DECISIONS.md`. Unresolved items live in `OPEN-QUESTIONS.md`.

**Stability.** The 1.0 surface classification (Stable / Experimental / Internal)
is ratified by the spec-freeze audit (#206, D-0076); Appendix B carries the
per-section tier and `docs/STABILITY.md` §4.4 is the cross-surface source of
truth. The editorial `Version:` above is still `0.x` (the toolchain version in
`Cargo.toml` is authoritative, `docs/STABILITY.md` §2); it is reconciled to
`1.0` at the 1.0.0 **release** (#207), not at the freeze.

Zap is a statically typed, ahead-of-time compiled language with Python-like
readability, aimed at Python developers who need native performance.

---

## 1. Lexical structure

- **Encoding:** UTF-8 source. One file = one module; a directory = a package
  (D-0028).
- **Blocks:** indentation-based, like Python. A `:` introduces an indented
  block.
- **Comments:** `#` to end of line (no block comments). A docstring is a
  triple-quoted string as the first statement of a module/def.
- **Identifiers:** letters, digits, `_`; cannot start with a digit. A **leading
  `_`** marks a name private to its module (enforced, D-0028). Type keywords are
  reserved and cannot be identifiers.
- **Compilation unit:** whole-program AOT. Definitions may follow uses — **no
  forward declarations required** (D-0021).

### 1.1 Literals & line rules (Python-style, D-0041)

- **Integers:** `42`, `0x2A`, `0o52`, `0b101010`, with `_` grouping
  (`1_000_000`).
- **Floats:** `3.14`, `.5`, `1.`, `1e10`, `1.5e-3`.
- **Complex:** imaginary suffix `j` (`3 + 4j`).
- **Strings:** `'...'` and `"..."` are both strings; `'''...'''` / `"""..."""`
  are multiline; prefixes `r"..."` (raw), `b"..."` (bytes), `f"..."`
  (interpolation), and `rb`/`rf`; Python escape sequences.
- **Char:** no dedicated literal — obtain a `char` from `s[0]` or `char(...)`.
- **Lines:** newline ends a statement; `\` continues a line; lines continue
  implicitly inside `()` / `[]` / `{}`; `;` may separate statements on one line.
- **Indentation:** spaces, consistent per block; no tab/space mixing.

## 2. Program entry (D-0021)

Top-level code runs top to bottom:

```python
print("hello from Zap")
```

…or a program may define `main`:

```python
def main():
    print("hello from Zap")
```

## 3. Declarations (D-0003)

A declaration is **exactly** one of these two forms:

```python
int64 count = 0          # explicit type
auto name = "zap"        # inferred type (statically typed)
```

Reassignment uses no type and no `auto`:

```python
count = count + 1
```

Bindings are **mutable by default** (D-0006); `const` makes one immutable:

```python
const f64 PI = 3.14159
```

## 4. Types

### 4.1 Scalars (value semantics — copied on assign/pass, D-0023)

| Type | Notes |
|---|---|
| `int64` (default int), `int32` | **signed**; overflow **wraps** (D-0005, D-0019) |
| `float64`/`f64`, `float32`/`f32` | IEEE floating point |
| `bool` | `True` / `False` |
| `char` | a Unicode scalar (codepoint) |
| `bigint` | arbitrary-precision integer (D-0019) |
| `complex` | complex number (D-0019) |

Short aliases `i32`, `i64`, `f32`, `f64` are available. All integers are
**signed** (D-0019).

**Conversions (D-0020):** widening is implicit (`int32→int64`, `int→float`);
narrowing that can lose data is explicit (`int32(x)`, `int64(y)`).

### 4.2 Strings & bytes (D-0025)

`str` is **immutable, UTF-8**. Indexing and slicing are **codepoint-based**
(Python semantics), with an ASCII fast path; iteration yields `char`.

`bytes` is an **immutable byte sequence** (ticket #337), a distinct type from
`str`. It shares `str`'s length-prefixed, GC-managed buffer but its operations
are **byte-based**: `len(b)` is the byte count, `b[i]` is an `int64` byte value
in `0..=255` (not a `char`), and a slice `b[lo:hi:step]` is `bytes`. `==`/`!=`
and ordering are byte-wise; `b1 + b2` concatenates. Construct it with the
`b"..."` literal (its content is the UTF-8 encoding of the escaped text — there
is no `\xNN` escape yet, so arbitrary non-UTF-8 bytes use `bytes([...])`),
`bytes(s)` (UTF-8 encode), `bytes([…])` (from a `list` of ints in `0..=255`), or
`bytes()` (empty). `str(b)` decodes the bytes as UTF-8, raising on invalid
UTF-8. Encoding codecs live in the `base64` and `hex` modules (§8). String
slices are cheap **views** (strings are immutable).

```python
auto s = "héllo"
print(s[0])          # 'h'  (a char)
print(s[1:3])        # "él" (a view)
for ch in s:
    print(ch)
auto raw = bytes(s)  # UTF-8 bytes (a `bytes` value)
print(raw[0])        # 104   (a byte, as int64)
print(str(raw))      # "héllo" (UTF-8 decode)
auto blob = bytes([0, 255, 16])
```

### 4.3 Reference types (shared — passed by reference, D-0023)

```python
auto xs = list[int64]([1, 2, 3])
auto m  = dict[str, int64]({"a": 1, "b": 2})
auto a  = array[f64]([1, 2, 3])          # dense numeric, SIMD (design/0002)
auto h  = heap[int64]()                  # min-heap priority queue (D-0057; comparator optional)
```

(Tuples are **value** aggregates, not reference types — see §4.8.)

`None` (D-0014) is the null value for reference types; scalars are not nullable.
Member access on `None` throws (D-0007).

### 4.4 Structs & methods (D-0015)

Composite types are **structs** with methods. **Composition only — no
inheritance.** Methods are **inline** `def`s with an explicit `self` (D-0050).

```python
struct Point:
    f64 x
    f64 y

    def norm(self) -> f64:
        return sqrt(self.x*self.x + self.y*self.y)

auto p = Point(x=3.0, y=4.0)
print(p.norm())       # 5.0
```

Struct instances are reference types (D-0023). A **`value struct`** is the
opt-in **value type** — copied, laid out inline, no heap header — for hot
aggregates (D-0051); a plain `struct` stays a reference type. Full OO (classes +
inheritance) is deferred.

### 4.5 Interfaces (D-0016)

Interfaces are **structural** and satisfied **implicitly**, but conformance is
checked **at compile time** — a missing or misspelled method is a compile error.

```python
interface Shape:
    def area(self) -> f64

struct Circle:
    f64 r
    def area(self) -> f64:
        return 3.14159 * self.r * self.r

Shape s = Circle(r=2.0)   # OK: Circle structurally satisfies Shape
print(s.area())
```

### 4.6 Enums (D-0018 tier 1, D-0046 tier 3)

**Tier 1** — named-constant enums: a distinct type, optional explicit values,
`.name`, iterable; an unboxed integer value.

```python
enum Color:
    RED
    GREEN = 10
    BLUE

auto c = Color.GREEN
print(c.name)         # "GREEN"
```

**Tier 3 (D-0046)** — data-carrying variants (sum types) with type-first named
payload fields; a tagged **value** (copied by value), with a GC pointer inserted
only for recursive or oversized fields. Constructed qualified (named or
positional); destructured by `match` (positional or named), with exhaustiveness.

```python
enum Shape:
    Circle(f64 r)
    Rect(f64 w, f64 h)
    Unit

auto sh = Shape.Circle(r=2.0)
match sh:
    case Shape.Circle(r):      area = 3.14159 * r * r
    case Shape.Rect(w=w, h=h): area = w * h
    case Shape.Unit:           area = 1.0
```

Generic enums (`enum Optional[T]: Some(T v); None`) ship today, monomorphized at
compile time (D-0017, I#154). Methods on an enum are allowed (D-0046). The
prelude `Optional[T]` (D-0065) is the canonical optional type (Python
`typing.Optional`).

### 4.7 Generics (D-0017)

Built-in containers are generic today (`list[T]`, `dict[K,V]`, `set[T]`,
`array[T]`).
User-defined generics (monomorphized) are staged in later; the syntax is
reserved now:

```python
def map[T, U](xs: list[T], f: fn(T) -> U) -> list[U]:
    ...
```

### 4.8 Tuples (D-0019, D-0049)

Fixed-arity, heterogeneous, **immutable** by-value aggregates.

```python
auto t = (1, "two", 3.0)      # (int64, str, f64)
auto one = (42,)              # 1-tuple needs the trailing comma; () is empty
print(t[0])                   # access by COMPILE-TIME-CONSTANT index -> 1
                              # t[i] with a runtime i is a compile error
auto (a, b, c) = t            # positional destructuring
auto (first, *rest) = xs      # single *rest captures the remainder (a tuple)
(x, y) = (y, x)               # swap

def divmod(int64 a, int64 b) -> (int64, int64):
    return a // b, a % b      # multiple return IS tuple return
auto (q, r) = divmod(17, 5)
```

Compared element-wise by `==`; `v, ok = <-ch` (D-0031) is this same
destructuring. Not a reference type; no heap header unless boxed.

## 5. Operators & expressions

- Arithmetic `+ - * / // % **`; **`/` is true division** (float), **`//` is
  floor division** (D-0024).
- Comparison `== != < <= > >=`; `==` is built-in for scalars and strings —
  collections and user types **define their own equality** (D-0024).
- Boolean `and`, `or`, `not`; **truthiness** is Python-style (empty/zero/`None`
  falsy) *and* strict `bool` is allowed (D-0022).
- Bitwise `& | ^ ~ << >>` (on signed integers).
- Regex bind `=~` / `!~` and in-place `~=` (D-0010).
- Channel send/receive `ch <- v` / `<-ch` (D-0031).
- **Operator overloading (D-0047):** user types overload operators by defining
  **dunder methods** (`__add__`, `__eq__`, `__getitem__`, `__call__`, `__hash__`,
  …) that satisfy compiler-known structural interfaces (`Add`, `Eq`, `Index`,
  …), checked at compile time. Equality is `__eq__` (`!=` derived); ordering is a
  single `__cmp__` (the four relational operators derived). `@derive(Eq, Ord,
  Hash, Repr)` generates them field-by-field. `+=` derives from the binary
  operator; in-place `__iadd__`-family methods are reserved, implementation
  deferred. Iteration (`__iter__`) is specified separately.

## 6. Control flow

```python
if x > 0:
    print("pos")
elif x == 0:
    print("zero")
else:
    print("neg")

while n > 0:
    n = n - 1

for i in range(0, 10):
    print(i)

for item in xs:
    print(item)
```

**Iteration protocol (D-0048):** `for x in obj` desugars to `it = obj.__iter__();
while (v, ok) = it.__next__(); ok: ...`. A user type is iterable by defining
`__iter__` + a comma-ok `__next__(self) -> (T, bool)` (exception-free, preferred);
a Python-style `__next__ -> T` raising `StopIteration` is also accepted for
porting. Generators (`yield`), built-in collections, `str`, and channels all use
this one protocol; comprehensions and `in`-fallback do too.

**do-while** (bottom-tested; body runs once before the test, D-0039):

```python
do:
    line = read()
    process(line)
while line != ""          # trailing while (no colon) closes the do block
```

**match-case** — Python-style structural pattern matching (D-0040): literals,
capture, wildcard `_`, or-patterns, guards, and sequence/tuple/struct patterns.
Matches on `enum` scrutinees are checked for exhaustiveness at compile time.

```python
match command:
    case "quit":
        stop()
    case ["move", direction]:
        move(direction)
    case Point(x=0, y=y):
        on_axis(y)
    case n if n > 100:
        big(n)
    case _:
        unknown()
```

**Conditional compilation — `when target.*` (I#139, D-0079; Experimental).**
A compile-time `when` selects code by the build target. The compiler evaluates
each arm's predicate against the active target and keeps only the first match
(or the `else`); the other arms are **dead arms** — parsed, so they must be
well-formed, but never name-resolved, type-checked, or lowered, so an arm
written for another target cannot break the build ("dead-arm skip"). It is
block-level (declaration/import and statement position) and introduces no new
scope: the surviving arm's declarations land in the enclosing module/block. The
v1 predicates are `target.arch == .x86_64|.aarch64`, `target.os ==
.linux|.windows|.macos`, and `target.has(.<feature>)` for a **compile-time**
feature baked into the build via `--target-feature` (distinct from runtime CPU
detection). Boolean combinators are not in v1. This is the portable guard for
the opt-in `arch.*` intrinsics (a later phase of D-0079): an off-target intrinsic
import is a hard error, and `when` is how a program writes both paths.

```python
when target.os == .windows:
    def path_sep() -> str:
        return "\\"
else:
    def path_sep() -> str:
        return "/"

# Later phases add an off-target-import-guarded `arch.*` intrinsic module; a
# program selects the right one the same way:
when target.arch == .x86_64:
    import arch.x86_64 as a           # (a later phase adds the arch module)
elif target.arch == .aarch64:
    import arch.aarch64 as a
```

In Phase 1 the architecture/OS come from the host the compiler runs on (Zap
builds natively; cross-target selection via `--target` arrives with
cross-compilation in a later phase). `--target-feature <name>` is repeatable and
makes `target.has(.name)` true in the build.

## 7. Functions, lambdas, generators (D-0026)

```python
def add(int64 a, int64 b) -> int64:
    return a + b

auto inc = lambda x: x + 1        # lambda

def count(int64 n):               # generator
    auto i = 0
    while i < n:
        yield i
        i = i + 1
```

Parameters are type-first (`TYPE name`); return type follows `->`.

**Default arguments (D-0038)** read like a declaration; required params precede
defaulted ones, and defaults are evaluated **per call** (a fresh value each
time, so mutable defaults like `[]` are safe — no Python once-at-def bug):

```python
def connect(str host, int64 port = 8080, bool tls = False) -> Conn:
    ...
connect("localhost")                 # port=8080, tls=False
```

**Keyword arguments** — any parameter may be passed by name; positional args
precede keyword args at a call site:

```python
connect("localhost", tls=True)
connect(host="localhost", port=80)
```

**Variadics** — homogeneous, Java-style `TYPE... name`, received as a list:

```python
def sum_all(int64... nums) -> int64:
    auto total = 0
    for n in nums:
        total = total + n
    return total
sum_all(1, 2, 3)                     # 6
```

`**kwargs`-style arbitrary named arguments are deferred (D-0038).

## 8. Comprehensions & slicing (D-0026)

```python
auto squares = [x*x for x in range(10)]
auto evens   = [x for x in xs if x % 2 == 0]
auto lookup  = {k: v*2 for k, v in items}

auto head = xs[0:3]       # list slice -> a copy (D-0030)
auto view = a[0:3]        # array slice -> a view (D-0030)
```

## 9. Strings: operations, methods, interpolation

### 9.1 Operations (ticket #284, D-0025)

`str` indexing, slicing and iteration are **by Unicode codepoint**; `s[i]`
yields a `char`, a slice yields a `str`, and iteration yields `char`. Comparison
is content-based (`==`/`!=`) and lexicographic by codepoint (`< <= > >=`, which
for UTF-8 equals byte order). `+` concatenates; `in` tests for a substring.

```python
auto s = "Hello, World"
print(ord(s[0]))         # 72   (s[i] -> char; ord(char) -> int)
print(s[7:])             # "World"   (slice -> str; Python bounds, negatives, step)
print(s[::-1])           # "dlroW ,olleH"
print("abc" < "abd")     # True
for c in s:              # iterate codepoints (char)
    pass
if "World" in s:         # substring membership
    pass
```

`char` is a Unicode scalar (an `i32`). Conversions: `ord(c)` → codepoint `int`;
`chr(n)` / `char(n)` → the `char` for codepoint `n` (a non-scalar `n` is a
`ValueError`); `str(c)` → the one-codepoint string; `str(n)`/`str(x)` renders an
int, float, bool, char or str.

Value ⇄ string conversion builtins (#285/#282, D-0026):

| Builtin | Result | Notes |
|---|---|---|
| `str(x)` | `str` | renders an int/float/bool/char/str; decodes `bytes` as UTF-8 (raises on invalid); user types → Display protocol, #274 |
| `bytes(x)` | `bytes` | `bytes()` empty · `bytes(str)` UTF-8 encode · `bytes(list[int])` each byte `0..=255` (#337) |
| `int(s)` | `int64` | parses a decimal integer; faults on a malformed string |
| `float(s)` | `float64` | parses a float; faults on a malformed string (also converts a number/bool) |
| `bool(x)` | `bool` | Python truthiness: a number/char is false at zero, a str false when empty, a bool is itself |
| `format(x, spec)` | `str` | renders `x` through the format spec (§9.3); `spec` is a string literal in v1 |

`int(x)` also converts a number to `int64`; `float(x)` converts a number/bool
to `float64`. `int`/`float` are the string-**parsing** conversions; the
width-specific numeric casts (`int32`/`int64`/`float32`/`float64`, D-0020) do
not parse strings.

> `len(s)` currently returns the **byte** length (it equals the codepoint count
> for ASCII). A codepoint-based `len(str)` is pending the `str`/`bytes`
> representation split (#284). The reachable byte view for byte-level work (e.g.
> SigV4 percent-encoding) is `s.encode()`, a `list[int]` of the UTF-8 bytes.

### 9.2 Methods (ticket #284)

String operations are **methods on `str`** (there is no `strings` module):

| Method | Result | Notes |
|---|---|---|
| `s.upper()` / `s.lower()` | `str` | ASCII case in v1 |
| `s.strip()` / `s.lstrip()` / `s.rstrip()` | `str` | ASCII whitespace |
| `s.split(sep)` | `list[str]` | non-empty `sep` |
| `sep.join(parts)` | `str` | `parts : list[str]` |
| `s.find(sub)` / `s.index(sub)` | `int` | codepoint index; `find` → -1, `index` raises when absent |
| `s.replace(old, new)` | `str` | all occurrences |
| `s.startswith(p)` / `s.endswith(p)` / `s.contains(sub)` | `bool` | |
| `s.encode()` | `list[int]` | UTF-8 byte values |

### 9.3 Interpolation (f-strings, #285/D-0026)

An f-string `f"...{expr[!conv][:spec]}..."` interpolates expressions. Each
replacement field holds a full Zap expression; `{{` and `}}` are literal
braces. A field with no spec renders via `str(expr)` (so `str(n)` and `f"{n}"`
produce byte-identical text), and a field with a `:spec` renders via
`format(expr, "spec")`. An f-string lowers to the concatenation of its literal
runs and formatted fields.

```python
auto n = 5
auto x = 3.14159
print(f"there are {n} items ({n*2} halves)")
print(f"{n:04d} {n:#x}")        # 0005 0x5
print(f"{x:.2f}")               # 3.14
print(f"%{255:02X}")            # %FF   (byte -> percent-encoding, SigV4)
```

**Format mini-language** (`spec`, a subset of Python's): `[[fill]align][sign]["#"]["0"][width]["." precision][type]`.

| Field | Meaning |
|---|---|
| `fill`+`align` | `<` left, `>` right, `^` center, `=` pad after the sign; `fill` is any char |
| `sign` | `+` always, `-` negatives only (default), space for a leading space on non-negatives |
| `#` | alternate form — `0b`/`0o`/`0x` prefix on int types |
| `0` | zero-pad to `width` (sign-aware) |
| `width` | minimum field width (counted in bytes; exact for ASCII) |
| `.precision` | float decimal places, or a string truncation length (codepoints) |
| `type` | int `b`/`o`/`x`/`X`/`d`, float `e`/`E`/`f`/`F`/`g`/`G`, string `s` |

v1 limitations (documented): the `!r`/`!a` conversions (only `!s` is accepted)
and user-type rendering need the Display protocol (#274); a field cannot contain
a string delimited by the same quote as the f-string (use the other quote);
nested `{...}` inside a spec, and non-ASCII `fill` characters, are not supported;
`format`'s spec must be a string literal. A malformed field is `ZAP2017`; an
invalid spec is `ZAP4026`.

## 10. Errors & exceptions (D-0007, D-0029)

Unchecked exceptions; nothing is ever forced to be caught.

```python
def parse_int(str s) -> int64:
    if not is_number(s):
        raise ParseError("bad int: " + s)
    return ...

try:
    auto n = parse_int(user_input)
except ParseError as e:
    print("recovering:", e)
finally:
    cleanup()
```

- `raise` throws; built-in faults (divide-by-zero, index-out-of-range, `None`
  access) also throw.
- `except` catches (optionally); `finally` always runs, including during
  unwinding.
- Uncaught exceptions unwind to the top of the coroutine and terminate with a
  stack trace. Exception types form a Python-like hierarchy.

## 11. Resource cleanup (D-0027)

Context managers and `try/finally` (no Go-style `defer`):

```python
with open("f.txt") as fh:
    process(fh)         # fh is closed on block exit, even on exception
```

## 12. Concurrency (D-0008, D-0013, D-0031–D-0034)

Lightweight coroutines + channels (CSP) on an M:N scheduler.

```python
auto ch = channel[int64]()        # unbuffered (rendezvous)
auto bch = channel[int64](100)    # buffered, capacity 100

spawn worker(ch)                  # launch a coroutine (fire-and-forget)

ch <- 42                          # send
auto v = <-ch                     # receive
auto w, ok = <-ch                 # comma-ok receive
close(ch)

select:
    case v = <-ch:
        handle(v)
    case bch <- 1:
        sent()

scope:                            # structured concurrency (D-0044)
    spawn producer(ch)
    spawn consumer(ch)
    # dedent joins both; a child's failure cancels siblings and re-raises

spawn detached metrics_flusher()  # daemon: opts out of all scopes (D-0044)
```

- **Channels (D-0031):** unbuffered by default; `(capacity)` makes it buffered;
  `close`, comma-ok receive, and `select` for multiplexing.
- **Sync primitives (D-0032):** `Mutex`, `RWMutex`, `WaitGroup`, atomics, `Once`.
  `WaitGroup` is the primary way to wait for `spawn`ed coroutines (results come
  back over channels).
- **Cancellation (D-0033; surface D-0044):** structured concurrency — a `scope:`
  block owns the coroutines spawned inside it and joins them at the dedent; a
  child failure cancels siblings and re-raises (an aggregate `ExceptionGroup` if
  several fail, D-0045). `scope(timeout=…, on_error=OnError.Cancel|Wait)` tunes
  it. `spawn detached <call>` opts out of all scope ownership (daemon; Go exit
  semantics). `Cancelled` unwinds via `finally`/`with`, outside `Exception`.
- **Memory model (D-0034):** data-race-free programs are sequentially
  consistent; data races are bugs.

## 13. Modules, imports, visibility (D-0028, D-0052)

```python
import math                    # a Zap module (logical name)
import datetime as dt          # aliasing
import jsonlib, boto           # multiple on one line
import (jsonlib, boto as b)    # grouped form, per-item alias
import python "numpy" as np    # CPython interop (design/0003)
```

One file = one module; a directory = a package. Everything is public **except**
names starting with `_`, which are private and enforced at compile time.

**Packaging (D-0052).** Dependencies are declared in **`zap.toml`** and pinned in
**`zap.lock`**; the resolver is **MVS** (minimal version selection, Go-style —
reproducible, no dependency-hell), and sources are **decentralized** (VCS URL +
semver tag, checksum-verified; no central registry). Source uses clean logical
names that `zap.toml` maps to a source + version. A project-local,
env-var-activated **`zenv`** (`source zenv/activate`) isolates both the Zap
module cache and a nested Python venv at **`zenv/venv`** (which the embedded
CPython uses), for a fully reproducible environment.

**Optional registry (D-0027, experimental).** On top of the decentralized model
there is an *additive* static-file package registry: a fourth `registry` source
resolved through an HTTPS/`file://` index of names → versions → `zap1:`
checksums, verified client-side against the same tree hash a `git`/`path` pin
uses. The CLI covers both ends: `zap publish` packages the current project and
appends its index line + archive to a **static index** (today a local/`file://`
index; an `https://` index needs the hosted authenticated write API, which is
gated on the org move); `zap search <query>` lists packages in a local index and
`zap info <name>` shows a package's published versions, checksums and yank state
from any static index. `zap login` stores a bearer token outside the repo for
the (gated) hosted write path. These verbs are **experimental** (the 1.0 CLI
surface is frozen, D-0076, so they are additive). Full design in
`design/0027-registry.md`.

## 14. Regular expressions (D-0010)

Perl-style literals with PCRE2; `m`/`s` prefixes required. Full details in
`design/0001-perl-regex.md`.

```python
if line =~ m/\bfoo\d+\b/:
    ...
auto cleaned = raw =~ s/\s+/ /g       # returns a new string
name ~= s/^\s+|\s+$//g                # in-place shorthand
if line =~ m/(?<year>\d{4})-(?<month>\d{2})/ as mt:
    print(mt.year, mt["month"])
```

## 15. Numeric arrays (D-0011)

Dense, shaped, SIMD-accelerated numeric arrays — no import. Slices are **views**
(D-0030). See `design/0002-arrays.md`.

```python
auto a = array[f64]([1, 2, 3])
auto c = a + 1.0          # element-wise + scalar broadcast (SIMD)
auto col = m[:, 1]        # a view
```

---

## 16. Decorators & annotations (D-0036)

Unified `@`-syntax metaprogramming in two kinds, applicable to functions,
methods, structs, fields, variables, and parameters. **Compile-time-first**
(generates real, type-checked code at zero runtime cost) with optional runtime
reflection. Full details in `design/0004-decorators.md`.

**Annotations** — declarative metadata:

```python
annotation Column:
    str name = ""
    bool primary = False

@Table(name="users")
struct User:
    @Column(name="id", primary=True)
    int64 id
```

**Decorators** — behavioral / generative (`before`/`after` or general `wrap`;
may take arguments):

```python
@retry(times=3)
def fetch(str url) -> str:
    ...
```

**Field decorators** — construction-time defaults (per-instance), validation,
and derive-driven codegen (pydantic-style, zero runtime cost):

```python
@derive(Json)
struct Signup:
    @default(now())
    DateTime created
    @validate(max_len=50)
    str name
```

Multiple decorators stack bottom-up. `zap expand` shows the generated code.
Built-ins (v1): `@cache`, `@derive(...)`, `@default`, `@validate`, `@range`,
`@deprecated`, `@inject`.

## 17. Toolchain & CLI (D-0053)

One user-facing tool, **`zap`** (the `zapc` compiler becomes internal):

```
zap run <file> | zap run      # run a single file, or the project (zap.toml)
zap build                     # build project -> native binary (target/)
zap test [--watch] [--json]   # run tests; --watch re-runs (targeting changed paths); --json for agents
zap test --doc [path]         # run docstring examples (doctests, #234) through the REPL engine
zap fmt                       # canonical formatter (D-0037)
zap doc [--format html|md|json] [--out <dir>] [--private]  # render API docs (I#232/233)
zap new <name> | zap init     # scaffold a project
zap add <dep> | zap remove    # manage deps (zap.toml + zap.lock, D-0052)
zap zenv create               # project env (activate via `source zenv/activate`)
zap expand                    # show @derive/decorator-generated code
zap ir <file>                 # dump ZIR (diagnostic)
zap mcp                       # MCP server for coding agents (syntax/compile/test)
zap repl                      # interactive REPL (JIT, I#159)
zap lsp                       # language server over JSON-RPC stdio (I#229)
zap playground [--port N]     # local web playground: editor + sandboxed server-side run (I#238)
zap inspect <pid>             # stacks / GC+memory stats / (later) profile of a running process
zap explain <ZAPnnnn>         # explain a diagnostic's stable error code (I#157/F1)
zap version | zap help
```

**`zap repl` (I#159).** An interactive read-eval-print loop. Each entry is
compiled and executed through the real `AST → ZIR → LLVM` backend in one
long-lived process via an in-process LLVM MCJIT, so REPL code has **identical
semantics to compiled code** (same monomorphization, value-struct layout,
exceptions, Immix GC) rather than a second interpreter. Session `let`/binding
values (including floats, I#252) persist across entries in runtime slots, and
top-level `def`/`type`/`enum`/interface declarations persist too, so a helper
defined in one entry is usable in later ones (I#252). A trailing expression is
auto-printed (a user value with no `__str__`/`__repr__`/`@derive(Repr)` renderer
prints a `<TypeName>` placeholder; the general `repr` protocol is a follow-up); a
compile error (`ZAPnnnn`) or an uncaught runtime exception is reported and the
session continues. An interactive terminal gets line editing and persistent
history (`~/.zap_history`); piped stdin uses a plain reader. Meta-commands:
`:type <expr>`, `:reset`, `:help`, `:quit`. The same session engine backs
runnable doc examples (#234) and the web playground (#238). See
design/0025-repl.md.

**Error codes (I#157/F1).** Every diagnostic carries a stable `ZAPnnnn` code
(`ZAP` + four digits; the leading digit is the phase/category — lex `1`, parse
`2`, name `3`, type `4`, semantic `5`, back end `6`). The code — not the
human-readable message, which may change — is the machine-parseable identifier
tools and coding agents key off; it is rendered as `error[ZAPnnnn]: …` on the
CLI and as a `code` field on the `zap mcp` / `--json` diagnostic surface, and
`zap explain <ZAPnnnn>` prints its documented meaning. The #206 spec-freeze audit
(D-0076) ratifies the Stable code set: every non-meta code's documented *meaning*
is **Stable**; the meta band (`ZAP0001` ICE, `ZAP0002` not-yet-implemented) is
excluded; message **text** is never stable (STABILITY.md §3.4).

**Testing (D-0054).** Tests use a **`@test`** annotation suite (`@test`,
`@before`/`@after`, `@before_all`/`@after_all`, `@parametrize`, `@skip`/`@skip_if`,
`@expect_raises`) built on decorators (D-0036). Assertions use a plain **`assert`**
statement with power-assert diagnostics (failure prints the expression and operand
values); `with expect_raises(E):` checks exceptions. Tests live inline
(`@test`, compiled only under `zap test`) or in `*_test.zap` / `tests/`, and
resolve imports through the **same** module resolution as the build (D-0052) — no
test-only import path. Docstrings are triple-quoted first statements (D-0041),
Google-style (summary + `Args`/`Returns`/`Raises`).

**Doctests (D-0074).** A fenced ```` ```zap ```` block in a docstring is a
runnable example. `zap test --doc [path]` extracts every such block under `path`
and runs it through the REPL `Session` engine (#159) — identical semantics to
compiled code, no second executor — checking it against its `#`-comment
directives: `# => <line>` for expected stdout (a trailing bare expression
auto-prints), `# raises: <substr>` for an uncaught runtime exception, and
`# error: ZAPnnnn` for an expected compile error; a block with no directive must
compile and run clean. Each example runs in a fresh session, so first-cut
examples are self-contained (they cannot yet call the item they document by name;
that waits on cross-module session loading, I#252). Doctests reuse the `@test`
report surface (human + `--json`), exit non-zero on any failure, and are
codegen-gated like `zap run`. See design/0025-repl.md §Reuse and D-0074.

**Playground (D-0075).** `zap playground` starts a local web UI (an editor page
and a `POST /run` endpoint) that compiles and runs a snippet server-side through
the same REPL `Session` engine (#159) and shows its stdout, auto-printed value,
and `ZAPnnnn` diagnostics. Because the WASM target is post-1.0, a snippet cannot
run in the browser; the server executes untrusted native code, so **each request
runs in its own short-lived worker process** (`zap playground-worker`) under
resource limits (CPU, address space, file size, no core) with the parent
enforcing a per-snippet time budget (an `alarm` armed after startup), an output
cap, and a cap on concurrent workers. A hard fault in a snippet kills only its
worker — the server reports it and stays up — and no two requests share a heap,
GC, or scheduler. It binds `127.0.0.1` by default; true filesystem/network
denial (an OS sandbox) is deployment hardening required before any public
exposure. Share links are stateless (the snippet is encoded in the URL
fragment). Codegen-gated like `zap run`. See design/0026-playground.md.

**`zap mcp` (D-0055)** is the agent interface: it serves the language
reference/grammar/examples (so LLMs write correct Zap without a training corpus),
does fast **parse + type-check** validation (no full build), maintains a compact
**code-graph** of the project (token-efficient context for agents), and runs
**targeted** tests — all with structured output.

## 18. Standard library & runtime (D-0056, D-0057, D-0058)

A curated **"Go-plus"** standard library in three tiers.

**Built-ins** (no import): `print`/`len`/`range`/`assert`; the scalar and
container types incl. **`heap`**; enums; structs + `@derive`. String operations
are **methods on `str`** (there is no `strings` module), regex is built-in
**operator** syntax (`=~`/`!~`/`~=`, §14 — not a module), and the core
containers (`list`/`dict`/`set`/`heap`) are built-in (there is no `collections`
module — the collection *extras* ship as the `deque`/`bisect`/`oset` modules).

**Bundled std** imported whole by flat short name (`import datetime`; there is
**no `from X import Y`**). The modules shipped today
(`crates/sema/src/modules/`): `io`, `fs`, `os`, `sys`, `process`, `math`,
`random`, `time`, `datetime`, `json`, `logging`, `argparse`, `deque`, `bisect`,
`oset`, `sync`, `signal`, `gc`, `runtime` — **Stable**; `crypto`, `base64`,
`hex`, `net`, `http`,
`tls`, `zlib`, `gzip`, `yaml`, `threading`, `concurrent` — **Experimental**
(STABILITY.md §4.4). The `concurrent` module (D-0078) adds a lock-free `Counter`
and an unbounded MPSC `Queue[T]` built on `Atomic[T]`; concurrency lives in the
type name (no new syntax), and a reference-payload `Queue[T]` inherits the open
aarch64 evacuation caveat (I#223). CSV is a **`@derive(Csv)`** (`to_csv`/`from_csv`), not a module; `testing`
is the **`@test`** annotation framework (§17), not an importable module.

**`datetime` formatting (#286).** A `DateTime` renders as ISO-8601 extended via
`datetime.format(dt)`; `datetime.strftime(dt, fmt)` (equivalently the 2-arg
`datetime.format(dt, fmt)`) renders a custom format with a Python-faithful
`strftime` directive set — `%Y %y %m %d %H %I %M %S %p %j %A %a %B %b %Z %z %%`,
built directly from the numeric fields. `DateTime` is naive (it carries no
zone), so `%Z`/`%z` expand to the empty string, as in Python; a literal `Z` in
the format (e.g. `"%Y%m%dT%H%M%SZ"`) is emitted verbatim. This produces the
SigV4 `X-Amz-Date` (`%Y%m%dT%H%M%SZ`) and credential-scope (`%Y%m%d`) stamps,
HTTP `Date`, and similar, from a UTC `DateTime` without run-time string slicing.

**`base64` / `hex` codecs (#337).** Transfer encodings over `bytes`/`str`.
`base64.encode(data)` (standard, padded) plus `encode_nopad`, `urlsafe_encode`
and `urlsafe_encode_nopad` cover the standard and URL-safe alphabets with and
without `=` padding; `base64.decode(s)` and `base64.urlsafe_decode(s)` return
`bytes` and tolerate missing padding (both accept either alphabet).
`hex.encode(data)` is lowercase hex and `hex.decode(s)` parses `[0-9a-fA-F]`
(even length). Malformed codec input raises a catchable exception. Together with
`crypto` (SHA-256/HMAC over `str`/`bytes`), these cover request-body, checksum
and SigV4 payload-hashing needs.

**`http` client (#104, #280).** The plaintext/TLS HTTP/1.1 client verbs
`http.get(url)`, `http.post(url, body)` and `http.request(method, url, body)`
each take an **optional trailing `dict[str, str]` of request headers** —
`http.get(url, headers)`, `http.post(url, body, headers)`,
`http.request(method, url, body, headers)` — sent verbatim on the wire. A
caller-supplied `Content-Type` overrides the default `text/plain`; the
transport-managed `Host`, `Content-Length` and `Connection` headers are always
computed by the client and any same-named map entry is ignored (so the request
carries exactly one). The response is an opaque handle read back with
`http.status(resp) -> int64`, `http.header(resp, name) -> str` (case-insensitive,
`""` when absent) and `http.body(resp) -> str`. The server-side `http.respond`
(#104) likewise takes an **optional trailing `dict[str, str]` of response
headers** — `http.respond(conn, status, body, headers)` (#279) — written on the
wire; a caller-supplied `Content-Type` overrides the default `text/plain`, while
the server-managed `Content-Length` and `Connection: close` are always computed
and any same-named map entry is ignored. `http` is **Experimental**
(STABILITY.md §4.4).

**Official packages** via the package manager (`sqlite3`, `xml`, `email`, …).
The concurrency model replaces Python threading/multiprocessing; the
`threading` compat module and `import python` (interop) are rare relief valves.

**`heap[T]` (D-0057)** — built-in min-heap priority queue (comparator optional):
`push`/`pop`/`peek`/`len`.

**Signals & runtime diagnostics (D-0058).** `signal.notify(SIG…) -> channel[Signal]`
delivers signals over a channel (async, coalesced, fan-out, `notify` takes over
disposition); `@on_signal(SIG…)` is decorator sugar; `scope(cancel_on=[SIG…],
grace=…)` turns `SIGTERM`/`SIGINT` into structured graceful shutdown. Fault
signals (`SIGSEGV`/`SIGFPE`/…) print a crash diagnostic (location + backtrace).
Default, opt-out, overridable handlers: `SIGQUIT` (stack+runtime dump), `SIGUSR1`
(force GC + memory stats), `SIGUSR2` (profiling — deferred). The `gc`/`runtime`
module (`gc.collect()`, `gc.stats()`, `runtime.coroutine_dump()`) exposes the same
programmatically; `zap inspect <pid>` triggers them against a running process.

## Appendix A — Reserved words (provisional)

`def`, `return`, `if`, `elif`, `else`, `while`, `do`, `for`, `in`, `range`,
`match`, `when`, `break`, `continue`, `auto`,
`const`, `True`, `False`, `None`, `and`, `or`, `not`, `raise`, `try`, `except`,
`finally`, `with`, `as`, `yield`, `lambda`, `assert`, `spawn`, `scope`, `channel`,
`select`, `case`, `close`, `struct`, `interface`, `enum`, `annotation`,
`decorator`, `import`, `python`, `m`, `s`
(contextual, before a delimiter; `detached` is likewise contextual, only after
`spawn`), and the type keywords (`int32`, `int64`,
`float32`, `float64`, `i32`, `i64`, `f32`, `f64`, `bool`, `char`, `str`,
`bytes`, `bigint`, `complex`, `list`, `dict`, `set`, `heap`, `array`).

## Appendix B — Status & 1.0 stability tier of sections

Tier is the ratified 1.0 classification (#206 / D-0076; see `docs/STABILITY.md`
§4.4): **S** Stable (frozen for 1.x — a valid Stable program keeps compiling and
keeps its specified behavior), **E** Experimental (shipped, not promised, may
change), **I** Internal (no contract). Sections marked **S** are **frozen for
1.0**.

| Section | Status | Tier |
|---|---|---|
| Declarations, scalars, functions, control flow | Locked | **S** |
| Strings, conversions, value/reference semantics | Locked | **S** |
| Structs, interfaces, enums (tier 1 + tier 3 sum types) | Locked | **S** |
| Generics (incl. bounds) | Locked (D-0064) | **S** |
| First-class function values + closures | Locked (D-0067) | **S** |
| Concurrency model + surface (`select`, `scope:`, `spawn detached`) | Locked (D-0031/D-0044/D-0045) | **S** |
| Sync primitives + `Atomic[T]` | Locked (D-0032/D-0066) | **S** |
| Exceptions, resource cleanup (`with` / context managers) | Locked (D-0069) | **S** |
| Modules & visibility | Locked | **S** |
| Regex | Locked (design 0001) | **S** |
| Function args: defaults, keyword, variadics | Locked (D-0038) | **S** |
| Decorators & annotations core | Locked (D-0036, design/0004) | **S** |
| do-while, match-case | Locked (D-0039, D-0040) | **S** |
| Numeric arrays / SIMD | Locked design, promotion open | **E** |
| `set` type | Shipped, surface open | **E** |
| C-style bitfields on `value struct` | Locked (D-0068) | **E** |
| `when target.*` conditional compilation | Shipped Phase 1 (D-0079) | **E** |
| `**kwargs` arbitrary named args | Deferred (D-0038) | n/a |
