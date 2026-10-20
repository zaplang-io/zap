//! `os` module — runtime-symbol declarations (ticket #97, D-0056).
//!
//! Environment, working directory, directory entries and path helpers. `str`
//! flows as `ptr` (a `ZapStr*`); booleans flow as `int64` (`!= 0`), normalized
//! to `i1` by the lowerer. See the "os / sys / io / fs / process" section of
//! `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_os_getenv", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_os_hasenv", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_os_setenv", &[p.into(), p.into()], None);
    cg.declare_rt("zap_os_getpid", &[], Some(i.into()));
    cg.declare_rt("zap_os_getcwd", &[], Some(p.into()));
    cg.declare_rt("zap_os_chdir", &[p.into()], None);
    cg.declare_rt("zap_os_mkdir", &[p.into()], None);
    cg.declare_rt("zap_os_remove", &[p.into()], None);
    cg.declare_rt("zap_os_rmdir", &[p.into()], None);
    cg.declare_rt("zap_os_listdir", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_os_path_join", &[p.into(), p.into()], Some(p.into()));
    cg.declare_rt("zap_os_path_exists", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_os_path_basename", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_os_path_dirname", &[p.into()], Some(p.into()));
}
