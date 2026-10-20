//! `runtime` module — sema facet (ticket #77, D-0058).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "runtime",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // runtime.goroutine_dump(): dump goroutine stacks + scheduler summary.
        "goroutine_dump" => {
            expect_argc(checker, "runtime", "goroutine_dump", args, 0, span);
            Some(Ty::Unit)
        }
        // runtime.num_goroutines() -> int64.
        "num_goroutines" => {
            expect_argc(checker, "runtime", "num_goroutines", args, 0, span);
            Some(Ty::Int64)
        }
        _ => None,
    }
}
