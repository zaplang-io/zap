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

/// Emit a throwing runtime call returning a 0/1 `i64` word and compare it to 0
/// for a `bool` (`i1`) result — the throwing counterpart of `support::call_bool`,
/// used by the socket-option getters whose `getsockopt` may raise.
fn call_bool_throwing(lowerer: &mut FnLower, func: &str, args: Vec<Value>) -> (Value, Ty) {
    let word = lowerer.new_value(Ty::I64);
    lowerer.throwing_call(Some(word), func, args);
    let zero = lowerer.iconst(Ty::I64, 0);
    let d = lowerer.new_value(Ty::I1);
    lowerer.emit(Inst::ICmp {
        dest: d,
        pred: CmpPred::Ne,
        lhs: word,
        rhs: zero,
    });
    (d, Ty::I1)
}

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

/// Unpack a `ZapUnixDatagram` handle (from `zap_net_recvfrom_unix`) into a
/// `(str, str)` tuple — the AF_UNIX datagram counterpart of
/// [`unpack_datagram_tuple`], with the sender's path in place of host/port.
fn unpack_unix_datagram_tuple(lowerer: &mut FnLower, h: Value) -> (Value, Ty) {
    let data = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(data),
        func: "zap_net_unixdgram_data".to_string(),
        args: vec![h],
    });
    let path = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(path),
        func: "zap_net_unixdgram_path".to_string(),
        args: vec![h],
    });

    let sid = lowerer
        .ctx
        .tuples
        .borrow_mut()
        .intern(vec![Ty::Ptr, Ty::Ptr]);
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    for (idx, val) in [data, path].into_iter().enumerate() {
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val,
        });
    }
    (dest, Ty::Struct(sid))
}

/// Unpack a `ZapPeerCred` handle (from `zap_net_peer_cred`) into a
/// `(int64, int64, int64)` tuple via the three infallible field getters.
fn unpack_peer_cred_tuple(lowerer: &mut FnLower, h: Value) -> (Value, Ty) {
    let getters = [
        "zap_net_peer_cred_uid",
        "zap_net_peer_cred_gid",
        "zap_net_peer_cred_pid",
    ];
    let mut fields = Vec::with_capacity(3);
    for g in getters {
        let v = lowerer.new_value(Ty::I64);
        lowerer.emit(Inst::Call {
            dest: Some(v),
            func: g.to_string(),
            args: vec![h],
        });
        fields.push(v);
    }
    let sid = lowerer
        .ctx
        .tuples
        .borrow_mut()
        .intern(vec![Ty::I64, Ty::I64, Ty::I64]);
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    for (idx, val) in fields.into_iter().enumerate() {
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
        "connect_deadline" => call_ret(lowerer, "zap_net_connect_deadline", a, Ty::Bytes, true),
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

        // ---- DNS resolution surface (ticket #992, epic #985) ----
        // Each resolves to a `list[str]` (element kind `Ptr` = str, #1074) and may
        // raise (resolve failure / malformed IP), so emitted as a throwing call.
        "lookup_host" => call_ret(lowerer, "zap_net_lookup_host", a, Ty::List(Elem::Ptr), true),
        "lookup_ipv4" => call_ret(lowerer, "zap_net_lookup_ipv4", a, Ty::List(Elem::Ptr), true),
        "lookup_ipv6" => call_ret(lowerer, "zap_net_lookup_ipv6", a, Ty::List(Elem::Ptr), true),
        "lookup_addr" => call_ret(lowerer, "zap_net_lookup_addr", a, Ty::List(Elem::Ptr), true),

        // ---- Unix domain sockets (ticket #993, epic #985) ----
        "listen_unix" => call_ret(lowerer, "zap_net_listen_unix", a, Ty::Bytes, true),
        "connect_unix" => call_ret(lowerer, "zap_net_connect_unix", a, Ty::Bytes, true),
        "unixgram_socket" => call_ret(lowerer, "zap_net_unixgram_socket", a, Ty::Bytes, true),
        "unixgram_bind" => call_void(lowerer, "zap_net_unixgram_bind", a, true),
        "sendto_unix" => call_ret(lowerer, "zap_net_sendto_unix", a, Ty::I64, true),
        // recvfrom_unix -> (data, path): throwing call returns a ZapUnixDatagram
        // handle, then two infallible getters unpack it into the (str, str) tuple.
        "recvfrom_unix" => {
            let h = lowerer.new_value(Ty::Ptr);
            lowerer.throwing_call(Some(h), "zap_net_recvfrom_unix", a);
            unpack_unix_datagram_tuple(lowerer, h)
        }
        // peer_cred -> (uid, gid, pid): throwing call returns a ZapPeerCred
        // handle, then three infallible getters unpack it into the int64 tuple.
        "peer_cred" => {
            let h = lowerer.new_value(Ty::Ptr);
            lowerer.throwing_call(Some(h), "zap_net_peer_cred", a);
            unpack_peer_cred_tuple(lowerer, h)
        }

        // ---- socket options (ticket #994, epic #985) ----
        // Setters: bool/int value passes through as an i64 word (lower_args
        // coerces the bool). setsockopt may raise, so a throwing call.
        "set_reuseaddr" => call_void(lowerer, "zap_net_set_reuseaddr", a, true),
        "set_reuseport" => call_void(lowerer, "zap_net_set_reuseport", a, true),
        "set_nodelay" => call_void(lowerer, "zap_net_set_nodelay", a, true),
        "set_keepalive" => call_void(lowerer, "zap_net_set_keepalive", a, true),
        "set_recv_buffer" => call_void(lowerer, "zap_net_set_recv_buffer", a, true),
        "set_send_buffer" => call_void(lowerer, "zap_net_set_send_buffer", a, true),
        // Boolean getters: the runtime returns a 0/1 i64 word (throwing on a
        // getsockopt error), compared to 0 for the `bool` result.
        "get_nodelay" => call_bool_throwing(lowerer, "zap_net_get_nodelay", a),
        "get_keepalive" => call_bool_throwing(lowerer, "zap_net_get_keepalive", a),
        // Buffer-size getters -> int64.
        "get_recv_buffer" => call_ret(lowerer, "zap_net_get_recv_buffer", a, Ty::I64, true),
        "get_send_buffer" => call_ret(lowerer, "zap_net_get_send_buffer", a, Ty::I64, true),

        // ---- TCP keepalive interval tuning (ticket #1141, follow-up to #994) ----
        "set_keepalive_idle" => call_void(lowerer, "zap_net_set_keepalive_idle", a, true),
        "set_keepalive_interval" => call_void(lowerer, "zap_net_set_keepalive_interval", a, true),
        "set_keepalive_count" => call_void(lowerer, "zap_net_set_keepalive_count", a, true),
        "get_keepalive_idle" => call_ret(lowerer, "zap_net_get_keepalive_idle", a, Ty::I64, true),
        "get_keepalive_interval" => {
            call_ret(lowerer, "zap_net_get_keepalive_interval", a, Ty::I64, true)
        }
        "get_keepalive_count" => call_ret(lowerer, "zap_net_get_keepalive_count", a, Ty::I64, true),

        // ---- UDP multicast (ticket #1142, follow-up to #991/#994) ----
        "join_multicast" => call_void(lowerer, "zap_net_join_multicast", a, true),
        "leave_multicast" => call_void(lowerer, "zap_net_leave_multicast", a, true),
        "set_multicast_ttl" => call_void(lowerer, "zap_net_set_multicast_ttl", a, true),
        "set_multicast_loopback" => call_void(lowerer, "zap_net_set_multicast_loopback", a, true),
        _ => return Ok(None),
    }))
}
