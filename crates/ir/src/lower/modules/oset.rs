//! `oset` module — lowering facet (ticket #102, D-0056).
//!
//! Emits the backing `zap_oset_*` runtime calls. The handle is a `Ptr`; values
//! are `int64` words. `contains` returns a C `int64_t` boolean normalized to an
//! `i1` via [`call_bool`].

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "oset",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "new" => call_ret(lowerer, "zap_oset_new", a, Ty::Ptr, false),
        "add" => call_void(lowerer, "zap_oset_add", a, false),
        "contains" => call_bool(lowerer, "zap_oset_contains", a),
        "len" => call_ret(lowerer, "zap_oset_len", a, Ty::I64, false),
        "get" => call_ret(lowerer, "zap_oset_get", a, Ty::I64, false),
        _ => return Ok(None),
    }))
}
