//! `gzip` module — runtime-symbol declarations (ticket #106, D-0056/D-0060).
//!
//! DEFLATE with the gzip wrapper (RFC 1952, CRC32). The C implementations share
//! the zlib/gzip section of `zaprt.c` (system zlib, `-lz`). `str`/`bytes` values
//! are `ZapStr*` pointers:
//!   - `zap_gzip_compress(data, level) -> bytes`
//!   - `zap_gzip_decompress(data) -> bytes`

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_gzip_compress", &[p.into(), i.into()], Some(p.into()));
    cg.declare_rt("zap_gzip_decompress", &[p.into()], Some(p.into()));
}
