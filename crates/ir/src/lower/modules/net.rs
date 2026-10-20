//! `net` module — lowering facet (ticket #104, D-0056).
//!
//! Each call lowers to a backing `zap_net_*` runtime entry point over
//! `str`/`int64`/handle (`ptr`) words. Socket operations may raise (connection
//! refused, reset, resolve failure) so they are emitted with `throwing_call`
//! (catchable inside `try`); the infallible `port` query is a plain call.

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "net", lower };

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "listen" => call_ret(lowerer, "zap_net_listen", a, Ty::Ptr, true),
        "connect" => call_ret(lowerer, "zap_net_connect", a, Ty::Ptr, true),
        "accept" => call_ret(lowerer, "zap_net_accept", a, Ty::Ptr, true),
        "recv" => call_ret(lowerer, "zap_net_recv", a, Ty::Ptr, true),
        "send" => call_ret(lowerer, "zap_net_send", a, Ty::I64, true),
        "port" => call_ret(lowerer, "zap_net_port", a, Ty::I64, false),
        "close_conn" | "close_listener" => call_void(lowerer, "zap_net_close", a, false),
        _ => return Ok(None),
    }))
}
