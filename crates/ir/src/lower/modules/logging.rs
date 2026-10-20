//! `logging` module — lowering facet (ticket #101, D-0056).
//!
//! The per-level emitters (`info`/`warning`/…) lower to the generic
//! `zap_logging_log(level, msg)` with the level prepended as an `i64` constant;
//! `log(level, msg)` passes both through. The level thresholds (`logging.DEBUG`,
//! …) are attributes, lowered by [`const_lower`] via the const sub-registry in
//! `super` (like `math.pi`).

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
use super::{ModuleConst, ModuleLower};
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "logging",
    lower,
};

pub(super) const MODULE_CONST: ModuleConst = ModuleConst {
    name: "logging",
    lower: const_lower,
};

/// Lower a `logging.<LEVEL>` attribute access to its `i64` threshold value
/// (Python's numeric levels).
fn const_lower(lowerer: &mut FnLower, name: &str) -> Option<(Value, Ty)> {
    let v = level_value(name)?;
    Some((lowerer.iconst(Ty::I64, v), Ty::I64))
}

/// The numeric threshold for a named level, or `None` for an unknown name.
fn level_value(name: &str) -> Option<i64> {
    Some(match name {
        "DEBUG" => 10,
        "INFO" => 20,
        "WARNING" => 30,
        "ERROR" => 40,
        "CRITICAL" => 50,
        _ => return None,
    })
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // Per-level emitters: synthesize the level constant, then the lowered `msg`.
    if let Some(level) = level_value(&method.to_ascii_uppercase()) {
        let lvl = lowerer.iconst(Ty::I64, level);
        let mut a = vec![lvl];
        a.extend(lower_args(lowerer, args)?);
        return Ok(Some(call_void(lowerer, "zap_logging_log", a, false)));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "log" => call_void(lowerer, "zap_logging_log", a, false),
        "set_level" => call_void(lowerer, "zap_logging_set_level", a, false),
        "get_level" => call_ret(lowerer, "zap_logging_get_level", a, Ty::I64, false),
        "set_stream" => call_void(lowerer, "zap_logging_set_stream", a, false),
        "set_format" => call_void(lowerer, "zap_logging_set_format", a, false),
        _ => return Ok(None),
    }))
}
