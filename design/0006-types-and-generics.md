# Proposal: User types & generics — structs, interfaces, enums, tuples, generics

- **Author(s):** Zap team
- **Last updated:** 2026-10-18
- **Status:** Accepted (design) — implemented incrementally (T8 structs/methods first, generics staged)
- **Discussion:** github.com/parveshgarg/zap#15

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line.

## Abstract

This document pins the detailed design of Zap's user-defined type system: data
**structs** with **methods** (D-0015), compile-time **structural interfaces**
(D-0016), **enums** (tier-1 named constants now, D-0018, with the path to
data-carrying variants and `match`, D-0040), **tuples** (D-0019), and
**generics via monomorphization** (D-0017).
It is specification-level detail — syntax, semantics, and in-memory
representation — sufficient to implement T8 (structs/methods) and, later,
generics without re-deciding.
It resolves two standing OPEN-QUESTIONS items — the struct method-definition
syntax (inline vs receiver block) and the tuple literal/destructuring/multi-
return syntax — and coordinates all layouts with `design/RUNTIME-ABI.md`.

## Background

The surface forms were drafted in `SPEC.md` §4.4–4.7; this document makes them
precise and fills the gaps the spec defers.

Relevant locked decisions:

- **D-0002 / D-0003** — Python-feel surface; type-first declarations
  (`TYPE name = expr`), reassignment is `name = expr`.
- **D-0014** — `None` is the null value for reference types; scalars are not
  nullable.
- **D-0015** — composite types are structs with methods; **composition only,
  no inheritance, no classes** (Go-style). Method-definition syntax was left
  open.
- **D-0016** — interfaces are **structural**, satisfied implicitly, but
  conformance is **checked at compile time**; an optional explicit
  `impl Interface for Struct` form may be offered.
- **D-0017** — user generics via **monomorphization**; grammar reserves the
  syntax from the start so adding them is additive.
- **D-0018** — enums: tier-1 named constants now; data-carrying variants later.
- **D-0019** — tuples are provided; **no type aliases**.
- **D-0023** — struct instances are **reference types** (shared, heap, passed by
  pointer); scalars are values.
- **D-0040** — `match`/`case` is locked, with compile-time exhaustiveness for
  enum scrutinees.
- **D-0036** — decorators/`@derive` are the cross-cutting-reuse mechanism that
  replaces inheritance (equality, serialization, …); this design leans on it
  rather than re-inventing those features.

Layout and dispatch must agree with the runtime ABI
(`design/RUNTIME-ABI.md`): a one-word type-descriptor header on every heap
object, fields in declaration order with C alignment, methods taking the
instance pointer as the first argument (`self`), and name mangling that encodes
monomorphized type arguments.
This document references that contract and does not change it.

## Proposal

### 1. Structs

#### 1.1 Declaration and fields

A `struct` declares a named aggregate.
Fields use the type-first declaration form (D-0003), one per line in an
indented block, and may carry an inline default:

```python
struct Point:
    f64 x
    f64 y

struct Config:
    str   host
    int64 port = 8080          # field default (D-0038 semantics: per-construction)
    bool  tls  = False
```

- Fields are laid out **in declaration order** with natural C alignment
  (RUNTIME-ABI §"Struct layout"); field order is therefore part of the type's
  representation but *not* part of its construction API (see §1.3).
- A field whose name begins with `_` is module-private (D-0028), enforced at
  compile time.
- Field defaults are evaluated **per construction** (a fresh value each time,
  like default arguments, D-0038), so mutable defaults (`[]`, `{}`) are safe.
- Struct instances are **reference types** (D-0023): heap-allocated via
  `zap_alloc`, shared on assignment/pass, compared by identity unless the type
  defines its own equality (D-0024).

#### 1.2 Methods — inline (resolves the OPEN-QUESTIONS item)

**Decision: methods are defined *inline*, inside the struct body, as `def`s
whose first parameter is `self`.**
This resolves the "inline vs separate receiver/`impl` block" open question in
favor of inline.

```python
struct Point:
    f64 x
    f64 y

    def norm(self) -> f64:
        return sqrt(self.x*self.x + self.y*self.y)

    def translated(self, f64 dx, f64 dy) -> Point:
        return Point(x=self.x + dx, y=self.y + dy)
```

- `self` is an explicit first parameter (Python-style), **untyped in the source**
  — its type is the enclosing struct.
  It is a normal binding; because structs are reference types, mutating
  `self.field` mutates the instance the caller holds.
- Method call syntax is `receiver.method(args)`.
- Methods share the module namespace of the struct; a method name beginning
  with `_` is private (D-0028).
- There is **no implicit `self`** and no method-outside-the-body form: every
  method of `Point` is written in `Point`'s block.
  The only block that attaches methods to a struct from outside is the optional
  `impl Interface for Struct` form (§2.3), which exists for interface
  conformance clarity, not as a second way to define ordinary methods.

Rationale for inline over a Go-style receiver block or a Rust-style `impl`
block is in §Rationale.

#### 1.3 Construction and field access

A struct is constructed by **calling its type name**.
Arguments follow the call convention of D-0038: positional arguments bind to
fields in declaration order, keyword arguments bind by field name, positional
precede keyword, and any field with a default may be omitted.

```python
auto p = Point(3.0, 4.0)            # positional, declaration order
auto q = Point(x=3.0, y=4.0)        # keyword (recommended; order-independent)
auto c = Config(host="localhost")   # port, tls take their defaults

print(p.x)                          # field read
q.y = 1.0                           # field write (structs are mutable, D-0006)
print(q.norm())                     # method call
```

- **Keyword construction is the recommended form**; it is resilient to field
  reordering and documents call sites.
  Positional construction is permitted for small structs but makes field order
  an API surface, so a canonical lint may discourage it beyond a few fields.
- Every field without a default must be supplied, or it is a compile error —
  there is no partially-initialized struct and no `nil`-struct (an unset
  reference-typed field would be `None`, D-0014, only if explicitly defaulted
  to `None`).
- A struct does **not** get a user-written constructor method; construction is
  the generated field-wise initializer.
  Validation and derived construction logic are expressed with field decorators
  (`@default`, `@validate`) and `@derive` (D-0036), not a hand-written
  `__init__`.

#### 1.4 Composition (no inheritance)

Reuse is by **composition**: a struct embeds other structs as fields and
forwards or wraps their behavior explicitly.

```python
struct Engine:
    int64 power
    def start(self): ...

struct Car:
    Engine engine
    str    name

    def start(self):          # explicit forwarding — no implicit promotion
        self.engine.start()
```

There is no field/method promotion (unlike Go embedding) and no base type:
forwarding is always written out.
This keeps the method set of a struct syntactically visible in its body, which
matters for the compile-time interface check (§2).
Cross-cutting behavior that inheritance would provide elsewhere (equality,
ordering, serialization, logging) comes from `@derive`/decorators (D-0036).

#### 1.5 Value structs (opt-in value types, D-0051)

By default a `struct` is a **reference type** (D-0023): shared identity, mutation
visible to the caller — the Python-object feel. For hot, small aggregates where
that heap indirection costs too much, prefix the declaration with **`value`**:

```python
value struct Vec2:
    f64 x
    f64 y
    def dot(self, Vec2 o) -> f64:
        return self.x * o.x + self.y * o.y
```

- A `value struct` is **copied** on assignment, argument passing, and return
  (value semantics, no shared identity), laid out **inline** (stack / registers /
  field-inline, no heap header) — the C/Swift-`struct` model.
- It uses the **same** inline methods (§1.2), structural interfaces (§2),
  operator overloading (§2.5) and `@derive` — and, like every Zap type, has **no
  inheritance**.
- **Limits:** stored in a collection or placed behind an interface value it is
  **boxed** (copied to the heap), so the benefit is on locals/fields/args/returns,
  not when erased behind an interface; a value struct holding references is still
  GC-scanned; mutating it does not escape the caller's copy.

Full OO (classes + inheritance) stays **deferred** (not under active
consideration); `value struct` closes the performance gap that motivated it
without reintroducing OO.

### 2. Interfaces (structural, compile-time checked)

#### 2.1 Declaration

An `interface` names a **method set** — method signatures only, no fields, no
bodies.

```python
interface Shape:
    def area(self) -> f64
    def name(self) -> str

interface Writer:
    def write(self, bytes data) -> int64
```

A signature is its **name, parameter types (excluding `self`), and return
type**.
Parameter names are not part of the match (they are a call-site convenience,
D-0038); arity, types, and return type are.

#### 2.2 Implicit, structural conformance — checked at compile time

A struct satisfies an interface **implicitly** iff its method set contains a
method matching every signature the interface requires (D-0016).
No declaration is needed to make conformance hold.
Conformance is **verified at compile time** at every point a struct value is
used where the interface type is expected — assignment to an interface-typed
binding, passing as an interface-typed argument, returning as an interface
type, or storing in an interface-typed field/collection:

```python
struct Circle:
    f64 r
    def area(self) -> f64:  return 3.14159 * self.r * self.r
    def name(self) -> str:  return "circle"

Shape s = Circle(r=2.0)     # OK: Circle structurally satisfies Shape
print(s.area())

struct Square:
    f64 side
    def area(self) -> f64:  return self.side * self.side
    # no name() method

Shape bad = Square(side=1.0)   # COMPILE ERROR: Square is missing Shape.name()
```

- A **missing or misspelled** method (e.g. `aera`) is a **compile error** at the
  use site, not a runtime `NotImplemented` — this is the whole point of D-0016
  (typo-class bugs caught at build time).
- The error names the interface, the struct, and each missing/mismatched
  signature.
- Conformance is checked **where used**, so a struct never needs to anticipate
  which interfaces it will satisfy; structural matching makes conformance
  retroactive and decoupled.

#### 2.3 Optional explicit `impl Interface for Struct`

For clearer error locations and documented intent, a struct may **declare**
conformance explicitly:

```python
impl Shape for Square:        # asserts & checks conformance at THIS site
    def name(self) -> str:    # may supply the missing/interface-specific method
        return "square"
```

- `impl Interface for Struct:` asserts that `Struct` satisfies `Interface` and
  is checked **at the declaration**, so the error points at the `impl` rather
  than at a far-away use site.
- The block **may** contain method definitions; methods defined here are added
  to the struct's method set exactly as if written inline (it is the one
  outside-the-body method form, §1.2).
  This lets conformance-specific methods live next to the `impl` while ordinary
  methods stay in the struct body.
- It is **optional and additive**: a struct that already satisfies an interface
  inline needs no `impl`; adding one later changes nothing but error locations.
  Omitting it never weakens checking (use sites are still checked).

#### 2.4 Interface values and dispatch (coordinate with RUNTIME-ABI)

An interface-typed binding holds an **interface value**: a two-word fat pointer

```
{ data: *object, itable: *ITable }
```

- `data` is the underlying struct instance (a reference type, one word per
  RUNTIME-ABI).
- `itable` is a compile-time-generated **interface dispatch table** for the
  concrete (struct, interface) pair: an array of function pointers, one per
  interface method, in interface declaration order.
  Calling `s.area()` loads slot *k* from `itable` and calls it with `data` as
  `self`.
- The fat pointer is the **one reference type wider than a single word**; the
  RUNTIME-ABI header/type-descriptor word on the object is unchanged.
  The itable is built once per (struct, interface) pair at the conformance site
  and is immutable/static.
- `None` as an interface value is `{ data = null, itable = null }`; calling
  through it traps to an exception (D-0014/D-0007), consistent with member
  access on `None`.

This mirrors Go's interface representation and keeps dispatch to a single
indirect call with no per-object cost beyond the shared header.
Dispatch could alternatively route through the object header's type descriptor
(RUNTIME-ABI §"Heap object header"), which already carries the full method
table; the fat-pointer itable is preferred because it makes the method-slot
layout per-interface and resolves it at compile time (see §Rationale and
§Open issues).

#### 2.5 Operator overloading (D-0047)

A user type overloads an operator by defining the matching **dunder method**,
which the compiler recognizes through a fixed set of **structural operator
interfaces** (so conformance is compile-time-checked exactly like any interface,
§2, D-0016). `a + b` desugars to `a.__add__(b)` and type-checks iff `a`'s type
satisfies `Add`:

```python
interface Add:
    def __add__(self, other: Self) -> Self
```

**The full dunder set** — all operators are overloadable:

| `a op b` / form | dunder |
|---|---|
| `+ - * / // % **` | `__add__ __sub__ __mul__ __truediv__ __floordiv__ __mod__ __pow__` |
| `& \| ^ << >>` | `__and__ __or__ __xor__ __lshift__ __rshift__` |
| `-a +a ~a` | `__neg__ __pos__ __invert__` |
| `== !=` | `__eq__` (`!=` derived) |
| `< <= > >=` | `__cmp__(self, other) -> int` (all four derived) |
| `a[i]` / `a[i]=v` / `del a[i]` | `__getitem__ __setitem__ __delitem__` |
| `x in a` / `len(a)` / `if a:` | `__contains__ __len__ __bool__` |
| `a(args)` | `__call__` |
| `str(a)` / `print` | `__str__ __repr__` |
| dict/set key | `__hash__` |

(`__iter__` / `for x in a` is the iteration protocol, a separate decision.)

- **Single-method ordering.** Only `__cmp__` (returning −/0/+) is written; the
  four relational operators derive from it, and `!=` derives from `__eq__`.
- **`@derive` is the common case (D-0036).** `@derive(Eq, Ord, Hash, Repr)`
  generates these dunders field-by-field, zero hand-written operators:

  ```python
  @derive(Eq, Ord, Hash, Repr)
  struct Version:
      int64 major
      int64 minor
      int64 patch
  # => v1 < v2, v1 == v2, {v1: ...}, print(v1) all work; lexicographic by field.
  ```

- **Hash / struct keys.** `__hash__` (derived or hand-written) makes a struct a
  `dict`/`set` key (unblocks D-0043's deferred struct-element hashing). Invariant
  `a == b ⟹ hash(a) == hash(b)`: `@derive(Hash)` guarantees it; a hand-written
  `__hash__` (e.g. hashing only a stable `id`) must preserve it. The compiler
  warns on `Eq`-without-`Hash` used as a key.
- **Augmented assignment** (`+=`, …) derives from the binary dunder
  (`a = a.__add__(b)`). The **in-place** family (`__iadd__`, …) is **reserved**
  but deferred to a later ticket — a pure allocation optimization gated on
  aliasing rules (D-0023).
- **Operator interfaces double as generic bounds** (D-0017): once user generics
  land, `def sum[T: Add](xs: list[T]) -> T` constrains `T` to addable types.

#### 2.6 Iteration protocol (D-0048)

`for`, comprehensions, generators, and built-in/channel iteration all run through
one protocol, expressed as two compiler-known structural interfaces (§2, D-0016):

```python
interface Iterable:
    def __iter__(self) -> Iterator        # produce a fresh iterator
interface Iterator:
    def __iter__(self) -> Iterator        # an iterator is iterable (returns self)
    def __next__(self) -> (T, bool)       # comma-ok: (value, has_more)
```

`for x in obj` desugars to:

```python
auto __it = obj.__iter__()
while True:
    auto v, ok = __it.__next__()
    if not ok: break
    auto x = v
    # ... body ...
```

- **Native `__next__` is comma-ok** `-> (T, bool)`: exception-free, symmetric
  with the channel comma-ok receive (D-0031); the compiler infers element type
  `T` from the signature (no dependency on user generics).
- **Python-compat (support both):** a `__next__(self) -> T` that raises
  `StopIteration` is also accepted, so ported Python iterators run unchanged. The
  compiler picks the desugaring from `__next__`'s return type (`(T, bool)` →
  comma-ok; `T` → catch `StopIteration`). `StopIteration` fires once at
  exhaustion, not per element, so with zero-cost EH (D-0007) its cost is a single
  throw per loop; comma-ok is the preferred form for new code.
- **Generators** (`yield`, D-0026) auto-implement `Iterator` (compiler-generated
  state machine), ending by fall-off/`return`; they compile to comma-ok.
- **Built-ins use the same protocol:** `list`/`dict`/`set`/`array`, `str`
  (yields `char`), and channels (`for x in ch` until closed). Enums keep their
  compiler-generated iteration. Iterators are single-pass reference objects
  (D-0023); `__iter__` yields a fresh one.

```python
struct Range:
    int64 lo
    int64 hi
    def __iter__(self) -> RangeIter: return RangeIter(cur=self.lo, hi=self.hi)

struct RangeIter:
    int64 cur
    int64 hi
    def __iter__(self) -> RangeIter: return self
    def __next__(self) -> (int64, bool):
        if self.cur >= self.hi: return (0, False)
        auto v = self.cur
        self.cur += 1
        return (v, True)

for x in Range(lo=0, hi=5):   # 0 1 2 3 4
    print(x)
```

### 3. Enums

#### 3.1 Tier 1 — named constants (D-0018)

An `enum` is a **distinct type backed by a signed integer** with named members,
optional explicit values, a `.name` accessor, and iteration.

```python
enum Color:
    RED                # 0
    GREEN = 10
    BLUE               # 11  (continues from the previous value)

auto c = Color.GREEN
print(c.name)          # "GREEN"
print(int64(c))        # 10  (explicit narrowing conversion to its backing int)

for col in Color:      # iterable in declaration order
    print(col.name)
```

- The backing type is `int64` by default (D-0005); members without an explicit
  value take previous + 1, starting at 0.
- An enum is its **own type**, not interchangeable with `int64`: assigning an
  `int64` to a `Color` or vice versa requires an explicit conversion
  (`Color(x)` / `int64(c)`), matching D-0020's "explicit narrowing" ethos and
  preventing a raw int from masquerading as an enum.
- `Color.RED` names a member; `.name` yields its `str` identifier.
- **Representation (coordinate with RUNTIME-ABI):** a tier-1 enum value is an
  **unboxed scalar** (its backing integer), passed by value like any scalar
  (D-0023).
  The member-name table and the member list for iteration live in the enum's
  **type descriptor** (static data), not in each value, so `.name` is a lookup,
  not stored per value.

#### 3.2 `match` on enums — exhaustiveness today

`match` is locked (D-0040) and already exists; on an enum scrutinee the compiler
performs **exhaustiveness checking** — a `match` that omits a member and has no
`_` arm is a compile error:

```python
match c:
    case Color.RED:    handle_red()
    case Color.GREEN:  handle_green()
    case Color.BLUE:   handle_blue()
    # exhaustive: all members covered, no `_` needed
```

Qualified names (`Color.RED`) match that constant; a bare name would
capture-and-bind (D-0040), so enum members are always matched qualified.

#### 3.3 Tier 3 — data-carrying variants (locked, D-0046)

Rust-style sum types are **locked** (D-0046, resolving the tier-3 half of
D-0018; issue #51). A variant may carry **type-first, named payload fields**;
payload-free variants are allowed, and an enum may be generic:

```python
enum Shape:
    Circle(f64 r)
    Rect(f64 w, f64 h)
    Unit                       # payload-free variant, as in tier 1

auto sh = Shape.Circle(r=2.0)  # qualified construction; named OR positional:
auto s2 = Shape.Rect(3.0, 4.0) #   Shape.Rect(w=3.0, h=4.0) is equivalent

match sh:
    case Shape.Circle(r):      area = 3.14159 * r * r   # positional binding
    case Shape.Rect(w=w, h=h): area = w * h             # named binding
    case Shape.Unit:           area = 1.0
    # exhaustive: all variants covered, no `_` needed
```

**Representation — value-first (performance), pointer only where needed.**
A data-carrying enum is a **tagged value** `[ tag: i32 | payload-union ]` sized
to the largest variant and copied **by value** like a scalar (D-0023) — it does
**not** heap-box by default. The compiler inserts a **GC pointer only where
needed**: a field whose type is (mutually) **recursive** — to break the
otherwise-infinite size, e.g. an AST `Add(Expr l, Expr r)` whose children are
pointers to heap `Expr` values — or whose payload is oversized. An
all-bare-constant enum stays the **tier-1 unboxed integer** (§3.1). The two
representations do **not mix**: any payload-carrying variant makes the whole
enum a tagged value, so the type descriptor reserves a `tag` slot from the
start. (A niche/unboxed optimization beyond this is a later follow-up.)

**Construction** is **qualified** (`Shape.Circle(...)`, like `Color.RED`) and
accepts **named or positional** arguments, exactly like struct construction.

**Pattern matching** reuses the struct-pattern form already specified in D-0040,
so **no new `match` syntax is needed**: a qualified variant with fields bound
**positionally** (`Shape.Circle(r)`, declaration order) or **by name**
(`Shape.Circle(r=r)`), developer's choice; exhaustiveness extends to variant
coverage.

**Generics** — generic enums (`enum Option[T]: Some(T v); None`) are **reserved
in the grammar now** and implemented by monomorphization **when user generics
land** (D-0017 staging; §5); non-generic data-carrying enums ship first. Stdlib
`Option[T]`/`Result[T, E]` ride on that.

**Methods** on an enum (a `def` with `self` in the block, like structs §2.2,
D-0015) are **allowed** — dispatching via `match` internally — and may be staged
after the core destructuring.

#### 3.4 As built (ticket #51)

The first implementation lands the whole vertical — front-end (parse of
data-carrying variants and qualified/positional/named patterns), sema (distinct
int-backed tier-1 typing, variant construction, payload-field binding, and
**`match` exhaustiveness**), ZIR lowering, and codegen through LLVM + the Boehm
runtime, with tier-1, tier-3, and a recursive-`Expr` golden. Representation
choices and scoped deferrals, all consistent with D-0046:

- **Tier-1** is lowered as its unboxed `i64` backing value; member access is the
  discriminant constant and `match` is an equality chain.
- **Tier-3** is lowered as a **GC-allocated tagged struct** `{ tag:i32,
  payload… }`, reusing the struct machinery (`StructNew`/`FieldGet`/`FieldSet`).
  A recursive variant field (type = the enum itself) is therefore already a
  pointer to a heap enum value, which breaks the cycle for free — the motivating
  AST case works with no special node. The payload union is modelled as a
  **flattened record** (each variant's fields occupy their own slots) rather than
  an overlapping union sized to the largest variant; the space-optimal overlap,
  and the fully unboxed by-value representation, are a later follow-up
  (semantically unobservable today because enum values are immutable, cf. D-0023
  for `str`).
- **Deferred to follow-ups** (typed by sema, not yet lowered): `.name`, `for x in
  Enum` iteration (both need a ZIR-level enum identity that the `i64` collapse
  drops), enum methods, and generic enums (ride on user generics, D-0017).
  Nested sub-patterns inside a variant field (`case Shape.Circle(0.0)`) are not
  yet lowered — bind to a name or `_`.

### 4. Tuples (locked, D-0049)

Tuples are a **fixed-arity, heterogeneous, immutable** aggregate (D-0019).
This section fixes their literal, access, destructuring, and multi-return
syntax — locked as **D-0049** (issue #54).

```python
auto t = (1, "two", 3.0)           # tuple literal: (int64, str, f64)
auto pair = (x, y)                 # 2-tuple
auto single = (42,)                # 1-tuple needs the trailing comma
# ()  is the empty tuple (unit); rarely needed

print(t[0])                        # element access by CONSTANT index -> 1
# t[i] with a runtime i is a COMPILE ERROR: the element type must be static
```

**Element access** is `t[k]` where `k` is a **compile-time constant**; the
result type is that element's static type.
A non-constant index is rejected at compile time, because a tuple is
heterogeneous and the result type must be known statically (this is the key
difference from `list`, whose `[i]` takes a runtime index).

**Destructuring** binds positionally and is a declaration (D-0003 extended to
tuple targets):

```python
auto (a, b, c) = t                 # a: int64, b: str, c: f64
auto (first, *rest) = (1, 2, 3, 4) # first: int64, rest: tuple(int64, int64, int64)
(x, y) = (y, x)                    # reassignment form — swap, no `auto`
```

**Multiple return** is a tuple return; the caller may destructure:

```python
def divmod(int64 a, int64 b) -> (int64, int64):
    return (a // b, a % b)

auto (q, r) = divmod(17, 5)        # q=3, r=2
auto both   = divmod(17, 5)        # both: (int64, int64)
```

- A function's multi-value return **is** a tuple return — there is no separate
  multiple-return mechanism (unlike Go).
  The comma-ok channel receive `v, ok = <-ch` (D-0031) is this same tuple
  destructuring.
- Tuples are **immutable** (no element assignment `t[0] = ...`) and compared
  **element-wise** by `==` when every element type supports `==` (D-0024).
- **Representation (coordinate with RUNTIME-ABI):** a tuple is a **by-value
  aggregate**, laid out like an anonymous struct's fields (declaration/positional
  order, C alignment), returned in registers when small or via an `sret` pointer
  when large — exactly the "tuples / multiple returns" rule already in
  RUNTIME-ABI §"Value representation" and its open issue on the sret threshold.
  A tuple is **not** a reference type and has **no heap header** unless boxed
  into a reference-typed slot (e.g. stored in a `list`).

### 5. Generics (monomorphization)

#### 5.1 Syntax

Type parameters are a bracketed list `[T, U, ...]` after the name, on functions
and structs (and, later, enums and interfaces).
The grammar already reserves this (D-0017), matching built-in `list[T]` /
`dict[K, V]`.

```python
def map[T, U](list[T] xs, fn(T) -> U f) -> list[U]:
    auto out = list[U]()
    for x in xs:
        out.append(f(x))
    return out

struct Box[T]:
    T value
    def get(self) -> T:
        return self.value

struct Pair[K, V]:
    K key
    V value
```

- Type parameters are used wherever a type is written: field types, parameter
  types, return types, local declarations, and nested generic instantiations
  (`list[T]`).
- Instantiation is by supplying type arguments, usually **inferred** at the call
  / construction site, or written explicitly when inference cannot determine
  them:

```python
auto ys = map([1, 2, 3], lambda x: x * 2)   # T=int64, U=int64 inferred
auto b  = Box[str]("hi")                     # explicit type argument
auto b2 = Box(value=42)                      # T=int64 inferred from the field
```

#### 5.2 Constraints via interfaces

A type parameter may be **constrained by an interface** (D-0016); the body may
then use only that interface's methods, and instantiation checks conformance:

```python
interface Ordered:
    def less(self, other) -> bool

def max_of[T: Ordered](list[T] xs) -> T:
    auto best = xs[0]
    for x in xs[1:]:
        if best.less(x):
            best = x
    return best
```

- `T: Ordered` means "any type that structurally satisfies `Ordered`"; the
  structural, compile-time check (§2.2) applies at each instantiation.
- Unconstrained `T` admits only operations valid for *all* types (assignment,
  passing, equality via the type's own `==` if present) — the body cannot call
  methods not guaranteed by a constraint, and violating this is a compile error
  at the definition, not at instantiation.
  This keeps generic code checkable once, independent of instantiations.

#### 5.3 Specialization (how monomorphization works)

Zap compiles generics by **monomorphization** (D-0017): the compiler generates
one concrete copy of a generic per distinct tuple of type arguments actually
used, then compiles those copies as ordinary non-generic code.

1. **Collection.** Whole-program AOT (D-0021) means every instantiation is
   visible; the compiler walks the call/construction graph and collects the set
   of type-argument tuples each generic is used with
   (`map[int64, int64]`, `Box[str]`, `Pair[str, int64]`, …).
2. **Specialization.** For each tuple it emits a specialized copy with the type
   parameters substituted — real field layouts, real sizes, direct (non-boxed)
   calls, scalar `T` unboxed.
   `Box[int64]` and `Box[str]` are distinct concrete structs with distinct
   layouts.
3. **Name mangling (coordinate with RUNTIME-ABI).** Each specialization gets a
   stable mangled symbol encoding the module path, the generic's name, and its
   type arguments — this is exactly the "name mangling … encodes … type
   arguments for monomorphized generics" clause in RUNTIME-ABI §"Name mangling"
   (and its open issue to finalize the scheme).
4. **Dedup.** Identical specializations across the program are emitted once.

- **No runtime generics / no type erasure:** there is no per-call type
  descriptor passed for `T`, no boxing of scalars, and no dictionary passing —
  specialization is the whole mechanism, which is what makes generics free at
  runtime and is why monomorphization was chosen (perf goal + LLVM, D-0017).
- **Built-ins first:** `list`/`dict`/`array` are generic today; user generics
  reuse the same instantiation machinery and are purely additive (D-0017), so
  landing them later breaks nothing.
- **Code-size trade-off:** monomorphization can duplicate code across many
  instantiations; this is the accepted cost (see §Rationale) and dedup plus the
  usual "built-ins cover most cases" keep it bounded in practice.

### 6. Interaction summary

- Generic structs compose and define methods exactly like non-generic structs;
  a method of `Box[T]` is specialized with its enclosing type.
- A generic type may satisfy an interface (its specializations are checked
  structurally per §2.2).
- Tuples, enums, and structs are all valid type arguments.
- Decorators/`@derive` (D-0036) run at compile time on structs — including
  generic ones (per specialization) and enums — and are the intended source of
  equality, ordering, and serialization, so those are deliberately **not**
  built into the core type system here.

## Rationale

**Inline methods over a receiver/`impl` block (resolves OPEN-QUESTIONS).**
Three forms were considered: inline `def self` methods (chosen), a Go-style
out-of-body receiver (`def (p Point) norm()`), and a Rust-style `impl Point:`
block grouping methods outside the struct body.
Inline wins for Zap because:
(1) it matches Python's class-body feel (D-0002), which is the primary
readability target;
(2) it keeps a struct's entire method set visible in one place, which the
compile-time structural interface check (D-0016) benefits from;
(3) it avoids introducing a receiver-declaration micro-syntax that fights the
type-first, `self`-first convention already used in SPEC.md §4.4.
The Rust-style grouping is still available in the **narrow** form of
`impl Interface for Struct` (§2.3) precisely where its benefit — explicit,
well-located conformance — is real, without making it the general method form.

**Keyword construction as the recommended form.**
With no user-written constructors (D-0036 pushes construction logic to
decorators), the generated field-wise initializer is the only constructor;
keyword arguments (D-0038) make it order-independent and self-documenting, and
keep field *order* a representation detail rather than an API surface.

**Fat-pointer interface values** match Go, give single-indirect-call dispatch,
and localize the method-slot layout to each interface at compile time, which is
the natural fit for structural + compile-time conformance (D-0016).
Routing dispatch through the object header's type descriptor was the
alternative (it needs no second word); it was not chosen because it would
require a global method-id space and a per-type method map lookup, trading the
fat pointer's one extra word for more complex runtime dispatch — deferred as an
optimization, not a semantic change (§Open issues).

**Enums as value scalars in tier 1** keep the common case (named constants)
free — no allocation, no header — while the reserved `tag` slot in the enum type
descriptor leaves a clean upgrade to tier-3 tagged unions (D-0018).
Making an enum a *distinct* type (not an int alias) upholds D-0020's explicit-
narrowing ethos and D-0019's "no type aliases".

**Tuples as by-value aggregates, multi-return = tuple return** reuses one
concept for three needs (grouping, destructuring, multiple results) and aligns
with the RUNTIME-ABI small-aggregate/`sret` return rule, avoiding a second
multiple-return mechanism (Go has both; Zap collapses them).
Compile-time-constant tuple indexing is required by heterogeneity and is the
clean line between `tuple` (static, heterogeneous) and `list` (dynamic,
homogeneous).

**Monomorphization over boxed/erased generics** is the performance-correct
choice given whole-program AOT (D-0021) and LLVM (D-0042): it yields unboxed
scalars, direct calls, and no runtime type dictionaries, at the known cost of
code duplication — the same trade-off Rust and C++ accept and the one D-0017
already locked.

These choices also uphold the project's compile-time-safety ethos (D-0016):
interface typos, missing enum arms (D-0040), out-of-range tuple access, and
unsatisfied generic constraints are all **compile errors**.

## Compatibility

- **New surface syntax**, additive to a language with no prior user types; no
  existing `.zap` programs break.
- `impl`, and the type keywords already reserved in SPEC.md Appendix A
  (`struct`, `interface`, `enum`), are used as specified; `impl` must be added
  to the reserved-word list when this lands.
- **Generics are purely additive** (D-0017): built-in `list`/`dict`/`array`
  already use the bracket syntax, so adding user generics introduces no new
  grammar and no breaking change.
- **RUNTIME-ABI:** this design is consistent with the current
  `design/RUNTIME-ABI.md` and requires **no ABI change** — it fills in two of
  its open issues (tuple/multi-return representation, and the name-mangling
  scheme for monomorphized generics) with concrete direction, to be ratified in
  that document when the runtime tickets implement them.
  The only item that will *extend* the ABI later is tier-3 enums (a tagged-union
  heap layout), which the reserved `tag` slot anticipates.

## Implementation

Maps onto the roadmap in dependency order:

1. **T8 — structs & methods (first).** Parse `struct` bodies (fields +
   inline `def self` methods), lay out per RUNTIME-ABI §"Struct layout", emit
   the field-wise constructor and `self`-first method functions, field
   access/assignment, and composition/forwarding.
   This is the minimum for user types and unblocks most downstream work.
2. **Interfaces.** Interface declarations; the **structural conformance check**
   at use sites (the core D-0016 deliverable); fat-pointer interface values +
   per-(struct, interface) itable generation and indirect dispatch; then the
   optional `impl Interface for Struct` form.
3. **Enums (tier 1).** Distinct int-backed type, `.name` table and iteration in
   the type descriptor, explicit conversions, and **`match` exhaustiveness**
   wired into the existing `match` (D-0040).
4. **Tuples.** Literal/destructuring/multi-return syntax; by-value aggregate
   lowering and the small-vs-`sret` return decision (shared with the
   RUNTIME-ABI open issue); compile-time-constant indexing and element-wise
   `==`.
5. **Generics (staged, later).** Instantiation collection over the whole-program
   graph, monomorphization + dedup, interface-constraint checking, and the
   mangling scheme (shared with RUNTIME-ABI name mangling).
   Built-in containers already exercise the layout; user generics reuse the
   machinery.

**Phase boundaries touched:** the **runtime ABI** (struct/enum/tuple layout,
interface fat pointer, generic name mangling) — all consistent with the frozen
contract; **no IR schema change** beyond representing the new type forms.

**Parallel-safe:** steps 1–4 are largely independent of each other once struct
layout exists; generics (5) depend on structs and interfaces but not on enums
or tuples.

## Open issues

- **Operator overloading for user types** (OPEN-QUESTIONS, D-0024): whether
  structs/enums define `+`, `==`, `<`, … via named methods (e.g. an `Ordered`/
  `Eq` interface and `@derive(Eq)`) or a dunder-style contract.
  This design assumes equality/ordering come from `@derive`/interfaces but does
  not fix the operator-to-method mapping.
- **Iteration protocol** — RESOLVED (D-0048, §2.6): `Iterable`/`Iterator`
  structural interfaces with comma-ok `__next__ -> (T, bool)` (plus a
  `StopIteration`-raising form for Python porting); generators and built-ins use
  the same protocol. Enums remain compiler-generated, not via this protocol.
- **Data-carrying enum variants / tier 3** (D-0018, OPEN-QUESTIONS): the tagged-
  union layout, payload sizing (inline vs boxed for large/recursive variants),
  and the generic `Option[T]`/`Result[T, E]` that motivate it.
  §3.3 fixes the intended direction but tier 3 is not accepted here.
- **Interface dispatch representation:** fat pointer (chosen) vs
  type-descriptor-routed dispatch; a later optimization decision, not a semantic
  one (§2.4, §Rationale).
- **Generic method / interface genericity:** generic *methods* on non-generic
  structs, and generic *interfaces* (`interface Container[T]`), are implied by
  the syntax but their inference and constraint rules need their own pass.
- **Monomorphization code-size controls:** whether to offer any
  opt-in sharing (e.g. a boxed fallback) for pathological instantiation counts;
  deferred — monomorphization-only is the v1 rule (D-0017).
- **Zero-value / uninitialized fields:** this design requires all non-defaulted
  fields at construction (no zero-value structs); confirm this against any
  future need for staged initialization.
- **Tuple `sret` threshold** and **generic name-mangling scheme** are shared
  open issues with `design/RUNTIME-ABI.md` and are finalized there when the
  runtime implements them.
