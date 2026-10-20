//! `net` module — sema facet (ticket #104, D-0056).
//!
//! TCP sockets on the netpoller: a client `connect`, a `listen`/`accept` server
//! loop, and `recv`/`send` over opaque socket handles. A socket handle (listener
//! or connection) is an opaque GC object typed as `bytes` here (so `auto c =
//! net.connect(...)` binds and flows through args/assign without new `Ty`
//! machinery), exactly like the `crypto`/`sync` handle pattern. TLS and UDP are
//! out of scope for v1 (plaintext TCP only); `import net` is accepted but not
//! required.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "net", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // net.listen(host, port) -> listener handle
        "listen" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Bytes
        }
        // net.connect(host, port) -> connection handle
        "connect" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Bytes
        }
        // net.accept(listener) -> connection handle
        "accept" => {
            expect_argc(checker, "net", method, args, 1, span);
            Ty::Bytes
        }
        // net.recv(conn, n) -> str (up to n bytes; empty on EOF)
        "recv" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Str
        }
        // net.send(conn, data) -> int64 (bytes written)
        "send" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Int64
        }
        // net.port(listener) -> int64 (bound local port; resolves an ephemeral 0)
        "port" => {
            expect_argc(checker, "net", method, args, 1, span);
            Ty::Int64
        }
        // `close` is a reserved keyword (channel close); a socket close is spelled
        // `net.close_conn` / `net.close_listener` (both close any socket handle).
        "close_conn" | "close_listener" => {
            expect_argc(checker, "net", method, args, 1, span);
            Ty::Unit
        }
        _ => return None,
    })
}
