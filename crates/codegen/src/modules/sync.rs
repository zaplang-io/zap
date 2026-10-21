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
    // I#175 primitives, shared with the `threading` wrapper surface. A timeout
    // is an `f64` (seconds) and the timed waits return an `int64` C boolean.
    let f = cg.ctx.f64_type();
    cg.declare_rt(
        "zap_mutex_lock_timeout",
        &[p.into(), f.into()],
        Some(i.into()),
    );
    // RLock
    cg.declare_rt("zap_rlock_new", &[], Some(p.into()));
    cg.declare_rt("zap_rlock_lock", &[p.into()], None);
    cg.declare_rt("zap_rlock_unlock", &[p.into()], None);
    cg.declare_rt("zap_rlock_trylock", &[p.into()], Some(i.into()));
    // Event
    cg.declare_rt("zap_event_new", &[], Some(p.into()));
    cg.declare_rt("zap_event_set", &[p.into()], None);
    cg.declare_rt("zap_event_clear", &[p.into()], None);
    cg.declare_rt("zap_event_is_set", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_event_wait", &[p.into()], None);
    cg.declare_rt(
        "zap_event_wait_timeout",
        &[p.into(), f.into()],
        Some(i.into()),
    );
    // Semaphore
    cg.declare_rt("zap_sem_new", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_sem_acquire", &[p.into()], None);
    cg.declare_rt(
        "zap_sem_acquire_timeout",
        &[p.into(), f.into()],
        Some(i.into()),
    );
    cg.declare_rt("zap_sem_release", &[p.into()], None);
    // Condition
    cg.declare_rt("zap_cond_new", &[], Some(p.into()));
    cg.declare_rt("zap_cond_acquire", &[p.into()], None);
    cg.declare_rt("zap_cond_release", &[p.into()], None);
    cg.declare_rt("zap_cond_wait", &[p.into()], None);
    cg.declare_rt("zap_cond_notify", &[p.into(), i.into()], None);
    cg.declare_rt("zap_cond_notify_all", &[p.into()], None);
}
