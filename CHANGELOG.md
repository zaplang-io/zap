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

- **`@cache` built-in decorator — memoization over module-level state** (#666,
  D-0115; closes the language work of epic #661, supersedes the D-0109 `@cache`
  deferral). `@cache` on a free function memoizes it by the typed argument tuple,
  generating **ordinary, `zap expand`-visible, type-checked Zap over a
  module-level dict** — never a hidden compiler store. Expansion (before sema, in
  the same pass as user `before`/`after`/`wrap`, so it stacks bottom-up)
  synthesizes a `@derive(Hash, Eq)` key-struct (the argument tuple, as a named
  struct — a bare tuple dict key ICEs, #680), a `@shareable` store struct holding
  a `Mutex` and the memo `dict` (so the shared mutable store does not trip the
  #664 ZAP5125 lint), an immortal module global, and a wrapper that returns a hit
  or computes-and-stores a miss. The miss is computed **outside** the lock, so a
  recursive `@cache` function cannot self-deadlock; the lock guards only the dict,
  like `functools.lru_cache`. v1 is unbounded for the process lifetime (a
  documented memory cost). The key must be **value-hashable** — an integer,
  `bool`, or `str` in v1; a **reference type** (a `struct`/`enum` or a
  collection) is a **compile error (ZAP5056)**, not silently keyed by identity
  (which would miss on equal-but-distinct arguments and give no benefit). An
  un-memoizable signature — a non-value-hashable parameter (reference,
  `float`/`bytes`, tuple, …), a variadic/`**kwargs`, zero-parameter, generic, or
  no-return function, a method, or any `@cache` argument — is a stable
  **ZAP5056** error (never an ICE). The
  sema `@cache` arm no longer rejects with ZAP5055 (that was the deferral); it now
  only catches misapplication to a non-function target.
- **Runtime data-race detector surface: `--race`** — the *dynamic* backstop for
  the light concurrency model (#665, D-0114; locked-design point 6 of epic #661,
  complementing the static sharing lint from #664). A new **`--race`** flag
  (alias **`-race`**) on `run`/`build`/`ir`/`test` enables the runtime data-race
  detector: a **test-time** diagnostic, off by default, that never changes what
  the language accepts. D-0114 settles the design — a **Zap-native happens-before
  / vector-clock** detector rather than LLVM/Clang ThreadSanitizer (which assumes
  pthreads and `malloc`, not Zap's user-space coroutine scheduler and GC heap) —
  and scopes it to the ZAP5125 set (a mutable, non-`@shareable` module global), so
  an `Atomic`/`Mutex`/`@shareable`-guarded version is clean. The feature is being
  delivered in slices: this release lands the design decision, the flag plumbed
  end-to-end, and the racy-vs-clean fixture pair (`tests/race/`) as the detector's
  walled-garden spec; until the engine lands the flag is accepted but inert and
  says so explicitly, so default builds and existing goldens are unaffected. The
  access instrumentation and the happens-before engine are tracked follow-ups
  under #665.
- **Concurrency-safety surface: `@shareable` + sharing lint** — the advisory
  surface for the light concurrency model (#664, D-0113; locked-design points
  4–5 of epic #661, building on the mutable globals from #662). `@shareable` is a
  no-argument marker on a `struct` asserting — as a *trusted, unverified* claim —
  that the type is safe under concurrent shared access; it is reflectable via
  `reflect_has(T, "shareable")`, and the built-in `Atomic`/`Mutex`/`WaitGroup`/
  `Once`/`Counter`/`Queue` types carry it implicitly under one uniform rule (no
  hardcoded allowlist in the lint). The static **sharing lint** raises the
  non-fatal warning **`ZAP5125`** at each read/write of a **mutable** module
  global whose type is not `@shareable`; a `const` global never warns, and a
  mutable global of a `@shareable` (or built-in shareable) type is silent. A
  per-site **`# noqa`** (or `# noqa: ZAP5125`) comment suppresses one site. It is
  advisory — the program stays legal — never a dialect. A new
  **`--warnings-as-errors`** flag (alias **`-Werror`**) on `run`/`build`/`ir`
  makes any surviving warning fatal for CI; off by default.
- **Precise-Immix static roots + write barrier for module globals** (#663,
  D-0112; epic #661). Module-global pointer slots are now correct under the
  **precise Immix** collector, not only conservative Boehm. Every pointer slot is
  registered in the precise collector's static root table, which it walks at each
  GC and processes **move-aware** — marking the referent and rewriting the slot
  in place when the referent is evacuated — so a structure held only via a module
  global is never collected and never left with a stale pointer after relocation.
  Reassigning a **mutable** module-global pointer (`=`, `+=`, chained, tuple-leaf)
  now emits a `zap_gc_write_barrier` on the slot; an immutable reference `const`
  and the one-time initializer store emit none. The barrier is a linkable runtime
  no-op today (the stop-the-world collector re-scans every root each cycle) and is
  the reserved record point for a future generational/incremental mode. Boehm
  behavior is unchanged (the barrier and root registration are exported no-ops;
  the data segment is scanned conservatively).
- **Module-level mutable globals** — persistent, reassignable state at module
  scope (#662, D-0112; Phase 2 of the module-level-persistent-state epic #661,
  building on the `const` form from #658). A `T NAME = expr` (or
  `auto NAME = expr`) at module top level — scalars and reference/heap types
  alike — is evaluated **once before `main`** in the same import-topological +
  source order as `const`, is a module member reachable across modules as
  `module.NAME`, and may then be **reassigned** (`=`, `+=`) and **mutated**
  (container contents) from any function, persisting for the process lifetime.
  There is no `global` keyword: a bare assignment to a name that resolves to a
  module binding reassigns it in place (static lexical scoping, D-0021/D-0090).
  A reference-typed mutable global is a **permanent GC root** (its data-segment
  slot address is registered, so the collector keeps whatever heap object it
  currently holds live — including after reassignment or growth — under both the
  conservative Boehm and precise Immix collectors). Concurrency is the light
  Go/Python model of #661 (unsynchronized, never implicitly locked); the static
  sharing lint and the `@shareable` marker now ship (#664, above), and the
  precise-Immix static roots and write barrier landed in #663 (above), while a
  runtime `-race` detector is later work. A module-level tuple-destructuring
  declaration stays rejected
  (**ZAP3018**); a `const` module binding stays immutable (**ZAP3017**).
- **Module-level `const`** — persistent, immutable state at module scope (#658,
  D-0112; Phase 1 of the module-level-persistent-state epic #661). A
  `const T NAME = expr` (or `const auto NAME = expr`) at module top level, for
  scalars and reference/heap types (`const list[int64] XS = [...]`,
  `const dict[str, int64] TABLE = {...}`), is evaluated **once before `main`** in
  import-topological + source order (reusing the acyclic module graph, D-0081),
  is a module member reachable across modules as `module.NAME`, and is a
  permanent GC root when reference-typed. It is immutable: reassignment is a
  compile error (**ZAP3017**) — which also closes a latent gap by enforcing
  `const` immutability for **local** `const`s. A module-level *mutable* binding
  is reserved for #658 Phase 2 and is rejected with a dedicated **ZAP3018**
  (pointing at the planned support) rather than the blanket top-level-statement
  error. The broader design — mutable module state, a static concurrency-sharing
  lint, and the `@shareable` marker — is epic #661.
- **`as` sub-pattern capture in `match`** — `case <pattern> as name:` (#603,
  D-0111; CPython grammar-parity epic #614, D11). It matches the inner pattern
  and binds `name` to the *whole* matched value, typed as the scrutinee for that
  arm. The inner pattern still destructures, so nested captures bind alongside
  the `as` name (`case [x, y] as pair:` binds `pair`, `x`, `y`;
  `case Point(x=0) as origin:` binds `origin`), and the `as` wraps the entire
  or-pattern (`case 1 | 2 | 3 as n:`). The capture is in scope in the arm body
  and its guard (`case [x, y] as pair if len(pair) == 2:`). `name` must be a
  single plain identifier — not `_`, not dotted — rejected otherwise with the
  dedicated **`ZAP2021`**. Matches CPython's `as`-pattern surface.
- Built-in **`@deprecated`** decorator (#472, D-0109, D-0036). A function or
  method marked `@deprecated` (optionally `@deprecated("use X")`) generates no
  code but raises a non-fatal **use-site warning** (`ZAP5124`) at every call
  site — the mechanism for retiring a Stable surface with notice before removal
  (STABILITY §5). Sema warnings are now surfaced by the build pipeline at a new
  `warning[ZAPnnnn]` severity (non-fatal), which also surfaces the pre-existing
  `Eq`-without-`Hash` dict/set-key warning (D-0047) at the CLI for the first
  time. With this, the v1 built-in decorator set is complete: `@derive(...)`,
  `@default`, `@validate`, `@range`, `@deprecated`.
- Identity operators `is` / `is not` (#611, D-0110, epic #614 D18). `a is b`
  tests **reference identity** — same allocation — distinct from `==` (value /
  `__eq__` equality): two distinct reference structs with equal fields are
  `==`-equal but not `is`-equal. `x is None` / `x is not None` is the
  empty-variant / null check, consistent with `== None` and `match case None`,
  and works for both `Optional[T]` layouts and plain reference structs. `is`
  reserves a new keyword (as in Python) and, like the other comparison
  operators (and unlike `not in`), participates in Python-style comparison
  chaining (`a is b == c`). Operands must be reference types (a plain `struct`,
  a `list`/`dict`/`set`/`heap`, a `channel`, or `Optional[T]`); `is` on a value
  type or scalar is a compile error (ZAP4073), as is `is None` on a never-None
  value (ZAP4074), each pointing the user at `==` — Zap does not define `is` as
  `==` on values (see `PYTHON_DIFFERENCES.md`).
- Generator expressions: `(elem for CLAUSES)` is a lazy `Iterator`, not a
  materialized collection — it yields each element on demand (no intermediate
  list) and can be consumed once. It uses the comprehension clause grammar
  (multi-`for`/`if`), its loop variables do not leak into the enclosing scope,
  and as the sole argument of a call it needs no parentheses of its own
  (`sum(x*x for x in range(5))`; `f((x for x in xs), y)` with other arguments).
  It desugars to an anonymous generator function reusing the `yield` state
  machine, so it integrates with `for`/`sum`/etc. through the one iteration
  protocol. `sum(...)` now also accepts any iterable (generator, genexpr, list),
  not only numeric arrays (#596, D-0107, epic #614 D4).
- User-defined **behavioral decorators**, `before`/`after` form (#652 of epic
  #651, D-0108, D-0036). A `decorator` may declare `before(ctx)` and/or
  `after(ctx, result)` hooks; applying it to a free function generates — at
  compile time, before type-checking, visible via `zap expand` — a wrapper with
  the decorated function's **exact signature preserved** (automatic `@wraps`)
  that runs `before`, calls the original, runs `after` on the return value, and
  returns it. `ctx.name` is the decorated function's name; decorators stack
  bottom-up; decorator arguments (`@tag("x")`) wire into the generated code.
  Ill-formed uses are stable diagnostics (ZAP5049–ZAP5053);
  behavioral decorators on non-function targets report ZAP5050.
- User-defined **behavioral decorators**, general `wrap(call)` form (#653 of epic
  #651, D-0108, D-0036). A `decorator` may declare a `wrap(call)` hook whose body
  is a nested `def inner(*args): … return inner`; applying it generates — at
  compile time, visible via `zap expand` — a wrapper with the decorated
  function's **exact signature** in which the inner body is inlined and
  `call(*args)` becomes a typed call of the renamed original forwarding the
  decorated function's parameters. `*args` is a compile-time forwarding template,
  **not** a runtime varargs value (so it may appear only as `call(*args)`;
  reading it, using `call` as a value, mixing `wrap` with `before`/`after`, or
  omitting `return inner` are ZAP5052/ZAP5054). A declared-return wrapper whose
  body is not provably return-terminating (e.g. `retry`) gets an implicit
  `raise RuntimeError(...)` fall-through guard. `wrap` stacks with other
  decorators bottom-up. (An `args`-keyed `memoize`/`@cache` is not expressible by
  `wrap`: it needs persistent cross-call state, a separate capability.)
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

- Built-in decorators `@cache` and `@inject` are out of the v1 set (#472,
  D-0109). **`@cache` is deferred** — it needs module-level persistent state,
  not yet a language feature (tracked in #658/#661; the built-in in #666).
  **`@inject` is declined for core** — dependency injection is a framework
  concern, provided by reflection-driven `Depends(...)` adapters (D-0089/D-0092),
  not a core decorator. Both names stay reserved and are rejected with a
  dedicated diagnostic (`ZAP5055`) — for `@inject`, a redirect to `Depends(...)`
  — instead of being advertised and then failing at lowering.

### Deprecated

- _Nothing yet._

### Removed

- _Nothing yet._

### Fixed

- Preserve `bytes` result types for `crypto.digest`, `crypto.hmac_sha256_raw`,
  and `crypto.random_bytes` during IR lowering (#647), fixing binary indexing,
  slicing, concatenation, and equality on inferred locals. Hex/text APIs and
  opaque streaming handles retain their existing representation. Centralize
  crypto result typing in one explicit table and share call emission, including
  integer count coercion, RSA exception handling, and boolean normalization.

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
