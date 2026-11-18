//! `datetime` module — lowering facet (ticket #99, D-0056; timezones #1045,
//! D-0225).
//!
//! A `DateTime` is the built-in prelude `value struct` (year/month/day/hour/
//! minute/second, plus `offset` and `is_aware`, #1045). Building one from a
//! timestamp mirrors `gc.stats()`'s `MemStats`: `StructNew` then a `FieldSet`
//! per component, each civil field fed by a `zap_datetime_<field>(ts, is_local)`
//! runtime getter, then the `offset`/`is_aware` fields set directly.
//!
//! **Offset model (D-0225).** The civil fields are the *wall clock in `offset`*
//! (seconds east of UTC); the UTC instant is `timegm(civil) − offset`. A naive
//! value has `offset == 0, is_aware == false` and is byte-for-byte the old UTC
//! behaviour (`from_timestamp`/`now`/`parse`/`strptime`). The `*_local`
//! constructors attach the host offset and are aware. Arithmetic/comparisons
//! ([[D-0221]]) operate on the true instant and reduce to the old UTC math when
//! `offset == 0`; `weekday`/`day_of_year` keep using the local civil fields.
//!
//! **Zones.** `zone(name)`/`fixed_offset(minutes)` build the prelude `TimeZone`
//! value struct; `astimezone(dt, tz)` resolves the offset at the instant
//! (binary search over the bundled IANA tables, `zap_tz_resolve`) and rebuilds
//! the wall clock. `isoformat`/`rfc3339` render RFC-3339 with the offset suffix
//! when aware; `rfc3339_parse` parses `…(Z|±HH:MM)` to an aware value.

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

/// `DateTime` field index for the UTC offset (seconds east of UTC), #1045.
const F_OFFSET: u32 = 6;
/// `DateTime` field index for the aware flag (bool), #1045.
const F_AWARE: u32 = 7;

/// `TimeZone` field indices (name / offset-seconds / fixed-flag), #1045.
const TZ_NAME: u32 = 0;
const TZ_OFFSET: u32 = 1;
const TZ_FIXED: u32 = 2;

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

/// Resolve the prelude `TimeZone` struct id (#1045).
fn timezone_sid(lowerer: &FnLower) -> Result<u32, LowerError> {
    lowerer
        .ctx
        .ids
        .get("TimeZone")
        .copied()
        .ok_or_else(|| err("TimeZone struct unavailable (built-in prelude missing)"))
}

/// Build a `DateTime` value struct. The six civil fields come from the
/// `zap_datetime_<field>(getter_ts, is_local)` getters (so `getter_ts` is the
/// wall-clock-as-epoch when `is_local == 0`, or a UTC timestamp decomposed into
/// the host zone when `is_local == 1`). `offset` (seconds east of UTC) and
/// `aware` (an `i1`) are set directly (#1045, D-0225).
fn build_datetime(
    lowerer: &mut FnLower,
    getter_ts: Value,
    is_local: i64,
    offset: Value,
    aware: Value,
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
            args: vec![getter_ts, local],
        });
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val: v,
        });
    }
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: F_OFFSET,
        val: offset,
    });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: F_AWARE,
        val: aware,
    });
    Ok((dest, Ty::Struct(sid)))
}

/// Read the six civil `DateTime` fields of an already-lowered base value.
fn civil_fields(lowerer: &mut FnLower, base: Value, sid: u32) -> Vec<Value> {
    (0..6)
        .map(|idx| lowerer.field_get(base, sid, idx, Ty::I64))
        .collect()
}

/// Read the six `DateTime` fields of the (single) argument into `[y,mo,d,h,mi,s]`.
fn read_components(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<Vec<Value>, LowerError> {
    let sid = datetime_sid(lowerer)?;
    let (base, _) = lowerer.expr(&args[0].value)?;
    Ok(civil_fields(lowerer, base, sid))
}

/// Read the six `DateTime` fields of a single argument (#974).
fn components_of(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<Vec<Value>, LowerError> {
    let sid = datetime_sid(lowerer)?;
    let (base, _) = lowerer.expr(&arg.value)?;
    Ok(civil_fields(lowerer, base, sid))
}

/// A `DateTime` argument's UTC Unix instant (`timegm(civil) − offset`) together
/// with its stored `offset` (i64) and `is_aware` (i1). The shared basis for
/// arithmetic and comparisons (#974, D-0221; offset-aware since #1045): for a
/// naive value (`offset == 0`) the instant is exactly `zap_datetime_to_unix` of
/// the civil fields, so the behaviour reduces to the pre-#1045 UTC math.
fn instant_of(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Value, Value), LowerError> {
    let sid = datetime_sid(lowerer)?;
    let (base, _) = lowerer.expr(&arg.value)?;
    let comps = civil_fields(lowerer, base, sid);
    let local_ts = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Call {
        dest: Some(local_ts),
        func: "zap_datetime_to_unix".to_string(),
        args: comps,
    });
    let off = lowerer.field_get(base, sid, F_OFFSET, Ty::I64);
    let aware = lowerer.field_get(base, sid, F_AWARE, Ty::I1);
    let inst = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Bin {
        dest: inst,
        op: BinOp::Sub,
        lhs: local_ts,
        rhs: off,
    });
    Ok((inst, off, aware))
}

/// Lower a `(DateTime, fmt)` custom-format call: read the six civil fields plus
/// `offset`/`is_aware` of the first argument, lower the format string, and hand
/// everything to `zap_datetime_strftime` (#286; `%z`/`%Z` are meaningful once a
/// value is aware, #1045). The result is a fresh `str` (`Ty::Ptr`).
fn strftime(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
    let sid = datetime_sid(lowerer)?;
    let (base, _) = lowerer.expr(&args[0].value)?;
    let mut call_args = civil_fields(lowerer, base, sid);
    let (fmt, _) = lowerer.expr(&args[1].value)?;
    let off = lowerer.field_get(base, sid, F_OFFSET, Ty::I64);
    let aware_i1 = lowerer.field_get(base, sid, F_AWARE, Ty::I1);
    let aware_i64 = lowerer.coerce(aware_i1, Ty::I1, Ty::I64);
    call_args.push(fmt);
    call_args.push(off);
    call_args.push(aware_i64);
    let d = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: "zap_datetime_strftime".to_string(),
        args: call_args,
    });
    Ok((d, Ty::Ptr))
}

/// Lower `isoformat(dt)` / `rfc3339(dt)`: the six civil fields plus
/// `offset`/`is_aware` to `zap_datetime_isoformat` (RFC-3339 with the offset
/// suffix when aware, bare civil ISO when naive), #1045.
fn isoformat(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Ty), LowerError> {
    let sid = datetime_sid(lowerer)?;
    let (base, _) = lowerer.expr(&arg.value)?;
    let mut call_args = civil_fields(lowerer, base, sid);
    let off = lowerer.field_get(base, sid, F_OFFSET, Ty::I64);
    let aware_i1 = lowerer.field_get(base, sid, F_AWARE, Ty::I1);
    let aware_i64 = lowerer.coerce(aware_i1, Ty::I1, Ty::I64);
    call_args.push(off);
    call_args.push(aware_i64);
    let d = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: "zap_datetime_isoformat".to_string(),
        args: call_args,
    });
    Ok((d, Ty::Ptr))
}

/// Lower `fixed_offset(minutes)` / `zone(name)` into a `TimeZone` value struct.
fn build_timezone(
    lowerer: &mut FnLower,
    name: Value,
    offset: Value,
    fixed: Value,
) -> Result<(Value, Ty), LowerError> {
    let sid = timezone_sid(lowerer)?;
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: TZ_NAME,
        val: name,
    });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: TZ_OFFSET,
        val: offset,
    });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: TZ_FIXED,
        val: fixed,
    });
    Ok((dest, Ty::Struct(sid)))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let r = match method {
        "from_timestamp" | "from_timestamp_local" => {
            let (v, vt) = lowerer.expr(&args[0].value)?;
            // A float timestamp (AWS fractional `unixTimestamp`) is truncated to
            // whole seconds via `convert` (FPToSI); `coerce` only widens (#338).
            let ts = if vt.is_float() {
                lowerer.convert(v, vt, Ty::I64)
            } else {
                lowerer.coerce(v, vt, Ty::I64)
            };
            if method == "from_timestamp" {
                let off = lowerer.iconst(Ty::I64, 0);
                let aware = lowerer.bconst(false);
                build_datetime(lowerer, ts, 0, off, aware)?
            } else {
                let off = lowerer.new_value(Ty::I64);
                lowerer.emit(Inst::Call {
                    dest: Some(off),
                    func: "zap_datetime_local_offset".to_string(),
                    args: vec![ts],
                });
                let aware = lowerer.bconst(true);
                build_datetime(lowerer, ts, 1, off, aware)?
            }
        }
        "now" | "now_local" => {
            let ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(ts),
                func: "zap_datetime_now_unix".to_string(),
                args: vec![],
            });
            if method == "now" {
                let off = lowerer.iconst(Ty::I64, 0);
                let aware = lowerer.bconst(false);
                build_datetime(lowerer, ts, 0, off, aware)?
            } else {
                let off = lowerer.new_value(Ty::I64);
                lowerer.emit(Inst::Call {
                    dest: Some(off),
                    func: "zap_datetime_local_offset".to_string(),
                    args: vec![ts],
                });
                let aware = lowerer.bconst(true);
                build_datetime(lowerer, ts, 1, off, aware)?
            }
        }
        "parse" => {
            let (s, _) = lowerer.expr(&args[0].value)?;
            let ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(ts),
                func: "zap_datetime_parse_iso".to_string(),
                args: vec![s],
            });
            let off = lowerer.iconst(Ty::I64, 0);
            let aware = lowerer.bconst(false);
            build_datetime(lowerer, ts, 0, off, aware)?
        }
        // (str, strftime format) -> UTC Unix timestamp -> naive DateTime (#338).
        "strptime" => {
            let (s, _) = lowerer.expr(&args[0].value)?;
            let (fmt, _) = lowerer.expr(&args[1].value)?;
            let ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(ts),
                func: "zap_datetime_strptime".to_string(),
                args: vec![s, fmt],
            });
            let off = lowerer.iconst(Ty::I64, 0);
            let aware = lowerer.bconst(false);
            build_datetime(lowerer, ts, 0, off, aware)?
        }
        "to_timestamp" => {
            let (inst, _off, _aware) = instant_of(lowerer, &args[0])?;
            (inst, Ty::I64)
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
        "isoformat" | "rfc3339" => isoformat(lowerer, &args[0])?,
        "rfc3339_parse" => {
            let (s, _) = lowerer.expr(&args[0].value)?;
            // Civil wall clock as written (ISO parse ignores the zone suffix).
            let local_ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(local_ts),
                func: "zap_datetime_parse_iso".to_string(),
                args: vec![s],
            });
            // The `(Z|±HH:MM)` offset, in seconds (0 if none / `Z`).
            let off = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(off),
                func: "zap_datetime_rfc3339_offset".to_string(),
                args: vec![s],
            });
            let aware = lowerer.bconst(true);
            build_datetime(lowerer, local_ts, 0, off, aware)?
        }
        // Date arithmetic + comparisons over the true UTC instant (#974,
        // D-0221; offset-aware since #1045). `add` keeps the source zone.
        "add" => {
            let (inst, off, aware) = instant_of(lowerer, &args[0])?;
            let (sv, st) = lowerer.expr(&args[1].value)?;
            let sec = lowerer.coerce(sv, st, Ty::I64);
            let inst2 = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: inst2,
                op: BinOp::Add,
                lhs: inst,
                rhs: sec,
            });
            // Rebuild the wall clock in the same offset: getter_ts = inst2 + off.
            let local_ts2 = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: local_ts2,
                op: BinOp::Add,
                lhs: inst2,
                rhs: off,
            });
            build_datetime(lowerer, local_ts2, 0, off, aware)?
        }
        "diff" => {
            let (a, _, _) = instant_of(lowerer, &args[0])?;
            let (b, _, _) = instant_of(lowerer, &args[1])?;
            let d = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: d,
                op: BinOp::Sub,
                lhs: a,
                rhs: b,
            });
            (d, Ty::I64)
        }
        "before" | "after" | "equal" => {
            let (a, _, _) = instant_of(lowerer, &args[0])?;
            let (b, _, _) = instant_of(lowerer, &args[1])?;
            let pred = match method {
                "before" => CmpPred::Lt,
                "after" => CmpPred::Gt,
                _ => CmpPred::Eq,
            };
            let d = lowerer.new_value(Ty::I1);
            lowerer.emit(Inst::ICmp {
                dest: d,
                pred,
                lhs: a,
                rhs: b,
            });
            (d, Ty::I1)
        }
        "weekday" | "day_of_year" => {
            let comps = components_of(lowerer, &args[0])?;
            let func = if method == "weekday" {
                "zap_datetime_weekday"
            } else {
                "zap_datetime_day_of_year"
            };
            let d = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: func.to_string(),
                args: comps,
            });
            (d, Ty::I64)
        }
        "isoweekday" => {
            let comps = components_of(lowerer, &args[0])?;
            let w = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(w),
                func: "zap_datetime_weekday".to_string(),
                args: comps,
            });
            let one = lowerer.iconst(Ty::I64, 1);
            let d = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: d,
                op: BinOp::Add,
                lhs: w,
                rhs: one,
            });
            (d, Ty::I64)
        }
        // Timezone database (#1045, D-0225).
        "zone" => {
            let (nm, _) = lowerer.expr(&args[0].value)?;
            // Validate against the bundled zone index; returns the name back,
            // raising a catchable error for an unknown zone.
            let valid = lowerer.new_value(Ty::Ptr);
            lowerer.throwing_call(Some(valid), "zap_tz_zone_require", vec![nm]);
            let off = lowerer.iconst(Ty::I64, 0);
            let fixed = lowerer.bconst(false);
            build_timezone(lowerer, valid, off, fixed)?
        }
        "fixed_offset" => {
            let (mv, mt) = lowerer.expr(&args[0].value)?;
            let mins = lowerer.coerce(mv, mt, Ty::I64);
            let sixty = lowerer.iconst(Ty::I64, 60);
            let off = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: off,
                op: BinOp::Mul,
                lhs: mins,
                rhs: sixty,
            });
            let empty = lowerer.new_value(Ty::Ptr);
            lowerer.emit(Inst::StrConst {
                dest: empty,
                bytes: Vec::new(),
            });
            let fixed = lowerer.bconst(true);
            build_timezone(lowerer, empty, off, fixed)?
        }
        "astimezone" => {
            let (inst, _off, _aware) = instant_of(lowerer, &args[0])?;
            let tzsid = timezone_sid(lowerer)?;
            let (tzbase, _) = lowerer.expr(&args[1].value)?;
            let zname = lowerer.field_get(tzbase, tzsid, TZ_NAME, Ty::Ptr);
            let zoff = lowerer.field_get(tzbase, tzsid, TZ_OFFSET, Ty::I64);
            let zfixed_i1 = lowerer.field_get(tzbase, tzsid, TZ_FIXED, Ty::I1);
            let zfixed = lowerer.coerce(zfixed_i1, Ty::I1, Ty::I64);
            // Resolve the offset at the instant (fixed zone → its constant;
            // named zone → binary search the bundled IANA table, raising a
            // catchable error for an unknown name).
            let off = lowerer.new_value(Ty::I64);
            lowerer.throwing_call(Some(off), "zap_tz_resolve", vec![zname, zoff, zfixed, inst]);
            let local_ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: local_ts,
                op: BinOp::Add,
                lhs: inst,
                rhs: off,
            });
            let aware = lowerer.bconst(true);
            build_datetime(lowerer, local_ts, 0, off, aware)?
        }
        _ => return Ok(None),
    };
    Ok(Some(r))
}
