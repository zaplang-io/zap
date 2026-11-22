---
title: First-class MemoryError exception type; arena cap/OOM raises it (not RuntimeError)
status: Locked
issue: 882
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# First-class `MemoryError` exception type

**Status:** Locked

## Decision

Add `MemoryError` as a **first-class built-in exception type** whose parent is
`Exception` (CPython parity, where `MemoryError` sits directly under
`Exception`). It joins the built-in hierarchy exactly like `TimeoutError` /
`AssertionError` / `EOFError`: a new `ZAP_EXC_MEMORY_ERROR = 18` runtime id, an
`Exception` parent edge in `zap_exception_parent`, an entry in sema's builtin
name table and `is_exception_type`, an entry in lowering's
`exception_type_id_name`, and an entry in the pyinterop name→id map.

The **arena** (`#553`, D-0158) cap-overflow / heap-OOM raise site — the
generated NULL-check calling `zap_arena_oom` — now raises `MemoryError` instead
of the arena v1 stopgap `RuntimeError`. The raise still goes through
`throwing_call`/invoke, so an enclosing `try/except` catches it and the arena
`with` tears the arena down on the unwind path.

```zap
with arena(cap=64) as a:
    try:
        b = a.alloc(bytes, 1 << 30)   # over cap
    except MemoryError as e:          # caught by name
        print("recovered")
# also catchable as its parent:  except Exception
```

## Rationale

- **The design already specified it.** `docs/design/arena.md` §2.3/§3 named
  `MemoryError` as the intended exhaustion exception from the start; §10 recorded
  the arena v1 `RuntimeError` as an explicit, temporary stopgap "because Zap has
  no `MemoryError` type yet" and listed its promotion as a tracked follow-up.
  This issue implements that already-recorded intent; it is not a new design.

- **Parent is `Exception`, matching CPython.** `MemoryError` is an ordinary
  catchable exception under `Exception` (not under `RuntimeError`), so
  `except MemoryError` and `except Exception` both catch it. This mirrors how
  every other leaf built-in error that is not in the `LookupError` /
  `ArithmeticError` sub-trees is parented.

- **Mirror an existing type, touch every wiring site.** `MemoryError` is added
  at exactly the sites `TimeoutError`/`EOFError` occupy so no exhaustive match
  (host or LLVM-only) breaks and the compile-time and runtime name resolutions
  agree.

## Consequences

- **Breaking change (arena v1, experimental):** an arena cap-overflow / heap-OOM
  now raises `MemoryError`, not `RuntimeError`. Code that caught the v1 behaviour
  with `except RuntimeError` no longer catches it; `except Exception` is
  unaffected (both types descend from `Exception`), and `except MemoryError` is
  the specified form. The arena surface is experimental v1, the `RuntimeError`
  was always documented as a stopgap, and no stable surface depended on it, so
  the change is made without a deprecation window. The uncaught-report message
  keeps its `"MemoryError:"` text prefix because the runtime's uncaught reporter
  renders only the message string, never the exception's type name.

- `raise MemoryError(...)` and `except MemoryError` are now available to user
  code, and a Python-interop `MemoryError` maps to the same type.

See also D-0158 (`docs/design/arena.md`, the arena epic #553) and the
`TimeoutError` (I#485) / `EOFError` (#1036, D-0215) wiring precedents.
