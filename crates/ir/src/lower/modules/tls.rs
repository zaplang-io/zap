//! `tls` module — lowering facet (tickets #147, #488; D-0062, D-0093).
//!
//! Each call lowers to a backing `zap_tls_*` runtime entry point over
//! `str`/`int64`/handle (`ptr`) words. The handshake, I/O, `listen`/`accept` and
//! the test server can raise (connect/bind failure, certificate verification
//! failure, protocol error, accept on a closed listener) so they are emitted
//! with `throwing_call` (catchable inside `try`); `port`, `close_conn` and
//! `close_listener` are infallible plain calls. See the "tls" sections of
//! `zaprt.c`.

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "tls", lower };

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "connect" => call_ret(lowerer, "zap_tls_connect", a, Ty::Ptr, true),
        "connect_resolved" => call_ret(lowerer, "zap_tls_connect_resolved", a, Ty::Ptr, true),
        "connect_insecure" => call_ret(lowerer, "zap_tls_connect_insecure", a, Ty::Ptr, true),
        "listen" => call_ret(lowerer, "zap_tls_listen", a, Ty::Ptr, true),
        "accept" => call_ret(lowerer, "zap_tls_accept", a, Ty::Ptr, true),
        "port" => call_ret(lowerer, "zap_tls_port", a, Ty::I64, false),
        "recv" => call_ret(lowerer, "zap_tls_recv", a, Ty::Ptr, true),
        "send" => call_ret(lowerer, "zap_tls_send", a, Ty::I64, true),
        "close_conn" => call_void(lowerer, "zap_tls_close", a, false),
        "close_listener" => call_void(lowerer, "zap_tls_close_listener", a, false),
        "_test_server" => call_ret(lowerer, "zap_tls_test_server", a, Ty::I64, true),
        _ => return Ok(None),
    }))
}
