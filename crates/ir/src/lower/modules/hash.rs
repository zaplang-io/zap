//! `hash` module — lowering facet (I#139 Tier 2, #962, D-0199).
//!
//! `hash.crc32c(data)` lowers to the `zap_hash_crc32c` runtime shim (a software
//! CRC32C, Castagnoli, bit-identical to the hardware `arch.*` path). The `data`
//! argument is a `bytes`/`str` reference, passed through as its pointer word; the
//! shim returns the 32-bit checksum.

use zapc_syntax::ast;

use super::support::{call_ret, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "hash",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "crc32c" => call_ret(lowerer, "zap_hash_crc32c", a, Ty::U32, false),
        _ => return Ok(None),
    }))
}
