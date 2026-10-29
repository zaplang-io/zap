//! `hex` module — runtime-symbol declarations (ticket #337, D-0056).
//!
//! The C implementations live in the base64/hex section of `zaprt.c`. All
//! `str`/`bytes` values are `ZapStr*` pointers:
//!   - `zap_hex_encode(data) -> str`
//!   - `zap_hex_decode(s) -> bytes`

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_hex_encode", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_hex_decode", &[p.into()], Some(p.into()));
}
