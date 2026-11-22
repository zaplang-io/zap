//! The comptime **leaf table** (#1254): the single closed set of impure
//! operations the comptime fragment refuses, shared by the static reject-lint
//! ([`crate::lint`]) and the bounded interpreter ([`crate::eval`]).
//!
//! The two layers consult this one table, so they can never disagree about
//! membership (the issue's shared-table invariant). Each entry is a
//! **runtime-observable effect** a pure, deterministic comptime evaluation may
//! not perform (epic #1249, locked decision 3):
//!
//! - **I/O / FFI** — files, console, network, process, OS.
//! - **spawn / channel** — concurrency.
//! - **time / RNG** — nondeterministic sources.
//! - **global mutation** — observable mutable state.
//! - **`Nondet`** — `dict`/`set` iteration, whose order is unspecified and
//!   seed-dependent (#1066), so iterating one is nondeterministic even though
//!   building one is not.
//!
//! **Allocation is deliberately NOT a leaf** — comptime legitimately builds
//! strings and lists. Only runtime-observable-identity effects are tagged.
//!
//! The exact membership is a v1 closed set, intentionally tunable (#1249); the
//! lint is best-effort (it only ever *rejects*, moving common errors to the
//! definition site) while the interpreter is the soundness authority.

use zapc_syntax::ast::Expr;

/// The kind of impure leaf an operation is — the closed set the comptime
/// fragment refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeafKind {
    /// File / console / network / OS / process I/O.
    Io,
    /// A foreign-function / C-ABI call.
    Ffi,
    /// `spawn` — starting a coroutine.
    Spawn,
    /// A channel send/receive/select.
    Channel,
    /// A clock / time source.
    Time,
    /// A random-number source.
    Rng,
    /// Mutation of observable global state.
    GlobalMutation,
    /// `dict`/`set` iteration — unspecified, seed-dependent order (#1066).
    Nondet,
}

impl LeafKind {
    /// A short lowercase label for diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            LeafKind::Io => "I/O",
            LeafKind::Ffi => "FFI",
            LeafKind::Spawn => "spawn",
            LeafKind::Channel => "channel",
            LeafKind::Time => "time",
            LeafKind::Rng => "RNG",
            LeafKind::GlobalMutation => "global mutation",
            LeafKind::Nondet => "dict/set iteration (nondeterministic order)",
        }
    }
}

/// Bare builtin names that are impure leaves.
fn classify_bare(name: &str) -> Option<LeafKind> {
    match name {
        "print" | "input" => Some(LeafKind::Io),
        _ => None,
    }
}

/// Classify a stdlib module whose functions are impure leaves. A function from
/// one of these modules (`<module>.<fn>(...)`) is tagged by the module's effect
/// — the "may not even mention I/O" contract is per-module (best-effort v1).
fn classify_module(module: &str) -> Option<LeafKind> {
    match module {
        // I/O-ish: files, console, network, process, OS, logging, TLS, signals.
        "os" | "io" | "fs" | "net" | "http" | "process" | "sys" | "tls" | "logging" | "signal"
        | "gc" => Some(LeafKind::Io),
        // Clocks.
        "time" => Some(LeafKind::Time),
        // Random sources (uuid draws on the RNG).
        "random" | "uuid" => Some(LeafKind::Rng),
        // Concurrency surfaces.
        "threading" | "concurrent" | "sync" => Some(LeafKind::Channel),
        _ => None,
    }
}

/// Classify a *call* by its callee: a bare impure builtin (`print(...)`), or an
/// impure stdlib module function (`time.now()`, `random.rand()`). Returns `None`
/// for a pure or unknown callee — the lint then **defers** to the interpreter
/// (locked decision 2a), never rejecting a callee it cannot resolve.
pub fn classify_call(callee: &Expr) -> Option<LeafKind> {
    match callee {
        Expr::Name(name, _) => classify_bare(name),
        Expr::Attr { base, .. } => match &**base {
            Expr::Name(module, _) => classify_module(module),
            _ => None,
        },
        _ => None,
    }
}

/// Every leaf kind, for the shared-table invariant checks.
pub const ALL_LEAF_KINDS: &[LeafKind] = &[
    LeafKind::Io,
    LeafKind::Ffi,
    LeafKind::Spawn,
    LeafKind::Channel,
    LeafKind::Time,
    LeafKind::Rng,
    LeafKind::GlobalMutation,
    LeafKind::Nondet,
];
