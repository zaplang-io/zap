# Changelog

All notable changes to the **Zap toolchain** (compiler, runtime, bundled
standard library, CLI, and package tooling — one artifact, one version) are
recorded in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project versions by [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html)
as defined in [`docs/STABILITY.md`](docs/STABILITY.md) (D-0070). How entries are
written and how a release is cut is described in
[`docs/RELEASING.md`](docs/RELEASING.md).

> **Pre-1.0.** Zap is in the `0.x` regime: **no compatibility is promised** and
> any release may change anything (STABILITY §2). The `1.0.0` promise and the
> frozen Stable set (D-0076) take effect only when 1.0 is actually cut; the
> version stays editorial until then. No release has been tagged yet, so
> everything built to date sits under **[Unreleased]** and will roll into the
> first tagged version when it is cut (see `docs/RELEASING.md`).

## [Unreleased]

The initial pre-1.0 toolchain. Summarised by surface rather than listed
commit-by-commit; the authoritative per-feature record is `DECISIONS.md`
(D-0001…D-0079) and the design docs under `design/`.

### Added

- Transparent type aliases: `type Name = <type>`, plus the generic form
  `type Name[T] = <type>` (#606, D-0105). An alias is not a new nominal type — it
  resolves to its target with the same identity, so an alias and the spelled-out
  type are interchangeable in signatures and collections (`list[Row]`,
  `dict[str, Row]`). Generic aliases are expanded during monomorphization
  alongside the existing generics (`Pair[int64]` → `(int64, int64)`). `type` is a
  soft keyword recognized only at item start, so it stays a usable identifier
  elsewhere. A cyclic alias (`type A = B; type B = A`) is rejected (`ZAP4090`).
- Positional-only parameters via a bare `/` separator: parameters before `/`
  may only be supplied positionally, never by name (the mirror of the keyword-only
  `*` separator). `/` appears at most once and must precede any `*`; passing a
  positional-only parameter by name is `ZAP4034`, a misplaced `/` is `ZAP2019`.
  Enforced front-end only — no IR/codegen change (#602, D-0104, epic #614 D10).
- Walrus assignment-expression `(name := expr)`: evaluates `expr` once, binds
  `name` by inference in the lexically enclosing scope, and yields the value, so
  the name outlives the construct — usable in `if`/`while` conditions and
  comprehension `if` filters (`if (n := len(xs)) > 10:`, `[y for x in xs if
  (y := f(x)) > 0]`; a comprehension target binds in the enclosing scope per PEP
  572). It is parenthesized-only and deliberately not a declaration shorthand: a
  bare `a := 10` statement is rejected (`ZAP2020`, steering to `auto a = 10`)
  (#600, D-0106, epic #614 D8).

- Array length mismatches raise `ValueError` before SIMD kernels run; the
  Experimental array contract records supported 1-D dtypes, construction,
  arithmetic and contiguous views. Unsupported array constructors and native
  floating-point collection slots have semantic diagnostics (#476).

- `scope(timeout=<ms>)` deadlines (integer milliseconds, no unit suffix): on
  expiry the scope cancels and awaits its children through the shared channel-aware
  cancellation path (preserving `finally`/`with` cleanup), then raises the new
  `ScopeTimeout` exception to the caller. `ScopeTimeout` reports the deadline and
  every coroutine still outstanding at it (by name and park reason), never a sample.
  `timeout`, `on_error`, and `cancel_on` are orthogonal; `grace` still bounds an
  uncooperative drain. Nested joins park their coroutine (#475, D-0094).

- Scope cleanup cancels and awaits children on return/break/continue; unsupported
  spawn call forms now report `ZAP5076` during semantic checking (#475). The
  concurrency contract documents supported scope options without a timeout API.

- Structural `match` lowering for tuples and lists with rest captures, recursive
  enum/struct payload tests and bindings, and guard fallthrough (#474).

- **Language.** Type-first declarations and `const` (D-0003); scalars and
  conversions, including arbitrary-precision `bigint` and native `complex`
  arithmetic (D-0019); `str`/`bytes` with interpolation, codepoint indexing/slicing/
  iteration, comparison, substring `in`, the core method set
  (`upper`/`lower`/`strip`/`split`/`join`/`find`/`index`/`replace`/
  `startswith`/`endswith`/`contains`/`encode`) and `ord`/`chr`/`char`/`str`/
  `int` conversions (#284);
  `list`/`dict`/tuples/`set` with value/reference semantics; structs, methods
  and `value struct` (D-0051); interfaces; enums — tier-1 values with
  canonical `.name` and source-order iteration, plus tier-3 sum types
  (D-0018/D-0046); operators with overloading and `@derive(Eq, Ord, Hash, Repr)`;
  control flow including `match` and the iteration protocol (D-0048);
  functions, lambdas, generators, default/keyword/variadic args, comprehensions
  and slicing; first-class function values and closures (D-0067); typed built-in
  exceptions with source-order `except` dispatch and subclass matching; `with`/
  context managers (D-0069); modules, imports and visibility; regex
  (D-0010); decorators and annotations (D-0036); generics with bounds (D-0064);
  coroutines, channels, `select`, `scope:` and `spawn detached`
  (D-0031/0044/0045); sync primitives and `Atomic[T]` (D-0032/0066);
  `Optional[T]` (D-0065); numeric arrays / SIMD with dtype promotion (D-0011);
  C-style bitfields (D-0068); Python interop; `@derive(Json/Yaml/Csv)`; and
  `when target.*` compile-time conditional compilation (D-0079, Experimental).
- **Standard library.** `io`, `fs`, `os`, `sys`, `process`, `math`, `random`,
  `time`, `datetime` (incl. `strftime` / 2-arg `format` custom date formatting,
  #286); `collections` extras (`deque`, `bisect`, `oset`), `sync`,
  `json`, `logging`, `argparse`; the `signal`/`gc`/`runtime` APIs; and the
  Experimental `crypto`, `net`, `http`, `tls`, `zlib`, `gzip`, `yaml`,
  `xml` — a core DOM parser + deterministic serializer for the AWS rest-xml /
  query protocols (D-0085, #351);
  migration-only `threading`, and `concurrent` lock-free `Counter` + MPSC
  `Queue[T]` (D-0078).
- **Runtime & GC.** Boehm and precise moving-Immix collectors with finalizers
  and evacuation (D-0009); the coroutine scheduler with preemption; the
  netpoller (epoll / IOCP); signal-triggered preemption and runtime diagnostics.
- **Tooling.** The `zap` CLI (`run`, `build`, `test`, `fmt`, `new`, `init`,
  `add`, `remove`, `zenv`, `explain`, `doc`, `version`, `help`); `zap fmt`
  canonical formatter (D-0037); doctests via `zap test --doc` (D-0074); the
  `ZAPnnnn` diagnostic scheme with `zap explain` (D-0071); the LSP server and
  VS Code extension; the in-process-JIT REPL (D-0073); the web playground
  (D-0075); the MCP server; and the package manager with `zenv`, MVS resolver,
  lockfile, checksum DB and read-only registry index (D-0052/D-0027).
- **Platform.** Linux x86-64 and Windows x86-64 (tier-1); macOS x86-64
  (tier-2, CI runner pending); aarch64-Linux and macOS arm64 (experimental).
  See the ratified matrix in STABILITY §7.3.
- **Process & docs.** `SPEC.md`, `docs/GRAMMAR.ebnf`, the `DECISIONS.md` log,
  `docs/STABILITY.md` versioning & stability policy (D-0070/D-0071) with the
  D-0076 1.0 spec-freeze, and this changelog plus the release process
  (`docs/RELEASING.md`, I#207).

### Changed

- _Nothing yet relative to a released version._

### Deprecated

- _Nothing yet._

### Removed

- _Nothing yet._

### Fixed

- _Pre-release bug fixes are folded into the initial release; post-1.0, defect
  fixes that affect real code are called out here (STABILITY §3.2)._

### Security

- Bind RSA-named signing and verification to ordinary RSA keys and explicit
  PKCS#1 v1.5 padding (#642). Previously generic EVP dispatch could sign with
  an EC/DSA key or verify a foreign-family signature. Wrong-family signing
  raises `ValueError`; verification returns `false`, including RSA-PSS keys.
  Emit sign calls through the throwing-call path so local handlers catch
  parsing, key-family, and signing errors.

- Preserve the RSA sign result as `bytes` during IR lowering (#644), fixing
  byte operations on inferred signature locals without changing the runtime ABI.
- Share RSA key-family/padding enforcement between signing and verification;
  emit DEBUG diagnostics for verifier key/configuration errors through the
  existing logging configuration, without including cryptographic inputs.

[Unreleased]: https://github.com/zaplang-io/zap/commits/main
