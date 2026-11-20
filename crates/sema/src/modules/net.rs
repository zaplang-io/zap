//! `net` module — sema facet (ticket #104, D-0056).
//!
//! TCP sockets on the netpoller: a client `connect`, a `listen`/`accept` server
//! loop, and `recv`/`send` over opaque socket handles. A socket handle (listener
//! or connection) is an opaque GC object typed as `bytes` here (so `auto c =
//! net.connect(...)` binds and flows through args/assign without new `Ty`
//! machinery), exactly like the `crypto`/`sync` handle pattern. TLS lives in the
//! `tls` module; UDP datagram sockets (`udp_socket`/`udp_bind`/`udp_connect`/
//! `sendto`/`recvfrom`) were added in #991, the DNS resolution surface
//! (`lookup_host`/`lookup_ipv4`/`lookup_ipv6`/`lookup_addr`, returning
//! `list[str]`) in #992, and Unix domain sockets (`listen_unix`/`connect_unix`
//! stream + `unixgram_socket`/`unixgram_bind`/`sendto_unix`/`recvfrom_unix`
//! datagram) in #993 (epic #985). `import net` is accepted but not required.

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
        "connect_ip_deadline" => {
            expect_argc(checker, "net", method, args, 3, span);
            Ty::Bytes
        }
        "set_deadline_ns" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Unit
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
        // net.set_read_timeout(conn, ms) -> unit. Bounds each read park by `ms`
        // (0 = no timeout, the default); a `recv`/`read_request` that exceeds it
        // raises TimeoutError. Negative `ms` is a ValueError at runtime.
        "set_read_timeout" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Unit
        }
        // net.set_read_deadline(conn, ms) -> unit. Bounds the WHOLE read operation
        // (a `recv`, or an entire `http.read_request`) by `ms` regardless of byte
        // pacing (0 = none, the default), so a slow-dribble peer that keeps
        // resetting the per-park `set_read_timeout` clock is still cut off with a
        // TimeoutError. Composes with `set_read_timeout`. Negative `ms` is a
        // ValueError at runtime.
        "set_read_deadline" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Unit
        }
        // `close` is a reserved keyword (channel close); a socket close is spelled
        // `net.close_conn` / `net.close_listener` (both close any socket handle).
        "close_conn" | "close_listener" => {
            expect_argc(checker, "net", method, args, 1, span);
            Ty::Unit
        }

        // ---- UDP datagram sockets (ticket #991, epic #985) ----
        // net.udp_socket() -> sock handle (AF_INET). net.udp_socket6() -> AF_INET6.
        // An unbound datagram socket; typed `bytes` like every other net handle.
        "udp_socket" | "udp_socket6" => {
            expect_argc(checker, "net", method, args, 0, span);
            Ty::Bytes
        }
        // net.udp_bind(sock, host, port) -> unit. Bind a datagram socket to a
        // local address (port 0 = OS-assigned ephemeral, read back via net.port).
        "udp_bind" => {
            expect_argc(checker, "net", method, args, 3, span);
            Ty::Unit
        }
        // net.udp_connect(sock, host, port) -> unit. Record the default peer so
        // net.send / net.recv work on the datagram socket (connected UDP).
        "udp_connect" => {
            expect_argc(checker, "net", method, args, 3, span);
            Ty::Unit
        }
        // net.sendto(sock, data, host, port) -> int64 (bytes sent in the one
        // datagram). The connectionless counterpart of net.send.
        "sendto" => {
            expect_argc(checker, "net", method, args, 4, span);
            Ty::Int64
        }
        // net.recvfrom(sock, n) -> (data, host, port): the datagram payload (up
        // to `n` bytes), the numeric sender IP, and the sender port. The
        // connectionless counterpart of net.recv.
        "recvfrom" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Tuple(vec![Ty::Str, Ty::Str, Ty::Int64])
        }

        // ---- DNS resolution surface (ticket #992, epic #985) ----
        // net.lookup_host(name) -> list[str]: numeric IPs (A + AAAA).
        // net.lookup_ipv4 / lookup_ipv6 restrict to one family.
        "lookup_host" | "lookup_ipv4" | "lookup_ipv6" => {
            expect_argc(checker, "net", method, args, 1, span);
            Ty::List(Box::new(Ty::Str))
        }
        // net.lookup_addr(ip) -> list[str]: reverse (PTR) hostname(s) of a
        // numeric IP; empty when the address has no reverse record.
        "lookup_addr" => {
            expect_argc(checker, "net", method, args, 1, span);
            Ty::List(Box::new(Ty::Str))
        }

        // ---- Unix domain sockets (ticket #993, epic #985) ----
        // net.listen_unix(path) -> listener handle; net.connect_unix(path) ->
        // connection handle. Both reuse net.accept/recv/send/close_* (opaque
        // `bytes` handle, as the TCP side).
        "listen_unix" | "connect_unix" => {
            expect_argc(checker, "net", method, args, 1, span);
            Ty::Bytes
        }
        // net.unixgram_socket() -> datagram socket handle (unbound AF_UNIX).
        "unixgram_socket" => {
            expect_argc(checker, "net", method, args, 0, span);
            Ty::Bytes
        }
        // net.unixgram_bind(sock, path) -> unit.
        "unixgram_bind" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Unit
        }
        // net.sendto_unix(sock, data, path) -> int64 (bytes sent in the one
        // datagram): the AF_UNIX counterpart of net.sendto.
        "sendto_unix" => {
            expect_argc(checker, "net", method, args, 3, span);
            Ty::Int64
        }
        // net.recvfrom_unix(sock, n) -> (data, path): the datagram payload and
        // the sender's bound path ("" for an unbound sender).
        "recvfrom_unix" => {
            expect_argc(checker, "net", method, args, 2, span);
            Ty::Tuple(vec![Ty::Str, Ty::Str])
        }
        _ => return None,
    })
}
