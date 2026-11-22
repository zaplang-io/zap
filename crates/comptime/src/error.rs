//! Comptime evaluation errors (#1250).
//!
//! Every failure is a span-attributed [`CtError`], never a panic — the
//! evaluator is the physical soundness backstop (epic #1249, locked decision
//! 2b), so an unsupported construct, an impure leaf, recursion, or a genuine
//! value fault (division by zero, out-of-range index) all surface as a clean
//! compile error that a later phase (#1256) renders with a dual-span derive
//! backtrace.
//!
//! The canonical out-of-fragment message contains the exact phrase
//! **`not available at comptime`** (see [`CtError::not_available`]); the static
//! early-reject lint (#1254) shares the same wording so the two layers read
//! identically.
//!
//! One failure mode is *not* a fault in the fragment: a library generator may
//! **deliberately** abort compilation with [`CtError::user`] — the engine
//! primitive behind `comptime.error(at, msg)` (#1256, the Zig `@compileError`
//! model). Its message is the author's own text and it is rendered as a clean,
//! span-attributed diagnostic (`ZAP7002`), optionally wrapped by the derive
//! driver in a consumer-grade "cannot derive X for T" backtrace (see
//! [`crate::diagnostics`]).

use zapc_syntax::span::Span;

/// Why a comptime evaluation failed, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtError {
    /// The human-readable message. For an out-of-fragment construct it ends in
    /// `… is not available at comptime`.
    pub message: String,
    /// The source span the error anchors to.
    pub span: Span,
    /// A coarse classification, for callers that branch on the failure mode
    /// without parsing `message`.
    pub kind: CtErrorKind,
}

/// The coarse category of a [`CtError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtErrorKind {
    /// A construct outside the comptime fragment — an impure leaf (I/O, FFI,
    /// spawn, channel, time, RNG, global mutation, dict/set iteration),
    /// recursion, `match`-over-comptime-values, a float/bigint/bytes value, or
    /// any other expression the fragment does not admit. The message contains
    /// `not available at comptime`.
    NotAvailable,
    /// A genuine value fault during evaluation that is *not* an
    /// out-of-fragment construct: division or modulo by zero, a zero slice
    /// step, an out-of-range index, an invalid codepoint, or a type mismatch
    /// in an operator (which a well-typed program never reaches, but the
    /// untyped evaluator guards anyway).
    Value,
    /// A resource cap was exceeded — the fixed comptime step budget or the
    /// recursion/call-depth limit (epic #1249, HQ5). A hard limit with a clear
    /// diagnostic, never a tunable quota.
    Budget,
    /// A **library-authored** abort via `comptime.error(at, msg)` (#1256): a
    /// generator deliberately stopped compilation with its own message and span
    /// (the Zig `@compileError` model). Not a fault in the fragment — the
    /// comptime code ran correctly and *chose* to fail — so it renders as the
    /// author's text (`ZAP7002`), never the "not available at comptime" wording.
    UserError,
}

impl CtError {
    /// An out-of-fragment construct: `<what> is not available at comptime`.
    /// `what` names the construct (`` floating-point value ``,
    /// `` `match` ``, `` recursion (call to `fact`) ``, …).
    pub fn not_available(what: impl Into<String>, span: Span) -> CtError {
        CtError {
            message: format!("{} is not available at comptime", what.into()),
            span,
            kind: CtErrorKind::NotAvailable,
        }
    }

    /// A genuine value fault (division by zero, out-of-range index, …).
    pub fn value(message: impl Into<String>, span: Span) -> CtError {
        CtError {
            message: message.into(),
            span,
            kind: CtErrorKind::Value,
        }
    }

    /// A resource-cap breach (step budget / recursion depth).
    pub fn budget(message: impl Into<String>, span: Span) -> CtError {
        CtError {
            message: message.into(),
            span,
            kind: CtErrorKind::Budget,
        }
    }

    /// A library-authored abort: `comptime.error(at, msg)` (#1256). `message` is
    /// the author's own text and `span` is the declaration it chose to point at.
    pub fn user(message: impl Into<String>, span: Span) -> CtError {
        CtError {
            message: message.into(),
            span,
            kind: CtErrorKind::UserError,
        }
    }

    /// `true` when this is the canonical out-of-fragment error.
    pub fn is_not_available(&self) -> bool {
        self.kind == CtErrorKind::NotAvailable
    }

    /// `true` when this is a library-authored `comptime.error(...)` abort.
    pub fn is_user(&self) -> bool {
        self.kind == CtErrorKind::UserError
    }
}
