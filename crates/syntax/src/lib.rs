//! Zap syntax crate.
//!
//! Phase 0 scope: the data structures the rest of the compiler is built
//! against — source [`span`]s, lexical [`token`]s, and the [`ast`]. Freezing
//! these lets the lexer, parser and type-checker be built independently
//! (see `ROADMAP.md`). No lexing or parsing *logic* lives here yet.

pub mod ast;
pub mod cfg;
pub mod fstring;
pub mod genanalysis;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;
pub mod trivia;

/// The language/compiler version these definitions target.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The `@shareable` concurrency-safety marker (epic #661, #664, D-0113).
///
/// A nominal, metadata-only annotation (D-0036) a type author applies to a
/// `struct` to assert — as a *trusted, unverified* claim — that the type is
/// safe under concurrent shared access. It is reflectable via
/// `reflect_has(T, "shareable")` and is the single predicate the static sharing
/// lint (ZAP5125) consults: there is no hardcoded type allowlist in the lint.
pub const SHAREABLE_MARKER: &str = "shareable";

/// The `@scoped` affine-resource marker (#876, D-0159).
///
/// A nominal annotation (D-0036) a type author applies to a `struct` to make it
/// a *with-bound affine resource*: its values must be consumed exactly once — by
/// a `with` head or by a move into another `@scoped` struct's field — and are
/// torn down deterministically at scope exit, enforced statically. A `@scoped`
/// struct MUST define both `__enter__` and `__exit__` (the context-manager
/// protocol, D-0069). Scopedness is contagious through ownership (a struct owning
/// a `@scoped` field is itself `@scoped`). The first consumer is the #553 arena
/// (D-0158); the full spec is `docs/design/scoped.md`.
pub const SCOPED_MARKER: &str = "scoped";

/// The built-in types that carry the `@shareable` marker implicitly (#664,
/// D-0113): the concurrency primitives and concurrent collections whose whole
/// purpose is safe concurrent shared access. Seeding the lint's one membership
/// test with these — rather than special-casing them inside the lint — is what
/// lets `@shareable` be "one uniform rule" across user and built-in types.
///
/// - `Atomic` — the prelude `Atomic[T]` (I#176, D-0066).
/// - `Mutex`, `WaitGroup`, `Once` — the `sync.*` primitives (D-0063).
/// - `Counter`, `Queue` — the `concurrent.*` lock-free collections (#184,
///   D-0078).
///
/// Shareability is decided on a type's nominal base name, so a generic spelling
/// such as `Atomic[int64]` or `Queue[str]` is covered by its base (`Atomic`,
/// `Queue`).
pub const BUILTIN_SHAREABLE_TYPES: &[&str] = &[
    "Atomic",
    "Mutex",
    "WaitGroup",
    "Once",
    "Counter",
    "Queue",
    "AtomicArray",
    "ConcurrentMap",
    "ConcurrentSet",
    "ConcurrentStack",
];
