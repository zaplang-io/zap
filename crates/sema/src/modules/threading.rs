//! `threading` module — sema facet (ticket #175).
//!
//! Python-`threading`-shaped OS-thread compatibility: a **migration escape
//! hatch**, explicitly not recommended (idiomatic Zap concurrency is coroutines
//! and channels). `threading.Thread(target=...)` returns the prelude `struct
//! Thread` (`crates/syntax/src/parser.rs`), whose methods (`start`, `join`,
//! `join_timeout`, `is_alive`) delegate to the flat `threading._*` runtime entry
//! points typed below. The thread runs its target on a dedicated OS thread
//! beyond `MAXPROCS` by pinning its coroutine via the #172 LockOSThread
//! substrate, so it still participates in precise-GC stop-the-world and the #120
//! moving collector through the ordinary shadow-stack roots.
//!
//! The `Lock`/`RLock`/`Event`/`Condition`/`Semaphore` primitives are shared with
//! the `sync` module (implemented once in the runtime, exposed on both
//! surfaces); their Python-shaped wrapper constructors are typed here.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "threading",
    check,
};

/// Infer every argument (so each is still type-checked) and return the given
/// result type, requiring the argument count to fall within `[min, max]`.
fn infer_all(
    checker: &mut Checker,
    method: &str,
    args: &[Arg],
    min: usize,
    max: usize,
    span: Span,
) {
    if args.len() < min || args.len() > max {
        let want = if min == max {
            format!("{min}")
        } else {
            format!("{min}..{max}")
        };
        checker.error(
            format!(
                "threading.{method}() takes {want} argument(s), found {}",
                args.len()
            ),
            span,
        );
    }
    for a in args {
        checker.infer(&a.value);
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // --- Python-shaped wrapper constructors --------------------------
        // threading.Thread(target=fn(), daemon=bool): a Thread over a dedicated
        // OS thread. `daemon` is optional (defaults to non-daemon).
        "Thread" => {
            infer_all(checker, method, args, 1, 2, span);
            Some(Ty::Named("Thread".to_string()))
        }
        // threading.Lock()/RLock()/Event()/Condition(): no-argument primitives.
        "Lock" => {
            expect_argc(checker, "threading", method, args, 0, span);
            Some(Ty::Named("Lock".to_string()))
        }
        "RLock" => {
            expect_argc(checker, "threading", method, args, 0, span);
            Some(Ty::Named("RLock".to_string()))
        }
        "Event" => {
            expect_argc(checker, "threading", method, args, 0, span);
            Some(Ty::Named("Event".to_string()))
        }
        "Condition" => {
            expect_argc(checker, "threading", method, args, 0, span);
            Some(Ty::Named("Condition".to_string()))
        }
        // threading.Semaphore(value): a counting semaphore (value defaults to 1).
        "Semaphore" => {
            infer_all(checker, method, args, 0, 1, span);
            Some(Ty::Named("Semaphore".to_string()))
        }

        // --- Flat runtime entry points (used by the prelude wrappers) ----
        // Thread lifecycle.
        "_pin" | "_unpin" => {
            expect_argc(checker, "threading", method, args, 0, span);
            Some(Ty::Unit)
        }
        "_register" | "_finish" => {
            infer_all(checker, method, args, 2, 2, span);
            Some(Ty::Unit)
        }
        "_join" => {
            infer_all(checker, method, args, 1, 1, span);
            Some(Ty::Unit)
        }
        "_join_timeout" => {
            infer_all(checker, method, args, 2, 2, span);
            Some(Ty::Bool)
        }
        "_is_alive" => {
            infer_all(checker, method, args, 1, 1, span);
            Some(Ty::Bool)
        }
        _ => None,
    }
}
