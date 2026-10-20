//! `sync` module — runtime-symbol declarations (ticket #103, D-0032).
//!
//! `Mutex`, `WaitGroup`, and `Once` handles are opaque GC pointers (`ptr`); the
//! `WaitGroup` count argument is an `int64` word. `trylock`/`once_begin` return
//! an `int64` C boolean (normalized to `i1` in the lowering). The C
//! implementations live in the "sync" section of `zaprt.c`, built on the
//! scheduler's shared park/wake primitive so a blocked coroutine frees its M.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // Mutex
    cg.declare_rt("zap_mutex_new", &[], Some(p.into()));
    cg.declare_rt("zap_mutex_lock", &[p.into()], None);
    cg.declare_rt("zap_mutex_unlock", &[p.into()], None);
    cg.declare_rt("zap_mutex_trylock", &[p.into()], Some(i.into()));
    // WaitGroup
    cg.declare_rt("zap_waitgroup_new", &[], Some(p.into()));
    cg.declare_rt("zap_waitgroup_add", &[p.into(), i.into()], None);
    cg.declare_rt("zap_waitgroup_done", &[p.into()], None);
    cg.declare_rt("zap_waitgroup_wait", &[p.into()], None);
    // Once
    cg.declare_rt("zap_once_new", &[], Some(p.into()));
    cg.declare_rt("zap_once_begin", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_once_end", &[p.into()], None);
}
