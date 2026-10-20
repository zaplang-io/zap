//! `sys` module — sema facet (ticket #97, D-0056).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "sys", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        "argv" => {
            expect_argc(checker, "sys", method, args, 0, span);
            Ty::List(Box::new(Ty::Str))
        }
        "exit" => {
            expect_argc(checker, "sys", method, args, 1, span);
            Ty::Unit
        }
        "platform" => {
            expect_argc(checker, "sys", method, args, 0, span);
            Ty::Str
        }
        // Standard stream fds (0/1/2); lowered as constants.
        "stdin_fd" | "stdout_fd" | "stderr_fd" => {
            expect_argc(checker, "sys", method, args, 0, span);
            Ty::Int64
        }
        _ => return None,
    })
}
