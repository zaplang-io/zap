//! Zap syntax crate.
//!
//! Phase 0 scope: the data structures the rest of the compiler is built
//! against — source [`span`]s, lexical [`token`]s, and the [`ast`]. Freezing
//! these lets the lexer, parser and type-checker be built independently
//! (see `ROADMAP.md`). No lexing or parsing *logic* lives here yet.

pub mod ast;
pub mod lexer;
pub mod span;
pub mod token;

/// The language/compiler version these definitions target.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
