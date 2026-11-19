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
        // The socket/listener handle is an opaque GC pointer word. Sema types it
        // as `bytes` (`crates/sema/src/modules/net.rs`), so the IR carries it as
        // `Ty::Bytes`, not a bare `Ty::Ptr` (`str`): the two are the same pointer
        // word at codegen, but since #1074 (D-0241) a `bytes` and a `str` element
        // are distinct `Elem` kinds, so a handle stored in the `list[bytes]`
        // sema accepts must lower as `Ty::Bytes` or `list.append` rejects it as a
        // wrong-element-type mismatch (regression caught by
        // `net_connect_deadline_stall`).
        "listen" => call_ret(lowerer, "zap_net_listen", a, Ty::Bytes, true),
        "connect" => call_ret(lowerer, "zap_net_connect", a, Ty::Bytes, true),
        "connect_ip_deadline" => {
            call_ret(lowerer, "zap_net_connect_ip_deadline", a, Ty::Bytes, true)
        }
        "set_deadline_ns" => call_void(lowerer, "zap_net_set_deadline_ns", a, true),
        "accept" => call_ret(lowerer, "zap_net_accept", a, Ty::Bytes, true),
        "recv" => call_ret(lowerer, "zap_net_recv", a, Ty::Ptr, true),
        "send" => call_ret(lowerer, "zap_net_send", a, Ty::I64, true),
        "port" => call_ret(lowerer, "zap_net_port", a, Ty::I64, false),
        // Throwing: a negative timeout raises ValueError (checked in the runtime).
        "set_read_timeout" => call_void(lowerer, "zap_net_set_read_timeout", a, true),
        // Throwing: a negative deadline raises ValueError (checked in the runtime).
        "set_read_deadline" => call_void(lowerer, "zap_net_set_read_deadline", a, true),
        "close_conn" | "close_listener" => call_void(lowerer, "zap_net_close", a, false),
        _ => return Ok(None),
    }))
}
