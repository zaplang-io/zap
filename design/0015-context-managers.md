# 0015 — Context managers & the `with` statement (I#185)

Status: **Proposed (design-first; awaiting confirmation before implementation)**
Decision: **D-0069** (see `DECISIONS.md`)
Depends on: D-0007/D-0029 exceptions & unwind (I#22), D-0027 cleanup policy,
I#175 threading locks (`Lock`/`RLock`/…), D-0048 iterator-protocol dispatch (the
dunder-method lookup pattern reused here), D-0054 `with expect_raises(...)`.

## Goal

Python-faithful `with` statement driving the context-manager protocol, so a
resource is released on **every** exit path without hand-paired acquire/release.
Motivating case:

```python
with lock:
    # critical section
# lock released here — on normal fall-through, early return/break/continue,
# AND panic/unwind.

with open(path) as f:
    data = f.read()
# f closed here on every path.
```

Syntax is already parsed (grammar `with_stmt`, `with_item`; AST `Stmt::With`,
`WithItem`); today only `with expect_raises(ErrType):` (D-0054) lowers — every
other form errors in `lower_with`. This design generalises the lowering.

## 1. Protocol method names (Python-faithful)

A context manager is any value whose (struct) type defines:

| Method | Signature (v1) | Role |
| --- | --- | --- |
| `__enter__` | `def __enter__(self) -> T` | runs on entry; its result is bound by `as NAME` |
| `__exit__`  | `def __exit__(self)` | runs on **every** exit path |

Dunder spelling matches the already-shipped `__iter__`/`__next__`/`__json__`
convention, so there is **no reserved-keyword clash** (contrast I#175's
`set` → `set_`). Method dispatch reuses the D-0048 mangling
`"{StructName}__{__enter__}"` resolved against `self.sigs` (exactly how
`setup_iter` finds `__iter__`).

### v1 simplification vs CPython (deferred, D-0069 follow-ups)

CPython's `__exit__(self, exc_type, exc_val, exc_tb) -> bool` receives the
in-flight exception and may **suppress** it by returning truthy. v1 ships the
zero-argument, non-suppressing `__exit__(self)`:

- `__exit__` takes no exception triple and returns nothing; its result is
  ignored. It runs identically on the normal and unwinding paths.
- Exception **suppression** is deferred: a manager cannot swallow a panic in
  v1. This matches lock/file semantics (neither suppresses) and keeps the
  landing-pad lowering a straight re-raise. When suppression lands, `__exit__`
  grows the exc arguments and the unwind path branches on its bool result.

This is the one place v1 diverges from Python; called out explicitly because
"keep syntax Python-faithful" is the top priority — the *surface* is faithful
(`with M as x:`, `__enter__`/`__exit__`), only the `__exit__` *arity* is reduced
for v1.

## 2. Desugaring

`with M as x:` desugars to an `__enter__` bind + a finally-equivalent that runs
`__exit__`:

```python
__mgr = M                 # evaluated once
x = __mgr.__enter__()     # (no `as`: result discarded, __enter__ still called)
try:
    BODY
finally:
    __mgr.__exit__()
```

**Multiple managers** `with a, b:` nest left-to-right (Python semantics):
`with a: with b: BODY`. Supported in v1 (grammar/parser/AST already carry the
item list; the lowering recurses over items). `a.__exit__` runs after
`b.__exit__`, and if `a.__enter__` succeeds but `b.__enter__` raises, `a`'s
`__exit__` still runs (the outer finally wraps the inner `__enter__`).

The lowering is **not** a textual re-lowering through `lower_try`. `with` gets
its own `lower_with_single(mgr_expr, alias, body)` so it directly owns the
landing pad and the cleanup registration (below). This avoids a double-`__exit__`
hazard that arises if an early-exit cleanup call, emitted inside the try body,
were itself routed back to the try's own landing pad.

## 3. Lowering: the three exit paths

The lowerer gains a **cleanup stack** `self.cleanups: Vec<Cleanup>`, where
`Cleanup { mgr: Value, exit_fn: String }` names the manager SSA value and its
mangled `__exit__`. `self.loops` entries gain a cleanup **baseline**
(`cleanups.len()` captured when the loop opened) so `break`/`continue` know how
far to unwind.

`lower_with_single`:

1. Evaluate `M` → `mgr`. Call `__enter__(mgr)`; bind `x` (if present) to the
   result.
2. `lp = new_block(); after = new_block()`.
3. Push `lp` onto `landing_pads`; push `Cleanup{mgr, exit_fn}` onto `cleanups`.
4. Lower `BODY`.
5. Pop `cleanups`; pop `landing_pads`.
6. **Normal fall-through** (`!terminated()`): emit `__exit__(mgr)`; `br after`.
7. **Unwind path**: if `used_lps` contains `lp`, position `lp`, emit
   `LandingPad`, emit `__exit__(mgr)` as a cleanup call, then `end_catch` +
   re-raise via `throwing_call(zap_panic, exc)` — which, with `lp` already
   popped, routes to the *enclosing* manager/try landing pad, so multi-manager
   and `with`-inside-`try` cleanup **chains** correctly on exceptions (identical
   to the existing finally-only `try` branch).
8. `position(after)`.

**Early `return` / `break` / `continue`** (`lower.rs` `Stmt::Return/Break/
Continue`): before setting the terminator, walk the cleanup stack innermost-first
and emit each pending `__exit__`:

- `return`: evaluate the return value first, then run **all** `cleanups`, then
  `Term::Ret`.
- `break`/`continue`: run `cleanups` from the top down to the enclosing loop's
  baseline, then branch.

During this structured-early-exit walk, `landing_pads` is **saved and cleared**
so the emitted `__exit__` calls are plain `Call`s, not `Invoke`s. This both
prevents an `__exit__` from re-entering its own landing pad (double-exit) and
encodes the v1 rule: **if an `__exit__` throws during early-exit cleanup it
propagates without running the remaining pending `__exit__`s.** Lock/file
`__exit__` never throw, so the motivating cases are fully correct; the general
throw-during-early-exit-cleanup chaining is a deferred follow-up.

### Why a cleanup stack (and the I#22 interaction)

I#22 explicitly **deferred** running `finally` on early `return`/`break`/
`continue` (today those lower straight to `Term::Ret`/`Term::Br`, skipping any
enclosing `finally`). `with lock:` *cannot* inherit that gap — a `return` inside
the critical section would leak the lock. The cleanup stack is the standard
"block stack" fix (cf. CPython `SETUP_FINALLY`).

Scope decision for the orchestrator (open question Q1): the same machinery, if
`try/finally` also registers its `finally` block as a `Cleanup`, closes the I#22
early-exit gap for `try/finally` too. Recommended, since it is a latent
correctness bug and the cost is one extra registration — but it widens the blast
radius beyond `with`, so it is called out rather than assumed.

The **unwind** path needs no cleanup stack: it is handled entirely by the
per-`with` landing pad (step 7), chaining through enclosing pads exactly as
nested `try/finally` already does.

## 4. Std lock guard — `with lock:`

`Lock`/`RLock`/`Semaphore`/`Condition` are prelude wrapper structs
(`parser.rs` `PRELUDE_SRC`) over an opaque `sync` handle, with `acquire`/
`release` methods. Each gains the protocol:

```python
struct Lock:
    bytes _h
    def acquire(self): sync.lock(self._h)
    def release(self): sync.unlock(self._h)
    def __enter__(self) -> Lock:
        sync.lock(self._h)
        return self
    def __exit__(self):
        sync.unlock(self._h)
```

(`RLock` → rlock_lock/unlock; `Semaphore` → sem_acquire/release; `Condition` →
cond_acquire/release.) So `with lock:` acquires on entry, releases on every
exit. **`as NAME` is optional** for locks — `__enter__` returns `self`, so
`with lock as l:` is legal but redundant; the idiomatic guard is the bare
`with lock:`. (`Event` is a latch, not a guard, and gets no protocol in v1.)

No runtime-C changes: the guard is pure prelude Zap over the existing
`zap_mutex_*` entry points.

## 5. Sema

`check_stmt(Stmt::With)` keeps the `expect_raises` special-case (its argument is
a type name, not a value) and otherwise: infer each item's manager expression;
bind `as NAME` to the manager's `__enter__` return type when resolvable (today
it binds `Ty::Unknown`), else `Ty::Unknown`. Verifying `__enter__`/`__exit__`
exist is enforced at lowering (consistent with how `__iter__` is enforced in
`setup_iter`), with a diagnostic pointing at the protocol.

## 6. EBNF

The productions already exist and are unchanged:

```ebnf
with_stmt  ::= "with" with_item { "," with_item } block ;   (* D-0027, D-0069 *)
with_item  ::= expr [ "as" IDENT ] ;
```

The grammar note is updated to record that `with` now drives the general
`__enter__`/`__exit__` protocol (not only `expect_raises`), and D-0069 is added
to the trailing decision reference. Grammar edits stay confined to this
production (coordinating with the concurrent I#70 multi-import grammar work).

## Goldens (post-approval)

- `with_lock` — `with lock:` mutates shared state under contention; release
  observed (second acquirer proceeds).
- `with_open_as` — `with open(path) as f:` reads, file closed after.
- `with_exit_on_unwind` — body raises inside `with lock:`; `__exit__` runs, lock
  released, exception still propagates (`.exit`/`.stderr` convention).
- `with_early_return` — `return` inside `with lock:` runs `__exit__`.
- `with_multi` — `with a, b:` nests; exit order b-then-a.
- `with_expect_raises` — existing D-0054 form still works (regression).

## Open questions (gating implementation — reported to orchestrator)

- **Q1 — try/finally early-exit:** also route `try/finally` through the new
  cleanup stack (fixing I#22's deferred `return`/`break`/`continue` gap), or
  scope the cleanup stack to `with` only this increment? (Recommend: fix both.)
- **Q2 — `__exit__` arity:** ship zero-arg non-suppressing `__exit__(self)` for
  v1 (recommended), deferring the `(exc_type, exc_val, exc_tb) -> bool`
  suppression form? Or build the suppressing form now?
- **Q3 — multiple managers:** support `with a, b:` in v1 (recommended; grammar
  already allows it, lowering recurses) or defer to single-manager only?
- **Q4 — `as` binding type:** bind `as NAME` to `__enter__`'s return type in
  sema v1, or accept `Ty::Unknown` (looser; `x.method()` won't typecheck) to
  keep the sema change minimal?
