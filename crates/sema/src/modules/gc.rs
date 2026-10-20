//! `gc` module — sema facet (ticket #77, D-0058).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "gc", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // gc.collect(): force a full collection.
        "collect" => {
            expect_argc(checker, "gc", "collect", args, 0, span);
            Some(Ty::Unit)
        }
        // gc.stats() -> MemStats: heap/live/free bytes, GC count, #goroutines.
        "stats" => {
            expect_argc(checker, "gc", "stats", args, 0, span);
            Some(Ty::Named("MemStats".to_string()))
        }
        _ => None,
    }
}
