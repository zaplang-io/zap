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

use super::support::{call_ret, call_void, empty_str, lower_args};
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
        // 2-arg: fresh system-roots verified dial (unchanged). 3-arg: dial using
        // a client context (#560) — reorder to zap_tls_connect_ctx(ctx, host, port).
        "connect" if a.len() == 3 => call_ret(
            lowerer,
            "zap_tls_connect_ctx",
            vec![a[2], a[0], a[1]],
            Ty::Ptr,
            true,
        ),
        "connect" => call_ret(lowerer, "zap_tls_connect", a, Ty::Ptr, true),
        "connect_resolved" => call_ret(lowerer, "zap_tls_connect_resolved", a, Ty::Ptr, true),
        "connect_insecure" => call_ret(lowerer, "zap_tls_connect_insecure", a, Ty::Ptr, true),
        // TLS client contexts (#560): constructors return an opaque handle; the
        // loaders mutate the context and return unit. The optional trailing string
        // argument (capath / keyfile) is padded with "" for the fixed-arity symbol.
        "create_default_context" => {
            call_ret(lowerer, "zap_tls_create_default_context", a, Ty::Ptr, true)
        }
        "SSLContext" => call_ret(lowerer, "zap_tls_ssl_context", a, Ty::Ptr, true),
        "load_verify_locations" => {
            let mut args = a;
            if args.len() == 2 {
                let e = empty_str(lowerer);
                args.push(e);
            }
            call_void(lowerer, "zap_tls_load_verify_locations", args, true)
        }
        "load_cert_chain" => {
            let mut args = a;
            if args.len() == 2 {
                let e = empty_str(lowerer);
                args.push(e);
            }
            call_void(lowerer, "zap_tls_load_cert_chain", args, true)
        }
        "listen" => call_ret(lowerer, "zap_tls_listen", a, Ty::Ptr, true),
        "accept" => call_ret(lowerer, "zap_tls_accept", a, Ty::Ptr, true),
        "port" => call_ret(lowerer, "zap_tls_port", a, Ty::I64, false),
        "recv" => call_ret(lowerer, "zap_tls_recv", a, Ty::Ptr, true),
        "send" => call_ret(lowerer, "zap_tls_send", a, Ty::I64, true),
        "close_conn" => call_void(lowerer, "zap_tls_close", a, false),
        "close_listener" => call_void(lowerer, "zap_tls_close_listener", a, false),
        "_test_server" => call_ret(lowerer, "zap_tls_test_server", a, Ty::I64, true),
        "_test_mtls_server" => call_ret(lowerer, "zap_tls_test_mtls_server", a, Ty::I64, true),
        _ => return Ok(None),
    }))
}
