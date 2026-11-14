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

## 2. Program entry (D-0021, D-0090)

A compiled Zap program runs from `def main()`. Executable statements are not
allowed at module scope, in either the entry module or imported modules; move
them into `main` (or another function). A bare, non-f-string string literal as
the first statement is a module docstring and is allowed at module scope
(D-0041). The grammar accepts statements in module position, but the compiler
rejects executable ones semantically.

```python
"""Example program."""

def greet():
    print("hello from Zap")

def main():
    greet()
```

Definitions may follow uses because the whole program is AOT-compiled; no
forward declarations are required (D-0021).

This is invalid because the call is an executable module-scope statement:

```python
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

**Chained assignment** (#594) binds one value to several targets at once:

```python
a = b = c = 0        # c? no — the right-hand side is evaluated once and
                     # assigned to a, then b, then c (left to right)
xs[0] = y = compute()   # targets may be names, subscripts, or fields
```

The right-hand side is evaluated **exactly once** and assigned to every target
in left-to-right order (Python semantics). Each target is an ordinary
single-assignment lvalue and, exactly like a plain reassignment, must already be
a declared binding — chained assignment never introduces a binding (that stays
the type-first / `auto` declaration above).

Bindings are **mutable by default** (D-0006); `const` makes one immutable:

```python
const f64 PI = 3.14159
```

A `const` cannot be reassigned, augmented (`+=`), or otherwise rebound after its
declaration; doing so is a compile error (ZAP3017). This holds for a `const`
anywhere — a local, or a module-level one (below).

### 3.1 Module-level persistent state (D-0112, epic #661)

A binding declaration may also appear at **module top level**, giving the module
persistent state. Both the immutable `const` form (#658 Phase 1) and the
**mutable** form (#662 Phase 2) are available:

```python
const int64 LIMIT = 100               # immutable (const)
const dict[str, int64] WEIGHTS = {"a": 1, "b": 2}
const auto PI = 3.14159               # `auto` infers the type from the initializer

int64 counter = 0                     # mutable — reassignable and mutable
list[int64] log = []
auto total = 0                        # mutable with inferred type
```

- **Scalars and reference/heap types** are both allowed, for `const` and mutable
  bindings alike (a reference-typed module binding is a permanent GC root).
- Each is **evaluated once, before `main`**, in import-topological order across
  modules (a module's state initializes after every module it imports, reusing
  the acyclic module graph of D-0081) and in source order within a module. A
  later binding's initializer may read an earlier module binding or call a
  function; a reference to a not-yet-initialized one is an error.
- Each is a **module member**: reachable from another module as `module.NAME`
  unless it is `_`-private (D-0028/D-0081).
- A **mutable** module binding may be **reassigned** (`=`, `+=`) and **mutated**
  (container contents) from any function; the value persists for the process
  lifetime. There is **no `global` keyword** (unlike Python): a bare assignment
  to a name that resolves to a module binding reassigns that binding in place
  (Zap's static lexical scoping, D-0021/D-0090). To introduce a function-local
  of the same name instead, declare it (`auto x = ...` / `T x = ...`). A `const`
  module binding cannot be reassigned (ZAP3017).

Concurrency follows the light Go/Python model of epic #661: a mutable module
binding is shared, **unsynchronized, and never implicitly locked**. `const` is
the safe default; `Atomic[T]`/`Mutex`/concurrent collections are the tools for
shared mutable state. This model is kept honest by an **advisory** surface —
never a dialect — described in §12.1: the `@shareable` marker and the static
sharing lint (ZAP5125), with the runtime `--race` detector (D-0114, #665) as the
dynamic backstop.

Only a **single-name** module-level binding is admitted. A module-level
tuple-destructuring declaration (`T a, b = expr`) is rejected (ZAP3018) — declare
each binding on its own line. Loose executable statements at module scope remain
rejected (ZAP3014/ZAP3016); only module bindings — and a leading module
docstring — are admitted.

## 4. Types

### 4.1 Scalars (value semantics — copied on assign/pass, D-0023)

| Type | Notes |
|---|---|
| `int64` (default int), `int32` | **signed**; overflow **wraps** (D-0005, D-0019) |
| `uint8`, `uint16`, `uint32`, `uint64` | **unsigned**; overflow **wraps** (#552) |
| `usize` | pointer-width **unsigned** (64-bit on supported targets, #552) |
| `float64`/`f64`, `float32`/`f32` | IEEE floating point |
| `bool` | `True` / `False` |
| `char` | a Unicode scalar (codepoint) |
| `bigint` | arbitrary-precision signed integer (D-0019) |
| `complex` | pair of IEEE-754 `float64` components (D-0019) |

Short aliases `i32`, `i64`, `f32`, `f64` are available, and the Python-facing
names `int` (a synonym for `int64`, I#581) and `float` (a synonym for
`float64`) are accepted wherever a type is written — `int x = 0` is `int64 x = 0`
— just as in Python. Signed integers default to `int64`.

**Unsigned integers (#552, D-0019 amendment).** `uint8`/`uint16`/`uint32`/
`uint64` (short spellings `u8`/`u16`/`u32`/`u64`) and the pointer-width `usize`
are the deliberate systems/C-interop surface (Python has no unsigned ints).
Overflow wraps (two's complement), they print as unsigned decimal (`uint64`
values above `INT64_MAX` print their true magnitude), and `>>` is a logical
shift. There is **no implicit conversion between signed and unsigned** — a mixed
operation is a compile error (`ZAP4015`) — so convert one side explicitly first.
Implicit widening is allowed only up the fixed-width unsigned tower
(`uint8`→`uint64`); `usize` converts only explicitly (its width is
platform-dependent). There is no unsigned literal suffix: `uintN(x)` / `usize(x)`
produce unsigned values. `usize` is the canonical pointer-width name; `size_t`
is only the C mapping target at the FFI boundary (`@repr(c)`, #542), never a Zap
alias. Unsigned arithmetic promotes to `uint64` and wraps at 2^64 — a narrower
result needs an explicit `uintN(...)` cast, exactly as `int32` arithmetic
promotes to `int64`.

**Conversions (D-0020):** widening is implicit (`int32→int64`, `int→float`,
`uint8→uint64`); narrowing that can lose data is explicit (`int32(x)`,
`int64(y)`, `uint8(x)`); every signed↔unsigned crossing is explicit. An
out-of-range or negative value wraps under a C reinterpret/truncate cast
(`uint8(-1) == 255`, `int64(uint64(-1)) == -1`).

**`bigint`.** Integer literals outside the signed `int64` range have type
`bigint`; smaller integer literals default to `int64`. `bigint(x)` converts an
integer or truncates a finite float toward zero. `int64(x)` / `int32(x)` convert
back and raise an overflow error when the value does not fit. `float64(x)`
converts to the nearest representable binary floating-point value. `+`, `-`,
`*`, `//`, `%`, and `**` preserve arbitrary precision; `/` returns `float64`.
Comparisons are exact. Negative `//` and `%` follow Python floor-division
semantics (the remainder has the divisor's sign).

**`complex`.** `2j` and `3.5j` are imaginary literals. `complex()` is zero;
`complex(x)` converts a real numeric value (or copies a complex value), and
`complex(real, imag)` constructs from two real numbers. `+`, `-`, `*`, `/`, and
`**` are supported. `==` and `!=` compare both components; ordering comparisons
are invalid. `bool(z)` is false only when both components are zero. `str(z)` and
`print(z)` use the form `(real±imagj)` with `%g`-style decimal component
formatting, such as `(1+2j)`.

### 4.2 Strings & bytes (D-0025)

`str` is **immutable, UTF-8**. Indexing, slicing and `len` are
**codepoint-based** (Python semantics), with an ASCII fast path; iteration
yields `char`, and `len(s)` is the Unicode codepoint count — not the byte length.

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
auto st = {1, 2, 3}                       # set[int64] — a brace set literal
auto e  = set[int64]()                    # empty set[int64] (there is no empty-set literal)
auto u  = set([1, 1, 2])                  # set[int64] {1, 2} — dedup from any iterable
auto cs = set("aab")                      # set[char] of the unique codepoints
```

A `{...}` brace display is a **set literal** (`{1, 2, 3}`) unless its first
element is followed by `:`, which makes it a dict; **empty `{}` is the empty
dict** (there is no empty-set literal — build one with a constructor such as
`set[T]()`). `set` is Stable (D-0076, graduated #608/#609).

**Set constructors (D17, #609).** `set[T]()` builds an empty `set[T]`, parallel
to `heap[T]()` / `channel[T]()`. `set(iterable)` builds a set of an iterable's
**deduplicated** elements (its element type is the iterable's), mirroring
Python's `set(xs)`; `set[T](iterable)` is the same with the element type stated
explicitly. The operand may be any iterable — a list, another set, a `dict`
(its keys), a `range`, or a `str` (its codepoints). `set()` with no argument is
rejected: an empty set has no element type to infer, so write `set[T]()`.

**Unpacking in displays (`*` / `**`, #598).** A `*expr` inside a list, set, or
tuple display splices an iterable's elements in place, and a `**expr` inside a
dict display merges another `dict[K, V]`'s entries:

```python
auto xs = [1, 2, 3]
auto ys = [0, *xs, 4, *xs]       # [0, 1, 2, 3, 4, 1, 2, 3]
auto s  = {*xs, 10}              # set[int64] {1, 2, 3, 10}
auto t  = (0, *(1, 2), 9)        # (0, 1, 2, 9) — a tuple splat must be a tuple
auto a  = {"x": 1}
auto b  = {"y": 2, **a, "x": 9}  # {"y": 2, "x": 9} — later entries win
```

Multiple splats and ordinary elements may be mixed freely. The spliced
operand's element type must match the display's element type, and a dict merge's
key/value types must match the display's (type-first; ZAP5123 otherwise). A
`*`-splat into a **tuple** display requires a statically-sized tuple operand, so
the result tuple's arity stays known; a list/set splat accepts any iterable. A
dict `**`-merge copies the operand's entries, so a key set by a later entry (a
pair or a later merge) overrides an earlier one — Python's last-wins semantics.
This reuses the spread machinery introduced for starred call arguments (#597).

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

A struct with no `__str__`/`__repr__`/`@derive(Repr)` renders through the Display
protocol's default field-wise form `TypeName(field=value, …)` in `print`, `str`,
f-strings, and the REPL (§9.4, D-0124).

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

Conformance is by method set over **any type**, not structs only (#731): an
`enum` — tier-1 or tier-3 — that declares every method an interface requires
conforms and is usable as an interface value, dispatching dynamically. A tier-3
enum boxes its tagged-struct pointer; a tier-1 enum boxes its scalar tag into a
small heap cell. Methods are also callable directly on a tier-1 enum value.

```python
interface Node:
    def rank(self) -> int

enum Expr:                 # tier-3 (data-carrying)
    Lit(int v)
    Add(int l, int r)
    def rank(self) -> int:
        return 1

Node n = Expr.Add(l=1, r=2)   # OK: the enum satisfies Node
print(n.rank())
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
print(c)              # Color.GREEN   (Display protocol, §9.4 / D-0124)
```

An explicit value may be reused as an alias. The first declaration for a value
is its canonical name: `.name` returns that name and iteration yields that value
once, in the position of its first declaration. A pass-only enum body declares
an empty enum; iterating it runs zero times. Accessing `.name` on an enum value
whose discriminant is not declared raises a runtime error.

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

Native floating-point values are currently unsupported as direct `list`, `dict`
(key or value), `set`, `heap`, or `channel` elements (`ZAP4027`). Integer, boolean,
character, string/bytes, enum, and supported reference/aggregate element types
retain their existing storage rules; tuple and struct fields may be floats.
Use `array[f64]` for dense floating-point data. `array` dtypes have the narrower
Experimental contract in §15; `bigint` and `complex` collection elements are
also unsupported.
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

A tuple of hashable elements is itself **hashable** (D-0122): it may be used as a
dict key or a set element — `{(1, 2): 7}`, `dict[(int64, str), int64]`, the set
literal `{(1, 2)}`, and nested tuples — with structural hash/equality over its
elements (CPython parity). A tuple containing an unhashable element (a `float`, a
nested `list`/`dict`/`set`, or a struct lacking `__hash__`/`__eq__`) is rejected
at compile time (`ZAP6001`), never an internal compiler error. The typed empty
**collection constructor** takes the same composite element/key types as the
literal and annotation forms (D-0123): `set[(int64, int64)]()` is accepted, as is
any valid element type expression; a `heap[(..)]()` of an unordered tuple and a
`channel[(..)]()` of a value aggregate remain clean coded rejects.

An **`Optional[T]`** (for hashable `T`) is also hashable and usable as a dict key
or set element (D-0123, CPython parity): `None` and a present value are both
hashable, `None` compares equal only to another `None`, and a present value
delegates to `T`'s hash/equality — `dict[Optional[int64], str]` with both a value
key and a `None` key works, and a bare `None` literal is accepted in key position
(`d[None]`, `None in d`). An `Optional` of an unhashable `T` (e.g.
`Optional[float]`) is rejected at compile time (`ZAP6001`), never an ICE.

### 4.9 Transparent type aliases (D-0105, #606)

A **type alias** gives an existing type a second name. It is **transparent**: it
introduces no new nominal type (use `struct`/`enum` for that), and resolves to
its target everywhere a type is resolved, with the **same identity**. The alias
and its target are therefore fully interchangeable.

```python
type Count = int64
type Row    = (int64, str)         # the tuple type is the parenthesized form
type Pair[T] = (T, T)              # generic alias
type Boxed[T] = Box[T]             # alias to a user generic type

def double(Count n) -> Count:      # Count IS int64
    return n + n
list[Row] rows = []                # Row IS (int64, str); usable as an element
def mk() -> Pair[int64]:           # Pair[int64] IS (int64, int64)
    return (1, 2)
```

`type` is a **soft keyword**: it is recognized as the alias introducer only at
item start, immediately before an identifier, and stays an ordinary identifier
everywhere else. A generic alias's target may reference its type parameters;
generic aliases are expanded during monomorphization alongside user generics
(§4.7), so `Pair[int64]` becomes `(int64, int64)` and `Boxed[int64]` becomes the
`Box[int64]` instance. An alias that expands back to itself (`type A = B; type B
= A`) is rejected with `ZAP4090` rather than looping. Union aliases
(`type J = A | B`) depend on anonymous union types and are not part of this
feature.

## 5. Operators & expressions

- Arithmetic `+ - * / // % **`; **`/` is true division** (float), **`//` is
  floor division** (D-0024). `@` is matrix multiply (PEP 465), defined for
  arrays (§15); it shares `*`'s precedence.
- Comparison `== != < <= > >=`; `==` is built-in for scalars and strings —
  collections and user types **define their own equality** (D-0024).
- Membership `in` / `not in` (D-0024, #593): `x in xs` tests whether `x` is a
  member of `xs` (a collection element, a `dict` key, or a substring of a
  `str`); the result is a strict `bool`. `x not in xs` is its negation —
  identical operand rules and result — and means exactly `not (x in xs)`. `in`
  participates in Python-style comparison chaining (below); `not in` does not.
- Identity `is` / `is not` (D-0110, #611): `a is b` tests **reference
  identity** — whether both operands are the same allocation — and the result is
  a strict `bool`. This is distinct from `==` (value / `__eq__` equality): two
  distinct reference-struct allocations with equal fields are `==`-equal but not
  `is`-equal. `x is None` / `x is not None` is the empty-variant / null check,
  consistent with `== None` and `match case None`. Operands must be reference
  types — a plain `struct` (D-0023), a `list`/`dict`/`set`/`heap`, a `channel`,
  or `Optional[T]`; a value type or scalar is a **compile error** (ZAP4073, or
  ZAP4074 for `is None` on a never-None value), which directs the user to `==`.
  Unlike `not in`, `is` / `is not` **do** participate in comparison chaining
  (below), so `a is b == c` is one chained comparison.
- Boolean `and`, `or`, `not`; **truthiness** is Python-style (empty/zero/`None`
  falsy) *and* strict `bool` is allowed (D-0022). `and`/`or` **short-circuit**
  left-to-right: the right operand is evaluated only when the left does not
  already decide the result (`a and b` skips `b` when `a` is falsy; `a or b`
  skips `b` when `a` is truthy), so guard idioms like `i < len(xs) and xs[i]`
  never evaluate the guarded side-effecting operand when the guard fails. The
  result is a strict `bool` (the surviving operand's truthiness).
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

**Walrus assignment-expression `(name := expr)` (#600, epic #614 D8):** the one
form that *binds and yields* in a single step. It evaluates `expr` exactly once,
binds `name` by inference (like `auto`) in the **lexically enclosing** scope, and
itself evaluates to that value — so the name is usable after the construct it
appears in. This is the capability `auto` cannot express: a binding introduced
where a statement cannot go. Zap permits the walrus only inside parentheses — as
part of an `if`/`while` condition or a comprehension `if` filter:

```python
while (line := read()) != "":   # `line` re-bound each iteration, live afterward
    use(line)
if (n := len(xs)) > 10:         # `n` usable in the body and after the `if`
    print(n)
kept = [y for x in xs if (y := f(x)) > 0]   # `y` binds in the enclosing scope
                                            # (PEP 572), live after the comp
```

A walrus target inside a comprehension binds in the scope containing the
(outermost) comprehension, not in the comprehension's own iteration scope. `:=`
is deliberately **not** a declaration shorthand: a bare statement `a := 10` is
rejected — it would duplicate `auto a = 10` and fight the single declaration
form (D-0003) — so introduce a binding with `auto` or a typed declaration.

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

**pass** (#591) is a no-op statement accepted anywhere a statement is expected —
a function body, any branch of `if`/`elif`/`else`, `for`/`while` (and their
`else`), `with`, `try`/`except`/`finally`, `scope`, and as the explicit spelling
of an empty `struct`/`enum` body. It lowers to nothing. `pass` is a contextual
statement-position word, not a reserved keyword (Appendix A): it is the no-op
statement only when it stands alone as a statement, and remains usable as an
ordinary identifier in every other position.

**del** (#595) removes a **collection element** through a subscript target:

```python
del d[key]       # remove a dict entry by key
del xs[i]        # remove a list element by index (negatives count from the end)
```

It is scoped to subscript targets and reuses the collection's own storage
operation, so its behavior matches element *access*: a missing dict key is a
no-op (matching the current non-raising dict read `d[k]`; a real `KeyError` is
tracked under #22), while an out-of-range list index raises the same catchable
`IndexError` as a read `xs[i]`. Deleting a binding (`del name`) is **not**
supported. Like `pass`, `del` is a contextual statement-position word, not a
reserved keyword (Appendix A): it is the removal statement only when immediately
followed by another identifier (`del d…`), and stays an ordinary identifier
everywhere else.

**do-while** (bottom-tested; body runs once before the test, D-0039):

```python
do:
    line = read()
    process(line)
while line != ""          # trailing while (no colon) closes the do block
```

**match-case** — Python-style structural pattern matching (D-0040): literals,
capture, wildcard `_`, or-patterns, guards, sequence/tuple/struct patterns, and
**qualified value patterns** — a bare dotted name with no parentheses
(`case Color.RED:`, `case Day.SAT:`, `case Shape.Unit:`) matches a tier-1 enum
constant or a payload-free variant by value (a single undotted name is a
capture, not a value pattern).
Matches on `enum` scrutinees are checked for exhaustiveness at compile time.
Tuple patterns require matching arity; list patterns require matching length.
One `*rest` may appear anywhere in either pattern: it captures the middle as a
tuple or a fresh list, respectively, and allows an empty remainder. Nested
patterns test fields recursively. Bindings belong to their arm; if its guard
fails, matching continues with the next arm. A guarded or refutable payload
pattern does not by itself cover its entire enum variant.

A pattern may carry a trailing **`as` capture** — `case <pattern> as name:`
(D11, #603). It matches the inner pattern and, on success, binds `name` to the
*whole* matched value, typed as the scrutinee for that arm; the inner pattern
still destructures, so nested captures bind alongside the `as` name
(`case [x, y] as pair:` binds `pair`, `x`, and `y`; `case Point(x=0) as origin:`
binds `origin`). The `as` binds the entire or-pattern, so `case 1 | 2 | 3 as n:`
is `(1 | 2 | 3) as n`. The capture is in scope in the arm body and in the arm's
guard (`case [x, y] as pair if len(pair) == 2:`). `name` must be a single plain
identifier — never `_` (drop the `as` to discard the value) and never a dotted
name (ZAP2021). An `as` capture does not change which enum variants its arm
covers for exhaustiveness.

```python
match command:
    case "quit":
        stop()
    case ["move", direction]:
        move(direction)
    case Point(x=0, y=y):
        on_axis(y)
    case [x, y] as pair if len(pair) == 2:
        both(pair, x, y)
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

**Keyword-only parameters (D-0095, I#496)** — a bare `*` separator in the
parameter list makes every parameter after it keyword-only: it may only be
supplied by name at a call site, never positionally (Python semantics). A
keyword-only parameter may be required or carry a default, and the `*` appears
at most once and must be followed by at least one parameter:

```python
def open(str path, *, bool create = False, int64 mode) -> File:
    ...
open("a", mode=0)                    # create defaults to False
open("a", create=True, mode=0)       # by name, in any order
# open("a", True, 0)                 # error: create/mode are keyword-only
# open("a")                          # error: required keyword-only `mode` omitted
```

**Positional-only parameters (D-0104, I#602)** — a bare `/` separator in the
parameter list makes every parameter *before* it positional-only: it may only be
supplied positionally at a call site, never by name (Python semantics; the mirror
of the keyword-only `*`). The `/` appears at most once, must follow at least one
parameter, and must come before any `*`. Parameters after `/` (and before any
`*`) stay positional-or-keyword:

```python
def f(int64 x, int64 y, /, int64 z) -> int64:
    return x + y + z
f(1, 2, 3)                           # all positional
f(1, 2, z=3)                         # z (after `/`) may be passed by name
# f(x=1, y=2, z=3)                   # error: x/y are positional-only
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

**Keyword-collector `**kwargs` (#601)** — a trailing `V **name` parameter
collects any extra keyword arguments into a homogeneous `dict[str, V]`, where
`V` is the declared value type (the key is always `str`). It is the mirror of
the positional variadic: at most one, always the last parameter, and it carries
neither a default nor the `...` marker. The body uses it as an ordinary dict:

```python
def tally(int64 base, int64 **counts) -> int64:
    auto total = base
    for k in counts.keys():
        total = total + counts[k]
    return total
tally(0, a=1, b=2, c=3)              # 6  (a/b/c collected into counts)
```

Fully heterogeneous Python kwargs (mixed value types) remain out of scope for
v1; model those through `pyobject`/interop.

**Starred call arguments `f(*args, **kwargs)` (#597)** — at a call site, `*expr`
unpacks an iterable into positional arguments and `**expr` unpacks a
`dict[str, V]` into keyword arguments. Python argument order holds: positionals,
then `*`, then keywords, then `**`.

```python
auto xs = [1, 2, 3]
sum_all(*xs)                          # 6   (list spread into the variadic)
sum_all(10, *xs)                      # 16  (fixed arg then spread)
auto p = (3, 4)
dist(*p)                              # tuple expands into fixed x, y
auto d = {"a": 1, "b": 2}
tally(0, **d)                         # dict spread into **counts

def forward(int64... args, int64 **opts) -> int64:
    return sum_all(*args) + tally(0, **opts)   # the forwarding idiom
```

Because Zap is statically typed with fixed arity, a statically-sized **tuple**
expands elementwise into fixed parameters, but a runtime-length `*list`/`*set`
can only feed a `TYPE... name` variadic, and `**dict` can only feed a
`V **name` collector — spreading an unknown-length value into fixed parameters
is rejected (ZAP5122). In this version spreads are supported in calls to named
functions, with at most one `*` and one `**` per call.

## 8. Comprehensions & slicing (D-0026)

```python
auto squares = [x*x for x in range(10)]
auto evens   = [x for x in xs if x % 2 == 0]
auto lookup  = {k: v*2 for k, v in items}
auto uniq    = {x % 3 for x in xs}        # set comprehension -> set[int64]

auto head = xs[0:3]       # list slice -> a copy (D-0030)
auto view = a[0:3]        # array slice -> a view (D-0030)
```

A list, set, dict, or generator comprehension takes **any number of `for`
clauses with interleaved `if` filters** (#599). Nested `for` clauses iterate in
Python's left-to-right, outer-to-inner order, and each `if` filters the clauses
to its left (it is evaluated inside the loop of the `for` it follows, before the
next clause runs):

```python
auto prod  = [x*y for x in xs for y in ys if x != y]
auto pairs = [(i, j) for i in range(3) for j in range(3) if i < j]
auto grid  = {x: y for x in xs if x > 0 for y in ys}
```

A single `for` clause with an optional `if` keeps its earlier behavior
unchanged; the general form desugars to nested loops with each filter at its
clause's level.

### 8.1 Generator expressions (#596, D-0026)

A **generator expression** `(elem for CLAUSES)` uses the same clause grammar as
a comprehension but is written with parentheses and is **lazy**: it evaluates to
an `Iterator` (§6.x, D-0048) that yields each `elem` on demand rather than
building a collection, so it holds no intermediate list and can be consumed once.

```python
auto g     = (x*x for x in xs if x > 0)   # a lazy Iterator, not a list
auto total = sum(x*x for x in range(5))   # summed without materializing a list
for y in (f(x) for x in xs):              # iterated one element at a time
    ...
```

A generator expression's loop variables live only inside the generator (they do
not leak into the enclosing scope, exactly like a comprehension); a walrus in a
filter still binds the enclosing scope. As the sole argument of a call it needs
no parentheses of its own — `sum(x for x in xs)` — but with any other argument
it must be parenthesized: `f((x for x in xs), y)`. It desugars to an anonymous
generator function (`yield`-based, I#67) whose parameters are the captured free
variables, so it reuses the one iteration protocol. A genexpr nested directly
inside another genexpr (`(v for v in (w for w in xs))`) is the "nested
generators" case I#67 deferred — one generator driving a second across its own
`yield` — and is not supported yet; iterate the inner one through an
intermediate `for` or a materialized collection instead.

## 9. Strings: operations, methods, interpolation

### 9.1 Operations (ticket #284, D-0025)

`str` indexing, slicing, iteration and `len` are **by Unicode codepoint**;
`s[i]` yields a `char`, a slice yields a `str`, iteration yields `char`, and
`len(s)` is the codepoint count (so `len("héllo") == 5`, #374/B6). Comparison
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
| `int(s)` | `int64` | parses a decimal integer; raises a catchable `ValueError` on a malformed string (#456) |
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

A **user type or container** renders through the Display protocol (§9.4): a
struct, tier-1 enum, tuple, or `list`/`dict`/`set` with no spec renders in an
f-string field exactly as `str(expr)` and `print(expr)` do.

v1 limitations (documented): the `!r`/`!a` conversions are not accepted (only
`!s`; Zap draws no `repr`/`str` quoting distinction — §9.4); a field cannot
contain a string delimited by the same quote as the f-string (use the other
quote); nested `{...}` inside a spec, and non-ASCII `fill` characters, are not
supported; `format`'s spec must be a string literal. A malformed field is
`ZAP2017`; an invalid spec is `ZAP4026`.

### 9.4 Value rendering (the Display protocol, #274/D-0124)

Every value renders to text through **one** protocol, shared by `print(x)`,
`str(x)`, f-string fields, the REPL, and doctests — so all four produce
byte-identical text for the same value. Built-in scalars render as expected
(numbers, `True`/`False`, `Optional[T]` as its payload or `None`).

**The `str`/`repr` distinction.** As in Python, a leaf `str`/`char`/`bytes`
renders differently at the top level vs. nested:

- Top level of `print`/`str`/an f-string field: a string renders **unquoted**
  (`hello`).
- Nested as a struct field, container element, or tuple element: a string
  renders **quoted** (`'a'`), `bytes` as `b'…'`, a `char` as `'c'`.

Every other value type (numbers, `bool`, enums, structs, tuples, containers)
renders identically in both positions.

**User types and containers:**

- A **struct** with no renderer gets the **default field-wise form**
  `TypeName(field=value, …)` — no opt-in required; fields render recursively
  (nested structs, tuples, enums, containers, `Optional`, scalars), strings
  quoted (`Point(name='ada')`).
- A **tier-1 enum** renders its qualified canonical variant name,
  `EnumName.VARIANT`.
- A **tier-3 data-carrying enum** (D-0046) renders structurally:
  `EnumName.Variant(field=value, …)` for a data-carrying variant (fields always
  labelled, each rendered quoted-when-string: `Shape.Circle(r=2.0)`) and
  `EnumName.Variant` for a payload-free variant (`Shape.Unit`). A recursive enum
  (`Expr.Add(l=…, r=…)`) renders recursively.
- A **tuple** renders the Python form `(v0, v1, …)` (a one-element tuple keeps
  its trailing comma, `(v0,)`).
- A **`list`/`dict`/`set`** renders the Python form `[v0, v1]` / `{k0: v0, …}` /
  `{v0, v1}`, with elements/keys/values quoted-when-string; the **empty forms**
  are Python-exact: `[]`, `{}` (empty dict), and `set()` (an empty set is never
  `{}`). Containers nest and carry structs/tuples/enums.
- **Overrides:** a hand-written `__str__` (preferred) or `__repr__`, or
  `@derive(Repr)`, overrides the default (D-0047); `@derive(Repr)` recurses
  through all field types and, like the default, now renders a `str` field
  **quoted**.

```python
struct Point:
    int64 x
    int64 y

auto p = Point(x=1, y=2)
print(p)                       # Point(x=1, y=2)
print(str(p))                  # Point(x=1, y=2)   (identical)
print(f"pt={p}")               # pt=Point(x=1, y=2)
print([1, 2, 3])               # [1, 2, 3]
print(["a", "b"])              # ['a', 'b']        (elements quoted)
print({"k": 1})                # {'k': 1}
print("a")                     # a                 (bare string unquoted)
print(Shape.Circle(r=2.0))     # Shape.Circle(r=2.0)   (tier-3 enum, #706)
print(Shape.Unit)              # Shape.Unit            (payload-free variant)

Drawable d = Point(x=1, y=2)   # a concrete struct behind an interface
print(d)                       # Point(x=1, y=2)       (interface value, #707)
```

An **interface value** renders as its concrete type would (#707): the render
dynamically dispatches through the interface's itable to the concrete type's
renderer, exactly as an interface method call dispatches (D-0016). Only structs
sit behind an interface today (interface conformance is structural over structs),
so the tier-1/tier-3 enum, tuple, and container renderers are reached through the
interface as the concrete struct's fields.

**Deferred (never silent, a #274 follow-up):** the recursive **cycle guard** — the
default renderer recurses with **no cycle guard** (see limitations). No
object-identity/address form (`<TypeName @ 0x…>`) is provided.

**Known limitations (v1).** The renderer recurses with **no cycle guard**, so a
cyclic object graph would recurse without bound — as a hand-written recursive
`__repr__` would (CPython guards only built-in containers). A cycle is not yet
constructible through the supported surface (a self-referential `Optional[Self]`
field does not lower; container back-edges need rebinding a container into
itself), so the guard stays latent. A quoted string's escaping is ASCII-focused:
non-printable non-ASCII codepoints are not escaped.

## 10. Errors & exceptions (D-0007, D-0029)

Unchecked exceptions; nothing is ever forced to be caught.

```python
try:
    raise ValueError("bad integer")
except TypeError:
    print("wrong kind of error")
except ValueError as e:
    print("recovering:", e)  # e is the exception message (a str)
finally:
    cleanup()
```

- `raise` throws; built-in faults (divide-by-zero, index-out-of-range, `None`
  access) also throw.
- `except` catches (optionally); `finally` always runs, including during
  unwinding.
- Uncaught exceptions unwind to the top of the coroutine and terminate with a
  stack trace. Exception types form a Python-like hierarchy.
- Built-in exception types are `BaseException`, `Exception`, `ValueError`,
  `TypeError`, `LookupError`, `IndexError`, `KeyError`, `ArithmeticError`,
  `ZeroDivisionError`, `RuntimeError`, `StopIteration`, `Cancelled`,
  `ScopeTimeout`, `ExceptionGroup`, `TimeoutError`, and `AssertionError`.
  `IndexError` and `KeyError`
  derive from `LookupError`; `ZeroDivisionError` derives from `ArithmeticError`; those
  intermediate types derive from `Exception`. `Cancelled` derives directly from
  `BaseException`, so `except Exception` does not suppress cancellation.
  `ScopeTimeout` (raised to the caller when a `scope(timeout=…)` deadline expires)
  derives from `Exception` and so is catchable by `except Exception`. `TimeoutError`
  (raised when a socket read exceeds the per-park deadline set by
  `net.set_read_timeout` or the whole-operation budget set by
  `net.set_read_deadline`) likewise derives from `Exception`, and is distinct from
  a clean EOF (an empty `net.recv` result) so a server can tell an idle/read
  timeout apart from a peer close. The remaining listed leaf types derive from `Exception`; `Exception`
  derives from `BaseException`.
- `raise E(message)` constructs a typed built-in exception. Raising a string is
  shorthand for raising `Exception(message)`. A typed `except E` catches `E`
  and its subclasses; clauses are tested in source order. A bare `except` is a
  catch-all and must be last. In `except E as e`, `e` is the message string.
- A user-defined struct may be raised as an exception with one string message
  (`raise MyError(message)`). User-defined exception types derive from
  `Exception`; they can be named in `except` clauses and retain their identity
  when propagated.
- `raise E(message) from cause` chains an explicit cause (D-0103). The `cause`
  is recorded on the raised exception; when `cause` is a caught exception it is
  stored as that exception (keeping its type and any further cause), otherwise a
  `str` cause is wrapped in `Exception`. In a handler, `e.cause` reads the
  explicit cause's message (an empty str when there is none). An uncaught
  chained exception renders a two-section traceback — the cause's own section,
  the line `The above exception was the direct cause of the following
  exception:`, then the new exception's section — matching CPython's explicit
  (`raise ... from`) chain. `from` is a contextual word, not a reserved keyword,
  so it remains usable as an ordinary identifier.
- A bare `raise` (no operand) inside an `except` handler re-raises the exception
  currently being handled, preserving its type and identity — the canonical
  Python re-raise (#657, D-0116). It re-throws the very object the handler
  caught, so a `ValueError` re-raised from an `except Exception` handler is
  still caught by an enclosing `except ValueError` (and `except E as e` then
  bare `raise` re-raises that same `e`). The innermost active exception is
  re-raised: a bare `raise` in a nested handler re-raises that nested handler's
  exception. Unlike Python, which defers an out-of-context bare `raise` to a
  runtime `RuntimeError: No active exception to re-raise`, Zap tracks the active
  exception lexically and per function and rejects the impossible cases at
  compile time (ZAP6002): a bare `raise` in a `try` body, in an `else`/`finally`
  that is not handling an exception, at top level, or in a function merely
  *called* from a handler (Zap does not implement Python's dynamic,
  cross-call-frame re-raise in v1). Full traceback preservation across the
  re-raise rides on the green-stack unwinder (I#475) and is a follow-up; type
  and value/identity are preserved today.

### 10.1 `except*` — exception-group handling (D-0129, #605)

A `scope:` that collects two or more concurrent child failures re-raises them as
an aggregate `ExceptionGroup` (§12, D-0045). `except*` handles such a group
declaratively — the Python 3.11 construct, with Zap's bare-for-single rule:

```python
try:
    scope:
        spawn fetch(a)          # raises TimeoutError
        spawn parse(b)          # raises ValueError
except* TimeoutError as timeouts:
    for m in timeouts.exceptions:   # eg is always a GROUP; iterate its messages
        log_retry(m)
except* ValueError as values:
    report(values)
```

- A `try` is **all-`except` or all-`except*`** — the two forms cannot be mixed
  (`ZAP3001`). An `except*` clause **requires a type**; a bare catch-all
  `except*:` is rejected (`ZAP3001`).
- Each `except*` clause peels the sub-exceptions of the in-flight group matching
  its type (subclasses included) into a **sub-`ExceptionGroup`**, binds `as NAME`
  to that group (always a group, even for a single match), and runs its handler.
  Unlike plain `except` (first-match-only), **every** `except*` clause whose type
  matches some sub-exception runs, in source order.
- A **bare** (non-group) exception is matched as a **one-element group**, so a
  single failure caught by `except* E` behaves uniformly — `try/except E` (single
  failure) and `except*` (fan-out) coexist.
- After the last clause the **unmatched remainder auto-re-raises**: nothing when
  all sub-exceptions were handled, the single survivor **bare** (so a plain
  enclosing `except E` still catches it, keeping bare-for-single), or a fresh
  `ExceptionGroup` when two or more remain.
- The `ExceptionGroup` type is retained: `except ExceptionGroup as eg` still
  catches a whole group, and `eg.exceptions` yields the sub-exception messages as
  a `list[str]` for manual inspection (iteration). Sub-exceptions are kept in a
  stable, sorted-by-message order, so group rendering and `.exceptions` agree
  regardless of coroutine scheduling.
- A handler that itself raises replaces the in-flight group and abandons the
  remainder (mirroring plain `except`); `finally` still runs.

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
  several fail, D-0045), which `except*` can split by type (§10.1, D-0129). `scope(timeout=<ms>, on_error=OnError.Cancel|Wait)`
  adds a deadline and selects the failure policy (D-0094). The `timeout` value is a
  **nonnegative integer number of milliseconds** — no unit suffix and no `Duration`
  type (`_` digit separators are allowed, e.g. `timeout=30_000`); a negative value
  raises `ValueError`, and zero cancels immediately. At expiry, cancellation reaches
  owned children (including nested scopes and parked channel operations);
  `finally`/`with` run and the scope awaits them. After draining, the scope raises
  **`ScopeTimeout`** to its caller — an `Exception` subtype carrying the deadline and
  the complete set of coroutines still outstanding at the deadline (each by name and
  what it was parked on). A recorded child failure takes precedence over the
  timeout. `timeout`, `on_error`, and `cancel_on` are orthogonal: none caps or
  references another. On a `timeout` deadline, `grace` is an optional
  integer-millisecond **cooperative-drain window** after which a child that still
  has not drained is **severed** — detached and leaked like `spawn detached` (never
  force-unwound or freed), surfaced to stderr / the coroutine dump — and the scope
  raises `ScopeTimeout` with the process still alive (#825, D-0142). A bare
  `scope(timeout=T)` with no `grace` severs immediately (implicit grace 0); the
  deadline join never hangs and never force-exits the process. Process hard exit is
  reserved for the `cancel_on` signal-shutdown path (`grace` there is the
  hard-exit deadline, measured from the first such signal). A severed orphan has no
  happens-before join edge to the caller, so reading its partial state is a bounded
  race (D-0141). Completion before the deadline disarms the timer. Cancellation
  remains cooperative; CPU loops must `checkpoint()`. `spawn detached <call>` opts
  out of scope ownership (daemon; Go exit semantics). The children receive `Cancelled`,
  which is outside `Exception`.
- **Spawn call forms (D-0118):** `spawn` accepts three callable forms — a named
  function (`spawn worker(ch)`), a user-struct method call
  (`spawn server.handle(conn)`), and a first-class function value (`spawn f(x)`,
  where `f` is a variable, field, or expression of `fn(...)->...` type — a bound
  lambda, a closure, or a function value). Arguments and the receiver/closure are
  captured **eagerly in the parent** (so capture cannot race the child, D-0034). All
  forms take **positional arguments only** — no keyword, spread, or default
  filling. A bare lambda *literal* operand (`spawn (lambda: log(x))()`) is a compile
  error (`ZAP5076`): a lambda has no type without a function-typed context, so bind
  it to a name first (`auto log_it = lambda: log(x); spawn log_it()`). Keyword/spread
  arguments, an arity mismatch, and a non-callable operand are also `ZAP5076`.
- **Memory model (D-0034):** data-race-free programs are sequentially
  consistent; data races are bugs.

### 12.1 Concurrency-safety surface: `@shareable` + the sharing lint (D-0113, #664)

Module-level mutable state (§3.1) is shared across coroutines **unsynchronized**.
Zap keeps the light model honest with an **advisory** surface that is *never* a
dialect: the same program compiles whether or not you heed it.

```python
@shareable                         # a trusted assertion: safe under sharing
struct Registry:
    Mutex guard
    dict[str, int64] entries

Registry reg = Registry(...)       # mutable global of a @shareable type — no warning

int64 hits = 0                     # mutable global of a plain type
def bump():
    hits += 1                      # warning[ZAP5125]: shared mutable access
    hits += 1  # noqa              # suppressed at this one site
```

- **`@shareable`** marks a `struct` as safe under concurrent shared access. It is
  a **trusted, unverified** claim — the compiler does not prove it — takes no
  arguments, and is reflectable: `reflect_has(T, "shareable")`.
- **The built-in concurrency primitives carry it implicitly:** `Atomic`, `Mutex`,
  `WaitGroup`, `Once`, and the `concurrent` collections `Counter`/`Queue`. The
  lint applies **one uniform rule** to user and built-in types alike — there is
  no separate hardcoded allowlist.
- **The sharing lint (`ZAP5125`, a warning)** fires at each read or write of a
  **mutable** module global whose type is **not `@shareable`**. A `const` global
  **never** warns (it cannot race); a mutable global of a `@shareable` type is
  silent. Accesses inside the single-threaded module-init pre-pass, and names
  shadowed by a local, do not warn.
- **Suppress one site** with a trailing `# noqa` (all codes on the line) or
  `# noqa: ZAP5125` (specific codes). Line-based, warning-only.
- **Policy:** `zap build/run/ir --warnings-as-errors` (alias `-Werror`) makes any
  surviving warning fatal, for CI. Off by default; it never changes what the
  language accepts.

The lint is advisory; a data race is still a bug (§12 memory model). The
*dynamic* backstop is the runtime **data-race detector**, `zap run/test --race`
(alias `-race`): a test-time diagnostic (D-0114, #665) that catches real races
the lint cannot see statically. It is a **Zap-native happens-before / vector-
clock** detector (not LLVM/Clang ThreadSanitizer, which assumes pthreads and
`malloc` rather than Zap's user-space coroutines and GC heap). It instruments
exactly the ZAP5125 set — a mutable, non-`@shareable` module global — so an
`Atomic`/`Mutex`/`@shareable`-guarded version is clean, and reports an
unsynchronized racing access with a source location. It is off by default and
**never** changes what the language accepts, only whether accesses are checked
at run time. The detector is now **live** (slices 1–3, #677/#678/#679): under
`--race` each coroutine carries a vector clock, each synchronization object a
release/acquire clock, and each instrumented global shadow state. The
happens-before edges now live are:

- **spawn** — parent → child at creation;
- **`Mutex`** — `unlock` (release) → `lock` (acquire);
- **channels** — a send → the matching receive, for both an unbuffered/rendezvous
  hand-off and a buffered one (through the ring-buffer slot);
- **`WaitGroup`** — every `done()` → a `wait()` that returns;
- **`Once`** — the winner's `once_end` → every other caller's `once_begin` return;
- **`Atomic`** — clean by exemption (a `@shareable` cell is not instrumented).

A conflicting access with no ordering edge is reported — naming the global, each
access's coroutine and source span — and the process exits non-zero. The report
is diagnostic output (not a stabilized surface). The detector is correct under
`ZAP_MAXPROCS>1` (true parallelism; validated across the fixture set). Still
tracked as follow-ups under #665: the **main/native thread's own accesses** are
untracked (a genuine main-vs-coroutine race is missed; races between spawned
coroutines are caught), **shared heap objects** beyond module globals are a
split-out slice-4 phase (#684), and full **multi-frame stack traces** await the
I#475 green-stack unwinder.

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

The current **Experimental** surface is dense, **N-dimensional** numeric arrays
with SIMD element-wise kernels. An array carries a **static rank** (`ndim`, part
of its type) and a row-major shape. This is the supported subset of the
longer-term N-D design in `design/0002-arrays.md`.

| Surface | Supported now |
| --- | --- |
| Dtypes | floats `f32`/`f64`, signed `i8`/`i16`/`i32`/`i64`, unsigned `u8`/`u16`/`u32`/`u64`, `bool` |
| Construction | A positional list literal, flat (`array[f64]([1, 2.5])`, 1-D) or **nested rectangular** (`array[f64]([[1, 2], [3, 4]])`, N-D); empty / zero-axis literals are allowed |
| Constructors | `zeros[T](shape)`, `ones[T](shape)`, `full[T](shape, v)` (shape = an int or a tuple literal); `arange[T](start, stop[, step])`; `linspace[T](start, stop, num)` |
| Elementwise math | `math.sqrt`/`exp`/`log`/`sin`/`cos`/`tan`/`fabs` applied to an array map over it, returning a new float array (`f32`→`f32`, else `f64`) |
| Access | One integer index per axis. A *full* index `m[i, j]` is a scalar get/set; a *partial* index `m[i]` is a rank-reduced **view**. Indices are in-bounds and nonnegative |
| Shape | `a.ndim` (rank), `a.shape` (a `tuple[int64; ndim]`), `a.size` (total elements); `len(a)` is the first axis |
| Arithmetic | Element-wise `+`, `-`, `*`, `/`; two arrays **broadcast** (right-align shapes, stretch size-1 axes, extend rank). `/` is **true division** (always a float result) |
| Scalar broadcast | `a + n`, `a - n`, `a * n`, `a / n`; and scalar-on-left `n + a`, `n * a`, `n - a`, `n / a` (shape preserved) |
| Reduction | `sum(a)` over **all** elements, or the methods `a.sum()`/`a.min()`/`a.max()`/`a.mean()`/`a.prod()`; an optional `a.sum(axis=k)` collapses one axis to a rank-(n-1) array. `sum`/`min`/`max`/`prod` keep the dtype, `mean` is a float |
| Slicing | Views that alias the parent: contiguous `a[lo:hi]`, stepped/reversed `a[::2]` / `a[::-1]`, N-D axis-0 windows `m[lo:hi]`, and a sliced axis in a multi-axis index `m[:, j]` (per-axis strides) |
| Linear algebra | `a @ b` (matrix multiply, PEP 465) / `dot(a, b)` / in-place `a @= b` on 1-D & 2-D arrays: 1-D·1-D → scalar dot, 2-D·2-D → matrix, mixed → vector |
| Shape ops | `a.reshape(d…)` and `a.transpose()` / `a.T` are views; `a.flatten()` is a 1-D copy; `concatenate([…], axis=)` / `stack([…], axis=)` build new arrays |
| Masks | Element-wise comparisons `a > 3` / `a == b` give an `array[bool]`; combine with `&` `\|` `^` `~`. An array in a boolean context (`if a:`, `and`/`or`, `lo < a < hi`) raises — use `.any()`/`.all()` (follow-up) or a mask |
| Advanced indexing | `a[mask]` (boolean) and `a[idx]` (integer array) gather a fresh **copy**; `a[mask]=`/`a[idx]=` scatter. Multiple index arrays and mixed `a[1:3, idx]` broadcast and follow NumPy's transpose rule. Out-of-bounds → `IndexError` |
| Selection | `where(cond, x, y)` picks element-wise with broadcasting (`x`/`y` may be scalars); `where(cond)` returns the index arrays of the true cells |

Mixed array dtypes promote by NumPy NEP-50 (a float operand forces the widest
float; same-signedness integers take the wider; mixed signed/unsigned take the
signed type that holds both, `u64`-with-signed falling back to `f64`); arithmetic
on two boolean arrays yields `uint8`. A weak integer scalar keeps the array dtype
and a weak float scalar keeps a float array's width (else `f64`). Integer dtypes
wrap modulo 2^width. Two arrays **broadcast** (NumPy rules:
right-align shapes, stretch size-1 axes, and extend the lower-rank operand with
leading 1s; the result takes the larger rank and the larger aligned dim). Shapes
that cannot broadcast together raise a catchable `ValueError` ("array shape
mismatch") before the kernel reads the buffers. Both the equal-shape and the
broadcast element-wise kernels are vectorized along the innermost axis (#896) —
a performance detail with no effect on results.

Division `/` is NumPy **true division**: the result dtype is always a float, so
an all-integer division lifts to `f64` (`array[i64] / 2 → f64`) while a float
operand keeps its width (`array[f32] / 2 → f32`). A zero divisor follows
IEEE-754 — `x/0 → ±inf`, `0/0 → nan` — and does **not** raise (unlike scalar
`/`, which raises `ZeroDivisionError`); array arithmetic follows NumPy, which
warns but does not raise. Floor division `//` and modulo `%` are not yet
supported on arrays.

Reductions (`sum`/`min`/`max`/`mean`/`prod`) reduce the whole array to a scalar,
or — with `axis=k` — one axis, giving a rank-(n-1) array (on a rank-1 array
`axis=0` is the scalar reduction). `sum`/`min`/`max`/`prod` keep the dtype (an
integer product wraps per width); `mean` is a float (`f32` source → `f32`, else
`f64`). Empty reductions are defined for `sum` (`0`), `prod` (`1`), and `mean`
(`nan`); `min`/`max` on an empty array, and an out-of-range `axis`, raise a
catchable `ValueError`. Reductions are vectorized (multiple accumulators, #911):
a performance detail only — integer reductions are bit-identical at any size and
every `axis=` reduction keeps its per-element accumulation order, while a
floating-point whole-array (or last-axis) `sum`/`prod`/`mean` may differ from a
strict left-to-right fold within IEEE-754 reassociation tolerance.

Slices are **views** (per-axis strides): a stepped `a[::2]`, reversed `a[::-1]`,
an N-D axis-0 window `m[lo:hi]` (rank-preserving), and a sliced axis inside a
multi-axis index `m[:, j]` all share the parent buffer, so a write through the
view is visible through the parent. A zero step is an error. Arithmetic over a
strided operand transparently compacts it to a contiguous copy first.

The `@` operator (PEP 465) and `dot(a, b)` are matrix multiplication on 1-D and
2-D arrays: 1-D · 1-D is a scalar dot product, 2-D · 2-D a matrix product, and a
mixed 1-D/2-D operand is promoted to a row/column with its singleton axis
dropped (2-D · 1-D and 1-D · 2-D yield a 1-D vector). The result dtype follows
NEP-50; a shape mismatch on the contracted axis raises `ValueError`. `@=` is the
in-place form. `@` is defined only for arrays — on any other type it is an
ordinary "no such operator" error — and batched (≥ 3-D) matmul is not yet
supported. The kernel is chosen automatically (#904): an own blocked+SIMD floor
(#913) by default, or a BLAS gemm for large float products when a provider is
linked, overridable with `ZAP_ACCEL` (`auto` / `native` / `fast`) and the BLAS
pool pinned to the CPU budget (`ZAP_ACCEL_THREADS`, default container-aware
MAXPROCS). Integer products always use the exact own kernel; a large float
product via BLAS matches the own kernel only within floating-point tolerance
(BLAS reassociates), as NumPy's BLAS-backed `@` does.

`import linalg` provides LAPACK-backed linear algebra over numeric arrays
(#926): `linalg.solve(a, b)` (solve `a @ x == b`), `linalg.inv(a)`, and
`linalg.det(a)`, matching `numpy.linalg`. `a` must be a square 2-D matrix;
integer input promotes to `f64` and the result is `f64`; a singular matrix or a
misaligned/non-square shape raises a catchable `ValueError`. (Eigensolvers,
decompositions, `lstsq`, and `norm` are later increments.)

Shape ops reshape the N-D view. `a.reshape(d0, d1, …)` takes positional dims
(one may be `-1`, inferred) and is a view when the source is contiguous, else a
copy; `a.transpose()` / `a.T` reverse all axes as a view (a stride permutation);
`a.flatten()` is a fresh 1-D row-major copy. `concatenate([a, …], axis=k)` joins
same-rank arrays along an existing axis (other dims must match) and `stack([a,
…], axis=k)` joins same-shape arrays along a new axis (rank + 1); both are fresh
copies. A size/shape/axis fault raises a catchable `ValueError`.

Element-wise comparisons (`a > 3`, `a == b`, with scalar broadcast) produce an
`array[bool]` **mask**; `&`, `|`, `^` combine masks and `~` inverts one. Because
the truth value of a multi-element array is ambiguous, an array in a boolean
context — `if a:`, `a and b`, `not a`, or a chained `lo < a < hi` — raises a
catchable `ValueError`. A mask indexes: `a[mask]` applies to the leading axes and
returns a fresh 1-D (or rank-reduced) **copy** of the selected elements, and
`a[mask] = v` scatters a scalar or array of values in place. Integer-array
indexing `a[idx]` gathers along axis 0 (result shape `idx.shape + a.shape[1:]`, a
copy; negative indices wrap, out-of-bounds raises `IndexError`), and `a[idx] = v`
scatters (duplicate indices resolve last-write-wins). A multi-axis subscript may
mix scalars, slices, and index arrays (`a[1:3, idx]`, `a[i, j]`): the index
arrays broadcast together and the result follows NumPy's layout rule (the
broadcast axes sit at the advanced block when its axes are contiguous, else at
the front). `where(cond, x, y)` selects element-wise with broadcasting (`x`/`y`
may be scalars); `where(cond)` returns a tuple of the true cells' index arrays.
Advanced indexing always copies (unlike a slice view).

A bare `array[T]` type annotation denotes a rank-1 array; N-D values arise
through inference (`auto`). List-variable/iterable `array[T]` construction,
a fully dynamic-rank (runtime-tuple) constructor shape, element-wise unary
negation and integer-array (non-mask) bitwise operations, floor division
`//`/modulo `%`, `keepdims` reductions, broadcasting in assignment or
partial-index (whole-row) stores, assigning through a sliced index (a
strided-view store), and explicit N-D type annotations are unsupported. Semantic
or lowering diagnostics reject these forms.

```python
auto a = array[f64]([1, 2, 3])
auto c = a + 1.0
auto window = a[1:3]      # contiguous 1-D view
window[0] = 9.0          # a[1] is now 9.0

auto m = array[i64]([[1, 2, 3], [4, 5, 6]])
print(m.shape, m.ndim)   # (2, 3)-style tuple, 2
auto x = m[1, 2]         # 6 (full index -> scalar)
auto row = m[0]          # rank-1 view [1, 2, 3]; row[0] = 9 writes m[0, 0]
auto s = sum(m)          # 21 (reduces every element)
auto b = m + array[i64]([10, 20, 30])  # broadcast (3,) across rows -> (2, 3)
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
may take arguments). A `decorator` body declares hooks: the `before(ctx)` /
`after(ctx, result)` convenience pair, or the general `wrap(call)`. `ctx`
exposes `ctx.name` (the decorated function's name). Applying a behavioral
decorator generates a wrapper **with the decorated function's exact signature
preserved** (the "`@wraps` is automatic" contract) at compile time, visible via
`zap expand` (D-0108, epic #651):

```python
decorator traced:
    def before(ctx):
        print("enter", ctx.name)
    def after(ctx, result):
        print("exit", ctx.name, result)

@traced
def fetch(str url) -> str:
    ...
```

The general **`wrap(call)`** form replaces the whole call. Its body is a nested
`def inner(*args):` (confined to the decorator-body parser — not a general
nested-`def`) followed by `return inner`; the inner body is inlined into the
generated wrapper and `call(*args)` becomes a typed call of the original:

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
def fetch(str url) -> str:
    ...
```

`*args` is a **compile-time forwarding template, not a runtime value**: it may
only appear as `call(*args)`, which forwards the decorated function's exact
typed parameters (so the wrapper keeps that exact signature). Reading/indexing
`*args`, using `call` as a value, mixing `wrap` with `before`/`after`, or
omitting `return inner` are stable errors (ZAP5052/ZAP5054). When the inlined
body is not provably return-terminating (e.g. `retry`'s loop), an implicit
`raise RuntimeError(...)` fall-through guard is appended so the wrapper
type-checks. A `wrap` stacks with other decorators bottom-up like any wrapper.
(An `args`-keyed `memoize` is not expressible by `wrap` alone: it needs
persistent cross-call state. The built-in **`@cache`** supplies exactly that,
over a module-level dict — see below.)

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

**Built-ins (v1):** `@derive(...)`, `@default`, `@validate`, `@range` (all
compile-time code generation, D-0101), **`@deprecated`** (I#472, D-0109),
**`@shareable`** (I#664, D-0113), and **`@cache`** (I#666, D-0115).
`@deprecated(msg?)` is a function/method
decorator that generates no code but emits a non-fatal **use-site warning**
(`ZAP5124`) at every call site of the decorated callable — the mechanism for
retiring a Stable surface with notice (STABILITY.md §5). An optional message is
appended (`@deprecated("use X")`). **`@shareable`** is a no-argument marker on a
`struct` asserting it is safe under concurrent shared access (a trusted,
unverified claim); it generates no code, is reflectable via
`reflect_has(T, "shareable")`, and drives the sharing lint (§12.1).

**`@cache`** memoizes a free function by its argument tuple. It is expanded —
before type-checking, and so `zap expand`-visible — into ordinary, type-checked
Zap over a **module-level dict** (never a hidden compiler store, a hard rule):

```python
@cache
def fib(int64 n) -> int64:
    return n if n < 2 else fib(n - 1) + fib(n - 2)
```

generates a `@derive(Hash, Eq)` key-struct (the typed argument tuple, as a named
struct), a `@shareable` store struct holding a `Mutex` handle and the memo
`dict`, an immortal module global, and a wrapper that returns a cached hit or
computes-and-stores a miss — so the body runs exactly once per distinct argument
tuple. The miss result is computed *outside* the lock (so a recursive `@cache`
function cannot self-deadlock, and recursion goes through the memoizing wrapper);
the lock guards only the dict bookkeeping, matching `functools.lru_cache`. v1
takes no arguments and is **unbounded for the process lifetime** — a documented
memory cost, since the global is an immortal GC root (D-0115), not an eviction
policy. The key must be **value-hashable**: in v1 every parameter is an integer,
`bool`, or `str`. A **reference type** (a `struct`/`enum`, or a collection) is
**rejected** (`ZAP5056`), not silently keyed by identity — identity keying would
miss on equal-but-distinct arguments and give no benefit while looking correct.
The function must also return a value, and `@cache` is a free-function decorator
in v1; otherwise `ZAP5056`. It stacks bottom-up with other decorators.

**Not in the v1 set (I#472/D-0109):** `@inject` is **declined for core** —
dependency injection is a framework concern, served by reflection-driven
`Depends(...)` adapters (D-0089/D-0092), not a core decorator; there is no
planned core `@inject`. It stays reserved and is rejected with a dedicated
diagnostic (`ZAP5055`) that redirects to `Depends(...)`.

## 17. Toolchain & CLI (D-0053)

One user-facing tool, **`zap`** (the `zapc` compiler becomes internal):

```
zap run <file> | zap run      # run a single file, or the project (zap.toml)
zap build                     # build project -> native binary (target/)
#   run/build/ir also accept --warnings-as-errors (alias -Werror): make surviving
#   warnings fatal, for CI; off by default and never changes what compiles (D-0113)
#   run/build/ir/test also accept --race (alias -race): enable the runtime
#   data-race detector (test-time diagnostic, off by default; D-0114, #665)
zap test [--watch] [--json] [--race]   # run tests; --watch re-runs (targeting changed paths); --json for agents; --race under the detector
zap test --doc [path]         # run docstring examples (doctests, #234) through the REPL engine
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
`hex`, `net`, `http`, `tls`, `zlib`, `gzip`, `yaml`, `threading`, `concurrent`,
`url` — **Experimental**
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
and any same-named map entry is ignored. `http.read_request(conn)` parses a
request off a connection into an opaque handle read back with
`http.method(req) -> str`, `http.path(req) -> str`,
`http.req_version(req) -> str` (the request-line HTTP version token, e.g.
`"HTTP/1.1"`; `""` for an HTTP/0.9-style line with no version — #486),
`http.req_header(req, name) -> str` (case-insensitive, `""` when absent) and
`http.req_body(req) -> str`. Both `http.read_request` and `http.respond` accept
**either** a plaintext `net` connection **or** a TLS connection from
`tls.accept` (both are the one opaque connection handle) — the request parser and
response writer drive the connection through a transport seam, reading/writing
over `SSL_read`/`SSL_write` for a TLS conn and `recv`/`send` for a plaintext one,
so an HTTPS server terminates TLS through the same `http` surface (#556). Both
read deadlines apply uniformly over either transport: the per-park timeout
(`net.set_read_timeout`) bounds each wait-for-readiness, and the whole-operation
budget (`net.set_read_deadline`, #567) bounds an entire `http.read_request`
(headers + body) regardless of byte pacing — so a slow-dribble (slowloris) client
that keeps resetting the per-park clock is still cut off with a catchable
`TimeoutError`. The two compose: with both set, each park waits the nearer of the
per-park timeout and the whole-op deadline. `http` is **Experimental**
(STABILITY.md §4.4).

**`tls` client + server (#147, #488).** TLS over OpenSSL on the netpoller.
*Client (#147):* `tls.connect(host, port)` verifies the certificate chain
against the system trust store **and** the hostname (on by default);
`tls.connect_insecure(host, port)` is the loudly-named opt-out (never the
default). *Client contexts (#560):* for private CAs or mutual TLS, a per-connection
client context — mirroring Python's `ssl` module — carries its own trust roots and
an optional client certificate, fully isolated from other connections (its own
`SSL_CTX`, no process-global `SSL_CERT_FILE`/`SSL_CERT_DIR` mutation).
`tls.create_default_context()` preloads the system trust store (verify + hostname
on; the *augment* base), while `tls.SSLContext()` starts with an **empty** trust
store (trusts only what is loaded; the *replace* base). `tls.load_verify_locations(ctx,
cafile[, capath])` **additively** adds CA trust and `tls.load_cert_chain(ctx,
certfile[, keyfile])` adds a client certificate(-chain) + private key for mutual
TLS. Dial with a context via the optional trailing argument `tls.connect(host,
port, ctx)` — SNI and the hostname identity are still checked against `host`, so a
context is reusable across hosts. Bad CA/cert/key paths or a mismatched key pair
raise a catchable error with full resource cleanup. *Server (#488):* `tls.listen(host, port, cert_path, key_path)` loads a
certificate(-chain) and private key from **PEM files on disk** into a server
context and binds host:port (port `0` → OS-assigned ephemeral, read back with
`tls.port(listener) -> int64`); `tls.accept(listener)` accepts a socket, runs the
**server-side** handshake over the netpoller and returns a conn — the **same
opaque handle** the client produces, callable in a loop (one conn per accept).
`tls.close_listener(listener)` closes a listener, which makes a coroutine parked
in `tls.accept` **raise**, exactly as `net.close_listener` unblocks `net.accept`
(the graceful-shutdown contract). Both ends share `tls.send(conn, data) ->
int64`, `tls.recv(conn, n) -> str` and `tls.close_conn(conn)` (`close` is the
reserved channel-close token, so sockets spell it `close_conn` / `close_listener`,
as in `net`). A `tls.recv` honors the read deadlines set by `net.set_read_timeout`
/ `net.set_read_deadline` on the conn, raising a catchable `TimeoutError` exactly
as plaintext `net.recv` does (#566). `http` routes `https://` through the client
with verification on. `tls` is **Experimental** (STABILITY.md §4.4).

**`url` — `encoding/url` percent-encoding (#335).** RFC-3986 percent-encoding and
decoding over raw UTF-8 bytes, plus the AWS-query canonicalization variant used
by SigV4. A `str`'s bytes are its UTF-8 encoding, so every function encodes
byte-by-byte and is multibyte-correct. The surface:
`url.quote(s) -> str` leaves the **unreserved** set (`A-Z a-z 0-9 - . _ ~`,
RFC-3986 §2.3) as-is and encodes every other byte (including `/` and space,
which becomes `%20`, never `+`) as `%XX` with **UPPERCASE** hex — this is exactly
the SigV4 URI-encode, so it is the primitive the signer applies to query
keys/values and single path segments; `url.quote_path(s) -> str` is the same but
leaves `/` unescaped (a whole path), so a SigV4 CanonicalURI is
`url.quote_path(path)` for S3 and the double-encoded
`url.quote_path(url.quote_path(path))` for every other service;
`url.unquote(s) -> str` is the exact inverse (`%XX` back to a byte; a bare `+`
stays literal and a malformed/truncated escape passes through unchanged), and
`url.unquote_plus(s) -> str` additionally maps `+` -> space for
`application/x-www-form-urlencoded` values. `url.encode_query(d: dict[str,str])
-> str` builds `k=v&…` with each key/value `quote`-encoded in the dict's
(unspecified) iteration order, and `url.canonical_query(d: dict[str,str]) -> str`
is the SigV4 **CanonicalQueryString**: each key/value `quote`-encoded, the pairs
sorted by encoded key then value and joined with `&`. `url` is **Experimental**
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
`const`, `True`, `False`, `None`, `and`, `or`, `not`, `is`, `raise`, `try`, `except`,
`finally`, `with`, `as`, `yield`, `lambda`, `assert`, `spawn`, `scope`, `channel`,
`select`, `case`, `close`, `struct`, `interface`, `enum`, `annotation`,
`decorator`, `import`, `python`, `m`, `s`
(contextual, before a delimiter; `detached` is likewise contextual, only after
`spawn`; `pass` and `del` are also contextual statement-position words, not
reserved — see §6), and the type keywords (`int32`, `int64`,
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
| `set` type (literals, comprehensions, `set[T]()`/`set(iterable)` ctors) | Locked (#608/#609, D16/D17) | **S** |
| C-style bitfields on `value struct` | Locked (D-0068) | **E** |
| `when target.*` conditional compilation | Shipped Phase 1 (D-0079) | **E** |
| `**kwargs` arbitrary named args | Deferred (D-0038) | n/a |

### Absolute socket deadlines and cancellation (#558, #559; D-0099)

`net.set_deadline_ns(conn, deadline_ns)` installs an absolute CLOCK_MONOTONIC
nanosecond bound shared by reads and writes over a TCP or TLS connection.
Calculate it from `time.monotonic_ns()`. Zero clears it; negative values raise
ValueError. Unlike `set_read_timeout` and `set_read_deadline` (relative
milliseconds), this bound persists across calls and partial writes. The nearest
configured bound wins; expiration raises TimeoutError, distinct from EOF.
Configure before I/O; clearing does not revoke an already parked wait's bound.

`net.connect_ip_deadline(ip, port, deadline_ns)` is a bounded numeric-address
TCP dial. `tls.connect_resolved(hostname, ip, port, deadline_ns)` uses one budget
for the numeric TCP dial and verified TLS handshake, retaining hostname/SNI and
system trust verification. IPv4/IPv6 literals only on the address parameter;
DNS resolution is a separate prerequisite and is not covered by these APIs.
Existing named connects retain synchronous resolution. A zero dial deadline
is unbounded, a negative deadline or invalid numeric IP/port is ValueError.

Scoped cancellation wakes parked TCP/TLS I/O and raises Cancelled after native
operation scratch and partial connections are cleaned. A scope waits for its
children's finally cleanup. Existing established connections/listeners remain
caller-owned; close in finally. Cancellation/timeout does not imply that no
bytes or side effects reached the peer, so do not automatically replay them.

TLS socket writes, including OpenSSL alert/shutdown writes, suppress SIGPIPE per
socket. Peer failures remain catchable transport errors without changing the
application's process-wide signal dispositions.
