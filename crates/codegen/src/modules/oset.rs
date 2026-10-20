//! `oset` module — runtime-symbol declarations (ticket #102, D-0056).
//!
//! An insertion-ordered set handle (`ptr`) over `int64` words. `contains`
//! returns a C `int64_t` boolean (normalized to `i1` in lowering). The C
//! implementation lives in the "collections extras" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_oset_new", &[], Some(p.into()));
    cg.declare_rt("zap_oset_add", &[p.into(), i.into()], None);
    cg.declare_rt("zap_oset_contains", &[p.into(), i.into()], Some(i.into()));
    cg.declare_rt("zap_oset_len", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_oset_get", &[p.into(), i.into()], Some(i.into()));
}
