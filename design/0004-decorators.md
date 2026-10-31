# Proposal: Decorators & annotations

- **Author(s):** Zap team
- **Last updated:** 2026-10-17
- **Status:** Accepted (design) — v1 scope; user macros staged
- **Discussion:** (internal) — see `DECISIONS.md` D-0036

## Abstract

Zap provides a unified `@`-syntax metaprogramming feature in two kinds:
**annotations** (declarative metadata) and **decorators** (behavioral /
code-generating). Both apply to functions, methods, structs, fields, variables,
and parameters. They are **compile-time-first** — they generate real,
type-checked code with zero runtime cost — with optional **runtime reflection**
for dynamic frameworks. A curated built-in set ships in v1; user-defined
compile-time decorators (macros) are staged in. Generated code is always
inspectable (`zap expand`).

## Background

A core design value for Zap: **keep business logic free of boilerplate** — any
line that is not business logic is boilerplate to be eliminated. Two earlier
decisions make this feature load-bearing rather than optional:

- **No inheritance / composition-only (D-0015).** With no base classes, the
  mechanism for reusing cross-cutting behavior (equality, serialization,
  logging, validation) must be *derive/annotations*. Decorators are therefore
  Zap's primary cross-cutting-reuse mechanism, filling the role inheritance
  plays elsewhere.
- **Unchecked exceptions (D-0007)** were chosen to cut error-handling
  boilerplate. Decorators extend the same philosophy to everything else.

Python conflates metadata and behavior into one runtime mechanism and cannot
annotate variables/fields. Java separates annotations (metadata) from behavior
(AOP/processors). Zap takes the useful split but unifies the *syntax* and makes
the engine compile-time.

## Proposal

### Two kinds, one syntax

**Annotations** — declarative metadata. Declared like a struct; attaching one
constructs it and records it on the target. Pure data; read by compile-time
codegen or runtime reflection.

```python
annotation Column:
    str name = ""
    bool primary = False

@Table(name="users")
struct User:
    @Column(name="id", primary=True)
    int64 id
    @Column(name="email")
    str email
```

**Decorators** — behavioral / generative. A convenient `before`/`after` form for
the common case, and a general `wrap` for full control. Decorators may take
arguments.

```python
decorator logged:
    def before(ctx): print("calling", ctx.name)
    def after(ctx, result): print("->", result)

decorator retry(int64 times):
    def wrap(call):
        def inner(*args):
            for i in range(times):
                try:
                    return call(*args)
                except Error:
                    if i == times - 1: raise
        return inner

@retry(times=3)
def fetch(str url) -> str:
    ...
```

### Targets

Annotations and decorators may apply to **functions, methods, structs, fields,
variables, and parameters**. Field and variable targets are the key capability
Python lacks.

### Field decorators (the pydantic/prefill case)

Field decorators generate construction-time code — defaults (evaluated
**per instance**, consistent with per-call default evaluation, D-0038),
validation, and derive-driven serialization:

```python
@derive(Json)                          # generates to_json/from_json from fields
struct Signup:
    @default(now())                    # computed per instance
    DateTime created
    bool active = True                 # plain literal field default

    @validate(max_len=50)
    str name
    @validate(matches=m/^\S+@\S+$/)
    str email
    @range(min=0, max=150)
    int64 age
```

Constructing a `Signup` runs the generated validators; invalid input raises
(D-0007). This is pydantic-style ergonomics with the plumbing generated at
compile time — no runtime reflection cost.

### Variable-level

```python
@inject
Database db            # a DI/framework decorator fills it
```

### Compile-time-first + runtime reflection

- **Compile-time:** a decorator runs during compilation and may generate code
  (methods, validators, serializers) that is then type-checked like hand-written
  code. Zero runtime overhead. This is how `@derive`, `@validate`, `@default`,
  `@cache` work.
- **Runtime reflection:** annotations are *also* queryable at runtime, so
  dynamic frameworks (ORMs, DI containers, serializers chosen at runtime) can be
  written. Opt-in; pay only when used.

Two complementary string-keyed surfaces exist:

- **Type/function annotations** (D-0036, extended to functions by D-0087):
  `reflect_has(owner, name) -> bool` and `reflect_names(owner) -> str` report the
  annotation/decorator *names* carried by a struct/enum type or a function.
- **Signature reflection** (D-0089, I#458): the *shape* of a callable — its
  parameter names, declared types, defaults, and per-parameter annotations. The
  `owner` is a function name (top-level `def`) or `"Type.method"`:
  - `reflect_params(owner) -> str` — comma-joined parameter names, declaration
    order (the implicit `self` receiver is excluded).
  - `reflect_sig(owner) -> str` — a readable one-line signature, e.g.
    `(id: int, q: str = ...) -> str`; a defaulted parameter is marked `= ...` and
    an annotated one is `@anno`-prefixed.
  - `reflect_param_type(owner, param) -> str` — the rendered declared type (`""`
    if the parameter is untyped or not found).
  - `reflect_param_has_default(owner, param) -> bool`.
  - `reflect_param_annotations(owner, param) -> str` — comma-joined per-parameter
    annotation names.

  A *declared* annotation/decorator on a parameter (`def h(@Inject Svc dep)`) is
  recorded as reflectable metadata — the same model as the fn/type level — while
  behavioral or unknown names stay deferred. Opaque first-class function values
  and closures are not reflectable by value; reflect the named `def`. This is the
  substrate for typed per-parameter injection and `Depends(...)`-style DI (the
  fastzap web framework, zap#312 / fastzap#7).

### Composition & order

Multiple decorators stack; they apply **bottom-up** (the decorator nearest the
declaration runs first), matching Python. Order is significant and specified.

### Transparency (`zap expand`)

Because decorators generate code, the toolchain exposes **`zap expand`** to show
exactly what a decorator produced. Boilerplate leaves the source but stays
visible and debuggable — the antidote to "annotation magic" (the Spring
problem), and directly in service of the readability goal.

### Built-in set (v1)

Ship a curated set covering ~80% of boilerplate:
`@cache`, `@derive(Eq, Ord, Json, Repr, Hash)`, `@default(expr)`,
`@validate(...)`, `@range(...)`, `@deprecated`, `@inject`.

## Rationale

- **Unified `@`, two kinds:** keeps one familiar surface while preserving the
  metadata-vs-behavior distinction that keeps codebases legible.
- **Compile-time-first:** zero-cost boilerplate removal is the only version
  consistent with Zap's performance goal; it also fits the "catch at compile
  time" ethos (cf. compile-time interface checking, D-0016).
- **All targets:** field/variable annotations are exactly what Python lacks and
  what makes pydantic-style declarative structs possible.
- **Transparency:** inspectable generated code prevents decorators from
  undermining the readability they are meant to serve.

## Compatibility

Additive. Reserves `@`, `annotation`, and `decorator`. Built-in decorator names
live in the standard library namespace.

## Implementation

**v1 scope:** the `@` syntax; `annotation` declarations with compile-time +
runtime reflection; the built-in decorator set (compile-time codegen);
`before`/`after`/`wrap` behavioral decorators; `zap expand`.

**Staged (targeted for v1, hardest piece — may land as an immediate
fast-follow):** user-defined **compile-time decorators / macros** operating on
the AST, which requires stable AST + codegen hooks and a hygiene model.

Phase boundaries touched: the front-end (AST, a decorator-expansion pass before
type-checking) and a reflection-metadata section in the runtime ABI.

## Open issues

- The **user-macro API**: what a decorator sees (typed AST? a structured view?)
  and emits, plus **hygiene** (name capture) rules.
- The **runtime reflection API** surface (what metadata is queryable, and how).
- Interaction with **generics** (D-0017) — decorators on/inside generic types.
- Exact `before`/`after`/`wrap` context object (`ctx`) shape.
- Whether decorators may add fields/methods or only wrap/annotate existing ones.
