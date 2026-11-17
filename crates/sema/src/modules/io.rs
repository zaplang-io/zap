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
        // Buffered line-oriented input (#969, D-0212). `reader(fd)` returns an
        // opaque handle (a `bytes` value wrapping a pointer-free reader state,
        // the crypto-streaming pattern); the read methods take that handle.
        // io.reader(fd) -> reader
        "reader" => {
            expect_argc(checker, "io", method, args, 1, span);
            Ty::Bytes
        }
        // io.readline(reader) -> str (line incl. trailing '\n'; "" at EOF)
        "readline" => {
            expect_argc(checker, "io", method, args, 1, span);
            Ty::Str
        }
        // io.readall(reader) -> str (rest of the stream)
        "readall" => {
            expect_argc(checker, "io", method, args, 1, span);
            Ty::Str
        }
        // io.readn(reader, n) -> str (up to n bytes; "" once drained). As with
        // `io.read(fd, n)`, the count flows positionally and coerces to i64.
        "readn" => {
            expect_argc(checker, "io", method, args, 2, span);
            Ty::Str
        }
        // io.lines(reader) -> LineReader (#1031, D-0223). Wrap a buffered reader
        // handle in the prelude `LineReader` so `for line in io.lines(r)` streams
        // lines through the D-0048 iterator protocol. The raw `io.reader` /
        // `io.readline` surface is unchanged.
        "lines" => {
            expect_argc(checker, "io", method, args, 1, span);
            Ty::Named("LineReader".to_string())
        }
        _ => return None,
    })
}
