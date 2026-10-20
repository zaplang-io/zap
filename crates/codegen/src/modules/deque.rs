//! `deque` module — runtime-symbol declarations (ticket #102, D-0056).
//!
//! A double-ended ring buffer over `int64` words. The handle is an opaque GC
//! pointer (`ptr`); `append`/`appendleft`/`clear` return nothing. The C
//! implementation lives in the "collections extras" section of `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_deque_new", &[], Some(p.into()));
    cg.declare_rt("zap_deque_bounded", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_deque_append", &[p.into(), i.into()], None);
    cg.declare_rt("zap_deque_appendleft", &[p.into(), i.into()], None);
    cg.declare_rt("zap_deque_pop", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_deque_popleft", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_deque_len", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_deque_get", &[p.into(), i.into()], Some(i.into()));
    cg.declare_rt("zap_deque_clear", &[p.into()], None);
}
