//! `sync` module — lowering facet (ticket #103, D-0032).
//!
//! Emits the backing `zap_mutex_*` / `zap_waitgroup_*` / `zap_once_*` runtime
//! calls. The handle is a `Ptr`; the count argument is an `int64` word.
//! [`lower_args`] passes the handle through and coerces integer arguments to
//! `i64`. These are infallible from the lowering's view (a negative `WaitGroup`
//! counter panics in the runtime, matching the collections' abort-on-misuse
//! convention), so they lower to plain (non-throwing) calls.

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "sync",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        // Mutex
        "mutex" => call_ret(lowerer, "zap_mutex_new", a, Ty::Ptr, false),
        "lock" => call_void(lowerer, "zap_mutex_lock", a, false),
        "unlock" => call_void(lowerer, "zap_mutex_unlock", a, false),
        "trylock" => call_bool(lowerer, "zap_mutex_trylock", a),
        // WaitGroup
        "waitgroup" => call_ret(lowerer, "zap_waitgroup_new", a, Ty::Ptr, false),
        "add" => call_void(lowerer, "zap_waitgroup_add", a, false),
        "done" => call_void(lowerer, "zap_waitgroup_done", a, false),
        "wait" => call_void(lowerer, "zap_waitgroup_wait", a, false),
        // Once
        "once" => call_ret(lowerer, "zap_once_new", a, Ty::Ptr, false),
        "once_begin" => call_bool(lowerer, "zap_once_begin", a),
        "once_end" => call_void(lowerer, "zap_once_end", a, false),
        _ => return Ok(None),
    }))
}
