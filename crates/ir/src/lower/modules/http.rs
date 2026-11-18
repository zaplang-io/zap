//! `http` module — lowering facet (ticket #104, D-0056).
//!
//! Each call lowers to a backing `zap_http_*` runtime entry point. The network
//! calls (`get`/`post`/`request`/`read_request`/`respond`) may raise (connect
//! failure, malformed message, https-not-supported) so they are emitted with
//! `throwing_call`; the pure accessors on an already-parsed handle are plain
//! infallible calls.

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "http",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        // client — the headers-bearing forms (one extra `dict[str,str]` arg,
        // #280) lower to the `_h` runtime entry points, and the timeout-bearing
        // forms (a trailing `int64` ms total deadline, #986) to the `_ht` ones
        // (headers always present — the seam binder fills it with `{}` when only
        // `timeout` is supplied). Sema fixes the arities, so the arg count alone
        // selects the entry point.
        "get" if a.len() == 3 => call_ret(lowerer, "zap_http_get_ht", a, Ty::Ptr, true),
        "get" if a.len() == 2 => call_ret(lowerer, "zap_http_get_h", a, Ty::Ptr, true),
        "get" => call_ret(lowerer, "zap_http_get", a, Ty::Ptr, true),
        "post" if a.len() == 4 => call_ret(lowerer, "zap_http_post_ht", a, Ty::Ptr, true),
        "post" if a.len() == 3 => call_ret(lowerer, "zap_http_post_h", a, Ty::Ptr, true),
        "post" => call_ret(lowerer, "zap_http_post", a, Ty::Ptr, true),
        "request" if a.len() == 5 => call_ret(lowerer, "zap_http_request_ht", a, Ty::Ptr, true),
        "request" if a.len() == 4 => call_ret(lowerer, "zap_http_request_h", a, Ty::Ptr, true),
        "request" => call_ret(lowerer, "zap_http_request", a, Ty::Ptr, true),
        "status" => call_ret(lowerer, "zap_http_status", a, Ty::I64, false),
        "body" => call_ret(lowerer, "zap_http_body", a, Ty::Ptr, false),
        "header" => call_ret(lowerer, "zap_http_header", a, Ty::Ptr, false),
        // redirect-following accessors (#987): pure reads on the response handle.
        "final_url" => call_ret(lowerer, "zap_http_final_url", a, Ty::Ptr, false),
        "redirects" => call_ret(lowerer, "zap_http_redirects", a, Ty::Ptr, false),
        "url" => call_ret(lowerer, "zap_http_url", a, Ty::Ptr, false),
        // server
        "read_request" => call_ret(lowerer, "zap_http_read_request", a, Ty::Ptr, true),
        "method" => call_ret(lowerer, "zap_http_req_method", a, Ty::Ptr, false),
        "path" => call_ret(lowerer, "zap_http_req_path", a, Ty::Ptr, false),
        "req_header" => call_ret(lowerer, "zap_http_req_header", a, Ty::Ptr, false),
        // req_headers(req) -> list[(str, str)]: enumerate the raw header block
        // (#960, D-0197). Assembled in lowering (a counted loop over the three
        // `zap_http_req_header_{count,name,val}` accessors + the compiler's own
        // tuple path) so the `(str, str)` GC layout matches codegen, exactly like
        // `dict[str, str].items()`.
        "req_headers" => lowerer.lower_http_req_headers(a[0]),
        // client response header enumeration (#989): list[(str,str)] assembled in
        // lowering, the `req_headers` twin.
        "headers" => lowerer.lower_http_resp_headers(a[0]),
        // Set-Cookie parsing (#989): build the prelude `Cookie` value struct.
        "parse_set_cookie" => lowerer.lower_http_parse_set_cookie(a[0]),
        "req_body" => call_ret(lowerer, "zap_http_req_body", a, Ty::Ptr, false),
        "req_version" => call_ret(lowerer, "zap_http_req_version", a, Ty::Ptr, false),
        // respond — the headers-bearing form (one extra `dict[str,str]` arg,
        // #279) lowers to the `_h` entry point; sema fixes the arities.
        "respond" if a.len() == 4 => call_void(lowerer, "zap_http_respond_h", a, true),
        "respond" => call_void(lowerer, "zap_http_respond", a, true),
        // internal client-pool primitives (#990): keep-alive pool behind the
        // prelude `http.Client`. `_pool_new`/`_pool_do` can raise (OOM / network
        // / too-many-redirects), so they use throwing calls; `_pool_close` cannot.
        "_pool_new" => call_ret(lowerer, "zap_http_pool_new", a, Ty::I64, true),
        "_pool_do" => call_ret(lowerer, "zap_http_pool_do", a, Ty::Ptr, true),
        "_pool_close" => call_void(lowerer, "zap_http_pool_close", a, false),
        _ => return Ok(None),
    }))
}
