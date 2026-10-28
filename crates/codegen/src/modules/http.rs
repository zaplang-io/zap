//! `http` module — runtime-symbol declarations (ticket #104, D-0056).
//!
//! HTTP/1.1 over the `net` sockets. The client (`get`/`post`/`request`, with
//! optional `_h` request-header forms taking a `dict[str,str]` — #280) returns
//! an opaque `ZapHttpResp` handle unpacked by `status`/`body`/`header`; the
//! server (`read_request`) returns an opaque `ZapHttpReq` handle unpacked by
//! `req_method`/`req_path`/`req_header`/`req_body`, and `respond` writes a reply.
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
    cg.declare_rt("zap_http_status", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_http_body", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_header", &[p.into(), p.into()], Some(p.into()));
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
    cg.declare_rt("zap_http_req_body", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_http_respond", &[p.into(), i.into(), p.into()], None);
}
