//! `fs` module — runtime-symbol declarations (ticket #97, D-0056).
//!
//! Path-oriented file convenience + metadata: whole-file read/write, existence
//! and type/size queries, and `mkdtemp`. Directory listing reuses the `os`
//! module's `zap_os_listdir`. Booleans flow as `int64`. See `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_fs_read_file", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_fs_write_file", &[p.into(), p.into()], None);
    cg.declare_rt("zap_fs_exists", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_is_dir", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_is_file", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_size", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_mkdtemp", &[], Some(p.into()));
}
