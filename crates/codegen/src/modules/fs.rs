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
    // Tempfile (#979, D-0239). mkstemp returns an opaque handle the lowering
    // reads into an (fd, path) tuple via the two getters (the scandir/walk
    // handle pattern); gettempdir -> str; write_atomic -> unit (may raise).
    cg.declare_rt(
        "zap_fs_mkstemp",
        &[p.into(), p.into(), p.into()],
        Some(p.into()),
    );
    cg.declare_rt("zap_fs_mkstemp_fd", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_mkstemp_path", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_fs_gettempdir", &[], Some(p.into()));
    cg.declare_rt("zap_fs_write_atomic", &[p.into(), p.into()], None);
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
    // Line-oriented input (#969, D-0212): whole-file read split into lines.
    cg.declare_rt("zap_fs_lines", &[p.into()], Some(p.into()));
    // Path matching (#1033, D-0216): fnmatch -> int64 bool; glob -> list[str].
    cg.declare_rt("zap_fs_fnmatch", &[p.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_glob", &[p.into(), p.into()], Some(p.into()));
    // Directory traversal (#1032, D-0224). copytree may raise (unit). scandir /
    // walk return an opaque handle the lowering enumerates via count + indexed
    // getters (the req_headers pattern): the getters are infallible pointer /
    // int64 reads over the already-materialised handle.
    cg.declare_rt("zap_fs_copytree", &[p.into(), p.into()], None);
    cg.declare_rt("zap_fs_scandir", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_fs_scandir_count", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_scandir_name", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt(
        "zap_fs_scandir_is_dir",
        &[p.into(), i.into()],
        Some(i.into()),
    );
    cg.declare_rt(
        "zap_fs_scandir_is_file",
        &[p.into(), i.into()],
        Some(i.into()),
    );
    cg.declare_rt("zap_fs_walk", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_fs_walk_count", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_fs_walk_dirpath", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_fs_walk_dirs", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_fs_walk_files", &[p.into(), i.into()], Some(p.into()));
}
