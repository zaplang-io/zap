//! `deque` collection — runtime-symbol declarations (#102, generalized #983).
//!
//! A double-ended ring buffer, generic over the element kind (i64 / pointer /
//! float64). The handle is an opaque GC pointer (`ptr`); the constructor takes
//! an element-kind tag, and the element-typed entry points mirror `list`/`heap`
//! (`_i64` / `_ptr` / `_f64` suffixes). The C implementation lives in the
//! "collections extras" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let f = cg.ctx.f64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // Constructors carry the element-kind tag (DEQUE_ELEM_*).
    cg.declare_rt("zap_deque_new", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_deque_bounded", &[i.into(), i.into()], Some(p.into()));
    // append / appendleft, per element kind.
    cg.declare_rt("zap_deque_append_i64", &[p.into(), i.into()], None);
    cg.declare_rt("zap_deque_append_ptr", &[p.into(), p.into()], None);
    cg.declare_rt("zap_deque_append_f64", &[p.into(), f.into()], None);
    cg.declare_rt("zap_deque_appendleft_i64", &[p.into(), i.into()], None);
    cg.declare_rt("zap_deque_appendleft_ptr", &[p.into(), p.into()], None);
    cg.declare_rt("zap_deque_appendleft_f64", &[p.into(), f.into()], None);
    // pop / popleft, per element kind.
    cg.declare_rt("zap_deque_pop_i64", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_deque_pop_ptr", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_deque_pop_f64", &[p.into()], Some(f.into()));
    cg.declare_rt("zap_deque_popleft_i64", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_deque_popleft_ptr", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_deque_popleft_f64", &[p.into()], Some(f.into()));
    // len / get / clear / items.
    cg.declare_rt("zap_deque_len", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_deque_get_i64", &[p.into(), i.into()], Some(i.into()));
    cg.declare_rt("zap_deque_get_ptr", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_deque_get_f64", &[p.into(), i.into()], Some(f.into()));
    cg.declare_rt("zap_deque_clear", &[p.into()], None);
    cg.declare_rt("zap_deque_items", &[p.into()], Some(p.into()));
}
