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

    /// `true` when this is the canonical out-of-fragment error.
    pub fn is_not_available(&self) -> bool {
        self.kind == CtErrorKind::NotAvailable
    }
}
