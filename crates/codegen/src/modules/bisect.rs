//! `bisect` module — runtime-symbol declarations (ticket #102, D-0056).
//!
//! Binary search / sorted insert over a `ZapList` handle (`ptr`) and an `int64`
//! value. `bisect_*` return the insertion point; `insort_*` mutate in place and
//! return nothing. The C implementation lives in the "collections extras"
//! section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_bisect_left", &[p.into(), i.into()], Some(i.into()));
    cg.declare_rt("zap_bisect_right", &[p.into(), i.into()], Some(i.into()));
    cg.declare_rt("zap_bisect_insort_left", &[p.into(), i.into()], None);
    cg.declare_rt("zap_bisect_insort_right", &[p.into(), i.into()], None);
}
