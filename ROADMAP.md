# Zap — Roadmap

The ordering principle (learned from Go's compiler structure): **freeze the data
structures at phase boundaries early**, then build phases in parallel. The two
interfaces that must be locked before work fans out are the **typed-IR schema**
(`design/IR.md`, TBD) and the **runtime ABI** (`design/RUNTIME-ABI.md`, TBD).

Scope is cut ruthlessly per milestone. A feature may be *designed* (spec'd)
long before it is *implemented*.

---

## Phase 0 — Foundations (sequential; do first)

1. **SPEC.md v0.1** — examples-driven core (this repo). The contract everyone
   codes against.
2. **Token + AST definitions** — the data structures only. Once checked in,
   lexer/parser/type-checker authors proceed independently.
3. **Repo scaffolding** — Rust workspace, build, and a `tests/` dir of `.zap`
   programs with expected output (golden tests). Every phase is testable
   end-to-end as soon as it exists.

## Phase 1 — Vertical slice (prove the pipeline)

Get `fib.zap` compiling to a native binary with **only**: `int64`, functions,
`if`/`while`, arithmetic (wrapping). **No GC, no collections, no regex yet.**
This de-risks the whole Rust → LLVM → native toolchain. Milestone programs:
fibonacci, a simple loop sum.

Compiler pipeline (mirrors Go): **lex/parse → type-check → typed IR →
(middle-end) → lower/desugar → LLVM SSA → codegen → link.**

## Phase 2 — Core language + runtime

- Boehm GC integration (object layout + GC hooks) — **runtime ABI frozen here.**
- `str` (immutable UTF-8, D-0025), `bytes`, `bool`, `f64`, `char`.
- Exceptions: `raise`, unwinding, `try`/`except`/`finally` + `with` (D-0007,
  D-0027, D-0029).
- `list[T]`, `dict[K,V]`, tuples (runtime library behind the frozen ABI).
- Structs + methods (D-0015); compile-time structural interfaces (D-0016);
  tier-1 enums (D-0018).
- Front-end ergonomics: f-strings, comprehensions, `yield` generators, lambdas,
  slicing (D-0026).
- Modules, imports, enforced `_` visibility (D-0028).

## Phase 3 — Signature features (parallel tracks)

Once the IR schema and runtime ABI are frozen, these run concurrently:

- **Regex track** — `m//`, `s//`, `=~`/`!~`, `~=`, match objects (PCRE2). `D-0010`.
- **Arrays track** — `array[T]` 1-D first: element-wise ops + scalar broadcast +
  SIMD lowering; then N-D shape, broadcasting, slicing. `D-0011`.
- **Concurrency track** — coroutines + channels (unbuffered/buffered, `close`,
  `select`), sync primitives, structured concurrency, DRF-SC memory model.
  `D-0008`, `D-0031`–`D-0034`.
- **Generics track** — user-defined generics via monomorphization, building on
  the already-generic built-in containers. `D-0017`.
- **Later language features** — data-carrying enums + `match`, default/keyword
  args, `set` type, decorators/annotations (see `OPEN-QUESTIONS.md`).

## Phase 4 — Interop

- **Python runtime interop** — embed CPython, marshal Zap ↔ Python, GIL
  handling, Python-exception ↔ Zap-exception translation. `D-0012`.
- **Package manager** — Cargo/Go-modules-style manifest + lockfile (D-0035).

## Phase 5 — Hardening & tooling

- Precise GC (Immix-style) replacing Boehm.
- N-D array completeness + optimized SIMD kernels.
- Formatter, LSP, package tooling.
- C FFI (`extern "C"`) — generalizes interop to all languages, incl. Go via
  `c-shared`.

---

## Parallelization map

| Track | Depends on | Can start once |
|---|---|---|
| Runtime (GC, collections) | runtime ABI | ABI frozen (Phase 2) |
| Regex | runtime (str) + PCRE2 bind | str exists |
| Arrays | codegen + SIMD primitive | vertical slice + `vec` |
| Concurrency | runtime + scheduler | GC + runtime ABI |
| Python interop | C-API embedding + marshal | collections + exceptions |
| Tooling | AST + spec | AST frozen (Phase 0) |
