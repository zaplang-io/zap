//! `os` module — sema facet (ticket #97, D-0056).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "os", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // environment
        "getenv" => {
            expect_argc(checker, "os", method, args, 1, span);
            Ty::Str
        }
        "hasenv" => {
            expect_argc(checker, "os", method, args, 1, span);
            Ty::Bool
        }
        "setenv" => {
            expect_argc(checker, "os", method, args, 2, span);
            Ty::Unit
        }
        "getpid" => {
            expect_argc(checker, "os", method, args, 0, span);
            Ty::Int64
        }
        // working directory & entries
        "getcwd" => {
            expect_argc(checker, "os", method, args, 0, span);
            Ty::Str
        }
        "chdir" | "mkdir" | "remove" | "rmdir" => {
            expect_argc(checker, "os", method, args, 1, span);
            Ty::Unit
        }
        "listdir" => {
            expect_argc(checker, "os", method, args, 1, span);
            Ty::List(Box::new(Ty::Str))
        }
        // path helpers (flat names: `os.path_join`, …)
        "path_join" => {
            expect_argc(checker, "os", method, args, 2, span);
            Ty::Str
        }
        "path_exists" => {
            expect_argc(checker, "os", method, args, 1, span);
            Ty::Bool
        }
        "path_basename" | "path_dirname" => {
            expect_argc(checker, "os", method, args, 1, span);
            Ty::Str
        }
        _ => return None,
    })
}
