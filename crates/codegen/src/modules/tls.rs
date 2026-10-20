//! `tls` module — runtime-symbol declarations (ticket #147, D-0062).
//!
//! The TLS client over the netpoller (#80): `connect`/`connect_insecure` return
//! opaque `ZapTls` handles (flowing as `ptr`), `send`/`recv` carry plaintext as
//! `str`/`bytes` (the same `ZapStr` object) and byte counts as `int64`, and
//! `close` shuts a connection down. `zap_tls_test_server` is the internal
//! self-signed loopback echo server behind `tls._test_server()`. See the
//! "tls (#147)" section of `zaprt.c`.

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
    cg.declare_rt("zap_tls_recv", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_tls_send", &[p.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_tls_close", &[p.into()], None);
    cg.declare_rt("zap_tls_test_server", &[], Some(i.into()));
}
