//! `io` module — sema facet (ticket #97, D-0056). Files are raw int64 fds.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "io", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // io.open(path, mode) -> fd
        "open" => {
            expect_argc(checker, "io", method, args, 2, span);
            Ty::Int64
        }
        // io.read(fd, n) -> str
        "read" => {
            expect_argc(checker, "io", method, args, 2, span);
            Ty::Str
        }
        // io.write(fd, data) -> int64 (bytes written)
        "write" => {
            expect_argc(checker, "io", method, args, 2, span);
            Ty::Int64
        }
        // `close` is a reserved keyword (channel close), so the fd close is
        // spelled `io.close_fd`.
        "close_fd" => {
            expect_argc(checker, "io", method, args, 1, span);
            Ty::Unit
        }
        _ => return None,
    })
}
