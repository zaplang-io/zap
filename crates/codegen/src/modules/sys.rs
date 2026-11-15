//! `sys` module — runtime-symbol declarations (ticket #97, D-0056).
//!
//! `sys.argv()` -> list[str], `sys.exit(code)`, `sys.platform()` -> str. The
//! stdio handles (`sys.std{in,out,err}_fd()`) are the constants 0/1/2 emitted
//! inline by the lowerer, so they declare no runtime symbol. See `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_sys_argv", &[], Some(p.into()));
    cg.declare_rt("zap_sys_exit", &[i.into()], None);
    cg.declare_rt("zap_sys_platform", &[], Some(p.into()));
    // CPU-feature introspection (#943, epic #139). cpu_has takes a str (ptr) and
    // returns an i64 0/1; cpu_source returns a str (ptr); cpu_sve_vl returns i64.
    cg.declare_rt("zap_sys_cpu_has", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_sys_cpu_source", &[], Some(p.into()));
    cg.declare_rt("zap_sys_cpu_sve_vl", &[], Some(i.into()));
}
