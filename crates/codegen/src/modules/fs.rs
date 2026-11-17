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
    // Filesystem gaps (#968, D-0211). Stat getters (one per `Stat` field):
    // str -> int64 (size/mtime/ctime/mode) or int64 bool (is_dir/is_file/
    // is_symlink); `size`/`is_symlink` and friends raise on a bad path.
    cg.declare_rt("zap_fs_stat_size", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_stat_mtime", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_stat_ctime", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_stat_mode", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_stat_is_dir", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_stat_is_file", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_stat_is_symlink", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_mtime", &[p.into()], Some(i.into()));
    // Mutation: all return unit and may raise.
    cg.declare_rt("zap_fs_rename", &[p.into(), p.into()], None);
    cg.declare_rt("zap_fs_move", &[p.into(), p.into()], None);
    cg.declare_rt("zap_fs_copy", &[p.into(), p.into()], None);
    cg.declare_rt("zap_fs_makedirs", &[p.into()], None);
    cg.declare_rt("zap_fs_rmtree", &[p.into()], None);
    // Binary I/O: read_bytes -> bytes (ptr); write_bytes/append take a bytes/str.
    cg.declare_rt("zap_fs_read_bytes", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_fs_write_bytes", &[p.into(), p.into()], None);
    cg.declare_rt("zap_fs_append", &[p.into(), p.into()], None);
}
