---
title: Coroutine-local execution context (ContextVar[T] / ContextToken)
status: Proposed
issue: 1237
pr:
date: 2026-11-21
domain: lang
supersedes:
superseded-by:
---

# Coroutine-local execution context (`ContextVar[T]` / `ContextToken`)

**Status:** Proposed — **PROVISIONAL**. This is a permanent, public language
surface. The API shape and the inheritance/restore semantics below are the
first-cut design; the maintainer gates them at merge. The list of decisions that
need explicit confirmation is in *Open questions* at the end.

## Decision

Zap gains a coroutine-local execution-context facility — its analogue of Python
`contextvars`, resolving the "Coroutine-local storage" open question in
`design/0005-concurrency.md`. The public surface is two built-in reference types:

```
cv  = ContextVar[T]("name")              # no default: get() raises on an unbound read
cv  = ContextVar[T]("name", default=d)   # get() returns d when unbound
v   = cv.get()                           # current value for THIS coroutine
tok = cv.set(v)                          # bind v for this coroutine; returns a ContextToken
cv.reset(tok)                            # restore the context to the point of that set()
```

- **Model = Python `contextvars`, not Go `context.Context`.** The value is bound
  *implicitly* on the current coroutine and read by deep callees without being
  threaded through every signature. This is what OpenTelemetry-Python's
  `trace.get_current_span()` relies on, and `zaplang-io/otel#2` targets the
  Python API. Go's explicit-parameter `context.Context` is the deliberate
  contrast, not the target.
- **Storage lives on the coroutine `G`, not a thread-local.** Each `G` holds the
  head of an immutable singly-linked list of bindings (`G->coctx`). Because it
  lives in the heap `G` object it travels with the coroutine across park/resume
  **and** worker migration, and two interleaved coroutines on one worker have
  distinct heads, so neither can observe or overwrite the other's binding.
- **`set`/`reset` are an O(1) head save/restore.** `set` prepends an immutable
  node and returns a `ContextToken` capturing the previous head; `reset(token)`
  restores that head. The immutable tail is shared, so no copying and no
  unbounded growth under nesting (dropped nodes are collected).
- **`spawn` inherits a snapshot (Python `copy_context`).** A spawned child
  inherits the spawning coroutine's current head at spawn time; it shares the
  immutable tail, and its own `set` swaps only its head, so later mutations never
  leak between parent, child, or siblings. Done in one chokepoint — `galloc` —
  so `spawn`, `spawn detached`, and `spawn` inside a `scope:` all inherit
  uniformly. Spawned from a native thread (`main`, no current `G`), the child
  starts empty.
- **`scope:` does not itself push or pop context.** A `scope:` body runs in the
  current coroutine and sees/mutates its own context directly; its spawned
  children snapshot at spawn. Restoring an outer value on scope exit —
  normal **or** exceptional — is the caller's responsibility via a `ContextToken`
  in a `finally`/`with`, which Zap runs on both paths. This satisfies the
  restore-on-unwind requirement without the runtime force-restoring anything.

## Rationale

- **Why `ContextVar[T]`+token over a `with`-scoped binding.** OpenTelemetry-Python
  is built on `ContextVar.set`/`reset` tokens (its `attach`/`detach`), so the
  token primitive is what otel#2 needs. A `with ctx.bind(cv, v):` sugar can be
  layered on top later (a context manager whose `__exit__` calls `reset`); it is
  not the primitive. Go's explicit `context.Context` was rejected as the model
  because the whole point here is an *implicit* current value readable by deep
  callees (the `design/0005` open question, and otel#2).
- **Why snapshot-at-spawn over empty / live-shared.** Python `copy_context`
  semantics are what instrumentation expects: a task started under a span sees
  that span, but its own context edits do not corrupt the parent's. An empty
  child would lose the ambient span; a live-shared context would race and leak
  across coroutines.
- **Why head-save/restore over per-variable functional reset.** For strictly
  nested `set`/`reset` — the recommended usage and everything otel needs — a
  whole-head restore is O(1), allocation-free on reset, and *exactly* Python's
  observable behaviour. It avoids the unbounded list growth a prepend-only
  per-variable reset would cause in a long-lived coroutine. Python additionally
  supports per-variable out-of-order `reset`; that is deferred (see below).
- **Why on the `G`, mirroring `scope`/`no_preempt`.** `design/0008-precise-gc.md`
  already keeps per-coroutine state on the `G` and roots it there; the
  `no_preempt` field comment spells out the "lives on the G so it follows the
  coroutine across M migration" reasoning. Context has the same requirement.
- **GC representation — no boxing, no new trace kind.** Each binding node carries
  both a traced `value_ref` (for a reference-typed var) and an untraced
  `value_word` (for an `int64`-typed var); the node's precise descriptor traces
  `{var, value_ref, next}` only. A reference value is therefore always kept alive
  and forwarded, and a scalar word is never mis-scanned as a pointer — the
  D-0078 §4.4 invariant — using only the existing `ZAP_TD_PRECISE` offset-table
  mechanism (like `zap_desc_gobj`), with no `gc.inc.c` custom-trace case.

## Consequences

- New built-in reference types `ContextVar[T]` and `ContextToken` (`Ty::ContextVar`
  / `Ty::CtxToken`), modelled on `AtomicArray[T]`: construction and methods are
  hand-written in sema/lowering and lower to `zap_ctxvar_*` runtime calls. Both
  map to `ptr` in codegen. `ContextVar` is in `BUILTIN_SHAREABLE_TYPES` so it may
  back a module global (the realistic otel pattern).
- `struct G` gains a `void *coctx` field, added to the precise-GC root offset
  table; `galloc` snapshots the parent's head into every spawned child.
- **Parser refinement:** a built-in nominal **type name** keyword (`set`, `list`,
  `str`, …) is now accepted as a `.name` member/method name (unambiguous in
  member position). Without it a method literally named `set` — i.e.
  `ContextVar.set`, matching Python — could not be parsed. This broadens member
  syntax language-wide and should be reviewed as such.
- **Supported value type `T` (v1):** a reference type or `int64` (mirrors
  `AtomicArray`). Other scalar kinds (`float`/`bool`/`char`/`int32`) and
  by-value aggregate structs are a follow-up; a nested-generic value type such as
  `list[int64]` is not yet constructible (the shared `chan_elem_from_expr`
  resolves only simple/named element types, as `channel[T]`/`AtomicArray[T]`
  also do today).
- Most of the implementation (runtime C + codegen lowering) is validated only in
  CI; the host build has no LLVM-18. Sema type-checking and the end-to-end
  behaviour goldens (`tests/golden/ctxvar_*`) are the coverage.

## Open questions (need maintainer confirmation before Locked)

1. The API is `ContextVar[T]` + `ContextToken` with `get`/`set`/`reset` (Python
   model), not a `with`-scoped binding and not Go explicit-context.
2. `spawn` inheritance is snapshot-at-spawn (`copy_context`), not empty and not
   live-shared.
3. `reset` is LIFO whole-context restore; Python's per-variable out-of-order
   `reset` is deferred. Also deferred: `get(default)` as a per-call argument, and
   a `copy_context()` / context-run API.
4. `scope:` does not itself push/pop context (restore is via token/`with`).
5. The parser now accepts built-in type-name keywords as `.name` members
   (language-wide), to allow `cv.set(...)`.
6. Naming: `ContextVar` / `ContextToken` in the global prelude surface (vs a
   `contextvars` module path), and `T` restricted to reference-or-`int64` in v1.

<!-- Related: design/0005-concurrency.md (Structured concurrency, scope:/spawn),
     design/0008-precise-gc.md (per-G state on the shadow chain),
     decisions/runtime/1100-channel-element-type.md (the _i64/_ptr split). -->
