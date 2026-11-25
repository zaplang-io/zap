//! `http` module — runtime-symbol declarations (ticket #104, D-0056).
//!
//! HTTP/1.1 over the `net` sockets. The client (`get`/`post`/`request`, with
//! optional `_h` request-header forms taking a `dict[str,str]` — #280) returns
//! an opaque `ZapHttpResp` handle unpacked by `status`/`body`/`header`; the
//! server (`read_request`) returns an opaque `ZapHttpReq` handle unpacked by
//! `req_method`/`req_path`/`req_version`/`req_header`/`req_headers`/`req_body`, and `respond` writes a reply
//! (with an optional `_h` form taking a `dict[str,str]` of response headers —
//! #279).
//! Handles flow as `ptr`, status as `int64`, URLs/headers/bodies as `str`.
//! See the "net / http (#104)" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // client
    cg.declare_rt("zap_http_get", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_get_h", &[p.into(), p.into()], Some(p.into()));
    cg.declare_rt("zap_http_post", &[p.into(), p.into()], Some(p.into()));
    cg.declare_rt(
        "zap_http_post_h",
        &[p.into(), p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_http_request",
        &[p.into(), p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_http_request_h",
        &[p.into(), p.into(), p.into(), p.into()],
        Some(p.into()),
    );
    // timeout-bearing client forms (#986): a trailing `int64` total-deadline in
    // milliseconds. Headers are always present (the seam binder defaults them to
    // `{}` when a caller supplies only `timeout`), so each `_ht` entry is its
    // `_h` twin plus the trailing `i64`.
    cg.declare_rt(
        "zap_http_get_ht",
        &[p.into(), p.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_http_post_ht",
        &[p.into(), p.into(), p.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_http_request_ht",
        &[p.into(), p.into(), p.into(), p.into(), i.into()],
        Some(p.into()),
    );
    // internal client-pool primitives (#990): the keep-alive pool behind the
    // prelude `http.Client`. The pool is a stable non-GC pointer passed as an
    // int64 handle.
    cg.declare_rt("zap_http_pool_new", &[i.into()], Some(i.into()));
    cg.declare_rt(
        "zap_http_pool_do",
        &[
            i.into(), // pool handle
            p.into(), // method
            p.into(), // url
            p.into(), // body
            p.into(), // headers (dict, may be empty)
            i.into(), // timeout_ms
            i.into(), // follow (0/1)
            i.into(), // max_redirects
            i.into(), // max_response_bytes (0 = unlimited, #1448)
        ],
        Some(p.into()),
    );
    cg.declare_rt("zap_http_pool_close", &[i.into()], None);
    // client response header enumeration (#989): count + positional name/value,
    // the list[(str,str)] assembled in lowering (the `req_headers` pattern).
    cg.declare_rt("zap_http_resp_header_count", &[p.into()], Some(i.into()));
    cg.declare_rt(
        "zap_http_resp_header_name",
        &[p.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_http_resp_header_val",
        &[p.into(), i.into()],
        Some(p.into()),
    );
    // Set-Cookie parsing (#989): one getter per prelude `Cookie` field, the
    // struct built in lowering (the `url.split` / `UrlParts` pattern). String
    // fields return `str` (ptr); `secure` / `http_only` an int64 0/1 normalized
    // to the struct's bool field in lowering. `max_age` is an int64 parsed value
    // paired with a `_present` 0/1 flag (#1524) — lowering wraps the two into the
    // `Optional[int64] max_age` field (present -> Some, absent/malformed -> None).
    cg.declare_rt("zap_http_cookie_name", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_cookie_value", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_cookie_domain", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_cookie_path", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_cookie_expires", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_cookie_same_site", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_cookie_max_age", &[p.into()], Some(i.into()));
    cg.declare_rt(
        "zap_http_cookie_max_age_present",
        &[p.into()],
        Some(i.into()),
    );
    cg.declare_rt("zap_http_cookie_secure", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_http_cookie_http_only", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_http_status", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_http_body", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_header", &[p.into(), p.into()], Some(p.into()));
    // redirect-following result accessors (#987)
    cg.declare_rt("zap_http_final_url", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_redirects", &[p.into()], Some(p.into()));
    cg.declare_rt(
        "zap_http_url",
        &[p.into(), i.into(), p.into()],
        Some(p.into()),
    );
    // server
    cg.declare_rt("zap_http_read_request", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_req_method", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_req_path", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_req_header", &[p.into(), p.into()], Some(p.into()));
    // req_headers enumeration backing (#960): count + positional name/value, the
    // list[(str,str)] itself assembled in lowering.
    cg.declare_rt("zap_http_req_header_count", &[p.into()], Some(i.into()));
    cg.declare_rt(
        "zap_http_req_header_name",
        &[p.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_http_req_header_val",
        &[p.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt("zap_http_req_body", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_req_version", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_respond", &[p.into(), i.into(), p.into()], None);
    cg.declare_rt(
        "zap_http_respond_h",
        &[p.into(), i.into(), p.into(), p.into()],
        None,
    );
}
