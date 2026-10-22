# Proposal: Python runtime interop

- **Author(s):** Zap team
- **Last updated:** 2026-10-17
- **Status:** Accepted (design)
- **Discussion:** (internal) — see `DECISIONS.md` D-0012

## Abstract

Zap can import and call CPython modules at runtime — `import numpy`,
`pandas`, `requests`, etc. — by embedding the CPython interpreter and marshaling
values across the Zap ↔ Python boundary. This is the primary interop story.
General C FFI (and, through it, every other language) is deferred to a later
phase.

## Background

The audience already owns a large Python codebase and ecosystem. The most
compelling pitch is "keep your Python libraries, make your code fast." Embedding
CPython is well-trodden (Julia's PyCall, Mojo) and bounded in scope. By
contrast, direct Go-library import is **not** pursued: Go ships its own GC and
goroutine scheduler, which conflict with Zap's own scheduler (D-0008); Go stays
reachable only later via the C boundary.

## Proposal

### Importing and calling

```python
import python "numpy" as np          # import a CPython module

auto a = np.array([1, 2, 3])          # returns a Python-object handle
auto b = np.sum(a)                    # call Python functions
print(int64(b))                       # marshal back to a Zap value
```

- `import python "<module>" as <name>` loads a CPython module.
- Attribute access and calls on Python handles are dispatched through the
  CPython C-API.
- Values crossing the boundary are **marshaled** (see below).

### Marshaling

| Zap | Python |
|---|---|
| `int64` | `int` |
| `f64` | `float` |
| `bool` | `bool` |
| `str` | `str` |
| `bytes` | `bytes` |
| `list[T]` | `list` |
| `dict[K,V]` | `dict` |
| `array[T]` (D-0011) | NumPy `ndarray` (zero-copy where layout permits) |
| Python object | opaque `pyobject` handle |

Scalars and containers marshal by value/structure; arbitrary Python objects are
held as opaque `pyobject` handles that keep a CPython reference.

### Reference & GC bridging

- Python objects referenced from Zap hold a CPython reference (`Py_INCREF`);
  Zap's GC (Boehm in v1) frees the handle wrapper, whose finalizer calls
  `Py_DECREF`.
- The **GIL** is acquired around CPython calls. Interaction with Zap coroutines:
  calls into Python serialize on the GIL; long Python calls should release it
  where the callee allows (document the caveat — Python-heavy workloads won't
  parallelize across coroutines unless the Python side releases the GIL).

### Exception translation

Python exceptions map cleanly onto Zap's unchecked exceptions (D-0007): a raised
Python exception crossing the boundary becomes a Zap exception carrying the
Python type and message, propagating normally and catchable with `try`/`catch`.

## Rationale

- **Why Python-first:** highest audience value, bounded and proven approach,
  and the exception + dynamic-call model lines up with Zap's semantics.
- **Why defer C FFI:** doing C first is more foundational but less immediately
  valuable to users; once added, C FFI generalizes interop to all languages
  (including Go via `c-shared`) without special-casing each.

## Compatibility

Additive. Requires a CPython runtime available at build/run time; programs that
use `import python` depend on a compatible libpython.

## Implementation

Roadmap **Phase 4**. Depends on collections (`list`, `dict`) and exceptions
(Phase 2), and ideally `array[T]` (Phase 3) for zero-copy NumPy bridging.

1. Embed libpython; initialize interpreter; GIL management helpers.
2. `import python` statement + `pyobject` handle type.
3. Attribute/call dispatch via the C-API.
4. Marshaling for scalars, containers, and `ndarray` ↔ `array[T]`.
5. Exception translation both directions.
6. Finalizer-based `Py_DECREF` bridging to the GC.

## Open issues

- GIL vs coroutine parallelism story — document limits; consider a
  subinterpreter or process-pool escape hatch later.
- Zero-copy `ndarray` ↔ `array[T]` requires matching dtype + contiguous layout;
  define the fallback (copy) precisely.
- Calling *into* Zap from Python (the reverse direction) — later; needs a
  Python extension-module output mode.
- Pinning/lifetime of Zap `array` buffers shared into NumPy without copy.
