//! `tls` module — sema facet (ticket #147, D-0062).
//!
//! A TLS **client** over a `net`-style TCP connection, backed by system OpenSSL
//! (`-lssl -lcrypto`): `connect`/`connect_insecure` perform the TCP connect and
//! the TLS handshake (SNI, TLS 1.2/1.3) and return an opaque TLS connection
//! handle typed as `bytes` here (the `net`/`crypto` handle pattern — no new
//! `Ty`), then `send`/`recv` encrypt/decrypt over it and `close_conn` shuts it
//! down. `connect` verifies the certificate chain against the system trust store
//! AND the hostname by default; `connect_insecure` is the loudly-named opt-out
//! (never the default). `import tls` is accepted but not required.
//!
//! The TLS **server** is deferred (issue #147); `_test_server` is an internal
//! test scaffold (a self-signed loopback echo server) for the golden suite, not
//! stable API.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "tls", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // tls.connect(host, port) -> TLS connection handle (chain + hostname
        // verification against the system trust store ON).
        // tls.connect_insecure(host, port) -> same, verification OFF (opt-in).
        "connect" | "connect_insecure" => {
            expect_argc(checker, "tls", method, args, 2, span);
            Ty::Bytes
        }
        // tls.recv(conn, n) -> str (up to n decrypted bytes; empty on clean EOF)
        "recv" => {
            expect_argc(checker, "tls", method, args, 2, span);
            Ty::Str
        }
        // tls.send(conn, data) -> int64 (plaintext bytes written)
        "send" => {
            expect_argc(checker, "tls", method, args, 2, span);
            Ty::Int64
        }
        // `close` is the reserved channel-close token, so a TLS close is spelled
        // `tls.close_conn` (mirrors `net.close_conn`).
        "close_conn" => {
            expect_argc(checker, "tls", method, args, 1, span);
            Ty::Unit
        }
        // Internal test scaffold (deferred public server): start a background
        // self-signed TLS echo server and return its ephemeral port.
        "_test_server" => {
            expect_argc(checker, "tls", method, args, 0, span);
            Ty::Int64
        }
        _ => return None,
    })
}
