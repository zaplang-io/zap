//! `tls` module — lowering facet (ticket #147, D-0062).
//!
//! Each call lowers to a backing `zap_tls_*` runtime entry point over
//! `str`/`int64`/handle (`ptr`) words. The handshake, I/O and the test server
//! can raise (connect failure, certificate verification failure, protocol
//! error) so they are emitted with `throwing_call` (catchable inside `try`);
//! `close_conn` is infallible. See the "tls (#147)" section of `zaprt.c`.

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
        "connect_insecure" => call_ret(lowerer, "zap_tls_connect_insecure", a, Ty::Ptr, true),
        "recv" => call_ret(lowerer, "zap_tls_recv", a, Ty::Ptr, true),
        "send" => call_ret(lowerer, "zap_tls_send", a, Ty::I64, true),
        "close_conn" => call_void(lowerer, "zap_tls_close", a, false),
        "_test_server" => call_ret(lowerer, "zap_tls_test_server", a, Ty::I64, true),
        _ => return Ok(None),
    }))
}
