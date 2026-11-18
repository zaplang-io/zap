//! `time` module — lowering facet (ticket #99, D-0056; `Duration` #1047/D-0227).
//!
//! The clocks and sleeps lower to one backing `zap_time_*` runtime call (see
//! `zaprt.c`). A **duration is the prelude `Duration` value struct** (one
//! `int64 nanos` field, #1047): the unit constructors (`seconds`/`millis`/
//! `micros`/`nanos`) compute nanoseconds (reusing the existing `*_to_ns`
//! runtime entries) and **wrap** the result in a `Duration`; the converters
//! (`to_seconds`/…) and `sleep_for` **unwrap** `nanos` and reuse the existing
//! `ns_to_*` / `sleep_ns` entries. So the `Duration` migration adds no runtime
//! code — only struct wrap/unwrap in the lowering.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

/// Resolve the prelude `Duration` struct id (injected whenever the `time` or
/// `datetime` surface is used).
fn duration_sid(lowerer: &FnLower) -> Result<u32, LowerError> {
    lowerer
        .ctx
        .ids
        .get("Duration")
        .copied()
        .ok_or_else(|| err("Duration struct unavailable (built-in prelude missing)"))
}

/// Wrap an `int64`-nanosecond value in a fresh `Duration` value struct.
fn wrap_duration(lowerer: &mut FnLower, nanos: Value) -> Result<(Value, Ty), LowerError> {
    let sid = duration_sid(lowerer)?;
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: nanos,
    });
    Ok((dest, Ty::Struct(sid)))
}

/// Read the `nanos` field of a `Duration` argument.
fn unwrap_duration(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<Value, LowerError> {
    let sid = duration_sid(lowerer)?;
    let (base, _) = lowerer.expr(&arg.value)?;
    Ok(lowerer.field_get(base, sid, 0, Ty::I64))
}

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
        // sleep (raw-number forms, unchanged)
        "sleep" => return call1_void(lowerer, "zap_time_sleep_seconds", args, Ty::F64).map(Some),
        "sleep_ns" => return call1_void(lowerer, "zap_time_sleep_ns", args, Ty::I64).map(Some),
        // sleep_for(d: Duration): unwrap nanos, reuse the ns sleep (#1047).
        "sleep_for" => {
            let nanos = unwrap_duration(lowerer, &args[0])?;
            lowerer.emit(Inst::Call {
                dest: None,
                func: "zap_time_sleep_ns".to_string(),
                args: vec![nanos],
            });
            (lowerer.iconst(Ty::I32, 0), Ty::I32)
        }
        // duration unit constructors (-> Duration, #1047): compute nanoseconds
        // via the existing runtime entries, then wrap.
        "seconds" | "millis" | "micros" => {
            let n = arg1(lowerer, args, Ty::I64)?;
            let func = match method {
                "seconds" => "zap_time_seconds_to_ns",
                "millis" => "zap_time_millis_to_ns",
                _ => "zap_time_micros_to_ns",
            };
            let ns = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(ns),
                func: func.to_string(),
                args: vec![n],
            });
            return wrap_duration(lowerer, ns).map(Some);
        }
        // nanos(n): the identity on nanoseconds, wrapped.
        "nanos" => {
            let n = arg1(lowerer, args, Ty::I64)?;
            return wrap_duration(lowerer, n).map(Some);
        }
        // time.format(d) -> str: Go-style human-readable Duration (#1055).
        "format" => {
            let nanos = unwrap_duration(lowerer, &args[0])?;
            let d = lowerer.new_value(Ty::Ptr);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_duration_format".to_string(),
                args: vec![nanos],
            });
            (d, Ty::Ptr)
        }
        // time.since(start_ns) -> Duration: (monotonic_ns() - start_ns), wrapped.
        "since" => {
            let start = arg1(lowerer, args, Ty::I64)?;
            let now = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(now),
                func: "zap_time_monotonic_ns".to_string(),
                args: vec![],
            });
            let delta = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: delta,
                op: BinOp::Sub,
                lhs: now,
                rhs: start,
            });
            return wrap_duration(lowerer, delta).map(Some);
        }
        // duration unit converters (Duration -> unit as float, #1047): unwrap
        // nanos, reuse the existing ns->unit runtime entries.
        "to_seconds" | "to_millis" | "to_micros" => {
            let nanos = unwrap_duration(lowerer, &args[0])?;
            let func = match method {
                "to_seconds" => "zap_time_ns_to_seconds",
                "to_millis" => "zap_time_ns_to_millis",
                _ => "zap_time_ns_to_micros",
            };
            let d = lowerer.new_value(Ty::F64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: func.to_string(),
                args: vec![nanos],
            });
            (d, Ty::F64)
        }
        _ => return Ok(None),
    };
    Ok(Some(r))
}
