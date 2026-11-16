//! `arch.x86_64` / `arch.aarch64` modules — lowering facet (I#139 Tier 2, #962,
//! D-0199).
//!
//! Each intrinsic lowers to a thin `zap_arch_*` runtime shim (declared in the
//! codegen facet) that carries the right `__attribute__((target(...)))` so the C
//! compiler emits the exact hardware instruction / LLVM intrinsic (x86 SSE4.2
//! `crc32`, aarch64 CRC32C ext; `POPCNT`/`CNT`; `PREFETCHh`/`PRFM`), and which
//! LTO inlines into the caller. The shim word ABI is uniform `i64` in / fixed
//! width out (via [`super::support::lower_args`]), so argument widths never need
//! narrowing at the call site; the shim truncates internally.
//!
//! `crc32c_*` is **arch-specific** (a different instruction per target), so the
//! two arch modules have their own `lower` picking the x86 vs aarch64 symbol;
//! `popcount_*` and `prefetch` are architecture-universal (one shared symbol).

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

/// `arch.x86_64` — crc32c lowers to the SSE4.2 `crc32` shims.
pub(super) const MODULE_X86_64: ModuleLower = ModuleLower {
    name: "arch.x86_64",
    lower: lower_x86_64,
};

/// `arch.aarch64` — crc32c lowers to the aarch64 CRC32C-extension shims.
pub(super) const MODULE_AARCH64: ModuleLower = ModuleLower {
    name: "arch.aarch64",
    lower: lower_aarch64,
};

fn lower_x86_64(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    lower_arch(lowerer, "x86", method, args)
}

fn lower_aarch64(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    lower_arch(lowerer, "aarch64", method, args)
}

/// Shared lowering for both arch modules; `isa` (`"x86"` / `"aarch64"`) selects
/// the arch-specific `crc32c_*` symbol. `popcount_*` / `prefetch` are universal.
fn lower_arch(
    lowerer: &mut FnLower,
    isa: &str,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // `prefetch(buf[, locality])` is a void hint; pad an omitted locality with
    // the default T0 (3) so the fixed-arity shim always gets both words.
    if method == "prefetch" {
        let mut a = lower_args(lowerer, args)?;
        if a.len() == 1 {
            a.push(lowerer.iconst(Ty::I64, 3));
        }
        return Ok(Some(call_void(lowerer, "zap_arch_prefetch", a, false)));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "crc32c_u8" => call_ret(lowerer, &crc_sym(isa, "u8"), a, Ty::U32, false),
        "crc32c_u16" => call_ret(lowerer, &crc_sym(isa, "u16"), a, Ty::U32, false),
        "crc32c_u32" => call_ret(lowerer, &crc_sym(isa, "u32"), a, Ty::U32, false),
        "crc32c_u64" => call_ret(lowerer, &crc_sym(isa, "u64"), a, Ty::U64, false),
        "popcount_u32" => call_ret(lowerer, "zap_arch_popcount_u32", a, Ty::U32, false),
        "popcount_u64" => call_ret(lowerer, "zap_arch_popcount_u64", a, Ty::U32, false),
        _ => return Ok(None),
    }))
}

/// The per-ISA crc32c runtime symbol, e.g. `zap_arch_x86_crc32c_u64`.
fn crc_sym(isa: &str, width: &str) -> String {
    format!("zap_arch_{isa}_crc32c_{width}")
}
