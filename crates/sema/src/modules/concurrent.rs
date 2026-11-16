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
//!   lock-free FIFO. `Queue[T]` is **generic**; its underlying constructor is the
//!   bare prelude free function `new_queue[T]()` (the monomorphizer keys on bare
//!   names, D-0078), usable as `Queue[int64] q = new_queue()` (T from the
//!   annotation) or `Queue[str] q = new_queue[str]()` (explicit). The natural
//!   module-qualified spelling `concurrent.Queue[T]()` / `concurrent.Queue()`
//!   also works (I#996, D-0202): it is pure front-end **sugar** that the sema +
//!   mono facets rewrite to `new_queue` (see `zapc_syntax::seam`), so the
//!   monomorphizer still keys only on the bare name. `Queue` therefore reaches
//!   this check facet only as the bare prelude type; it needs no `"Queue"` arm
//!   here (the module facet handles only the non-generic `Counter`).
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
