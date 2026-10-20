//! `gzip` module — sema facet (ticket #106, D-0056/D-0060).
//!
//! DEFLATE compression with the gzip wrapper (RFC 1952): a CRC32-checked stream
//! (ISO CRC32, poly 0x04C11DB7 — the gzip checksum, NOT CRC32C). Backed by the
//! system zlib (`-lz`). Shares the argument type-checking with [`zlib`](super::zlib).
//! Surface (`import gzip` accepted but not required):
//!
//!   `gzip.compress(data: str|bytes, level?: int) -> bytes`
//!   `gzip.decompress(data: str|bytes) -> bytes`

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::zlib::{check_compress, check_decompress};
use super::ModuleCheck;
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "gzip",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // gzip.compress(data, level?) -> bytes (RFC 1952, CRC32 trailer).
        "compress" => {
            check_compress(checker, "gzip", args, span);
            Some(Ty::Bytes)
        }
        // gzip.decompress(data) -> bytes.
        "decompress" => {
            check_decompress(checker, "gzip", args, span);
            Some(Ty::Bytes)
        }
        _ => None,
    }
}
