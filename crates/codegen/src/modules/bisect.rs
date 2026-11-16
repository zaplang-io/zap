//! `bisect` module — runtime-symbol declarations (ticket #102, generalized #983).
//!
//! Binary search / sorted insert over a sorted `ZapList[T]` handle (`ptr`),
//! generic over any `Comparable` element. Each entry point takes the search
//! value already reinterpreted to an `int64` element word plus a `kind` tag
//! (BISECT_ELEM_*) selecting the ordering (integer / lexical string / IEEE float
//! total order); lowering emits the reinterpret. `bisect_*` return the insertion
//! point; `insort_*` mutate in place and return nothing. The C implementation
//! lives in the "collections extras" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let f = cg.ctx.f64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // Element-typed entry points (_i64 / _ptr / _f64), like `heap`.
    cg.declare_rt("zap_bisect_left_i64", &[p.into(), i.into()], Some(i.into()));
    cg.declare_rt("zap_bisect_left_ptr", &[p.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_bisect_left_f64", &[p.into(), f.into()], Some(i.into()));
    cg.declare_rt(
        "zap_bisect_right_i64",
        &[p.into(), i.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_bisect_right_ptr",
        &[p.into(), p.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_bisect_right_f64",
        &[p.into(), f.into()],
        Some(i.into()),
    );
    cg.declare_rt("zap_bisect_insort_left_i64", &[p.into(), i.into()], None);
    cg.declare_rt("zap_bisect_insort_left_ptr", &[p.into(), p.into()], None);
    cg.declare_rt("zap_bisect_insort_left_f64", &[p.into(), f.into()], None);
    cg.declare_rt("zap_bisect_insort_right_i64", &[p.into(), i.into()], None);
    cg.declare_rt("zap_bisect_insort_right_ptr", &[p.into(), p.into()], None);
    cg.declare_rt("zap_bisect_insort_right_f64", &[p.into(), f.into()], None);
}
