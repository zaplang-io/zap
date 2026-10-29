//! `base64` module — runtime-symbol declarations (ticket #337, D-0056).
//!
//! The C implementations live in the base64/hex section of `zaprt.c`. All
//! `str`/`bytes` values are `ZapStr*` pointers:
//!   - `zap_base64_encode(data, urlsafe, pad) -> str`
//!   - `zap_base64_decode(s, urlsafe) -> bytes`

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // (data, urlsafe, pad) -> str
    cg.declare_rt(
        "zap_base64_encode",
        &[p.into(), i.into(), i.into()],
        Some(p.into()),
    );
    // (s, urlsafe) -> bytes
    cg.declare_rt("zap_base64_decode", &[p.into(), i.into()], Some(p.into()));
}
