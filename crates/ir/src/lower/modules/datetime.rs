//! `datetime` module — lowering facet (ticket #99, D-0056).
//!
//! A `DateTime` is the built-in prelude `value struct` (year/month/day/hour/
//! minute/second). Building one from a timestamp mirrors `gc.stats()`'s
//! `MemStats`: `StructNew` then a `FieldSet` per component, each fed by a
//! `zap_datetime_<field>(ts, is_local)` runtime getter. `to_timestamp`/`format`
//! read the six fields back with `FieldGet` and hand them to a single runtime
//! call. `strftime` / 2-arg `format(dt, fmt)` read the same six fields and pass
//! them plus the format `ZapStr*` to `zap_datetime_strftime` (#286). `parse`
//! lowers an ISO-8601 string to a Unix timestamp and then builds a UTC
//! `DateTime` from it, reusing the getters.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "datetime",
    lower,
};

/// The prelude `DateTime` field getters, in field order
/// (year, month, day, hour, minute, second).
const GETTERS: [&str; 6] = [
    "zap_datetime_year",
    "zap_datetime_month",
    "zap_datetime_day",
    "zap_datetime_hour",
    "zap_datetime_minute",
    "zap_datetime_second",
];

/// Resolve the prelude `DateTime` struct id (injected by the parser's prelude
/// whenever the `datetime` surface is used).
fn datetime_sid(lowerer: &FnLower) -> Result<u32, LowerError> {
    lowerer
        .ctx
        .ids
        .get("DateTime")
        .copied()
        .ok_or_else(|| err("DateTime struct unavailable (built-in prelude missing)"))
}

/// Build a `DateTime` value struct from a Unix-timestamp value, interpreting it
/// in UTC (`is_local == 0`) or the host's local zone (`is_local == 1`). Each
/// field comes from its `zap_datetime_<field>(ts, is_local)` getter.
fn build_datetime(
    lowerer: &mut FnLower,
    ts: Value,
    is_local: i64,
) -> Result<(Value, Ty), LowerError> {
    let sid = datetime_sid(lowerer)?;
    let local = lowerer.iconst(Ty::I64, is_local);
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    for (idx, func) in GETTERS.iter().enumerate() {
        let v = lowerer.new_value(Ty::I64);
        lowerer.emit(Inst::Call {
            dest: Some(v),
            func: (*func).to_string(),
            args: vec![ts, local],
        });
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val: v,
        });
    }
    Ok((dest, Ty::Struct(sid)))
}

/// Read the six `DateTime` fields of the (single) argument into a `[y,mo,d,h,mi,s]`
/// value vector.
fn read_components(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<Vec<Value>, LowerError> {
    let sid = datetime_sid(lowerer)?;
    let (base, _) = lowerer.expr(&args[0].value)?;
    let mut comps = Vec::with_capacity(6);
    for idx in 0..6 {
        comps.push(lowerer.field_get(base, sid, idx, Ty::I64));
    }
    Ok(comps)
}

/// Lower a `(DateTime, fmt)` custom-format call: read the six fields of the
/// first argument, lower the format string, and hand both to the runtime
/// `zap_datetime_strftime` (#286). The result is a fresh `str` (`Ty::Ptr`).
fn strftime(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
    let mut call_args = read_components(lowerer, args)?;
    let (fmt, _) = lowerer.expr(&args[1].value)?;
    call_args.push(fmt);
    let d = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: "zap_datetime_strftime".to_string(),
        args: call_args,
    });
    Ok((d, Ty::Ptr))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let r = match method {
        "from_timestamp" | "from_timestamp_local" => {
            let (v, vt) = lowerer.expr(&args[0].value)?;
            let ts = lowerer.coerce(v, vt, Ty::I64);
            let is_local = i64::from(method == "from_timestamp_local");
            build_datetime(lowerer, ts, is_local)?
        }
        "now" | "now_local" => {
            let ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(ts),
                func: "zap_datetime_now_unix".to_string(),
                args: vec![],
            });
            let is_local = i64::from(method == "now_local");
            build_datetime(lowerer, ts, is_local)?
        }
        "parse" => {
            let (s, _) = lowerer.expr(&args[0].value)?;
            let ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(ts),
                func: "zap_datetime_parse_iso".to_string(),
                args: vec![s],
            });
            build_datetime(lowerer, ts, 0)?
        }
        "to_timestamp" => {
            let comps = read_components(lowerer, args)?;
            let d = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_datetime_to_unix".to_string(),
                args: comps,
            });
            (d, Ty::I64)
        }
        // One arg: ISO-8601. Two args: strftime-style custom format (#286).
        "format" if args.len() == 2 => strftime(lowerer, args)?,
        "format" => {
            let comps = read_components(lowerer, args)?;
            let d = lowerer.new_value(Ty::Ptr);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_datetime_format_iso".to_string(),
                args: comps,
            });
            (d, Ty::Ptr)
        }
        "strftime" => strftime(lowerer, args)?,
        _ => return Ok(None),
    };
    Ok(Some(r))
}
