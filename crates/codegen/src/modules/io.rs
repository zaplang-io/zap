//! `io` module — runtime-symbol declarations (ticket #97, D-0056).
//!
//! Fd-based file I/O: `open(path, mode) -> fd`, `read(fd, n) -> str`,
//! `write(fd, data) -> int64`, `close(fd)`. See `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_io_open", &[p.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_io_read", &[i.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_io_write", &[i.into(), p.into()], Some(i.into()));
    cg.declare_rt("zap_io_close", &[i.into()], None);
}
