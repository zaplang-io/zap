//! `bisect` module — lowering facet (ticket #102, D-0056).
//!
//! Emits the backing `zap_bisect_*` runtime calls over a `ZapList` handle. The
//! list argument is a `Ptr` (passed through by [`lower_args`]); the value is an
//! `int64`. `insort_*` mutate the list in place (no result).

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "bisect",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "bisect_left" => call_ret(lowerer, "zap_bisect_left", a, Ty::I64, false),
        "bisect_right" => call_ret(lowerer, "zap_bisect_right", a, Ty::I64, false),
        "insort_left" => call_void(lowerer, "zap_bisect_insort_left", a, false),
        "insort_right" => call_void(lowerer, "zap_bisect_insort_right", a, false),
        _ => return Ok(None),
    }))
}
