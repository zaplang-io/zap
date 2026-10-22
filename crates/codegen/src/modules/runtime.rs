//! `runtime` module — runtime-symbol declarations (ticket #77, D-0058).
//!
//! Scheduler/runtime diagnostics: `zap_runtime_goroutine_dump()` dumps goroutine
//! stacks + a scheduler summary; `zap_runtime_num_goroutines()` returns the live
//! goroutine count. See `zaprt.c`.

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    cg.declare_rt("zap_runtime_goroutine_dump", &[], None);
    cg.declare_rt("zap_runtime_num_goroutines", &[], Some(i.into()));
    // Scheduler controls (ticket #172): the parallelism knob `MAXPROCS` and the
    // LockOSThread (dedicated-OS-thread) pin/unpin pair.
    cg.declare_rt("zap_runtime_maxprocs", &[], Some(i.into()));
    cg.declare_rt("zap_runtime_lock_os_thread", &[], None);
    cg.declare_rt("zap_runtime_unlock_os_thread", &[], None);
    // CPU profiler controls (ticket #78, D-0058): start/stop sampling; folded
    // output is written at stop. The SIGUSR2 signal toggles the same machinery.
    cg.declare_rt("zap_profile_start", &[], None);
    cg.declare_rt("zap_profile_stop", &[], None);
}
