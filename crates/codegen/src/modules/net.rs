//! `net` module — runtime-symbol declarations (ticket #104, D-0056).
//!
//! TCP sockets on the netpoller (#80): `listen`/`accept` server, `connect`
//! client, `recv`/`send` over opaque `ZapSock` handles, `port` for an ephemeral
//! bound port, and a `close` shared by listeners and connections. Handles flow
//! as `ptr`; byte counts and ports as `int64`; payloads as `str`/`bytes`
//! (the same `ZapStr` object). See the "net / http (#104)" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_net_listen", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_net_connect", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_net_accept", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_net_recv", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_net_send", &[p.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_net_port", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_net_close", &[p.into()], None);
}
