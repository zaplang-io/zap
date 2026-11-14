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
- Decorators & annotations (D-0036): `@` syntax, annotations + runtime
  reflection, the built-in decorator set (`@derive`/`@validate`/`@cache`/…),
  `before`/`after`/`wrap`, and `zap expand`; user macros staged. `design/0004`.
- Modules, imports, enforced `_` visibility (D-0028).

RSA signing/verification now enforce the ordinary-RSA, PKCS#1 v1.5 contract
(#642, D-0102). Validated key metadata and P-256/Ed25519 primitives remain
separate work (#643); this correction does not complete asymmetric JWT support.

The RSA implementation shares its key-family/padding policy between sign and
verify, with optional DEBUG diagnostics for verifier configuration failures.

Raw digest, HMAC, and random-buffer results now preserve binary `bytes` typing
through IR lowering (#647). Known-answer and boundary tests cover both native
collectors; this correction does not add an algorithm or change the runtime ABI.

## Phase 3 — Signature features (parallel tracks)

Once the IR schema and runtime ABI are frozen, these run concurrently:

- **Regex track** — `m//`, `s//`, `=~`/`!~`, `~=`, match objects (PCRE2). `D-0010`.
- **Arrays track** — `array[T]` 1-D first: element-wise ops + scalar broadcast +
  SIMD lowering (done); then N-D static-rank shape + multi-axis indexing (#500,
  `D-0164`, done), element-wise broadcasting (#501, `D-0165`, done), the full
  dtype matrix + NEP-50 promotion (#503/#890, `D-0166`, done), constructors
  zeros/ones/full/arange/linspace (#504, `D-0167`, done), elementwise math
  intrinsics (#505, `D-0168`, done), true division `/` + non-commutative
  scalar-on-left (#506, `D-0169`, done), axis reductions + min/max/mean/prod
  (#507, `D-0170`, done), strided N-D slice views (#502, `D-0171`, done), matrix
  multiply `@`/`dot` (#509, `D-0172`, done; perf auto-dispatch own-vs-BLAS
  designed, #904/`D-0173`), shape ops reshape/transpose/flatten/concatenate/stack
  (#511, `D-0174`, done), and boolean-mask & fancy (integer-array) indexing +
  `where` (#510, `D-0175`, done) — which completes the #499 feature set. `D-0011`.
- **Concurrency track** — coroutines + channels (unbuffered/buffered, `close`,
  `select`), sync primitives, structured concurrency, DRF-SC memory model.
  `D-0008`, `D-0031`–`D-0034`.
- **Generics track** — user-defined generics via monomorphization, building on
  the already-generic built-in containers. `D-0017`.
- **Capability/bounds track** — unify collection-element typing onto the existing
  structural-interface bounds: adjectival capabilities `Equatable`/`Hashable`/
  `Comparable` (with `Hashable`/`Comparable` refining `Equatable`), conditional
  auto-derivation for value aggregates, and interface refinement `interface X: Y`
  as the one new primitive. `D-0139` (RFC #801, epic #812).
- **Later language features** — data-carrying enums + `match`, default/keyword
  args, `set` type, decorators/annotations (see `OPEN-QUESTIONS.md`).

## Phase 4 — Interop

- **Python runtime interop** — embed CPython, marshal Zap ↔ Python, GIL
  handling, Python-exception ↔ Zap-exception translation. `D-0012`.
- **Package manager** — Cargo/Go-modules-style manifest + lockfile (D-0035).

## Phase 5 — Hardening & tooling

- Precise GC (Immix-style) replacing Boehm. GC north star — concurrent +
  generational "Immix++" with FFI pinning — is `D-0127` /
  [`docs/design/gc-immixpp.md`](docs/design/gc-immixpp.md) (epic #714; Phase-2
  concurrent evacuation deferred to #713).
- N-D array completeness + optimized SIMD kernels.
- Formatter, LSP, package tooling.
- C FFI (`extern "C"`) — generalizes interop to all languages, incl. Go via
  `c-shared`.
- Arena / region allocation — scope-bound, non-moving, escape-checked (FFI
  accelerant, #553). Design is `D-0158` /
  [`docs/design/arena.md`](docs/design/arena.md); leaf-only v1 lands on today's
  Immix as the first consumer of the #714 region-tag model.

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
