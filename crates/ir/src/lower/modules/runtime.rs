//! `runtime` module — lowering facet (ticket #77, D-0058).

use zapc_syntax::ast;

use super::{call0, ModuleLower};
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "runtime",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    _args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        "coroutine_dump" => Ok(Some(call0(lowerer, "zap_runtime_coroutine_dump", Ty::Void))),
        "num_coroutines" => Ok(Some(call0(lowerer, "zap_runtime_num_coroutines", Ty::I64))),
        "maxprocs" => Ok(Some(call0(lowerer, "zap_runtime_maxprocs", Ty::I64))),
        "lock_os_thread" => Ok(Some(call0(lowerer, "zap_runtime_lock_os_thread", Ty::Void))),
        "unlock_os_thread" => Ok(Some(call0(
            lowerer,
            "zap_runtime_unlock_os_thread",
            Ty::Void,
        ))),
        "profile_start" => Ok(Some(call0(lowerer, "zap_profile_start", Ty::Void))),
        "profile_stop" => Ok(Some(call0(lowerer, "zap_profile_stop", Ty::Void))),
        _ => Ok(None),
    }
}
