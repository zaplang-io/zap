//! `sys` module — lowering facet (ticket #97, D-0056).

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "sys", lower };

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // The stdio handles are the fixed fds 0/1/2 — pure constants, no call.
    let fd = match method {
        "stdin_fd" => Some(0),
        "stdout_fd" => Some(1),
        "stderr_fd" => Some(2),
        _ => None,
    };
    if let Some(n) = fd {
        return Ok(Some((lowerer.iconst(Ty::I64, n), Ty::I64)));
    }
    // sys.getpass([prompt]) -> str (#1684). Hidden line input: suppress terminal
    // echo, read one line, restore. The optional prompt defaults to "Password: "
    // (filled here, so the runtime always receives the string to write). It can
    // raise (OSError when stdin is not a terminal, EOFError at end-of-input), so
    // it is a throwing call that `try` can catch.
    if method == "getpass" {
        let prompt = if let Some(a) = args.first() {
            let (v, _) = lowerer.expr(&a.value)?;
            v
        } else {
            let d = lowerer.new_value(Ty::Ptr);
            lowerer.emit(Inst::StrConst {
                dest: d,
                bytes: b"Password: ".to_vec(),
            });
            d
        };
        let dest = lowerer.new_value(Ty::Ptr);
        lowerer.throwing_call(Some(dest), "zap_sys_getpass", vec![prompt]);
        return Ok(Some((dest, Ty::Ptr)));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "argv" => call_ret(lowerer, "zap_sys_argv", a, Ty::List(Elem::Ptr), false),
        "exit" => call_void(lowerer, "zap_sys_exit", a, false),
        "platform" => call_ret(lowerer, "zap_sys_platform", a, Ty::Ptr, false),
        // CPU-feature introspection (#943, epic #139). `cpu_has(name)` returns a
        // C int64 0/1 normalized to i1 via call_bool; the others are infallible.
        "cpu_has" => call_bool(lowerer, "zap_sys_cpu_has", a),
        "cpu_source" => call_ret(lowerer, "zap_sys_cpu_source", a, Ty::Ptr, false),
        "cpu_sve_vl" => call_ret(lowerer, "zap_sys_cpu_sve_vl", a, Ty::I64, false),
        // Standard streams as objects (#1035, D-0217). stdin -> reader handle,
        // stdout/stderr -> writer handle; write -> bytes written (throwing: the
        // backing zap_io_write raises on an I/O error).
        // The standard-stream objects are opaque GC pointer handles sema types as
        // `bytes` (D-0217); the IR agrees (`Ty::Bytes`, not a bare `str`
        // `Ty::Ptr`) so a stream stored in a `bytes`-typed collection is not
        // mis-rejected now that `bytes`/`str` are distinct `Elem` kinds (#1074,
        // D-0241). `platform`/`cpu_source` return real `str`, so they stay `Ptr`.
        "stdin" => call_ret(lowerer, "zap_sys_stdin", a, Ty::Bytes, false),
        "stdout" => call_ret(lowerer, "zap_sys_stdout", a, Ty::Bytes, false),
        "stderr" => call_ret(lowerer, "zap_sys_stderr", a, Ty::Bytes, false),
        "write" => call_ret(lowerer, "zap_sys_write", a, Ty::I64, true),
        _ => return Ok(None),
    }))
}
