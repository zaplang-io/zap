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
}
