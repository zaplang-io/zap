# Compile-time reflection and `comptime` (epic #1248)

Zap programs can read a type's shape and generate code from it **at compile
time**, in ordinary Zap. This is the machinery behind `@derive(Json)` and the
`serde` codecs: a library — not the compiler — walks a struct's fields or an
enum's variants during monomorphization and emits the serialize/deserialize
code for each concrete instantiation. There is no runtime reflection, no boxing,
and no interpreter in the shipped binary; everything described here is evaluated
host-side by the compiler and folds away before codegen.

This chapter is the reader-facing normative reference for that surface. It is
backed by the ADR [`decisions/lang/1618-comptime-reference.md`](../decisions/lang/1618-comptime-reference.md)
and the grammar in [`docs/GRAMMAR.ebnf`](GRAMMAR.ebnf); the program epic is
#1248, phased E1 (reflection records) → E2 (the comptime evaluator) → E3
(`@derivable`/`@derive`) → E4–E6 (dogfooding: the Json codec and `serde` are now
built on it). Its stability tier is **Experimental** (see
[`docs/STABILITY.md`](STABILITY.md)).

> **What it guarantees, in one paragraph.** `reflect.type_info(T)` yields a
> read-only description of a concrete type. `comptime for` unrolls a loop over
> that description once per field/variant at compile time, cloning its body for
> each. Inside the unrolled body, `reflect.field` / `reflect.set_field` /
> `reflect.construct` read and build values by field index, and a `comptime
> if`/`elif` chain on `.kind` selects a branch per type. A function marked
> `@derivable(Trait)` becomes the generator a type opts into with
> `@derive(Trait)`. All of it runs in a **pure, deterministic, sandboxed**
> subset of Zap — no I/O, FFI, concurrency, recursion, or nondeterminism — and
> any attempt to leave that subset is a clean compile error, never a crash.

---

## 1. `reflect.type_info(T)` — the type description

`reflect.type_info(T)` takes a **type** as its single positional argument (a
named type, a generic parameter in scope, or a container type such as
`list[int64]`) and returns a read-only `TypeInfo` record. Inside a generic
`@derivable` generator the parameter `T` resolves to the *concrete* type of each
instantiation, so `reflect.type_info(T)` is what makes the generator
type-directed.

`TypeInfo` exposes these accessors:

| Accessor | Yields | Notes |
|---|---|---|
| `.kind` | a `TypeKind` | the structural kind — see §2 |
| `.name` | `str` | the type's own name for a *named* kind (`struct`/`enum`/`primitive`); the **empty string `""`** for an unnamed kind (`list`/`dict`/`optional`/`tuple`) |
| `.is_named` | `bool` | `True` for a named kind — distinguishes "no name" from an empty name |
| `.fields` | `list[FieldInfo]` | the struct's fields in declaration order (empty for a non-struct) |
| `.variants` | `list[VariantInfo]` | the enum's variants in declaration order (empty for a non-enum) |
| `.elem` | `TypeInfo` | the element type of a `list` |
| `.key` / `.value` | `TypeInfo` | the key / value type of a `dict` |
| `.inner` | `TypeInfo` | the element type of an `Optional` |
| `.elems` | `list[TypeInfo]` | the element types of a `tuple` |
| `.span` | a source span | the use-site declaration span, for `comptime.error` (only present when the type came from a declared field/element; a top-level `type_info(T)` has none) |

`.name` is deliberately **total** — it returns `""` rather than failing for an
unnamed kind — so a dispatch like `f.ty.name == "int64"` folds to a comptime
boolean for *every* field kind. Use `.is_named` when you must tell an unnamed
kind apart from a (non-existent) empty name, and `.kind.name` to switch on the
structural kind.

The container accessors (`.elem`/`.key`/`.value`/`.inner`/`.elems`) each return
a nested `TypeInfo`, so a kind-switch can **recurse** into `list[list[T]]`,
`dict[str, Widget]`, `Optional[Gadget]`, and so on (see the recursive element
codec in §8).

### Field, variant, and attribute records

**`FieldInfo`** (one struct field): `.name` (`str`), `.index` (`int64`, its
declaration position), `.ty` — equivalently `.type_info` — (the field's own
`TypeInfo`, so a kind-switch can recurse through it), `.attrs`
(`list[AttrInfo]`), and `.span`.

**`VariantInfo`** (one enum variant): `.name` (`str`), `.index` (`int64`),
`.fields` (`list[FieldInfo]` — the variant's payload fields), `.attrs`, and
`.span`.

**`AttrInfo`** (one field/type attribute, e.g. `@json(timestamp="iso8601")`):
`.name` (`str`), `.args` (`list[AttrArg]`), and `.span`.

**`AttrArg`** (one attribute argument): `.value` (`str` — the argument rendered
as a canonical string: a literal without quotes, a numeric lexeme, `true`/
`false`, or a bare name), `.is_named` (`bool`), and `.name` (`str`; reading
`.name` on a positional argument is a comptime error — guard it with
`.is_named`).

---

## 2. The kind-switch

A `TypeKind` has one accessor, `.name`, a lowercase `str` drawn from the fixed
set:

```
"struct"   "enum"   "tuple"   "primitive"   "list"   "dict"   "optional"
```

There is **no `match` over comptime values** (that is outside the sandbox — §6).
Type dispatch is instead a **`comptime if`/`elif` chain** on `.kind.name` (or on
`.name` for a specific primitive). At specialization the compiler evaluates each
condition for the concrete type, keeps only the branch that is comptime-true,
and drops the rest — so the chain costs nothing at run time:

```python
comptime for f in reflect.type_info(T).fields:
    if f.ty.kind.name == "optional":
        ...                       # only this branch survives for an Optional field
    elif f.ty.kind.name == "list":
        ...
    else:
        ...
```

The `TypeKind` names are also seeded as comptime constants (`Struct`, `Enum`,
`Tuple`, `Primitive`, `List`, `Dict`, `Optional`) in every comptime scope, so
`if info.kind == Struct:` is equivalent to `if info.kind.name == "struct":`.

---

## 3. `comptime for` and `comptime <expr>`

```ebnf
comptime_for_stmt ::= "comptime" "for" target "in" expr block ;
```

`comptime for x in <seq>:` unrolls at compile time: `<seq>` must be a
comptime sequence (typically `reflect.type_info(T).fields` or `.variants`), and
the loop **body is cloned and specialized once per element**, with `x` bound
(at comptime) to that element. It is not a runtime loop — there is no iterator
and no per-iteration branch in the emitted code. Nested `comptime for` and
comptime `if` chains inside the body are unrolled/pruned in place.

`comptime <expr>` (the prefix form, in `not_test`) forces an expression to be
evaluated at compile time; its most common use is `comptime f.index` to pass a
field index as a compile-time constant to `reflect.field` / `reflect.set_field`
(those require a constant index — §4).

### The comptime variant-match arm

Inside a `@derivable` generator, an enum can be walked with a single
**comptime-generated match arm** instead of a `comptime for`:

```ebnf
closed_pattern ::= ... | "comptime" IDENT "in" expr [ "as" IDENT ] ;
```

```python
match value:
    case comptime vv in reflect.type_info(T).variants as payload:
        if len(vv.fields) == 0:
            out = json.dumps(JsonValue.Str(vv.name))
        else:
            out = json.dumps(JsonValue.Str(reflect.field(payload, comptime 0)))
```

At monomorphization this one arm expands into one ordinary variant-pattern arm
per variant — the body shared, `vv` bound (comptime) to each variant, and the
optional `as payload` handle bound to that variant's payload so
`reflect.field(payload, comptime i)` resolves to its *i*-th payload field. The
arm is treated as covering every variant, so exhaustiveness is satisfied.

---

## 4. `reflect.field`, `reflect.set_field`, `reflect.construct`

These are the read/write/build primitives. Each requires a **comptime-constant**
field or variant index (normally `comptime f.index`); they are bounds-checked at
compile time and never produce a runtime out-of-bounds access.

- **`reflect.field(value, comptime i)`** — read. Desugars to the typed field
  access `value.<name_of_field_i>`, reusing the ordinary per-layout lowering
  (value-struct inline, heap-struct pointer+offset). It is read-only.

- **`reflect.set_field(target, comptime i, newvalue)`** — write (a
  *statement*). Desugars to `target.<name_of_field_i> = newvalue`.

- **`reflect.construct(T)`** — build a zero/empty value of `T`. For a `list` /
  `dict` / `set` it emits the typed empty constructor (so
  `auto acc = reflect.construct(E)` pins the element type); for an `Optional` or
  an enum, a zero variant (a decoder then overwrites fields via `set_field`);
  for a struct, the concrete constructor with each field zero-initialized
  (generic struct instances included).

- **`reflect.construct_variant(T, comptime i [, payload...])`** — build enum
  variant *i* of `T`, with any payload arguments. `T` must be a concrete (non-
  generic) enum; the index is a comptime constant and the payload arity is
  checked against the variant at compile time.

- **`reflect.has_override(info, "method")`** — `bool`, true when the type
  already defines `method` itself (so a generator can defer to a hand-written
  implementation).

---

## 5. `@derivable` and `@derive`

A **derivable** is a user-written code generator a type can opt into.

**Declaring one** takes two pieces: a structural `interface` listing the
method(s) the derive provides, and a generic free function marked
`@derivable(Interface)` that generates the body:

```python
interface __JsonDeriveEncode:
    def to_json(self) -> str

@derivable(__JsonDeriveEncode)
def __derive_json_to[T](T value) -> str:
    list[str] parts = []
    comptime for f in reflect.type_info(T).fields:
        parts.append("\"" + f.name + "\":" + __json_enc_elem[f.ty](reflect.field(value, comptime f.index)))
    return "{" + ",".join(parts) + "}"
```

**Applying it** is `@derive(Interface)` on a struct or enum:

```python
@derive(Json)
struct Point:
    int64 x
    int64 y

@derive(Json)
struct Player:
    str name
    int64 level
    float64 score
    bool active
    Point pos

def main():
    auto p = Point(x=3, y=-4)
    print(p.to_json())            # {"x":3,"y":-4}
```

(`@derive(Json)` is sugar for deriving both the encode and decode interfaces.)

**How synthesis works.** Before sema, each `@derive(Trait)` on a type emits one
thin **forwarding stub** per interface method — `to_json(self)` becomes
`return __derive_json_to[Self](self)` — and the derived trait is stripped from
the `@derive(...)` list. A hand-written method of the same name **wins** (the
stub is not emitted). The generator's `comptime for … reflect.type_info(T)` body
is then unrolled per concrete instantiation during monomorphization. For a
generic type, the structural bounds are inferred from which type parameters
appear in a field/variant payload, so `@derive` composes through generic fields.
A type that derives a trait whose generator can't handle one of its fields gets
a `ZAP5035` (missing interface) or a `ZAP7003` derive error with the full
instantiation backtrace (§7).

---

## 6. The pure, sandboxed subset

The comptime evaluator runs a **restricted, pure, deterministic** subset of Zap.
This is a hard soundness boundary, enforced two ways: a static reject-lint
(fail-closed, checking every branch including dead ones) and the evaluator
itself, which simply has **no impure primitive** — so reaching one is a clean,
span-attributed compile error ending with the exact phrase `is not available at
comptime`, never a panic, hang, or miscompile.

**In the subset.** The value model is `int64`, `bool`, `str`, `char`;
`list`/`tuple` of comptime values; and the read-only reflection records above.
Evaluable constructs: `let`/typed bindings (lexically scoped, shadowing),
`comptime if`, `return`, `pass`; field-access-by-index on the records; `int64`
arithmetic/comparison/bitwise/shift (with exact two's-complement wrap, Python
floor `//` and `%`, and the `INT64_MIN` edge cases, matching the runtime bit for
bit); short-circuit `and`/`or`; string operations (`+` concat, codepoint
indexing `s[i]` and slicing `s[a:b]`, `.upper()`/`.lower()`, `in`); list/tuple
index/slice/`in`; chained comparison; the ternary `a if c else b`; the builtins
`len`/`ord`/`chr`/`char`/`str`; and **non-recursive** calls to other comptime
functions.

**Out of the subset** (each a clean `not available at comptime` error): all
I/O, FFI, `spawn`/channels/`select`/`scope`, module-global mutation, time, RNG,
and `dict`/`set` *iteration* (unspecified, seed-dependent order, #1066);
`float`, bigint, and `bytes` **values**; f-strings, comprehensions and generator
expressions, lambdas/closures, regex; `match` over comptime values, `for` /
`while` / `do`-`while` runtime loops, `raise`/`assert`/`try`/`with`,
tuple-destructuring `let`, the walrus `:=`, true division `/`, `**`, `@`
matmul, `is`/`is not`; keyword and unpacked arguments in comptime calls; and
**recursion** — a direct or transitive self-call is rejected outright, so the
evaluator always terminates. (Heap allocation of strings and lists is *not* a
prohibited effect — those are fine.)

---

## 7. Diagnostics

A comptime failure is reported as a diagnostic with a `file:line:column` caret
and a `ZAP7xxx` code:

- **`ZAP7001`** — the evaluator left the sandbox (`not available at comptime`),
  or hit a genuine fault (division by zero, out-of-range index, a type
  mismatch), or a cost cap (§8).
- **`ZAP7002`** — a library generator aborted deliberately via
  `comptime.error(span, message)`.
- **`ZAP7003`** — "cannot derive `Trait` for `Type`": a derive failed during
  specialization. This diagnostic is anchored at the user's `@derive`/field
  site and carries an **instantiation backtrace** — a `required because of …`
  note per frame, from the `@derive(Trait) on T` the user wrote down through
  each field and container level (`the element type of list[Widget]`, `the
  key/value type of dict[…]`, element *N* of a tuple, the element of an
  Optional) to the leaf that actually failed — plus the root-cause note. A
  failing enum variant payload names its variant
  (`payload field `r` of variant `Circle` of `Shape``); a generator fault
  additionally dual-spans the generator line and the instantiation site.

  The chain also crosses **type boundaries**: when a nested *derived* type
  `Inner` fails and was pulled in by a field of an enclosing type `Outer` that
  also derives the trait, the frames climb outward through each enclosing field,
  giving the full `Outer → … → Inner` path (the header still names `Inner`, where
  the unsupported leaf is):

  ```
  error[ZAP7003]: cannot derive `Ser` for `Inner`
    note: required because of field `b` of `Inner`
    note: required because of field `inner` of `Mid`
    note: required because of field `mid` of `Outer`
    note: the generator reported: `Ser` is not implemented for `Bad`
  ```

  A cross-type frame crosses container levels and names an enum-outer's variant
  exactly like a within-type frame; a standalone derive failure (one not required
  by another deriving type's field) shows its within-type chain alone.

The backtrace is what turns "something deep in a nested generic codec failed"
into a readable path back to the type the user wrote.

---

## 8. The compile-time cost model

Comptime evaluation is bounded by three **fixed hard limits** (not tunable
quotas) so a pathological derive cannot blow up the build:

- a **step cap** on total expressions/statements evaluated across all loops
  (default 1 000 000) — the total-work bound;
- a **call-depth cap** on nested comptime calls (default 128; recursion is
  rejected outright, so this only bounds finite nesting);
- an **unroll-breadth cap** on the elements a single `comptime for` may unroll
  (default 100 000) — a per-loop bound that layers under the step cap. It is
  deliberately generous: a derive unrolls over a type's fields or variants (a
  handful), far under it.

Exceeding any cap is a clean `ZAP7001` budget diagnostic (carrying the §7
backtrace), not a crash.

Comptime cost is **observable**: the evaluator reports `steps_used()` and
`unrolled_iterations()`, and `reflect.type_info(T)` is **memoized** per canonical
type with `computed`/`reused` telemetry (`MemoStats`). Together these are a
stable, deterministic, comparable compile-time-cost metric — the CI
regression signal. Memoization is in-process only (no on-disk cache).

---

## 9. A worked example — a recursive element codec

The shipped Json codec is the canonical real use of the whole surface. Its
element codec is a single generic comptime function that dispatches on the
element kind and recurses into container element types named through
`reflect.type_info(E).elem` / `.value`:

```python
def __json_enc_elem[E](E x) -> str:
    if reflect.type_info(E).name == "int64":
        return json.dumps(JsonValue.Int(x))
    elif reflect.type_info(E).name == "str":
        return json.dumps(JsonValue.Str(x))
    elif reflect.type_info(E).kind.name == "list":
        list[str] parts = []
        for y in x:
            parts.append(__json_enc_elem[reflect.type_info(E).elem](y))   # recurse into the element type
        return "[" + ",".join(parts) + "]"
    elif reflect.type_info(E).kind.name == "dict":
        list[str] entries = []
        for k, v in x.items():
            entries.append(json.dumps(JsonValue.Str(k)) + ":" + __json_enc_elem[reflect.type_info(E).value](v))
        return "{" + ",".join(entries) + "}"
    else:
        return x.to_json()                                                # a nested derived struct/enum
```

The decode direction (not shown) builds the result with `reflect.construct(T)`
and fills it field by field with `reflect.set_field(out, comptime f.index, …)`,
constructs enum variants with `reflect.construct_variant`, reads field
attributes with nested `comptime for a in f.attrs` / `comptime for arg in
a.args`, and aborts on an unsupported field with `comptime.error(f.span, …)`.

The full source lives in `crates/syntax/src/json_derive.zap` (the prelude Json
codec) and `crates/driver/stdlib_zap/serde.zap` (the `serde` core).

---

## References

- Program epic #1248 — compile-time reflection and user-extensible `@derive`.
- [`docs/GRAMMAR.ebnf`](GRAMMAR.ebnf) — `comptime_for_stmt`, the `comptime
  <expr>` prefix, and the comptime variant-match arm.
- [`decisions/lang/1618-comptime-reference.md`](../decisions/lang/1618-comptime-reference.md)
  — the decision that this reference is normative and lives in the repo.
- [`docs/STABILITY.md`](STABILITY.md) — the surface's stability tier.
- `crates/syntax/src/json_derive.zap`, `crates/driver/stdlib_zap/serde.zap` —
  the shipped codecs that use this surface.
