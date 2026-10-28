//! `http` module — sema facet (ticket #104, D-0056).
//!
//! HTTP/1.1 over the `net` TCP sockets: a plaintext client (`get`/`post`/
//! `request`, each also taking an optional trailing `dict[str,str]` of request
//! headers — #280 — and returning an opaque response handle with
//! `status`/`body`/`header` accessors) and the server-side helpers (`read_request` parses a request off a
//! connection into an opaque request handle with `method`/`path`/`req_header`/
//! `req_body` accessors; `respond` writes a response). Response/request handles
//! are opaque GC objects typed as `bytes` (the `crypto`/`sync` pattern). `https`
//! is rejected at runtime — TLS is a follow-up (out of scope for this PR).
//! `import http` is accepted but not required.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

/// Require either `n` or `n + 1` arguments (the extra one is the optional
/// `headers: dict[str,str]` map, #280): the client verbs `get`/`post`/`request`
/// each come in a bare form and a headers-bearing form. Every argument is still
/// inferred so argument expressions are type-checked regardless of arity.
fn expect_argc_or_headers(checker: &mut Checker, method: &str, args: &[Arg], n: usize, span: Span) {
    if args.len() != n && args.len() != n + 1 {
        checker.error(
            ErrorCode::ZAP4025,
            format!(
                "http.{method}() takes {n} or {} argument(s), found {}",
                n + 1,
                args.len()
            ),
            span,
        );
    }
    for a in args {
        checker.infer(&a.value);
    }
}

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "http",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // --- client ---------------------------------------------------------
        // http.get(url) / http.get(url, headers) -> response handle
        "get" => {
            expect_argc_or_headers(checker, method, args, 1, span);
            Ty::Bytes
        }
        // http.post(url, body) / http.post(url, body, headers) -> response handle
        "post" => {
            expect_argc_or_headers(checker, method, args, 2, span);
            Ty::Bytes
        }
        // http.request(method, url, body) / http.request(method, url, body, headers)
        // -> response handle. The optional trailing arg is a `dict[str,str]` of
        // request headers sent on the wire (#280).
        "request" => {
            expect_argc_or_headers(checker, method, args, 3, span);
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
        // http.respond(conn, status, body) / http.respond(conn, status, body,
        // headers) -> unit. The optional trailing arg is a `dict[str,str]` of
        // response headers written on the wire (#279).
        "respond" => {
            expect_argc_or_headers(checker, method, args, 3, span);
            Ty::Unit
        }
        _ => return None,
    })
}
