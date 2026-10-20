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
        // client
        "get" => call_ret(lowerer, "zap_http_get", a, Ty::Ptr, true),
        "post" => call_ret(lowerer, "zap_http_post", a, Ty::Ptr, true),
        "request" => call_ret(lowerer, "zap_http_request", a, Ty::Ptr, true),
        "status" => call_ret(lowerer, "zap_http_status", a, Ty::I64, false),
        "body" => call_ret(lowerer, "zap_http_body", a, Ty::Ptr, false),
        "header" => call_ret(lowerer, "zap_http_header", a, Ty::Ptr, false),
        "url" => call_ret(lowerer, "zap_http_url", a, Ty::Ptr, false),
        // server
        "read_request" => call_ret(lowerer, "zap_http_read_request", a, Ty::Ptr, true),
        "method" => call_ret(lowerer, "zap_http_req_method", a, Ty::Ptr, false),
        "path" => call_ret(lowerer, "zap_http_req_path", a, Ty::Ptr, false),
        "req_header" => call_ret(lowerer, "zap_http_req_header", a, Ty::Ptr, false),
        "req_body" => call_ret(lowerer, "zap_http_req_body", a, Ty::Ptr, false),
        "respond" => call_void(lowerer, "zap_http_respond", a, true),
        _ => return Ok(None),
    }))
}
