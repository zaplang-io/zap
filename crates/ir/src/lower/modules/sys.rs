//! `sys` module — lowering facet (ticket #97, D-0056).

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
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
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "argv" => call_ret(lowerer, "zap_sys_argv", a, Ty::List(Elem::Ptr), false),
        "exit" => call_void(lowerer, "zap_sys_exit", a, false),
        "platform" => call_ret(lowerer, "zap_sys_platform", a, Ty::Ptr, false),
        _ => return Ok(None),
    }))
}
