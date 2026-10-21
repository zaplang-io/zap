//! `threading` module — runtime-symbol declarations (ticket #175).
//!
//! The Python-`threading`-shaped OS-thread compat surface. `Thread` handles and
//! the sync-primitive handles are opaque GC pointers (`ptr`); a `daemon` flag is
//! an `int64` C boolean; a timeout is an `f64` (seconds) and the timed waits
//! return an `int64` C boolean. The C implementations live in the "threading"
//! section of `zaprt.c`, built on the #172 LockOSThread substrate and the
//! scheduler's shared park/wake primitive. `_unpin` reuses the `runtime`
//! module's `zap_runtime_unlock_os_thread` (declared there, not here).

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let f = cg.ctx.f64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // Thread lifecycle.
    cg.declare_rt("zap_thread_new", &[], Some(p.into()));
    cg.declare_rt("zap_thread_register", &[p.into(), i.into()], None);
    cg.declare_rt("zap_thread_finish", &[p.into(), i.into()], None);
    cg.declare_rt("zap_thread_join", &[p.into()], None);
    cg.declare_rt(
        "zap_thread_join_timeout",
        &[p.into(), f.into()],
        Some(i.into()),
    );
    cg.declare_rt("zap_thread_is_alive", &[p.into()], Some(i.into()));
    // Dedicated-OS-thread pin (threading-compat, not counted in the #172 join
    // barrier). Unpin reuses runtime::zap_runtime_unlock_os_thread.
    cg.declare_rt("zap_threading_pin", &[], None);
}
