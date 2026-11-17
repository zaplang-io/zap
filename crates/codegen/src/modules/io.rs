//! `io` module — runtime-symbol declarations (ticket #97, D-0056).
//!
//! Fd-based file I/O: `open(path, mode) -> fd`, `read(fd, n) -> str`,
//! `write(fd, data) -> int64`, `close(fd)`. See `zaprt.c`.
//!
//! Line-oriented input (#969, D-0212): the `input([prompt])` builtin and the
//! buffered reader (`reader(fd) -> handle`, `readline`/`readall`/`readn`). The
//! `input` symbol is declared here (its lowering lives with the other builtins)
//! because this module's `declare_rt` always runs. The reader handle is an
//! opaque `bytes` value, so it is declared as a pointer result.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_io_open", &[p.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_io_read", &[i.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_io_write", &[i.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_io_close", &[i.into()], None);
    // Line-oriented input (#969, D-0212).
    cg.declare_rt("zap_input", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_io_reader_new", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_io_readline", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_io_readall", &[p.into()], Some(p.into()));
    cg.declare_rt("zap_io_readn", &[p.into(), i.into()], Some(p.into()));
}
