//! `fs` module — sema facet (ticket #97, D-0056).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "fs", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        "read_file" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Str
        }
        "write_file" => {
            expect_argc(checker, "fs", method, args, 2, span);
            Ty::Unit
        }
        "exists" | "is_dir" | "is_file" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Bool
        }
        "size" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::Int64
        }
        "listdir" => {
            expect_argc(checker, "fs", method, args, 1, span);
            Ty::List(Box::new(Ty::Str))
        }
        "mkdtemp" => {
            expect_argc(checker, "fs", method, args, 0, span);
            Ty::Str
        }
        _ => return None,
    })
}
