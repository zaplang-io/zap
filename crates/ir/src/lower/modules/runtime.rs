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
        "goroutine_dump" => Ok(Some(call0(lowerer, "zap_runtime_goroutine_dump", Ty::Void))),
        "num_goroutines" => Ok(Some(call0(lowerer, "zap_runtime_num_goroutines", Ty::I64))),
        _ => Ok(None),
    }
}
