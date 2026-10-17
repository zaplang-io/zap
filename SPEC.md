# Zap Language Specification

**Version:** 0.2 (draft, living document)
**Last updated:** 2026-10-17

This is the authoritative, examples-driven description of the Zap language.
Where this document and any implementation disagree, this document wins (until
amended). Significant changes go through a design doc in `design/` and are
recorded in `DECISIONS.md`. Unresolved items live in `OPEN-QUESTIONS.md`.

Zap is a statically typed, ahead-of-time compiled language with Python-like
readability, aimed at Python developers who need native performance.

---

## 1. Lexical structure

- **Encoding:** UTF-8 source. One file = one module; a directory = a package
  (D-0028).
- **Blocks:** indentation-based, like Python. A `:` introduces an indented
  block.
- **Comments:** `#` to end of line.
- **Identifiers:** letters, digits, `_`; cannot start with a digit. A **leading
  `_`** marks a name private to its module (enforced, D-0028). Type keywords are
  reserved and cannot be identifiers.
- **Compilation unit:** whole-program AOT. Definitions may follow uses — **no
  forward declarations required** (D-0021).

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
| `bool` | `true` / `false` |
| `char` | a Unicode scalar (codepoint) |
| `bigint` | arbitrary-precision integer (D-0019) |
| `complex` | complex number (D-0019) |

Short aliases `i32`, `i64`, `f32`, `f64` are available. All integers are
**signed** (D-0019).

**Conversions (D-0020):** widening is implicit (`int32→int64`, `int→float`);
narrowing that can lose data is explicit (`int32(x)`, `int64(y)`).

### 4.2 Strings & bytes (D-0025)

`str` is **immutable, UTF-8**. Indexing and slicing are **codepoint-based**
(Python semantics), with an ASCII fast path; iteration yields `char`. `bytes`
holds raw bytes. String slices are cheap **views** (strings are immutable).

```python
auto s = "héllo"
print(s[0])          # 'h'  (a char)
print(s[1:3])        # "él" (a view)
for ch in s:
    print(ch)
auto raw = bytes(s)  # UTF-8 bytes
```

### 4.3 Reference types (shared — passed by reference, D-0023)

```python
auto xs = list[int64]([1, 2, 3])
auto m  = dict[str, int64]({"a": 1, "b": 2})
auto a  = array[f64]([1, 2, 3])          # dense numeric, SIMD (design/0002)
auto t  = (1, "two", 3.0)                 # tuple
```

`None` (D-0014) is the null value for reference types; scalars are not nullable.
Member access on `None` throws (D-0007).

### 4.4 Structs & methods (D-0015)

Composite types are **structs** with methods. **Composition only — no
inheritance.** (Method-definition syntax is being finalized — see
OPEN-QUESTIONS.)

```python
struct Point:
    f64 x
    f64 y

    def norm(self) -> f64:
        return sqrt(self.x*self.x + self.y*self.y)

auto p = Point(x=3.0, y=4.0)
print(p.norm())       # 5.0
```

Struct instances are reference types (D-0023).

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

### 4.6 Enums (D-0018, tier 1)

Named-constant enums — a distinct type, optional explicit values, `.name`,
iterable. (Data-carrying variants + `match` are deferred; OPEN-QUESTIONS.)

```python
enum Color:
    RED
    GREEN = 10
    BLUE

auto c = Color.GREEN
print(c.name)         # "GREEN"
```

### 4.7 Generics (D-0017)

Built-in containers are generic today (`list[T]`, `dict[K,V]`, `array[T]`).
User-defined generics (monomorphized) are staged in later; the syntax is
reserved now:

```python
def map[T, U](xs: list[T], f: fn(T) -> U) -> list[U]:
    ...
```

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
def connect(str host, int64 port = 8080, bool tls = false) -> Conn:
    ...
connect("localhost")                 # port=8080, tls=false
```

**Keyword arguments** — any parameter may be passed by name; positional args
precede keyword args at a call site:

```python
connect("localhost", tls=true)
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

## 9. Strings: interpolation

f-strings (D-0026):

```python
auto n = 5
print(f"there are {n} items ({n*2} halves)")
```

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
```

- **Channels (D-0031):** unbuffered by default; `(capacity)` makes it buffered;
  `close`, comma-ok receive, and `select` for multiplexing.
- **Sync primitives (D-0032):** `Mutex`, `RWMutex`, `WaitGroup`, atomics, `Once`.
  `WaitGroup` is the primary way to wait for `spawn`ed coroutines (results come
  back over channels).
- **Cancellation (D-0033):** structured concurrency — child coroutines are
  scoped to a parent; leaving the scope awaits/cancels them. Surface syntax TBD.
- **Memory model (D-0034):** data-race-free programs are sequentially
  consistent; data races are bugs.

## 13. Modules, imports, visibility (D-0028)

```python
import strings                 # a Zap module
import collections as col      # aliasing
import python "numpy" as np    # CPython interop (design/0003)
```

One file = one module; a directory = a package. Everything is public **except**
names starting with `_`, which are private and enforced at compile time.

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

## Appendix A — Reserved words (provisional)

`def`, `return`, `if`, `elif`, `else`, `while`, `for`, `in`, `range`, `auto`,
`const`, `true`, `false`, `None`, `and`, `or`, `not`, `raise`, `try`, `except`,
`finally`, `with`, `as`, `yield`, `lambda`, `spawn`, `channel`, `select`,
`case`, `close`, `struct`, `interface`, `enum`, `import`, `python`, `m`, `s`
(contextual, before a delimiter), and the type keywords (`int32`, `int64`,
`float32`, `float64`, `i32`, `i64`, `f32`, `f64`, `bool`, `char`, `str`,
`bytes`, `bigint`, `complex`, `list`, `dict`, `array`).

## Appendix B — Status of sections

| Section | Status |
|---|---|
| Declarations, scalars, functions, control flow | Locked |
| Strings, conversions, value/reference semantics | Locked |
| Structs, interfaces, enums (tier 1) | Locked (syntax draft) |
| Generics | Reserved; staged implementation |
| Exceptions, resource cleanup | Locked |
| Concurrency | Model locked; `select`/structured-concurrency syntax draft |
| Modules & visibility | Locked |
| Regex, arrays | Locked (design docs 0001/0002) |
| Function args: defaults, keyword, variadics | Locked (D-0038) |
| `match`, `set`, `**kwargs`, decorators | Open (see OPEN-QUESTIONS) |
