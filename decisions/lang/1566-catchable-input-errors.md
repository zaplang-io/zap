---
title: Recoverable input/state faults raise catchable exceptions instead of aborting
status: Accepted
issue: 1566
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Recoverable input/state faults are catchable

**Status:** Accepted (mechanical scope); three policy items **Held** — see below.

## Decision

Ordinary recoverable input or state errors raise a **catchable** Zap exception
that a matching `try`/`except` in the same function handles, with `finally` and
`with`/context cleanup running on the unwind path, instead of terminating the
process. This aligns the implementation with SPEC (which already specifies these
operations as *raising*). Two defects are fixed per surface:

1. **Runtime direct termination** — entry points that did `fprintf(stderr, …);
   abort()` (or, for regex, `exit(1)`) now `zap_panic_typed(ZAP_EXC_…, msg)`,
   joining the C++-unwinder path like every other raise.
2. **Missing unwind edge** — ZIR lowerings that emitted a plain `Inst::Call` to a
   runtime function that can raise now emit a `throwing_call` (an `Inst::Invoke`
   whose unwind edge targets the active landing pad inside a `try`; a plain
   `call` otherwise), so the raise reaches the local handler rather than
   unwinding past it. Same requirement as #383 / #803 / #841.

Surfaces fixed in this first cut (PR1) and their exception types:

| surface | type |
|---|---|
| `str.index` / `str.rindex` (absent substring) | `ValueError` |
| `chr(n)` / `char(n)` (invalid Unicode scalar) | `ValueError` |
| `bytes[i]` (out of range) | `IndexError` |
| `str` / `bytes` slice, zero step | `ValueError` |
| `list` / `tuple` slice, zero step | `ValueError` |
| `deque` `pop` / `popleft` / `get` / `d[i]` (empty / OOB) | `IndexError` |
| `heap` `pop` / `peek` (empty) | `IndexError` |
| `random.randint(a, b)` with `b < a` | `ValueError` |
| `random.choice` of an empty sequence | `IndexError` |
| generator `choice` sample > population (no replacement) | `ValueError` |
| regex invalid pattern / bad replacement group | `ValueError` (see Rationale) |
| `AtomicArray[T](cap)` with `cap < 0` | `ValueError` (runtime already raised; added the unwind edge) |
| `sync.add` / `sync.done` (negative counter), `sync.rlock_unlock` (un-acquired) | existing `Exception` (added the unwind edge) |
| `argparse` duplicate-name / limit registration | existing `Exception` (added the unwind edge) |
| `yaml.load` / derived `from_yaml` (and sibling `from_json` / `from_csv`) malformed input | existing exception (added the unwind edge) |
| `print(py_obj)` whose Python `__str__` raises / returns non-str | existing (translated) exception (added the unwind edge) |
| dict/set op with a **function-key** (`Elem::Struct`/`VarTuple`) key whose hash/eq may raise (e.g. a float leaf, NaN) | existing `ValueError`/user exception (added the unwind edge) |

Numeric `array[T]` N-D indexing and slicing bounds were the **sequenced
follow-up** and are now also catchable (PR2): the `zap_array_bounds` /
`zap_array_ckidx` / `zap_array_index_axis` / `zap_array_dim` runtime guards
raise `IndexError` and `zap_array_slice_axis`'s zero-step raises `ValueError`,
with the `lower.rs` subscript/slice/partial-view/augmented-assign sites carrying
the unwind edge (the contiguous 1-D `zap_array_slice` clamps and does not raise;
`array_ckidx` and `array_fold_rows` are the per-axis / partial-view chokepoints).
The orphaned `crates/ir/src/lower/arrays.rs` (not declared as a module anywhere,
so never compiled) is left untouched.

## Rationale

- **Regex error type = `ValueError`.** An invalid pattern or a bad replacement
  group reference is a bad *value* supplied to the regex engine; `ValueError` is
  the least-surprising built-in (CPython raises `re.error`, a `ValueError`
  subclass). A dedicated `RegexError` type can be introduced later without
  breaking `except ValueError` (it would subclass it); this is the reasonable
  default the issue names.
- **Function-key dict/set ops always take the unwind edge.** Rather than prove a
  particular key layout float-free (a *missed* edge terminates the process — the
  exact #1566 failure mode — while an *unused* edge to a callback that does not
  raise is inert), `key_rt_call` routes every function-key op (and every bare
  float key) through `throwing_call`. This also makes a user-authored `__hash__`
  / `__eq__` that raises catchable, which is correct.
- **`heap` pop/peek vs replace.** `pop`/`peek` on an empty heap now raise
  `IndexError`. Empty `replace` was deliberately left aborting here (a D-0236
  contract change held for sign-off); it was subsequently made catchable in
  [lang/1579-heap-replace-catchable](1579-heap-replace-catchable.md), resolving
  Held item 1 below.

## Consequences

- `except`-based recovery now works for these operations; the **uncaught** form
  still prints the normal `Uncaught exception: <message>` diagnostic and exits 1
  (for regex this preserves the former `exit(1)` status while making it
  catchable). Intentional termination — `sys.exit`, process signals, the race
  detector `_exit`, graceful-shutdown forced exit, and the heap-exhaustion
  fail-stop `abort()`s (D-0119) — is unchanged.
- Runtime tests run under both Immix and Boehm; host ZIR checks pin the invoke
  edges (`crates/ir/src/lib.rs`), and golden + diff fixtures pin the
  caught/finally/continuation and the uncaught exit-1 behavior.

### Held for maintainer decision (NOT changed here)

These policy/contract forks are flagged, not decided:

1. **Empty `heap.replace`** — RESOLVED in
   [lang/1579-heap-replace-catchable](1579-heap-replace-catchable.md): empty
   `replace` now raises a catchable `IndexError` (superseding the D-0236 abort),
   matching `pop`/`peek`.
2. **CSPRNG entropy failure** (`zap_crypto_fill_random`; shared by crypto
   random/token and UUID/NanoID) still `abort()`s. Catchable `OSError` vs a
   documented fail-stop is a genuine policy fork — left unchanged.
3. **`MemoryError` vs fail-stop** — D-0119 (heap-exhaustion fail-stop) vs SPEC
   §10 ("an allocation failure raises `MemoryError`") plus the temp-alloc
   wrappers in `str.expandtabs`/`str.translate`/`http.url`. Reconciliation is a
   policy decision — left unchanged.

<!-- Related: [lang/882-memoryerror-exception](882-memoryerror-exception.md). -->
