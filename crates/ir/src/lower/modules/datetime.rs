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
use crate::lower::{err, optional_layout, FnLower, LowerError};

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
/// `DateTime` field index for the named-zone handle (#1149): `-1` for a naive or
/// fixed-offset value, else an index into the bundled name-sorted zone table.
/// Keeps `DateTime` a non-GC-traced POD (a plain integer, not a zone pointer).
const F_ZONE_ID: u32 = 8;
/// `DateTime` field index for the fractional second (#1148): nanoseconds-of-
/// second, `0..=999_999_999`. A plain integer, so `DateTime` stays a non-GC-
/// traced POD; `StructNew` zero-inits it, so whole-second values default to 0.
const F_NANOSECOND: u32 = 9;
/// The zone handle for a value with no retained IANA zone (naive / fixed-offset).
const ZONE_NONE: i64 = -1;

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

/// Resolve a prelude struct id by name (`Date` / `Time`, #1046).
fn struct_sid(lowerer: &FnLower, name: &str) -> Result<u32, LowerError> {
    lowerer
        .ctx
        .ids
        .get(name)
        .copied()
        .ok_or_else(|| err("prelude struct unavailable (built-in prelude missing)"))
}

/// The `DateTime`/`Date`/`Time` civil fields `replace` may override, in field
/// order (#1046). Shared with the sema facet's `REPLACE_FIELDS`.
const REPLACE_FIELDS: [&str; 6] = ["year", "month", "day", "hour", "minute", "second"];

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
    zone_id: Value,
    nanos: Value,
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
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: F_ZONE_ID,
        val: zone_id,
    });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: F_NANOSECOND,
        val: nanos,
    });
    Ok((dest, Ty::Struct(sid)))
}

/// A `zone_id` constant for a value carrying no retained IANA zone.
fn zone_none(lowerer: &mut FnLower) -> Value {
    lowerer.iconst(Ty::I64, ZONE_NONE)
}

/// `build_datetime` for a value with no retained IANA zone (`zone_id == -1`):
/// every naive constructor, the host-`*_local` ones, `combine`, and the
/// fixed-offset / offset-parsed paths. Only `astimezone` (a named zone) and
/// `add` (zone preserved) pass an explicit `zone_id` to `build_datetime`.
fn build_datetime_naive(
    lowerer: &mut FnLower,
    getter_ts: Value,
    is_local: i64,
    offset: Value,
    aware: Value,
) -> Result<(Value, Ty), LowerError> {
    let zid = zone_none(lowerer);
    let nanos = lowerer.iconst(Ty::I64, 0);
    build_datetime(lowerer, getter_ts, is_local, offset, aware, zid, nanos)
}

/// `build_datetime` for a value with no retained IANA zone but a fractional
/// second (#1148): the ISO / RFC-3339 parsers, which read a `.fff…` component.
fn build_datetime_naive_frac(
    lowerer: &mut FnLower,
    getter_ts: Value,
    is_local: i64,
    offset: Value,
    aware: Value,
    nanos: Value,
) -> Result<(Value, Ty), LowerError> {
    let zid = zone_none(lowerer);
    build_datetime(lowerer, getter_ts, is_local, offset, aware, zid, nanos)
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
fn instant_of(
    lowerer: &mut FnLower,
    arg: &ast::Arg,
) -> Result<(Value, Value, Value, Value, Value), LowerError> {
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
    let zone_id = lowerer.field_get(base, sid, F_ZONE_ID, Ty::I64);
    let nanos = lowerer.field_get(base, sid, F_NANOSECOND, Ty::I64);
    let inst = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Bin {
        dest: inst,
        op: BinOp::Sub,
        lhs: local_ts,
        rhs: off,
    });
    Ok((inst, off, aware, zone_id, nanos))
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

/// Lower `isoformat(x)` / `rfc3339(dt)`, overloaded on the argument type
/// (#1045/#1046): a `DateTime` renders RFC-3339 (`zap_datetime_isoformat`, with
/// the offset suffix when aware); a `Date` renders `YYYY-MM-DD`
/// (`zap_date_isoformat`); a `Time` renders `HH:MM:SS` (`zap_time_isoformat`).
/// Dispatch is by the lowered struct id (sema guarantees one of the three).
fn isoformat(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Ty), LowerError> {
    let (base, ty) = lowerer.expr(&arg.value)?;
    let date_sid = struct_sid(lowerer, "Date")?;
    let time_sid = struct_sid(lowerer, "Time")?;
    let d = lowerer.new_value(Ty::Ptr);
    let (func, args): (&str, Vec<Value>) = match ty {
        Ty::Struct(s) if s == date_sid => (
            "zap_date_isoformat",
            (0..3)
                .map(|i| lowerer.field_get(base, date_sid, i, Ty::I64))
                .collect(),
        ),
        Ty::Struct(s) if s == time_sid => (
            "zap_time_isoformat",
            (0..3)
                .map(|i| lowerer.field_get(base, time_sid, i, Ty::I64))
                .collect(),
        ),
        _ => {
            // DateTime: six civil fields + offset + is_aware + nanosecond (#1148).
            let sid = datetime_sid(lowerer)?;
            let mut call_args = civil_fields(lowerer, base, sid);
            let off = lowerer.field_get(base, sid, F_OFFSET, Ty::I64);
            let aware_i1 = lowerer.field_get(base, sid, F_AWARE, Ty::I1);
            let aware_i64 = lowerer.coerce(aware_i1, Ty::I1, Ty::I64);
            let nanos = lowerer.field_get(base, sid, F_NANOSECOND, Ty::I64);
            call_args.push(off);
            call_args.push(aware_i64);
            call_args.push(nanos);
            ("zap_datetime_isoformat", call_args)
        }
    };
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args,
    });
    Ok((d, Ty::Ptr))
}

/// Build a 3-field `Date`/`Time` value struct from three `i64` values.
fn build_triple(
    lowerer: &mut FnLower,
    sid: u32,
    vals: [Value; 3],
) -> Result<(Value, Ty), LowerError> {
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    for (idx, v) in vals.into_iter().enumerate() {
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val: v,
        });
    }
    Ok((dest, Ty::Struct(sid)))
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
                build_datetime_naive(lowerer, ts, 0, off, aware)?
            } else {
                let off = lowerer.new_value(Ty::I64);
                lowerer.emit(Inst::Call {
                    dest: Some(off),
                    func: "zap_datetime_local_offset".to_string(),
                    args: vec![ts],
                });
                let aware = lowerer.bconst(true);
                build_datetime_naive(lowerer, ts, 1, off, aware)?
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
                build_datetime_naive(lowerer, ts, 0, off, aware)?
            } else {
                let off = lowerer.new_value(Ty::I64);
                lowerer.emit(Inst::Call {
                    dest: Some(off),
                    func: "zap_datetime_local_offset".to_string(),
                    args: vec![ts],
                });
                let aware = lowerer.bconst(true);
                build_datetime_naive(lowerer, ts, 1, off, aware)?
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
            // Fractional second (#1148): nanoseconds-of-second, 0 when absent.
            let nanos = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(nanos),
                func: "zap_datetime_parse_nanos".to_string(),
                args: vec![s],
            });
            let off = lowerer.iconst(Ty::I64, 0);
            let aware = lowerer.bconst(false);
            build_datetime_naive_frac(lowerer, ts, 0, off, aware, nanos)?
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
            build_datetime_naive(lowerer, ts, 0, off, aware)?
        }
        "to_timestamp" => {
            let (inst, _off, _aware, _zid, _ns) = instant_of(lowerer, &args[0])?;
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
            // Fractional second (#1148): nanoseconds-of-second, 0 when absent.
            let nanos = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(nanos),
                func: "zap_datetime_parse_nanos".to_string(),
                args: vec![s],
            });
            let aware = lowerer.bconst(true);
            // An offset-parsed value carries a fixed numeric offset, no IANA zone.
            build_datetime_naive_frac(lowerer, local_ts, 0, off, aware, nanos)?
        }
        // add(dt, delta) -> DateTime: pure-Python WALL-CLOCK arithmetic (#974,
        // D-0221; zone-aware since #1149, §4). The Duration advances the NAIVE
        // wall-clock fields (days and sub-day units alike — in Python
        // `timedelta(days=1) == timedelta(seconds=86400)`, both naive-field
        // additions), then the offset is RE-RESOLVED at the new naive wall time
        // (fold = 0). For a naive / fixed-offset value (`zone_id == -1`) the
        // re-resolve returns the stored offset unchanged, and advancing the naive
        // wall clock equals the old instant-plus-delta math, so those results are
        // byte-for-byte identical to pre-#1149.
        "add" => {
            let sid = datetime_sid(lowerer)?;
            let (base, _) = lowerer.expr(&args[0].value)?;
            let civil = civil_fields(lowerer, base, sid);
            let old_off = lowerer.field_get(base, sid, F_OFFSET, Ty::I64);
            let aware = lowerer.field_get(base, sid, F_AWARE, Ty::I1);
            let zone_id = lowerer.field_get(base, sid, F_ZONE_ID, Ty::I64);
            let old_nanos = lowerer.field_get(base, sid, F_NANOSECOND, Ty::I64);
            // The naive wall clock as an epoch (timegm of the civil fields).
            let wall0 = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(wall0),
                func: "zap_datetime_to_unix".to_string(),
                args: civil,
            });
            // delta is a Duration (ns, #1047). Combine it with the base's
            // nanoseconds-of-second and split FLOORED into a whole-second carry
            // and the new fractional second (#1148), so a ±1µs delta is preserved
            // and borrows correctly across a second (and thus day/month/year)
            // boundary — no more truncating the Duration to whole seconds.
            let dur_sid = struct_sid(lowerer, "Duration")?;
            let (dbase, _) = lowerer.expr(&args[1].value)?;
            let delta = lowerer.field_get(dbase, dur_sid, 0, Ty::I64);
            let carry = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(carry),
                func: "zap_datetime_nanos_carry".to_string(),
                args: vec![old_nanos, delta],
            });
            let new_nanos = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(new_nanos),
                func: "zap_datetime_nanos_of_second".to_string(),
                args: vec![old_nanos, delta],
            });
            // Advance the NAIVE wall clock by the whole-second carry (not the UTC
            // instant); the fractional remainder rides in `new_nanos`.
            let wall1 = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: wall1,
                op: BinOp::Add,
                lhs: wall0,
                rhs: carry,
            });
            // Recompose the new civil fields from the advanced wall clock
            // (is_local = 0: the wall epoch is decomposed as UTC civil fields).
            let utc = lowerer.iconst(Ty::I64, 0);
            let mut new_civil: Vec<Value> = Vec::with_capacity(6);
            for func in GETTERS {
                let v = lowerer.new_value(Ty::I64);
                lowerer.emit(Inst::Call {
                    dest: Some(v),
                    func: func.to_string(),
                    args: vec![wall1, utc],
                });
                new_civil.push(v);
            }
            // Re-resolve the offset at the new naive wall time (fold = 0); a
            // naive / fixed value (zone_id == -1) returns `old_off` unchanged.
            let fold = lowerer.iconst(Ty::I64, 0);
            let mut rl = Vec::with_capacity(9);
            rl.push(zone_id);
            rl.extend_from_slice(&new_civil);
            rl.push(fold);
            rl.push(old_off);
            let off2 = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(off2),
                func: "zap_tz_resolve_local".to_string(),
                args: rl,
            });
            // Build with the advanced civil fields verbatim + the resolved offset.
            let dest = lowerer.new_value(Ty::Struct(sid));
            lowerer.emit(Inst::StructNew { dest, sid });
            for (idx, v) in new_civil.into_iter().enumerate() {
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
                val: off2,
            });
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: F_AWARE,
                val: aware,
            });
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: F_ZONE_ID,
                val: zone_id,
            });
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: F_NANOSECOND,
                val: new_nanos,
            });
            (dest, Ty::Struct(sid))
        }
        "diff" => {
            let (a, _, _, _, a_ns) = instant_of(lowerer, &args[0])?;
            let (b, _, _, _, b_ns) = instant_of(lowerer, &args[1])?;
            let secs = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: secs,
                op: BinOp::Sub,
                lhs: a,
                rhs: b,
            });
            // -> Duration (ns). The whole-second difference × 1e9 plus the
            // sub-second difference (#1148), so a microsecond-scale delta is
            // observed. (Within ±292 years the product fits int64, the Duration
            // range, D-0227.)
            let dur_sid = struct_sid(lowerer, "Duration")?;
            let billion = lowerer.iconst(Ty::I64, 1_000_000_000);
            let sec_ns = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: sec_ns,
                op: BinOp::Mul,
                lhs: secs,
                rhs: billion,
            });
            let frac = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: frac,
                op: BinOp::Sub,
                lhs: a_ns,
                rhs: b_ns,
            });
            let nanos = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: nanos,
                op: BinOp::Add,
                lhs: sec_ns,
                rhs: frac,
            });
            let dest = lowerer.new_value(Ty::Struct(dur_sid));
            lowerer.emit(Inst::StructNew { dest, sid: dur_sid });
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid: dur_sid,
                idx: 0,
                val: nanos,
            });
            (dest, Ty::Struct(dur_sid))
        }
        "before" | "after" | "equal" => {
            let (a, _, _, _, a_ns) = instant_of(lowerer, &args[0])?;
            let (b, _, _, _, b_ns) = instant_of(lowerer, &args[1])?;
            // Compare (instant, nanosecond) LEXICOGRAPHICALLY (#1148): the
            // sub-second fraction is observed without narrowing the date range a
            // single `instant·1e9 + nanos` int64 would (overflows outside
            // ≈1678–2262). `equal` = both equal; `before`/`after` = instant
            // ordered, or equal instants with the fraction ordered.
            let inst_eq = lowerer.new_value(Ty::I1);
            lowerer.emit(Inst::ICmp {
                dest: inst_eq,
                pred: CmpPred::Eq,
                lhs: a,
                rhs: b,
            });
            let d = if method == "equal" {
                let ns_eq = lowerer.new_value(Ty::I1);
                lowerer.emit(Inst::ICmp {
                    dest: ns_eq,
                    pred: CmpPred::Eq,
                    lhs: a_ns,
                    rhs: b_ns,
                });
                let d = lowerer.new_value(Ty::I1);
                lowerer.emit(Inst::Bin {
                    dest: d,
                    op: BinOp::And,
                    lhs: inst_eq,
                    rhs: ns_eq,
                });
                d
            } else {
                let pred = if method == "before" {
                    CmpPred::Lt
                } else {
                    CmpPred::Gt
                };
                let inst_ord = lowerer.new_value(Ty::I1);
                lowerer.emit(Inst::ICmp {
                    dest: inst_ord,
                    pred,
                    lhs: a,
                    rhs: b,
                });
                let ns_ord = lowerer.new_value(Ty::I1);
                lowerer.emit(Inst::ICmp {
                    dest: ns_ord,
                    pred,
                    lhs: a_ns,
                    rhs: b_ns,
                });
                // inst_ord | (inst_eq & ns_ord)
                let tie = lowerer.new_value(Ty::I1);
                lowerer.emit(Inst::Bin {
                    dest: tie,
                    op: BinOp::And,
                    lhs: inst_eq,
                    rhs: ns_ord,
                });
                let d = lowerer.new_value(Ty::I1);
                lowerer.emit(Inst::Bin {
                    dest: d,
                    op: BinOp::Or,
                    lhs: inst_ord,
                    rhs: tie,
                });
                d
            };
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
        // local_zone() -> TimeZone (#1171): the host system's configured zone as
        // a NAMED handle (a bundled IANA name), so it re-resolves across DST in
        // astimezone/resolve just like zone(name). Raises (throwing) when the
        // host zone cannot be resolved to a bundled key.
        "local_zone" => {
            let name = lowerer.new_value(Ty::Ptr);
            lowerer.throwing_call(Some(name), "zap_datetime_local_zone_name", vec![]);
            let off = lowerer.iconst(Ty::I64, 0);
            let fixed = lowerer.bconst(false);
            build_timezone(lowerer, name, off, fixed)?
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
            let (inst, _off, _aware, _zid, src_ns) = instant_of(lowerer, &args[0])?;
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
            // Retain the zone identity on the value (#1149): a named zone stores
            // its bundled-table index so later wall-clock ops re-apply its rules;
            // a fixed zone stores -1 (its constant offset is already in `off`).
            // Non-throwing — the name was validated by `zone()`.
            let zid = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(zid),
                func: "zap_tz_zone_id".to_string(),
                args: vec![zname, zfixed],
            });
            let local_ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: local_ts,
                op: BinOp::Add,
                lhs: inst,
                rhs: off,
            });
            let aware = lowerer.bconst(true);
            // The sub-second fraction is independent of the zone — carry it (#1148).
            build_datetime(lowerer, local_ts, 0, off, aware, zid, src_ns)?
        }
        // replace(dt, field=val, ...) -> DateTime (#1046, D-0226; zone-aware
        // since #1149): copy the civil fields, override the supplied ones, carry
        // the zone (is_aware / zone_id). For a NAMED-zone value (zone_id >= 0)
        // the offset is RE-RESOLVED against the new wall clock (fold = 0), so
        // e.g. local noon Mar 9 replaced to day=10 moves -05:00 → -04:00. A
        // naive / fixed value (zone_id == -1) keeps its stored offset, matching
        // the pre-#1149 pure-civil-substitution behaviour.
        "replace" => {
            let sid = datetime_sid(lowerer)?;
            let (base, _) = lowerer.expr(&args[0].value)?;
            let mut civil = civil_fields(lowerer, base, sid);
            let old_off = lowerer.field_get(base, sid, F_OFFSET, Ty::I64);
            let aware = lowerer.field_get(base, sid, F_AWARE, Ty::I1);
            let zone_id = lowerer.field_get(base, sid, F_ZONE_ID, Ty::I64);
            // The fractional second rides through unchanged unless `microsecond=`
            // / `nanosecond=` overrides it (#1148); sema rejects both at once.
            let mut nanos = lowerer.field_get(base, sid, F_NANOSECOND, Ty::I64);
            for a in &args[1..] {
                let name = a
                    .name
                    .as_deref()
                    .ok_or_else(|| err("datetime.replace() field must be a keyword argument"))?;
                let (v, vt) = lowerer.expr(&a.value)?;
                let iv = lowerer.coerce(v, vt, Ty::I64);
                match name {
                    "microsecond" => {
                        let thousand = lowerer.iconst(Ty::I64, 1000);
                        let n = lowerer.new_value(Ty::I64);
                        lowerer.emit(Inst::Bin {
                            dest: n,
                            op: BinOp::Mul,
                            lhs: iv,
                            rhs: thousand,
                        });
                        nanos = n;
                    }
                    "nanosecond" => {
                        nanos = iv;
                    }
                    _ => {
                        let idx = REPLACE_FIELDS
                            .iter()
                            .position(|f| *f == name)
                            .ok_or_else(|| err("datetime.replace() unknown field"))?;
                        civil[idx] = iv;
                    }
                }
            }
            // Re-resolve the offset at the new wall clock for a named zone;
            // `zap_tz_resolve_local` returns `old_off` unchanged for zone_id < 0.
            // fold = 0 (the earlier instant on an overlap, PEP 495 default).
            let fold = lowerer.iconst(Ty::I64, 0);
            let mut rl_args = Vec::with_capacity(9);
            rl_args.push(zone_id);
            rl_args.extend_from_slice(&civil);
            rl_args.push(fold);
            rl_args.push(old_off);
            let off = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(off),
                func: "zap_tz_resolve_local".to_string(),
                args: rl_args,
            });
            let dest = lowerer.new_value(Ty::Struct(sid));
            lowerer.emit(Inst::StructNew { dest, sid });
            for (idx, v) in civil.into_iter().enumerate() {
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
                val: off,
            });
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: F_AWARE,
                val: aware,
            });
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: F_ZONE_ID,
                val: zone_id,
            });
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: F_NANOSECOND,
                val: nanos,
            });
            (dest, Ty::Struct(sid))
        }
        // date(y, mo, d) -> Date / time(h, mi, s) -> Time (#1046): civil-field
        // constructors.
        "date" | "time" => {
            let sid = struct_sid(lowerer, if method == "date" { "Date" } else { "Time" })?;
            let mut vals: Vec<Value> = Vec::with_capacity(3);
            for a in args.iter().take(3) {
                let (v, vt) = lowerer.expr(&a.value)?;
                vals.push(lowerer.coerce(v, vt, Ty::I64));
            }
            build_triple(lowerer, sid, [vals[0], vals[1], vals[2]])?
        }
        // to_date(dt) -> Date (fields 0..=2) / to_time(dt) -> Time (fields 3..=5).
        "to_date" | "to_time" => {
            let dtsid = datetime_sid(lowerer)?;
            let (base, _) = lowerer.expr(&args[0].value)?;
            let (dst_name, start) = if method == "to_date" {
                ("Date", 0u32)
            } else {
                ("Time", 3u32)
            };
            let sid = struct_sid(lowerer, dst_name)?;
            let vals = [
                lowerer.field_get(base, dtsid, start, Ty::I64),
                lowerer.field_get(base, dtsid, start + 1, Ty::I64),
                lowerer.field_get(base, dtsid, start + 2, Ty::I64),
            ];
            build_triple(lowerer, sid, vals)?
        }
        // combine(date, time) -> naive DateTime (#1046).
        "combine" => {
            let dtsid = datetime_sid(lowerer)?;
            let date_sid = struct_sid(lowerer, "Date")?;
            let time_sid = struct_sid(lowerer, "Time")?;
            let (dbase, _) = lowerer.expr(&args[0].value)?;
            let (tbase, _) = lowerer.expr(&args[1].value)?;
            let civil = [
                lowerer.field_get(dbase, date_sid, 0, Ty::I64),
                lowerer.field_get(dbase, date_sid, 1, Ty::I64),
                lowerer.field_get(dbase, date_sid, 2, Ty::I64),
                lowerer.field_get(tbase, time_sid, 0, Ty::I64),
                lowerer.field_get(tbase, time_sid, 1, Ty::I64),
                lowerer.field_get(tbase, time_sid, 2, Ty::I64),
            ];
            let dest = lowerer.new_value(Ty::Struct(dtsid));
            lowerer.emit(Inst::StructNew { dest, sid: dtsid });
            for (idx, v) in civil.into_iter().enumerate() {
                lowerer.emit(Inst::FieldSet {
                    base: dest,
                    sid: dtsid,
                    idx: idx as u32,
                    val: v,
                });
            }
            let zero = lowerer.iconst(Ty::I64, 0);
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid: dtsid,
                idx: F_OFFSET,
                val: zero,
            });
            let naive = lowerer.bconst(false);
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid: dtsid,
                idx: F_AWARE,
                val: naive,
            });
            let zid = zone_none(lowerer);
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid: dtsid,
                idx: F_ZONE_ID,
                val: zid,
            });
            // Date + Time carry no fraction (#1148).
            let zero_ns = lowerer.iconst(Ty::I64, 0);
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid: dtsid,
                idx: F_NANOSECOND,
                val: zero_ns,
            });
            (dest, Ty::Struct(dtsid))
        }
        // date_today() -> Date: the current LOCAL calendar date.
        "date_today" => {
            let sid = struct_sid(lowerer, "Date")?;
            let ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(ts),
                func: "zap_datetime_now_unix".to_string(),
                args: vec![],
            });
            let local = lowerer.iconst(Ty::I64, 1);
            let vals = [
                {
                    let v = lowerer.new_value(Ty::I64);
                    lowerer.emit(Inst::Call {
                        dest: Some(v),
                        func: "zap_datetime_year".to_string(),
                        args: vec![ts, local],
                    });
                    v
                },
                {
                    let v = lowerer.new_value(Ty::I64);
                    lowerer.emit(Inst::Call {
                        dest: Some(v),
                        func: "zap_datetime_month".to_string(),
                        args: vec![ts, local],
                    });
                    v
                },
                {
                    let v = lowerer.new_value(Ty::I64);
                    lowerer.emit(Inst::Call {
                        dest: Some(v),
                        func: "zap_datetime_day".to_string(),
                        args: vec![ts, local],
                    });
                    v
                },
            ];
            build_triple(lowerer, sid, vals)?
        }
        // (Date) -> int64 weekday / isoweekday / day_of_year (#1055). Reuse the
        // DateTime runtime helpers over (y, mo, d, 0, 0, 0).
        "date_weekday" | "date_isoweekday" | "date_day_of_year" => {
            let date_sid = struct_sid(lowerer, "Date")?;
            let (base, _) = lowerer.expr(&args[0].value)?;
            let zero = lowerer.iconst(Ty::I64, 0);
            let comps = vec![
                lowerer.field_get(base, date_sid, 0, Ty::I64),
                lowerer.field_get(base, date_sid, 1, Ty::I64),
                lowerer.field_get(base, date_sid, 2, Ty::I64),
                zero,
                zero,
                zero,
            ];
            let func = if method == "date_day_of_year" {
                "zap_datetime_day_of_year"
            } else {
                "zap_datetime_weekday"
            };
            let w = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(w),
                func: func.to_string(),
                args: comps,
            });
            if method == "date_isoweekday" {
                let one = lowerer.iconst(Ty::I64, 1);
                let d = lowerer.new_value(Ty::I64);
                lowerer.emit(Inst::Bin {
                    dest: d,
                    op: BinOp::Add,
                    lhs: w,
                    rhs: one,
                });
                (d, Ty::I64)
            } else {
                (w, Ty::I64)
            }
        }
        // resolve(tz, year, month, day, hour, minute, second, fold=0) -> DateTime
        // (#1149): localize a wall clock into `tz`, picking the `fold` instant on
        // an overlap and PEP-495-resolving a gap. The civil fields are stored
        // verbatim; only the offset is zone-resolved (the UTC instant is derived
        // on demand as timegm(civil) − offset, as everywhere else).
        "resolve" => {
            let sid = datetime_sid(lowerer)?;
            let tzsid = timezone_sid(lowerer)?;
            let (tzbase, _) = lowerer.expr(&args[0].value)?;
            let zname = lowerer.field_get(tzbase, tzsid, TZ_NAME, Ty::Ptr);
            let zoff = lowerer.field_get(tzbase, tzsid, TZ_OFFSET, Ty::I64);
            let zfixed_i1 = lowerer.field_get(tzbase, tzsid, TZ_FIXED, Ty::I1);
            let zfixed = lowerer.coerce(zfixed_i1, Ty::I1, Ty::I64);
            let zone_id = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(zone_id),
                func: "zap_tz_zone_id".to_string(),
                args: vec![zname, zfixed],
            });
            // Six civil components (positional, after `tz`).
            let mut civil: Vec<Value> = Vec::with_capacity(6);
            for a in args.iter().skip(1).take(6) {
                let (v, vt) = lowerer.expr(&a.value)?;
                civil.push(lowerer.coerce(v, vt, Ty::I64));
            }
            let fold = resolve_fold(lowerer, args)?;
            // Offset for the wall clock in the zone; a fixed zone (zone_id == -1)
            // returns its constant `zoff`.
            let mut rl = Vec::with_capacity(9);
            rl.push(zone_id);
            rl.extend_from_slice(&civil);
            rl.push(fold);
            rl.push(zoff);
            let off = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(off),
                func: "zap_tz_resolve_local".to_string(),
                args: rl,
            });
            let aware = lowerer.bconst(true);
            let dest = lowerer.new_value(Ty::Struct(sid));
            lowerer.emit(Inst::StructNew { dest, sid });
            for (idx, v) in civil.into_iter().enumerate() {
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
                val: off,
            });
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: F_AWARE,
                val: aware,
            });
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: F_ZONE_ID,
                val: zone_id,
            });
            // A localized wall clock carries no sub-second fraction (#1148).
            let zero_ns = lowerer.iconst(Ty::I64, 0);
            lowerer.emit(Inst::FieldSet {
                base: dest,
                sid,
                idx: F_NANOSECOND,
                val: zero_ns,
            });
            (dest, Ty::Struct(sid))
        }
        // is_ambiguous(dt) / is_imaginary(dt) -> bool (#1149): classify dt's wall
        // clock against its retained zone. A naive / fixed value (zone_id == -1)
        // is always unique (kind 0), so both are false.
        "is_ambiguous" | "is_imaginary" => {
            let sid = datetime_sid(lowerer)?;
            let (base, _) = lowerer.expr(&args[0].value)?;
            let zone_id = lowerer.field_get(base, sid, F_ZONE_ID, Ty::I64);
            let mut ck = Vec::with_capacity(7);
            ck.push(zone_id);
            ck.extend(civil_fields(lowerer, base, sid));
            let kind = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(kind),
                func: "zap_tz_civil_kind".to_string(),
                args: ck,
            });
            let want = lowerer.iconst(Ty::I64, if method == "is_ambiguous" { 2 } else { 1 });
            let d = lowerer.new_value(Ty::I1);
            lowerer.emit(Inst::ICmp {
                dest: d,
                pred: CmpPred::Eq,
                lhs: kind,
                rhs: want,
            });
            (d, Ty::I1)
        }
        // resolve_imaginary(dt) -> DateTime (#1149): if dt's wall clock falls in a
        // spring-forward gap, shift it forward by the gap length onto the real
        // post-transition time; otherwise return dt unchanged. Computed via the
        // canonical real instant + its offset, so a non-gap value rebuilds to the
        // same wall clock.
        "resolve_imaginary" => {
            let sid = datetime_sid(lowerer)?;
            let (base, _) = lowerer.expr(&args[0].value)?;
            let zone_id = lowerer.field_get(base, sid, F_ZONE_ID, Ty::I64);
            let old_off = lowerer.field_get(base, sid, F_OFFSET, Ty::I64);
            let aware = lowerer.field_get(base, sid, F_AWARE, Ty::I1);
            // The gap shift is whole seconds, so the fraction rides through (#1148).
            let src_ns = lowerer.field_get(base, sid, F_NANOSECOND, Ty::I64);
            let mut ia = Vec::with_capacity(8);
            ia.push(zone_id);
            ia.extend(civil_fields(lowerer, base, sid));
            ia.push(old_off);
            let inst = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(inst),
                func: "zap_tz_imaginary_instant".to_string(),
                args: ia,
            });
            let off = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(off),
                func: "zap_tz_offset_at_id".to_string(),
                args: vec![zone_id, inst, old_off],
            });
            let local_ts = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Bin {
                dest: local_ts,
                op: BinOp::Add,
                lhs: inst,
                rhs: off,
            });
            build_datetime(lowerer, local_ts, 0, off, aware, zone_id, src_ns)?
        }
        // zone_abbreviation(dt) -> Optional[str] (#1345): the IANA abbreviation
        // of dt's retained named zone at its UTC instant. Monomorphization has
        // encoded the concrete `Optional$str` instance into the symbol
        // (`zone_abbreviation$Optional$str`), since this lowering cannot name it
        // — the same device the dict.get path uses (`get$Optional$str`).
        m if m.starts_with("zone_abbreviation$") => {
            let opt_name = m
                .strip_prefix("zone_abbreviation$")
                .expect("prefix checked");
            lower_zone_abbreviation(lowerer, args, opt_name)?
        }
        _ => return Ok(None),
    };
    Ok(Some(r))
}

/// `datetime.zone_abbreviation(dt) -> Optional[str]` (#1345). The runtime
/// returns the abbreviation at dt's UTC instant as a fresh `ZapStr*`, or a null
/// pointer when the value carries no named zone (`zone_id < 0`: naive,
/// fixed-offset, or unknown). `Optional[str]` has the niche layout — `Some(p)`
/// IS the payload pointer, `None` IS the null pointer — so the nullable result
/// is already the physical Optional value; it only needs to be reinterpreted to
/// the enum's ZIR type (via a slot) so `match` / assignment dispatch resolves.
fn lower_zone_abbreviation(
    lowerer: &mut FnLower,
    args: &[ast::Arg],
    opt_name: &str,
) -> Result<(Value, Ty), LowerError> {
    // The UTC instant and the retained zone id; the abbreviation is a pure
    // function of the two, so it is identical whether dt came from
    // `resolve(..., fold=)` or from a UTC conversion.
    let (inst, _off, _aware, zone_id, _nanos) = instant_of(lowerer, &args[0])?;
    let ptr = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(ptr),
        func: "zap_tz_abbr_at_id".to_string(),
        args: vec![zone_id, inst],
    });

    let sid = lowerer.ctx.ids.get(opt_name).copied().ok_or_else(|| {
        err(format!(
            "datetime.zone_abbreviation()'s result type '{opt_name}' was not \
             instantiated (internal; #1345)"
        ))
    })?;
    let opt = optional_layout(lowerer.ctx, Ty::Struct(sid)).ok_or_else(|| {
        err(format!(
            "datetime.zone_abbreviation()'s result type '{opt_name}' is not an \
             Optional instance (internal; #1345)"
        ))
    })?;
    if !opt.niche {
        // `str` is a pointer payload, so its Optional is always the niche
        // layout; a tagged layout here would be an internal invariant breach.
        return Err(err(
            "datetime.zone_abbreviation()'s Optional[str] is not niche-laid-out \
             (internal; #1345)",
        ));
    }
    // Reinterpret the nullable pointer as the Optional enum type (null == None,
    // non-null == Some) without a branch, mirroring `gen_opt_some`'s niche path.
    let slot = lowerer.alloca(Ty::Struct(opt.sid));
    lowerer.store(slot, ptr);
    let val = lowerer.load(slot, Ty::Struct(opt.sid));
    Ok((val, Ty::Struct(opt.sid)))
}

/// Extract the optional `fold` selector for `resolve` (#1149): a trailing
/// `fold=` keyword wins; otherwise a positional 8th argument; otherwise `0`
/// (the earlier instant on an overlap, PEP 495 default).
fn resolve_fold(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<Value, LowerError> {
    if let Some(a) = args.iter().find(|a| a.name.as_deref() == Some("fold")) {
        let (v, vt) = lowerer.expr(&a.value)?;
        return Ok(lowerer.coerce(v, vt, Ty::I64));
    }
    if args.len() > 7 && args[7].name.is_none() {
        let (v, vt) = lowerer.expr(&args[7].value)?;
        return Ok(lowerer.coerce(v, vt, Ty::I64));
    }
    Ok(lowerer.iconst(Ty::I64, 0))
}
