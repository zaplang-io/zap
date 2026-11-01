//! `tls` module — runtime-symbol declarations (tickets #147, #488; D-0062, D-0093).
//!
//! The TLS client over the netpoller (#80): `connect`/`connect_insecure` return
//! opaque `ZapTls` handles (flowing as `ptr`), `send`/`recv` carry plaintext as
//! `str`/`bytes` (the same `ZapStr` object) and byte counts as `int64`, and
//! `close` shuts a connection down. The TLS server (#488): `listen` returns an
//! opaque `ZapTlsListener` handle built from a cert+key on disk, `accept` runs
//! the server-side handshake and returns a `ZapTls` conn (the client's handle,
//! unified), `port` reads the bound port and `close_listener` closes it.
//! `zap_tls_test_server` is the internal self-signed loopback echo server behind
//! `tls._test_server()`. See the "tls" sections of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_tls_connect", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt(
        "zap_tls_connect_insecure",
        &[p.into(), i.into()],
        Some(p.into()),
    );
    cg.declare_rt(
        "zap_tls_listen",
        &[p.into(), i.into(), p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt("zap_tls_accept", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_tls_port", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_tls_recv", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_tls_send", &[p.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_tls_close", &[p.into()], None);
    cg.declare_rt("zap_tls_close_listener", &[p.into()], None);
    cg.declare_rt("zap_tls_test_server", &[], Some(i.into()));
}
