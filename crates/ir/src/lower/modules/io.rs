//! `io` module — lowering facet (ticket #97, D-0056). Files are raw int64 fds.

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "io", lower };

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "open" => call_ret(lowerer, "zap_io_open", a, Ty::I64, true),
        "read" => call_ret(lowerer, "zap_io_read", a, Ty::Ptr, true),
        "write" => call_ret(lowerer, "zap_io_write", a, Ty::I64, true),
        "close_fd" => call_void(lowerer, "zap_io_close", a, true),
        _ => return Ok(None),
    }))
}
