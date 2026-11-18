//! `http` module — sema facet (ticket #104, D-0056).
//!
//! HTTP/1.1 over the `net` TCP sockets: a plaintext client (`get`/`post`/
//! `request`, each also taking an optional trailing `dict[str,str]` of request
//! headers — #280 — and returning an opaque response handle with
//! `status`/`body`/`header` accessors). The client follows 3xx redirects by
//! default (#987, D-0222): up to a fixed hop cap, resolving each `Location`
//! against the current URL (RFC-3986, as `url.join`), with the browser/`requests`
//! method rules (303 → GET; 301/302 downgrade only POST → GET; 307/308 preserve);
//! `final_url` and `redirects` expose the outcome. The server-side helpers
//! (`read_request` parses a request off a
//! connection into an opaque request handle with `method`/`path`/`req_version`/
//! `req_header`/`req_headers`/`req_body` accessors; `respond` writes a response). Response/request handles
//! are opaque GC objects typed as `bytes` (the `crypto`/`sync` pattern). `https`
//! is rejected at runtime — TLS is a follow-up (out of scope for this PR).
//! `import http` is accepted but not required.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, expect_argc_range, ModuleCheck};
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
        // http.get(url) / http.get(url, headers) / http.get(url[, headers],
        // timeout=ms) -> response handle. `timeout` is the per-request total
        // deadline in milliseconds (#986): the whole call (connect + reads +
        // redirects) must finish within it or it raises TimeoutError; 0 / omitted
        // = unbounded, a negative value is a ValueError at runtime. `headers` is
        // an interior optional — the seam binder fills it with `{}` when only
        // `timeout` is supplied — so the argument count spans 1..=3.
        "get" => {
            expect_argc_range(checker, "http", method, args, 1, 3, span);
            Ty::Bytes
        }
        // http.post(url, body) / http.post(url, body, headers) / http.post(url,
        // body[, headers], timeout=ms) -> response handle. See `get` for the
        // `timeout` total-deadline semantics (#986).
        "post" => {
            expect_argc_range(checker, "http", method, args, 2, 4, span);
            Ty::Bytes
        }
        // http.request(method, url, body) / ..., headers) / ...[, headers],
        // timeout=ms) -> response handle. The optional `headers` is a
        // `dict[str,str]` of request headers sent on the wire (#280); `timeout` is
        // the per-request total deadline in milliseconds (#986).
        "request" => {
            expect_argc_range(checker, "http", method, args, 3, 5, span);
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
        // http.final_url(resp) -> str. The URL the returned response came from
        // after redirect following: the last URL dialed, or the original request
        // URL when nothing was followed (#987).
        "final_url" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Str
        }
        // http.redirects(resp) -> list[str]. The redirect targets followed, in
        // order; the empty list when none were (#987).
        "redirects" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::List(Box::new(Ty::Str))
        }
        // http.headers(resp) -> list[(str, str)]. Enumerate the response header
        // block as received, in order, duplicates preserved (#989) — so multiple
        // `Set-Cookie` lines are each visible (the single-value `http.header`
        // keeps by-name lookup). The client twin of server `http.req_headers`.
        "headers" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::List(Box::new(Ty::Tuple(vec![Ty::Str, Ty::Str])))
        }
        // http.parse_set_cookie(value) -> Cookie. Parse one `Set-Cookie` header
        // value into the prelude `Cookie` value struct (name/value/domain/path/
        // expires/max_age/secure/http_only/same_site) (#989). The cookie jar
        // (persistence + domain/path matching + auto-send) lives in
        // zaplang-io/requests, built on this primitive.
        "parse_set_cookie" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Named("Cookie".to_string())
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
        // http.req_headers(req) -> list[(str, str)]. Enumerates the full received
        // request header block as (name, value) pairs in received order, with
        // duplicate names preserved as separate pairs (#960, D-0197); by-name
        // discovery stays with the case-insensitive `req_header`.
        "req_headers" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::List(Box::new(Ty::Tuple(vec![Ty::Str, Ty::Str])))
        }
        // http.req_body(req) -> str
        "req_body" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Str
        }
        // http.req_version(req) -> str (request-line HTTP version, e.g. "HTTP/1.1")
        "req_version" => {
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
        // --- internal client-pool primitives (#990, prelude-only) -----------
        // Backing the prelude `http.Client`: a keep-alive connection pool
        // addressed by a stable non-GC int64 handle. Underscore-prefixed and
        // positional-only (no seam_sigs rows), like `json._lex_*`.
        // http._pool_new(max_idle) -> int64 handle.
        "_pool_new" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Int64
        }
        // http._pool_do(handle, method, url, body, headers, timeout, follow,
        // max_redirects) -> response handle. Does the whole exchange: pooled
        // dial/reuse, single-response framing, and redirect following per
        // `follow` / `max_redirects` (#1049).
        "_pool_do" => {
            expect_argc(checker, "http", method, args, 8, span);
            Ty::Bytes
        }
        // http._pool_close(handle) -> unit. Drains + frees the pool.
        "_pool_close" => {
            expect_argc(checker, "http", method, args, 1, span);
            Ty::Unit
        }
        _ => return None,
    })
}
