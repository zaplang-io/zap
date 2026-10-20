//! `http` module — sema facet (ticket #104, D-0056).
//!
//! HTTP/1.1 over the `net` TCP sockets: a plaintext client (`get`/`post`/
//! `request`, returning an opaque response handle with `status`/`body`/`header`
//! accessors) and the server-side helpers (`read_request` parses a request off a
//! connection into an opaque request handle with `method`/`path`/`req_header`/
//! `req_body` accessors; `respond` writes a response). Response/request handles
//! are opaque GC objects typed as `bytes` (the `crypto`/`sync` pattern). `https`
//! is rejected at runtime — TLS is a follow-up (out of scope for this PR).
//! `import http` is accepted but not required.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "http",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // --- client ---------------------------------------------------------
        // http.get(url) -> response handle
        "get" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Bytes
        }
        // http.post(url, body) -> response handle
        "post" => {
            expect_argc(checker, "http", method, args, 2, span);
            Ty::Bytes
        }
        // http.request(method, url, body) -> response handle
        "request" => {
            expect_argc(checker, "http", method, args, 3, span);
            Ty::Bytes
        }
        // http.status(resp) -> int64
        "status" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Int64
        }
        // http.body(resp) -> str
        "body" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Str
        }
        // http.header(resp, name) -> str ("" when absent)
        "header" => {
            expect_argc(checker, "http", method, args, 2, span);
            Ty::Str
        }
        // http.url(host, port, path) -> "http://host:port/path". A convenience
        // for dialing an ephemeral server whose port is only known at runtime,
        // since there is no int->str conversion in the language yet.
        "url" => {
            expect_argc(checker, "http", method, args, 3, span);
            Ty::Str
        }
        // --- server ---------------------------------------------------------
        // http.read_request(conn) -> request handle
        "read_request" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Bytes
        }
        // http.method(req) -> str
        "method" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Str
        }
        // http.path(req) -> str
        "path" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Str
        }
        // http.req_header(req, name) -> str ("" when absent)
        "req_header" => {
            expect_argc(checker, "http", method, args, 2, span);
            Ty::Str
        }
        // http.req_body(req) -> str
        "req_body" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Str
        }
        // http.respond(conn, status, body) -> unit
        "respond" => {
            expect_argc(checker, "http", method, args, 3, span);
            Ty::Unit
        }
        _ => return None,
    })
}
