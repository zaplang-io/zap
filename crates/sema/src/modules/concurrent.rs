//! `concurrent` module — sema facet (ticket #184, D-0078).
//!
//! Experimental lock-free collections built on the prelude `Atomic[T]`
//! (D-0066). This phase ships exactly two surfaces:
//!
//! - `concurrent.Counter()` / `concurrent.Counter(start)` — a lock-free counter
//!   (a real type, over `Atomic[int64]`). It returns the prelude `struct Counter`
//!   (`crates/syntax/src/parser.rs`), exactly the `threading.Thread()` /
//!   `threading.Lock()` pattern: a module constructor over a compiler-known
//!   prelude type.
//! - the MPSC `Queue[T]` — an unbounded multi-producer / single-consumer
//!   lock-free FIFO. Because `Queue[T]` is **generic**, it cannot ride a
//!   module-qualified constructor (a module call carries no type argument and the
//!   monomorphizer keys on bare names), so — like every other generic prelude
//!   type (`Atomic[T]`, `Optional[T]`) — it is written **bare** and built with
//!   the prelude free function `new_queue[T]()`, as either
//!   `Queue[int64] q = new_queue()` (T inferred from the annotation) or
//!   `Queue[str] q = new_queue[str]()` (explicit).
//!   This keeps the whole surface within the existing grammar (no new syntax,
//!   per D-0078) — the `concurrent.Queue[...]` dotted spelling in the design note
//!   is illustrative; module-qualified *generic type paths* do not exist in the
//!   grammar and adding them was explicitly out of scope.
//!
//! Reclamation leans entirely on the tracing moving Immix collector; the types
//! are pure Zap over the `Atomic[T]` intrinsics and add **no** `zaprt.c` / `zapgc`
//! / runtime-C code (the central finding of the design, §4).

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "concurrent",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // concurrent.Counter() / concurrent.Counter(start): a lock-free counter
        // over Atomic[int64]. `start` (the initial value) is optional and
        // defaults to 0.
        "Counter" => {
            if args.len() > 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "concurrent.Counter() takes 0 or 1 argument(s), found {}",
                        args.len()
                    ),
                    span,
                );
            }
            for a in args {
                checker.infer(&a.value);
            }
            Some(Ty::Named("Counter".to_string()))
        }
        _ => None,
    }
}
