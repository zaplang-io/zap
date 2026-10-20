//! `deque` module — lowering facet (ticket #102, D-0056).
//!
//! Emits the backing `zap_deque_*` runtime calls. The handle is a `Ptr`; value
//! arguments are `int64` words. [`lower_args`] passes the handle through and
//! coerces integer arguments to `i64`. Pop-from-empty aborts in the runtime
//! (no exception runtime yet, ticket #22), so these are plain (non-throwing)
//! calls.

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "deque",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "new" => call_ret(lowerer, "zap_deque_new", a, Ty::Ptr, false),
        "bounded" => call_ret(lowerer, "zap_deque_bounded", a, Ty::Ptr, false),
        "append" => call_void(lowerer, "zap_deque_append", a, false),
        "appendleft" => call_void(lowerer, "zap_deque_appendleft", a, false),
        "pop" => call_ret(lowerer, "zap_deque_pop", a, Ty::I64, false),
        "popleft" => call_ret(lowerer, "zap_deque_popleft", a, Ty::I64, false),
        "len" => call_ret(lowerer, "zap_deque_len", a, Ty::I64, false),
        "get" => call_ret(lowerer, "zap_deque_get", a, Ty::I64, false),
        "clear" => call_void(lowerer, "zap_deque_clear", a, false),
        _ => return Ok(None),
    }))
}
