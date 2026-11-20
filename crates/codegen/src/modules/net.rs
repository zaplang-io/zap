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
    cg.declare_rt(
        "zap_net_connect_ip_deadline",
        &[p.into(), i.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_net_connect_deadline",
        &[p.into(), i.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt("zap_net_set_deadline_ns", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_accept", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_net_recv", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_net_send", &[p.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_net_port", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_net_set_read_timeout", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_set_read_deadline", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_close", &[p.into()], None);

    // ---- UDP datagram sockets (ticket #991, epic #985) ----
    cg.declare_rt("zap_udp_socket", &[], Some(p.into()));
    cg.declare_rt("zap_udp_socket6", &[], Some(p.into()));
    cg.declare_rt("zap_udp_bind", &[p.into(), p.into(), i.into()], None);
    cg.declare_rt("zap_udp_connect", &[p.into(), p.into(), i.into()], None);
    cg.declare_rt(
        "zap_udp_sendto",
        &[p.into(), p.into(), p.into(), i.into()],
        Some(i.into()),
    );
    cg.declare_rt("zap_udp_recvfrom", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_udp_dgram_data", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_udp_dgram_host", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_udp_dgram_port", &[p.into()], Some(i.into()));

    // ---- DNS resolution surface (ticket #992, epic #985) ----
    cg.declare_rt("zap_net_lookup_host", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_net_lookup_ipv4", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_net_lookup_ipv6", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_net_lookup_addr", &[p.into()], Some(p.into()));

    // ---- Unix domain sockets (ticket #993, epic #985) ----
    cg.declare_rt("zap_net_listen_unix", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_net_connect_unix", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_net_unixgram_socket", &[], Some(p.into()));
    cg.declare_rt("zap_net_unixgram_bind", &[p.into(), p.into()], None);
    cg.declare_rt(
        "zap_net_sendto_unix",
        &[p.into(), p.into(), p.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_net_recvfrom_unix",
        &[p.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt("zap_net_unixdgram_data", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_net_unixdgram_path", &[p.into()], Some(p.into()));

    // ---- socket options (ticket #994, epic #985) ----
    cg.declare_rt("zap_net_set_reuseaddr", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_set_reuseport", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_set_nodelay", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_set_keepalive", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_set_recv_buffer", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_set_send_buffer", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_get_nodelay", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_net_get_keepalive", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_net_get_recv_buffer", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_net_get_send_buffer", &[p.into()], Some(i.into()));

    // ---- TCP keepalive interval tuning (ticket #1141, follow-up to #994) ----
    cg.declare_rt("zap_net_set_keepalive_idle", &[p.into(), i.into()], None);
    cg.declare_rt(
        "zap_net_set_keepalive_interval",
        &[p.into(), i.into()],
        None,
    );
    cg.declare_rt("zap_net_set_keepalive_count", &[p.into(), i.into()], None);
    cg.declare_rt("zap_net_get_keepalive_idle", &[p.into()], Some(i.into()));
    cg.declare_rt(
        "zap_net_get_keepalive_interval",
        &[p.into()],
        Some(i.into()),
    );
    cg.declare_rt("zap_net_get_keepalive_count", &[p.into()], Some(i.into()));

    // ---- UDP multicast (ticket #1142, follow-up to #991/#994) ----
    cg.declare_rt("zap_net_join_multicast", &[p.into(), p.into()], None);
    cg.declare_rt("zap_net_leave_multicast", &[p.into(), p.into()], None);
    cg.declare_rt("zap_net_set_multicast_ttl", &[p.into(), i.into()], None);
    cg.declare_rt(
        "zap_net_set_multicast_loopback",
        &[p.into(), i.into()],
        None,
    );
}
