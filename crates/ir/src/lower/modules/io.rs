//! `io` module — lowering facet (ticket #97, D-0056). Files are raw int64 fds.

use zapc_syntax::ast;

use super::support::{call_ret, call_void, line_reader_wrap, lower_args, writer_new_wrap};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "io", lower };

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // `io.lines(reader)` wraps an existing reader handle in the prelude
    // `LineReader` so `for line in io.lines(r)` iterates (#1031, D-0223). Its
    // argument is the handle itself; lower it and wrap — no flat runtime call.
    if method == "lines" {
        let arg = args
            .first()
            .ok_or_else(|| err("io.lines expects 1 argument"))?;
        let (h, _) = lowerer.expr(&arg.value)?;
        return Ok(Some(line_reader_wrap(lowerer, h)?));
    }
    // `io.writer(fd)` builds a prelude `Writer` wrapper over a fresh buffered
    // writer handle (#1071, D-0239). Its argument is the fd; the allocation
    // itself never raises, so no throwing call here — the object methods do.
    if method == "writer" {
        let arg = args
            .first()
            .ok_or_else(|| err("io.writer expects 1 argument"))?;
        let (fd, ft) = lowerer.expr(&arg.value)?;
        let fd = lowerer.coerce(fd, ft, Ty::I64);
        return Ok(Some(writer_new_wrap(lowerer, fd)?));
    }
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
        // Flat writer entry points (#1071, D-0239), called by the prelude
        // `Writer`. They do I/O and raise (closed writer / I/O error), so they
        // are throwing calls — a plain Call would make try/except unable to
        // catch the failure.
        "_writer_write" => call_ret(lowerer, "zap_io_writer_write", a, Ty::I64, true),
        "_writer_flush" => call_void(lowerer, "zap_io_writer_flush", a, true),
        "_writer_close" => call_void(lowerer, "zap_io_writer_close", a, true),
        _ => return Ok(None),
    }))
}
