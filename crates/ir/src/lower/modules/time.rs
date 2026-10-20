//! `time` module — lowering facet (ticket #99, D-0056).
//!
//! Every method lowers to one backing `zap_time_*` runtime call (see `zaprt.c`);
//! durations are plain `int64` nanoseconds, so arithmetic needs no runtime
//! support. `time.nanos(n)` is the identity on nanoseconds and lowers to its
//! (coerced) argument with no call.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "time",
    lower,
};

/// Emit a zero-argument runtime call returning `ret`.
fn call0(lowerer: &mut FnLower, func: &str, ret: Ty) -> (Value, Ty) {
    let d = lowerer.new_value(ret);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args: vec![],
    });
    (d, ret)
}

/// Lower the single argument and coerce it to `to`.
fn arg1(lowerer: &mut FnLower, args: &[ast::Arg], to: Ty) -> Result<Value, LowerError> {
    let (v, vt) = lowerer.expr(&args[0].value)?;
    Ok(lowerer.coerce(v, vt, to))
}

/// Emit a one-argument runtime call: coerce the argument to `at`, call `func`,
/// return its `ret`-typed result.
fn call1(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
    at: Ty,
    ret: Ty,
) -> Result<(Value, Ty), LowerError> {
    let a = arg1(lowerer, args, at)?;
    let d = lowerer.new_value(ret);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args: vec![a],
    });
    Ok((d, ret))
}

/// Emit a one-argument void runtime call (statement position), yielding a
/// throwaway `i32 0` so the call expression still has a value.
fn call1_void(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
    at: Ty,
) -> Result<(Value, Ty), LowerError> {
    let a = arg1(lowerer, args, at)?;
    lowerer.emit(Inst::Call {
        dest: None,
        func: func.to_string(),
        args: vec![a],
    });
    Ok((lowerer.iconst(Ty::I32, 0), Ty::I32))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let r = match method {
        // clocks
        "monotonic_ns" => call0(lowerer, "zap_time_monotonic_ns", Ty::I64),
        "monotonic" => call0(lowerer, "zap_time_monotonic_seconds", Ty::F64),
        "time_ns" => call0(lowerer, "zap_time_wall_ns", Ty::I64),
        "time" => call0(lowerer, "zap_time_wall_seconds", Ty::F64),
        // sleep
        "sleep" => return call1_void(lowerer, "zap_time_sleep_seconds", args, Ty::F64).map(Some),
        "sleep_ns" => return call1_void(lowerer, "zap_time_sleep_ns", args, Ty::I64).map(Some),
        // duration unit constructors (-> nanoseconds)
        "seconds" => {
            return call1(lowerer, "zap_time_seconds_to_ns", args, Ty::I64, Ty::I64).map(Some)
        }
        "millis" => {
            return call1(lowerer, "zap_time_millis_to_ns", args, Ty::I64, Ty::I64).map(Some)
        }
        "micros" => {
            return call1(lowerer, "zap_time_micros_to_ns", args, Ty::I64, Ty::I64).map(Some)
        }
        // nanos(n) is the identity on nanoseconds.
        "nanos" => (arg1(lowerer, args, Ty::I64)?, Ty::I64),
        // duration unit converters (nanoseconds -> unit as float)
        "to_seconds" => {
            return call1(lowerer, "zap_time_ns_to_seconds", args, Ty::I64, Ty::F64).map(Some)
        }
        "to_millis" => {
            return call1(lowerer, "zap_time_ns_to_millis", args, Ty::I64, Ty::F64).map(Some)
        }
        "to_micros" => {
            return call1(lowerer, "zap_time_ns_to_micros", args, Ty::I64, Ty::F64).map(Some)
        }
        _ => return Ok(None),
    };
    Ok(Some(r))
}
