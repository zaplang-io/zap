//! `tls` module — sema facet (tickets #147, #488; D-0062, D-0093).
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
//! The TLS **server** (#488, D-0093) mirrors the `net` server shape:
//! `listen(host, port, cert_path, key_path)` loads a cert+key from disk into a
//! server context and binds, `accept(listener)` runs the server-side handshake
//! and yields a conn (the SAME opaque `bytes` handle as the client, so
//! `send`/`recv`/`close_conn` are shared), `port(listener)` reports the bound
//! port, and `close_listener(listener)` closes the listener — which makes a
//! parked `accept` raise, as `net.close_listener` does for `net.accept`.
//! `_test_server` is an internal test scaffold (a self-signed loopback echo
//! server) for the golden suite, not stable API.
//!
//! TLS **client contexts** (#560, D-0143) mirror Python's `ssl` module for
//! per-connection trust configuration: `create_default_context()` (system roots
//! preloaded) and `SSLContext()` (empty trust store) build an opaque context
//! handle (also `bytes`), `load_verify_locations(ctx, cafile[, capath])` adds CA
//! trust additively and `load_cert_chain(ctx, certfile[, keyfile])` adds a client
//! cert/key for mutual TLS, and `connect(host, port, ctx)` (the optional trailing
//! context argument) dials with that context. Each context owns its own
//! `SSL_CTX`, so concurrent dials with different private CAs never share trust.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use zapc_diag::ErrorCode;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "tls", check };

/// Accept any argument count in `lo..=hi`, erroring with ZAP4025 otherwise, and
/// still infer every argument so later diagnostics fire. Mirrors `expect_argc`
/// for the `tls` surface methods that have an optional trailing argument.
fn expect_argc_range(
    checker: &mut Checker,
    method: &str,
    args: &[Arg],
    lo: usize,
    hi: usize,
    span: Span,
) {
    if args.len() < lo || args.len() > hi {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "tls.{method}() takes {lo} to {hi} argument(s), found {}",
                args.len()
            ),
            span,
        );
    }
    for a in args {
        checker.infer(&a.value);
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // tls.connect(host, port) -> TLS connection handle (chain + hostname
        // verification against the system trust store ON).
        // tls.connect(host, port, ctx) -> same, using a TLS client context
        // (#560): the context's trust roots + optional client cert, SNI/hostname
        // still checked against `host`.
        "connect" => {
            expect_argc_range(checker, method, args, 2, 3, span);
            Ty::Bytes
        }
        // tls.connect_insecure(host, port) -> same, verification OFF (opt-in).
        "connect_insecure" => {
            expect_argc(checker, "tls", method, args, 2, span);
            Ty::Bytes
        }
        // tls.create_default_context() -> client context with the system trust
        // store preloaded (verify + hostname on) — the "augment" base (#560).
        // tls.SSLContext() -> client context with an EMPTY trust store (trusts
        // only what is loaded; verify + hostname still on) — the "replace" base.
        "create_default_context" | "SSLContext" => {
            expect_argc(checker, "tls", method, args, 0, span);
            Ty::Bytes
        }
        // tls.load_verify_locations(ctx, cafile[, capath]) -> adds CA trust to the
        // context (additive, like Python). capath is optional ("" = omitted).
        "load_verify_locations" => {
            expect_argc_range(checker, method, args, 2, 3, span);
            Ty::Unit
        }
        // tls.load_cert_chain(ctx, certfile[, keyfile]) -> client cert + key for
        // mutual TLS. keyfile is optional ("" / omitted = key is in certfile).
        "load_cert_chain" => {
            expect_argc_range(checker, method, args, 2, 3, span);
            Ty::Unit
        }
        "connect_resolved" => {
            expect_argc(checker, "tls", method, args, 4, span);
            Ty::Bytes
        }
        // tls.listen(host, port, cert_path, key_path) -> TLS listener handle
        // (server SSL_CTX from the cert+key PEM files on disk, bound to host:port).
        "listen" => {
            expect_argc(checker, "tls", method, args, 4, span);
            Ty::Bytes
        }
        // tls.accept(listener) -> TLS connection handle (server-side handshake).
        // The handle unifies with the client's, so send/recv/close_conn are shared.
        "accept" => {
            expect_argc(checker, "tls", method, args, 1, span);
            Ty::Bytes
        }
        // tls.port(listener) -> int64 (bound local port; resolves an ephemeral 0).
        "port" => {
            expect_argc(checker, "tls", method, args, 1, span);
            Ty::Int64
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
        // `tls.close_conn` / `tls.close_listener` (mirrors `net`).
        "close_conn" | "close_listener" => {
            expect_argc(checker, "tls", method, args, 1, span);
            Ty::Unit
        }
        // Internal test scaffold (deferred public server): start a background
        // self-signed TLS echo server and return its ephemeral port.
        "_test_server" => {
            expect_argc(checker, "tls", method, args, 0, span);
            Ty::Int64
        }
        // Internal mutual-TLS test scaffold (#560): a client-auth-requiring echo
        // server used by the golden suite to drive the public client context's
        // `load_cert_chain`; not stable API.
        "_test_mtls_server" => {
            expect_argc(checker, "tls", method, args, 3, span);
            Ty::Int64
        }
        _ => return None,
    })
}
