# Zap `@scoped` — affine, with-bound resource marker

Normative design for **#876** (`@scoped` affine resource marker). This document
is the authoritative spec; the design debate and rationale live in GitHub
Discussion #874 (RFC). The decision record is **D-0159** in `DECISIONS.md`.

Builds on, and does not restate: **D-0069** / #185 (the `with` context-manager
protocol — `__enter__`/`__exit__`), **D-0151** (#839/#850, escape analysis —
reused here), and **D-0158** / `docs/design/arena.md` (the arena, the first
`@scoped` consumer).

## 1. What `@scoped` is, and why

Zap's `with` is a Python-faithful context-manager protocol (D-0069): any struct
with `__enter__`/`__exit__` is a context manager. But — exactly like Python — a
context manager can be constructed **detached**, and then its cleanup simply
never runs:

```zap
f = open(path)     # legal; __exit__ never runs — a file leak
a = arena()        # a detached arena would leak its whole region pool
```

For cleanup-critical RAII — the #553 arena, a lock/guard, a DB transaction,
`pin()` (#544) — leak-by-forgetting is a correctness hole a GC'd language should
not ship: the resource these types manage is *not* on the GC heap (it is a region
pool, an OS lock, a transaction handle), so a dropped-on-the-floor value is a
real leak the collector cannot clean up. `@scoped` closes that hole **statically**.

A struct marked `@scoped` is an **affine, with-bound resource**: each value must
be **consumed exactly once**, and the only two consumers are a `with` head or a
move into another `@scoped` struct's field. Everything else is a compile error.
The guarantee lives in the type (`@scoped` in the declaration), not in how a body
happens to be written — the same visible-over-inferred principle that led #553 to
prefer an explicit `@noescape` over inferred non-retention. For a
soundness-critical, expert-facing property, visible-and-checkable beats
clever-and-hidden.

This is **affine typing scoped narrowly to `@scoped` types** — not a general
linear type system for the language — so the cost is contained (see §6).

## 2. Surface

A struct is marked with the `@scoped` decorator (D-0159 Q1). This reuses the
existing struct-decorator surface (the `@shareable` precedent, D-0113); it adds
**no grammar** — `GRAMMAR.ebnf` is unchanged.

```zap
@scoped
struct Arena:
    # ... fields ...
    def __enter__(self) -> Arena:
        # acquire the region pool
        return self
    def __exit__(self):
        # bulk-free the region pool
        pass
```

There is **no** `move`/`consume` keyword (D-0159 Q4): ownership transfer into a
`@scoped` field is expressed by plain assignment, and the affine checker proves
single-consumption. No new surface syntax of any kind.

## 3. Rules (normative)

### 3.1 Well-formedness — a `@scoped` struct must be a context manager

A struct that is `@scoped` — whether marked explicitly with the decorator, or
`@scoped` **implicitly** by owning a `@scoped` field (§3.4) — **must define both
`__enter__` and `__exit__`**. Absence of either is a compile error:

> `ZAP5127: @scoped struct 'X' must define both __enter__ and __exit__`

Rationale (maintainer, #874): a `@scoped` type is by definition one that
participates in `with`, so it must implement the full context-manager protocol
(D-0069). A `@scoped` marker without the methods is meaningless and almost
certainly a mistake — caught at the definition, not deferred to the first `with`
use. This applies uniformly: an owner that becomes `@scoped` only by owning a
`@scoped` field must still hand-write both methods (D-0159 Q5); the compiler does
**not** synthesize them.

### 3.2 With-head-only construction

A `@scoped` value may be produced only as the **head of a `with`**, or as the
right-hand side of an assignment **into a `@scoped` field** (§3.3). Any other
appearance of a freshly produced `@scoped` value — a bare local binding, a
`return`, a store into a non-`@scoped` slot, an argument to a retaining call — is
a compile error:

```zap
with arena() as a:      # OK — with head
    ...

a = arena()             # ERROR ZAP5128: a @scoped value must be bound by `with`
                        #   (or moved into a @scoped field)
return arena()          # ERROR ZAP5128
store.x = arena()       # ERROR ZAP5128 (store.x is not a @scoped field)
```

> `ZAP5128: a @scoped value must be bound by a `with` block or moved into a
> @scoped field; it may not be constructed detached, returned, or stored`

### 3.3 Affine — consumed exactly once

Within its enclosing scope a `@scoped` binding is **affine**: it must be consumed
**exactly once**, and after it is consumed it may not be used again. The two
consumers are:

1. a **`with` head** — `with arena() as a:` enters on entry and runs `__exit__`
   on every exit path (normal and exceptional), or
2. a **move into another `@scoped` struct's field** — `self.a = <scoped value>`
   inside the owner's `__enter__`, transferring ownership to an owner that is
   itself `@scoped` (§3.4).

A freshly produced `@scoped` value that is consumed by neither consumer — never
`with`-bound and never moved into a `@scoped` field — is rejected at its
construction site by the with-head-only rule (**ZAP5128**, §3.2). A value that is
consumed by a `with` head and then also escapes that scope is rejected by the
no-escape rule (**ZAP5129**, §3.5). Together these give the affine
consumed-exactly-once guarantee for the positions v1 tracks; a dedicated
"used-after-move" diagnostic beyond these is a conservative future refinement.

A `with`-bound name is live for the body of the `with` and is **consumed** (torn
down) at block exit; it may be read and have its methods called inside the body,
but it may not be moved or returned out of the body (that would be an escape,
§3.5).

### 3.4 Contagion through ownership

A struct that owns a field of `@scoped` type is itself `@scoped`, transitively
(scopedness propagates container-ward). This is what lets a composite — e.g.
`arena_dict` (#873) — hold an `arena` *and* forces the composite to be
`with`-bound, so the inner arena cannot leak. (Cf. Rust: a struct owning a `Drop`
type is itself `Drop`.)

An implicitly-`@scoped` owner is subject to every rule above, including §3.1: it
must hand-write both `__enter__` and `__exit__`. Its `__enter__` sets the owner up
(and typically constructs/moves the owned scoped field into place); its `__exit__`
holds the owner's own teardown.

### 3.5 No escape from the binding scope

A `@scoped` value may not outlive its `with` scope. Returning it, storing it into
a longer-lived slot, capturing it in a closure, boxing it into an interface, or
carrying it across a `spawn` / `scope:` boundary is a compile error. This reuses
the D-0151 escape lattice (`crates/ir/src/escape.rs`) with the boundary set to the
`with` scope and the consequence **inverted** to a hard error (never the D-0151
heap fallback — there is no safe fallback; the resource is gone at block exit).
Ambiguity fails safe to *reject*. A `@scoped` value crossing a `spawn`/`scope:`
boundary is an escape and is forbidden (consistent with the arena's
coroutine-local rule, D-0158 decision 7).

## 4. Auto-teardown (drop-glue)

An owner's teardown **auto-runs each owned `@scoped` field's `__exit__` in reverse
construction order** (Rust `Drop`-glue / Python `ExitStack` semantics), *in
addition to* the owner's hand-written `__exit__` body. The author's `__exit__`
handles the owner's own cleanup; the compiler guarantees the owned scoped fields
are exited regardless, so an owned scoped field can never be forgotten even though
§3.1 requires the author to write `__exit__`.

Ordering (normative): at owner `with`-exit,

1. the owner's hand-written `__exit__` body runs first, then
2. the compiler-emitted drop-glue runs each owned `@scoped` field's `__exit__` in
   **reverse of construction order** (last field entered is first to exit),

on **every** exit path, normal and exceptional, matching the single-field `with`
unwinding of D-0069. Fields are considered "constructed" in the textual order
their moves execute inside the owner's `__enter__`.

## 5. Worked examples

```zap
# (a) valid — with-bound
with arena() as a:
    buf = a.alloc(bytes, 4096)
# a torn down here

# (b) valid — owned-field composition (contagion makes Pool @scoped)
@scoped
struct Pool:
    Arena a
    def __enter__(self) -> Pool:
        self.a = arena()          # move into a @scoped field — consumer (2)
        return self
    def __exit__(self):
        pass                      # self.a.__exit__ auto-runs (drop-glue, §4)

with Pool() as p:                 # Pool is @scoped by contagion → must be with-bound
    ...

# (c) rejected — detached construction            ZAP5128
a = arena()

# (d) rejected — escape by return                  ZAP5129
def leak() -> Arena:
    with arena() as a:
        return a                  # a escapes its with-scope

# (e) rejected — missing protocol method           ZAP5127
@scoped
struct Bad:
    int64 x
    # no __enter__/__exit__

# (f) rejected — cross-spawn move                   ZAP5129 (§3.5)
with arena() as a:
    spawn worker(a)               # a crosses a coroutine boundary
```

## 6. Implementation notes (non-normative)

- **Recognition.** `@scoped` is read off `StructDef::decorators` in sema, exactly
  as `@shareable` (D-0113) is. Contagion (§3.4) computes a fixpoint over struct
  field types: a struct is `@scoped` if marked or if any field's type is `@scoped`.
- **Well-formedness (§3.1).** Once a struct is known `@scoped` (marked or
  contagious), sema checks for `__enter__` and `__exit__` in its methods and emits
  `ZAP5127` if either is missing (`crates/sema/src/scoped.rs`).
- **Affine / with-head / escape (§3.2–3.5).** Implemented in
  `crates/sema/src/scoped.rs` as a per-function walk narrowly scoped to `@scoped`
  values — **not** general linear typing. The with-head-only check (`ZAP5128`)
  rejects any freshly produced `@scoped` value outside a `with` head or a `@scoped`
  field assignment; the no-escape check (`ZAP5129`) rejects a `with`-bound value
  that is returned, stored, or passed across a call / `spawn` / `scope:` boundary.
  The enumeration of escaping positions mirrors the D-0151 escape taxonomy
  (`crates/ir/src/escape.rs`), with the boundary set to the `with` scope and the
  consequence a hard error rather than the heap fallback.
- **Drop-glue (§4).** Emitted in the `with`-lowering (`crates/ir/src/lower.rs`,
  `lower_with_items` + `emit_scoped_field_teardown`): after the owner's `__exit__`
  call, reverse-declaration-order `__exit__` calls for owned `@scoped` fields on
  every exit edge (normal, exception-unwind, and early-exit via `run_cleanups`). A
  no-op for a manager that owns no `@scoped` field, so existing `with` lowering is
  unchanged.

## 7. Relationship to the arena (#553 / D-0158)

The arena is the **first** `@scoped` consumer. Arena v1 was specified (D-0158
decision 2) to ship a *degenerate* with-head-only construction check standalone,
not blocked on this feature. Once `@scoped` lands, that degenerate check is
**subsumed** by the general mechanism: the arena is simply `@scoped`, and its
with-binding guarantee is the §3.2 rule. The full affine/contagion/auto-teardown
machinery here is what unblocks **composite** `@scoped` types — `arena_dict`
(#873) and the arena-local collections — which own an arena and must themselves be
`with`-bound so the inner arena cannot leak. Other natural consumers: locks/guards,
DB transactions, `pin()` (#544).
