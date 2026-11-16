//! `arch.x86_64` / `arch.aarch64` modules — runtime-symbol declarations
//! (I#139 Tier 2, #962, D-0199).
//!
//! The intrinsics lower to the `zap_arch_*` shims defined in `zaparch.inc.c`
//! (each over a `target(...)`-attributed compiler builtin, so the C compiler
//! emits the hardware instruction and LTO inlines the shim). The word ABI is
//! uniform: integer args are `i64` words, results are the natural width (`i32`
//! for a u32 result / a popcount, `i64` for a u64 CRC accumulator). Both arches'
//! `crc32c_*` symbols are declared unconditionally; only the ones for the build
//! target are defined in C (the other arch's `crc32c_*` is never referenced
//! because its `arch.*` module cannot be imported off-target), and an unused
//! external declaration links away. `popcount_*` / `prefetch` are universal.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i64 = cg.ctx.i64_type();
    let i32 = cg.ctx.i32_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());

    // crc32c single-step intrinsics: (i64 crc, i64 v) -> u32 / u64.
    for isa in ["x86", "aarch64"] {
        cg.declare_rt(
            &format!("zap_arch_{isa}_crc32c_u8"),
            &[i64.into(), i64.into()],
            Some(i32.into()),
        );
        cg.declare_rt(
            &format!("zap_arch_{isa}_crc32c_u16"),
            &[i64.into(), i64.into()],
            Some(i32.into()),
        );
        cg.declare_rt(
            &format!("zap_arch_{isa}_crc32c_u32"),
            &[i64.into(), i64.into()],
            Some(i32.into()),
        );
        cg.declare_rt(
            &format!("zap_arch_{isa}_crc32c_u64"),
            &[i64.into(), i64.into()],
            Some(i64.into()),
        );
    }

    // popcount: (i64 v) -> u32.
    cg.declare_rt("zap_arch_popcount_u32", &[i64.into()], Some(i32.into()));
    cg.declare_rt("zap_arch_popcount_u64", &[i64.into()], Some(i32.into()));

    // prefetch: (ptr buf, i64 locality) -> void (a no-fault hint).
    cg.declare_rt("zap_arch_prefetch", &[p.into(), i64.into()], None);
}
