//! `gzip` module — lowering facet (ticket #106, D-0056/D-0060).
//!
//! Emits the `zap_gzip_*` runtime calls (gzip framing over the system zlib).
//! Reuses [`zlib`](super::zlib)'s `compress`/`decompress` argument lowering; the
//! only difference is the backing runtime symbol (which selects the gzip wrapper
//! and its CRC32 trailer).

use zapc_syntax::ast;

use super::zlib::{lower_compress, lower_decompress};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "gzip",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        "compress" => Ok(Some(lower_compress(lowerer, "zap_gzip_compress", args)?)),
        "decompress" => Ok(Some(lower_decompress(
            lowerer,
            "zap_gzip_decompress",
            args,
        )?)),
        _ => Ok(None),
    }
}
