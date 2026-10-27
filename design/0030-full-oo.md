# Proposal: Full OO — classes with inheritance (evaluation & verdict)

- **Author(s):** Zap language team
- **Last updated:** 2026-10-27
- **Status:** Accepted (design-only evaluation; verdict ratified as D-0077,
  2026-10-27)
- **Discussion:** issue #66 ([deferred] Full OO: classes with inheritance);
  re-examines the deferral against D-0015 (no classes/inheritance), D-0016
  (structural interfaces), D-0023/D-0051 (value vs reference), D-0050 (inline
  methods), and the just-frozen 1.0 surface (D-0076,
  `design/0029-spec-freeze.md`).

> Formatting convention (from Go): wrap text near the 80-column mark and start
> each sentence on a new line. This keeps review comments precise and diffs
> small.

## Abstract

Issue #66 tracks "full OO" — classes with implementation inheritance — as a
**deferred** idea, to be revisited *only* if composition + structural
interfaces + value structs prove insufficient in real use, and only after
weighing the vtable/dynamic-dispatch cost and the simplicity/anti-OO stance
that motivated the current design (D-0015).

This document is that revisit.
It is **design-only**: it writes no compiler, runtime, stdlib, or grammar code
and changes no `SPEC.md` / `GRAMMAR.ebnf` / `STABILITY.md`.
It surveys the real friction honestly, defines what "full OO" would actually
cost Zap, compares the alternatives short of classes, and ends in a verdict.

**Recommendation up front: decline full classes with implementation
inheritance, and keep #66 deferred.**
The evaluation finds exactly one genuine capability gap in the current model —
**sharing a method body / default behavior across the types that satisfy an
interface** — and classes are a disproportionately large, simplicity-breaking
way to close it.
If and when logged real-use friction justifies closing that gap, the
smallest defensible increment is **interface default methods** (additive, MINOR,
dispatched through the *existing* interface machinery), optionally paired with
**explicit delegation sugar** for composition — not classes, not
implementation inheritance, not a second nominal dispatch mechanism.
This records **D-0077 (Accepted; ratified by the maintainer 2026-10-27)** as
that verdict.

## Background

### What exists today

Zap deliberately has **no classes and no implementation inheritance**
(D-0015, locked).
The user-defined-type story is:

- **`struct`** — a reference type (D-0023): shared identity, heap-allocated,
  Python-object-like mutation, the default.
- **`value struct`** — an opt-in value type (D-0051): copied on assign/pass,
  laid out inline, no heap header, for hot small aggregates.
  Neither kind has inheritance.
- **Methods** are inline `def`s inside the type block with an explicit `self`
  (D-0050); there are **no extension methods** — you cannot add a method to a
  type you do not own.
- **Interfaces** are **structural** and satisfied **implicitly**, but
  conformance is **checked at compile time** (D-0016): using a type where an
  interface is required with a missing/misspelled method is a *compile error*.
- **Reuse** is by **composition** (embed a struct as a field, forward to it),
  not inheritance.

Crucially, Zap **already has both dispatch stories a class language needs**,
and they are already paid for:

- **Static dispatch** via generic *bounds* (D-0064, `design/0012-generics.md`):
  `def f[T: Shape](x: T)` monomorphizes and emits a **direct** call — no fat
  pointer, no indirection (Rust's `impl Trait`).
- **Dynamic dispatch** via *interface values* (I#84,
  `design/RUNTIME-ABI.md` §"Interface values"): an interface-typed binding is a
  two-word **`{data, itable}` fat pointer**; `s.m(args)` loads slot *k* from
  the itable and calls it (Rust's `dyn Trait`).
  The itable is a static, immutable per-`(struct, interface)` dispatch table
  built once at the conformance site, held *beside* the object, not in it.

So dynamic polymorphism — the headline reason languages add classes — is a
**solved, shipping feature** in Zap, done without putting a vtable pointer in
every object.

### The bar for revisiting, and the moment

The #66 body sets a deliberately high bar: revisit *only* if the current model
"proves insufficient in real use."
This document does **not** claim such insufficiency has been demonstrated; no
corpus of logged real-use friction exists yet (the language is pre-1.0-release).
That absence is itself load-bearing for the verdict.

The 1.0 surface was just frozen (D-0076): the struct/value-struct/interface/
enum core, operators, generics-with-bounds, and first-class fns are all
**Stable**.
Adding a `class` type kind is therefore a **post-1.0, SemVer-MAJOR** surface
change — the most expensive kind of addition the project can make — and #139
("full OO") already sits at the very *end* of the execution roadmap.
Nothing here should be read as scheduling it; the question is whether to keep it
deferred or to adopt a narrower increment.

## Problem: what would classes actually solve?

Six concrete frictions are commonly cited for "we need classes."
For each, below, is whether composition + structural interfaces + value structs
already cover it — with small sketches — and where the current model is honestly
clumsy.
Syntax in sketches is illustrative, within locked surface.

### 1. Shared implementation across types — the one real gap

The core OO win is writing a method body *once* and having many types use it.
Today Zap has no way to share a *method body* except a free function, and a free
function cannot participate in interface dispatch or be called as `x.method()`.

```zap
interface Shape:
    def area(self) -> float64

# Every shape must re-state describe(), even though the body is identical.
struct Circle:
    float64 r
    def area(self) -> float64:
        return 3.14159 * self.r * self.r
    def describe(self) -> str:
        return "a shape of area " + str(self.area())   # <-- duplicated

struct Square:
    float64 side
    def area(self) -> float64:
        return self.side * self.side
    def describe(self) -> str:
        return "a shape of area " + str(self.area())   # <-- duplicated again
```

You can factor the body into a free function `describe_shape(s: Shape) -> str`,
but then the call site is `describe_shape(c)`, not `c.describe()`, and
`describe` is not part of the `Shape` contract.
**This is the genuine gap: there is no "default method" — no interface-level
body expressed in terms of the interface's own required methods.**
It is a *method-body-reuse* gap, not a polymorphism gap.

### 2. Field inheritance / shared layout — covered, sometimes clumsy

"Every subclass has these base fields" is done by composition:

```zap
struct Base:
    int64 id
    str name

struct Widget:
    Base base          # embed
    int64 z_order
    def name(self) -> str:
        return self.base.name    # explicit forwarding
```

This works and is explicit, but the forwarding (`self.base.name`) is manual
boilerplate for every exposed base member — the clumsiness is real but it is
*ergonomic*, not a missing capability.
Alternative 2 (delegation sugar) targets exactly this.

### 3. Polymorphic collections — already covered

A heterogeneous collection is a `list` of interface values:

```zap
list[Shape] shapes = [Circle(1.0), Square(2.0)]
for s in shapes:
    print(s.area())     # dynamic dispatch through each element's itable
```

This is the existing fat-pointer path (I#84); no class needed.
A value struct placed in such a list is boxed (documented in D-0051), which is
the same cost a class instance would pay.

### 4. Framework extension points / template method — mostly covered

"Framework calls your hook" is a structural interface the framework depends on:

```zap
interface Handler:
    def handle(self, req: Request) -> Response

def serve(h: Handler): ...     # framework code dispatches to h.handle
```

Providing a hook = providing a method; this is clean.
What is missing is only the *default* hook body an abstract base class would
supply — i.e. the same gap as (1), reached from the framework angle.

### 5. Python-interop expectation — ergonomic, not a capability gap

Python users reach for `class` reflexively, and the Python-interop surface
(`design/0003-python-interop.md`) makes that expectation visible at the
boundary.
But the friction here is *pedagogical and idiomatic* — "where do I put my
class?" — not a program a Zap user cannot write.
The answer "a struct with methods, plus an interface for polymorphism" is a
teachable one-to-one mapping for the overwhelmingly common single-level case;
Python code that leans on deep implementation-inheritance hierarchies is
exactly the code Zap's thesis argues against, so matching it is not a goal.

### 6. Constructors / initialization — covered

Factory functions plus `@derive` cover construction; there is no base-class
constructor to chain because there is no base class.
No gap.

### Verdict of the survey

Of the six, **five are already covered** (three cleanly, two with honest
ergonomic clumsiness that a *much* smaller feature than classes would fix), and
**exactly one is a true capability gap**: sharing a method body / supplying
default behavior at the interface level (items 1 and 4).
Dynamic polymorphism, heterogeneous collections, and construction are all
solved today.
This is the empirical core of the recommendation: the problem classes would
solve is narrow, and classes are a very wide solution.

## What "full OO" would mean for Zap

Even a tasteful class design forces a position on a dozen coupled axes.
Enumerating them shows how invasive it is.

- **New type kind.**
  `class` would be a third user-defined type beside `struct` and `value
  struct`, with its own declaration, layout, and dispatch rules — a permanent
  fork in "how do I define a type?" that every tutorial, the formatter, the LSP,
  and every reader must now carry.
- **Single vs multiple inheritance.**
  Single implementation inheritance + multiple interface conformance (the
  Java/C# shape) is the only defensible choice; multiple implementation
  inheritance drags in diamonds, C3 linearization, and an MRO users must reason
  about — squarely against the readability thesis.
- **`super` and method resolution.**
  Single inheritance still needs `super`, override rules, and a defined
  resolution order; each is new surface and new teaching load.
- **Field inheritance and layout.**
  Subclass-extends-superclass implies prefix layout and a notion of
  substitutable references, which interacts with the **GC** (Immix/Boehm must
  scan the combined layout) and **forecloses value semantics**: a `value class`
  cannot be both inherited-from and copied by value without slicing, so classes
  would be reference-only — a second reason they do not compose with D-0051.
- **abstract / virtual / final / override.**
  A real class system needs most of these modifiers to be safe and fast;
  each is a keyword and a rule.
- **Dispatch: a *second* mechanism.**
  A class with virtual methods wants an **object-embedded vtable pointer** (one
  per instance), which is a *different* dispatch representation from the
  existing **beside-the-object `{data, itable}` fat pointer** (I#84).
  Zap would then have *two* dynamic-dispatch stories to specify, optimize, and
  teach, plus rules for how a class value becomes an interface value (wrap its
  vtable dispatch in an itable?).
- **Nominal vs structural tension.**
  Interfaces are structural (D-0016); classes are nominal.
  The only way to keep the thesis is "a class still satisfies any structural
  interface whose methods it has" — which is defensible, but now the language
  has nominal *hierarchies* sitting inside a structural *conformance* model, and
  readers must hold both.

The honest summary: "full OO for Zap" is not one feature, it is a cluster of a
dozen decisions, most of which re-introduce precisely the complexity D-0015
removed, and two of which (value semantics, dispatch representation) actively
conflict with shipped, frozen design.

## Cost

Rough magnitude per area (S / M / L / XL).

| Area | Magnitude | Why |
|------|-----------|-----|
| Runtime: object header + per-instance vtable | **M–L** | New object shape; GC must scan class layouts; coexists with the itable path. |
| Dispatch duplication | **M** | A second dynamic-dispatch representation beside `{data, itable}`; class→interface bridging rules. |
| GC / layout | **M** | Immix + Boehm both must handle inherited layouts and vtable words; interacts with precise-GC maps (D-0008). |
| Language complexity / teachability | **XL** | This *is* the anti-goal (D-0015); classes, `super`, MRO, abstract/virtual/final are the single largest readability-surface increase available. |
| Spec / grammar surface | **L** | New type kind + ~5 keywords + inheritance/override/resolution rules; post-1.0 **MAJOR** (D-0076). |
| Interaction: generics + bounds | **L** | Bounds are monomorphized static dispatch (D-0064); class hierarchies + bounds + variance is a known hard corner. |
| Interaction: value structs | **L** | Classes cannot be value types without slicing → they do not compose with D-0051; a permanent carve-out. |
| Interaction: interfaces | **L** | Nominal classes inside a structural conformance model; bridging class vtables to itables. |
| Interaction: first-class fns | **S** | Methods-as-values already exist (D-0067); mostly additive. |
| Migration / compat | **L** | Syntactically additive but a MAJOR *conceptual* surface and an idiom fork: "structs + composition" vs "classes + inheritance" guidance splits the ecosystem. |

The teachability row dominates.
Zap's positioning — "read by humans, written by agents" — treats a small,
single-idiom surface as a feature, not a limitation; classes are the canonical
way to make that surface large.

## Alternatives short of classes

Four increments that capture part of the benefit, compared on benefit vs cost.

### A. Interface default methods — *best ratio; the recommended escape hatch*

Let an interface carry a method *body* written in terms of its own required
methods; a type that provides the required methods gets the default for free and
may override it.

```zap
interface Shape:
    def area(self) -> float64            # required
    def describe(self) -> str:           # default, in terms of area()
        return "a shape of area " + str(self.area())
```

- **Benefit:** closes *the* gap from §Problem items 1 and 4 — method-body reuse
  and default/template behavior — directly and at the right level (the
  interface, which is already the contract).
- **Cost: S–M.**
  No field inheritance, no nominal hierarchy, no `super` tower, no new object
  layout.
  Dispatch reuses the **existing itable**: a default is simply the function a
  slot points at when the type did not supply its own — no object-embedded
  vtable, no second mechanism.
  It is purely **additive** (new optional interface-body syntax), so it is a
  **MINOR**, not a MAJOR.
- **Caveat:** default methods are the thin end of "traits"; the line to hold is
  *behavior only, never state* (no interface fields), which keeps it from
  becoming inheritance by another name.

### B. Explicit delegation / embedding sugar — *secondary candidate*

Sugar that auto-forwards an embedded field's methods, removing the §Problem item
2 boilerplate while staying pure composition:

```zap
struct Widget:
    use Base base         # 'use' forwards Base's methods onto Widget
    int64 z_order
# self.name() now resolves to self.base.name() with no hand-written forwarder
```

- **Benefit:** removes composition's real ergonomic tax (manual forwarding) and
  makes "has-a + exposes" as cheap as "is-a" — without inheritance semantics,
  substitutability, or a vtable.
- **Cost: M.** Name-resolution and conflict rules (what if two embedded fields
  both expose `name`?); additive MINOR.
- **Relation to A:** orthogonal — A shares *behavior at the interface*, B shares
  *members from a field*. Together they cover items 1, 2, and 4.

### C. Trait-like mixins (behavior + required-method sets) — *not recommended over A*

A heavier "trait" with associated required methods and provided methods, à la
Rust/Scala.

- **Benefit:** a superset of A.
- **Cost: M–L**, and it largely duplicates the structural-interface mechanism
  Zap already has; most of its extra power over A is *stateful* mixing, which is
  the slippery slope back toward inheritance.
  A is the 80% at 40% of the cost.

### D. Sealed / sum-type polymorphism — *already shipped*

Closed hierarchies ("a Shape is exactly Circle | Square") are already expressible
with tier-3 data-carrying enums + `match` (D-0046), with exhaustiveness
checking.

- **Benefit:** covers the *closed* polymorphism case that OO often reaches for
  inheritance to model.
- **Cost: none** — it exists.
  Worth stating explicitly in the verdict so "we need classes for variants" is
  answered before it is asked.

## Rationale and verdict

**Decline full classes with implementation inheritance.
Keep #66 deferred.**

The reasoning, against the #66 bar:

1. **The insufficiency bar is not met.**
   The survey finds five of six frictions already covered and one genuine gap
   (method-body reuse / defaults).
   No corpus of logged real-use insufficiency exists; the ticket says revisit
   *only* when it does.
2. **The one real gap has a cheap, in-idiom fix.**
   Interface default methods (Alt A) close it additively, through the dispatch
   machinery that already ships, at S–M cost, as a MINOR — versus classes'
   XL teachability cost and MAJOR, post-freeze surface break.
3. **Classes conflict with shipped design, not just with taste.**
   They cannot be value types (D-0051 slicing), and they introduce a second
   dynamic-dispatch representation beside the frozen `{data, itable}` fat
   pointer (I#84) — two concrete, not merely philosophical, conflicts.
4. **Dynamic polymorphism is already solved.**
   The headline reason to add classes (runtime polymorphism + heterogeneous
   collections) is a shipping feature done *without* per-object vtables.
5. **The simplicity stance is the product.**
   "A small surface humans can read and agents can emit correctly" is Zap's
   thesis; classes are the single largest available step away from it, and #139
   already sits last on the roadmap.

**Decision — D-0077 (Accepted; ratified by the maintainer 2026-10-27):**

> **D-0077 — Full OO (classes + implementation inheritance): declined; the
> method-reuse gap addressed, if justified, via interface default methods.**
> **Status:** Accepted — ratified by the maintainer 2026-10-27.
> Full OO is **declined** for Zap.
> Composition (D-0015), structural interfaces (D-0016), the struct/value-struct
> split (D-0023/D-0051), and the existing static + `{data, itable}` dynamic
> dispatch (D-0064/I#84) remain the model.
> The one genuine gap — sharing a method body / default behavior across the
> types satisfying an interface — is, **if and when logged real-use friction
> justifies it**, to be closed by **interface default methods** (behavior only,
> never interface state; dispatched through the existing itable; additive
> MINOR), optionally with **explicit delegation sugar** for composition.
> Classes, implementation inheritance, `super`/MRO, and field inheritance stay
> **rejected**.
> #66 remains the deferred tracker; a narrower default-methods ticket is opened
> only when the friction is recorded.

This keeps the door the ticket wanted open (a path exists if real use demands
it) while refusing the disproportionate option.

## Compatibility

Design-only: no code, and no change to `SPEC.md`, `GRAMMAR.ebnf`, or
`STABILITY.md` in this ticket.
The verdict itself breaks nothing.

Were the recommended escape hatch (Alt A, and/or B) later adopted, it would be
**additive and MINOR** (D-0076 §4.2): new optional interface-body / delegation
syntax, no existing program invalidated, the frozen 1.0 Stable set untouched.
Full classes, by contrast, would be a **MAJOR** conceptual surface addition and
are explicitly out of scope by this verdict.

## Implementation

**None in this ticket.**
This is a STEP 1 evaluation that ends in a verdict (D-0077), ratified by the
maintainer 2026-10-27.

Per the ratified D-0077, the follow-on is **not** scheduled now: a separate
design doc + implementation ticket for *interface default methods* (Alt A) would
be written only once real-use friction is logged, sequenced after the 1.0
release per the roadmap.
If the maintainer instead wants full classes scoped despite this
recommendation, that is a new, large design doc of its own — this document is
the argument for not writing it.

No IR-schema or runtime-ABI boundary is touched by this doc.

## Open issues

The verdict (D-0077) is ratified; these are the follow-on calls it leaves for
when the escape-hatch path is next considered:

1. **Accept the verdict?**
   Decline full classes + keep #66 deferred, with interface default methods as
   the named escape hatch — or do you want full classes scoped anyway?
2. **Is the one real gap real *enough* yet?**
   Greenlight interface default methods now, or hold even that until a corpus of
   logged real-use friction (per the #66 bar) exists?
3. **Dispatch commitment.**
   Confirm that any future default methods dispatch through the existing
   `{data, itable}` itable (no object-embedded vtable), so no second
   dynamic-dispatch mechanism is introduced.
4. **Delegation sugar (Alt B).**
   In scope alongside default methods, or a separate later question?
5. **Ticket hygiene.**
   Keep #66 open-but-deferred as the tracker, or close it in favor of a narrower
   "interface default methods" ticket when/if that path is taken?
6. **D-0077 id and wording** — ratified as written (number next free after
   D-0076); to be transcribed into `DECISIONS.md` as a Locked entry in a
   follow-up.
