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

/// Unpack a `ZapDatagram` handle (from `zap_udp_recvfrom`) into a
/// `(str, str, int64)` tuple via the three infallible field getters and the
/// tuple aggregate path (intern + StructNew + FieldSet) — the same shape
/// `process.run` uses for its `ZapProc` -> `(int64, str, str)`. The handle is
/// GC-rooted (the throwing-call dest), and no park occurs between the getters,
/// so the two `str` fields stay live while the tuple is built.
fn unpack_datagram_tuple(lowerer: &mut FnLower, h: Value) -> (Value, Ty) {
    let data = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(data),
        func: "zap_udp_dgram_data".to_string(),
        args: vec![h],
    });
    let host = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(host),
        func: "zap_udp_dgram_host".to_string(),
        args: vec![h],
    });
    let port = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Call {
        dest: Some(port),
        func: "zap_udp_dgram_port".to_string(),
        args: vec![h],
    });

    let sid = lowerer
        .ctx
        .tuples
        .borrow_mut()
        .intern(vec![Ty::Ptr, Ty::Ptr, Ty::I64]);
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    for (idx, val) in [data, host, port].into_iter().enumerate() {
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val,
        });
    }
    (dest, Ty::Struct(sid))
}

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

        // ---- UDP datagram sockets (ticket #991, epic #985) ----
        // Constructors: socket() can fail (fd exhaustion), so throwing.
        "udp_socket" => call_ret(lowerer, "zap_udp_socket", a, Ty::Bytes, true),
        "udp_socket6" => call_ret(lowerer, "zap_udp_socket6", a, Ty::Bytes, true),
        // bind / connect may raise (bad address, family mismatch, in-use port).
        "udp_bind" => call_void(lowerer, "zap_udp_bind", a, true),
        "udp_connect" => call_void(lowerer, "zap_udp_connect", a, true),
        // sendto -> int64 bytes sent; may raise (resolve/send failure, timeout).
        "sendto" => call_ret(lowerer, "zap_udp_sendto", a, Ty::I64, true),
        // recvfrom -> (data, host, port). The throwing call returns a ZapDatagram
        // handle (GC pointer, rooted as the call dest); the three infallible
        // getters then unpack it into the (str, str, int64) tuple.
        "recvfrom" => {
            let h = lowerer.new_value(Ty::Ptr);
            lowerer.throwing_call(Some(h), "zap_udp_recvfrom", a);
            unpack_datagram_tuple(lowerer, h)
        }
        _ => return Ok(None),
    }))
}
