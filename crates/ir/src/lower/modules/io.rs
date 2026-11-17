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
        // Buffered line-oriented input (#969, D-0212). The handle is an opaque
        // pointer (a `bytes` value whose buffer is a pointer-free reader state,
        // the crypto-streaming pattern). `reader` only allocates; the reads do
        // I/O and raise on error, so they are throwing calls.
        "reader" => call_ret(lowerer, "zap_io_reader_new", a, Ty::Ptr, false),
        "readline" => call_ret(lowerer, "zap_io_readline", a, Ty::Ptr, true),
        "readall" => call_ret(lowerer, "zap_io_readall", a, Ty::Ptr, true),
        "readn" => call_ret(lowerer, "zap_io_readn", a, Ty::Ptr, true),
        _ => return Ok(None),
    }))
}
