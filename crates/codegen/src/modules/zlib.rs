//! `zlib` module — runtime-symbol declarations (ticket #106, D-0056/D-0060).
//!
//! DEFLATE with the zlib wrapper (RFC 1950, Adler-32). The C implementations
//! wrap the system zlib (linked with `-lz`) and live in the zlib/gzip section of
//! `zaprt.c`. All `str`/`bytes` values are `ZapStr*` pointers:
//!   - `zap_zlib_compress(data, level) -> bytes`
//!   - `zap_zlib_decompress(data) -> bytes`
//!   - `zap_zlib_crc32(data) -> i64`, `zap_zlib_adler32(data) -> i64`

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // compress(data: ZapStr*, level: i64) -> ZapStr* (bytes).
    cg.declare_rt("zap_zlib_compress", &[p.into(), i.into()], Some(p.into()));
    // decompress(data: ZapStr*) -> ZapStr* (bytes).
    cg.declare_rt("zap_zlib_decompress", &[p.into()], Some(p.into()));
    // crc32 / adler32(data: ZapStr*) -> i64 checksum.
    cg.declare_rt("zap_zlib_crc32", &[p.into()], Some(i.into()));
    cg.declare_rt("zap_zlib_adler32", &[p.into()], Some(i.into()));
}
