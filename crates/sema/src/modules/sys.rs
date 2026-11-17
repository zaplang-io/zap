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
        // Standard streams as objects (#1035, D-0217). `stdin()` is a buffered
        // reader (read with io.readline/readall/readn); `stdout()`/`stderr()`
        // are writer handles (write with `sys.write`). All opaque handles.
        "stdin" | "stdout" | "stderr" => {
            expect_argc(checker, "sys", method, args, 0, span);
            Ty::Bytes
        }
        // sys.write(stream, data) -> int64: bytes written to a writer stream.
        "write" => {
            expect_argc(checker, "sys", method, args, 2, span);
            Ty::Int64
        }
        // CPU-feature introspection (#943, epic #139). Read-only, keyed by stable
        // documented NAMES — the internal bit taxonomy and dispatch stay hidden
        // (#920 c4). `cpu_has(name)` is false for an unknown or arch-foreign name.
        "cpu_has" => {
            expect_argc(checker, "sys", method, args, 1, span);
            Ty::Bool
        }
        "cpu_source" => {
            expect_argc(checker, "sys", method, args, 0, span);
            Ty::Str
        }
        "cpu_sve_vl" => {
            expect_argc(checker, "sys", method, args, 0, span);
            Ty::Int64
        }
        _ => return None,
    })
}
