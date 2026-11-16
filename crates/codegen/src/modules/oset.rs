//! `oset` collection — runtime-symbol declarations (#102, generalized #983).
//!
//! An insertion-ordered set, generic over any `Hashable` element (i64 or
//! pointer/string; floats are not Hashable, as for `set`). The handle is an
//! opaque GC pointer (`ptr`); the constructor takes an element-kind tag.
//! `contains` returns a C `int64_t` boolean (normalized to `i1` in lowering).
//! The C implementation lives in the "collections extras" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_oset_new", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_oset_add_i64", &[p.into(), i.into()], None);
    cg.declare_rt("zap_oset_add_ptr", &[p.into(), p.into()], None);
    cg.declare_rt(
        "zap_oset_contains_i64",
        &[p.into(), i.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_oset_contains_ptr",
        &[p.into(), p.into()],
        Some(i.into()),
    );
    cg.declare_rt("zap_oset_len", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_oset_get_i64", &[p.into(), i.into()], Some(i.into()));
    cg.declare_rt("zap_oset_get_ptr", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_oset_items", &[p.into()], Some(p.into()));
}
