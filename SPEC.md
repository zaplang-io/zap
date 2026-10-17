# Zap Language Specification

**Version:** 0.1 (draft, living document)
**Last updated:** 2026-10-17

This is the authoritative, examples-driven description of the Zap language.
Where this document and any implementation disagree, this document wins (until
amended). Significant changes go through a design doc in `design/` and are
recorded in `DECISIONS.md`.

Zap is a statically typed, ahead-of-time compiled language with Python-like
readability, aimed at Python developers who need native performance.

---

## 1. Lexical structure

- **Encoding:** UTF-8 source. One file = one module (module system: TBD).
- **Blocks:** indentation-based, like Python. A `:` introduces an indented
  block.
- **Comments:** `#` to end of line.
- **Identifiers:** letters, digits, `_`; cannot start with a digit. Type
  keywords are reserved and cannot be identifiers.

## 2. Declarations (D-0003)

A declaration is **exactly** one of these two forms:

```python
int64 count = 0          # explicit type
auto name = "zap"        # inferred type (statically typed)
```

Reassignment uses no type and no `auto`:

```python
count = count + 1
```

Bindings are **mutable by default** (D-0006). `const` makes one immutable:

```python
const f64 PI = 3.14159
```

## 3. Types

### Scalars

| Type | Notes |
|---|---|
| `int64` (default int), `int32` | integer; overflow **wraps** (D-0005) |
| `float64` / `f64`, `float32` / `f32` | IEEE floating point |
| `bool` | `true` / `false` |
| `str` | UTF-8 string |
| `bytes` | byte string |

Short aliases `i32`, `i64`, `f32`, `f64` are available.

### Collections

```python
auto xs = list[int64]([1, 2, 3])
auto m  = dict[str, int64]({"a": 1, "b": 2})
```

### Numeric arrays (D-0011)

Dense, shaped, SIMD-accelerated numeric arrays — no import required. Distinct
from `list[T]`. See `design/0002-arrays.md`.

```python
auto a = array[f64]([1, 2, 3])
auto c = a + 1.0          # element-wise + scalar broadcast
```

## 4. Functions

```python
def add(int64 a, int64 b) -> int64:
    auto c = a + b
    return c

def greet(str name) -> str:
    return "hello, " + name
```

- Parameters are type-first: `TYPE name`.
- Return type follows `->`. A function with no return value omits `-> T`.
- Public functions should have explicit signatures; inference is local to a
  body (D-0004).

## 5. Control flow

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

## 6. Errors & exceptions (D-0007)

Zap has **unchecked** exceptions. Nothing is ever forced to be caught.

```python
def parse_int(str s) -> int64:
    if not is_number(s):
        raise ParseError("bad int: " + s)
    return ...

# catching is optional:
try:
    auto n = parse_int(user_input)
    print(n)
catch ParseError as e:
    print("recovering:", e)
```

- `raise EXPR` throws.
- Built-in faults (divide-by-zero, index-out-of-range, nil access) also throw.
- An **uncaught** exception unwinds to the top of the goroutine and terminates
  with a stack trace.
- No checked exceptions, no `throws` declarations.

> Open: keyword `catch` vs `except` (see D-0007).

## 7. Concurrency (D-0008)

Goroutine + channel model (CSP), on an M:N scheduler. _(Surface syntax to be
finalized in a design doc; sketch below.)_

```python
ch := channel[int64]()        # channel (declaration form TBD)

go worker(ch)                 # start a goroutine

auto v = <-ch                 # receive
ch <- 42                      # send
```

## 8. Regular expressions (D-0010)

Perl-style literals with PCRE2. `m`/`s` prefixes are required (so a bare `/` is
always division). Full details in `design/0001-perl-regex.md`.

```python
if line =~ m/\bfoo\d+\b/:
    ...

auto cleaned = raw =~ s/\s+/ /g       # returns a new string
name ~= s/^\s+|\s+$//g                # in-place shorthand

# captures via a returned match object (no magic globals):
if line =~ m/(?<year>\d{4})-(?<month>\d{2})/ as mt:
    print(mt.year, mt["month"])
```

## 9. Modules & imports

One file = one module (granularity TBD). Zap-level imports and Python interop
imports are distinct:

```python
import strings                 # a Zap module (syntax TBD)
import python "numpy" as np    # CPython module (design/0003-python-interop.md)
```

---

## Appendix A — Reserved words (provisional)

`def`, `return`, `if`, `elif`, `else`, `while`, `for`, `in`, `range`, `auto`,
`const`, `true`, `false`, `nil`, `raise`, `try`, `catch`, `go`, `channel`,
`import`, `python`, `m`, `s` (contextual, before a delimiter), and the type
keywords (`int32`, `int64`, `float32`, `float64`, `i32`, `i64`, `f32`, `f64`,
`bool`, `str`, `bytes`, `list`, `dict`, `array`).

## Appendix B — Status of sections

| Section | Status |
|---|---|
| Declarations, scalars, functions, control flow | Locked |
| Exceptions | Locked (keyword TBD) |
| Regex | Locked (design/0001) |
| Arrays | Locked design, incremental impl (design/0002) |
| Concurrency | Model locked, syntax draft |
| Modules | Open |
