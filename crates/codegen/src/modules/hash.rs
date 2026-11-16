//! `hash` module — runtime-symbol declarations (I#139 Tier 2, #962, D-0199).
//!
//! `hash.crc32c(data)` -> uint32: a software CRC32C (Castagnoli) over the
//! `bytes`/`str` buffer, bit-identical to folding the hardware `arch.*.crc32c_*`
//! intrinsic. Defined in `zaparch.inc.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i32 = cg.ctx.i32_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // crc32c takes a bytes/str pointer and returns the 32-bit checksum.
    cg.declare_rt("zap_hash_crc32c", &[p.into()], Some(i32.into()));
}
