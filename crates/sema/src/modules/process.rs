//! `process` module — sema facet (ticket #97, D-0056).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "process",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // process.run(argv: list[str]) -> (exit_code, stdout, stderr).
        "run" => {
            expect_argc(checker, "process", method, args, 1, span);
            Ty::Tuple(vec![Ty::Int64, Ty::Str, Ty::Str])
        }
        _ => return None,
    })
}
