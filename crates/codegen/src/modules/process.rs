//! `process` module — runtime-symbol declarations (ticket #97, D-0056).
//!
//! `process.run(argv)` spawns and waits, returning a GC `ZapProc` handle; three
//! getters unpack its fields (exit code, captured stdout, captured stderr) which
//! the lowerer assembles into a `(int64, str, str)` tuple. See `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_process_run", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_process_result_code", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_process_result_stdout", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_process_result_stderr", &[p.into()], Some(p.into()));
}
