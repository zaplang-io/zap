//! `statistics` module — lowering facet (#980, D-0243, epic #966).
//!
//! Each function lowers to a single backing `zap_stats_*` runtime call over the
//! list handle (a `ptr`) plus an `is_float` `i64` flag (1 when the element words
//! are `f64` bits, 0 when they are `int64` words) so one C kernel serves both a
//! `list[float64]` and a `list[int64]`. Every kernel can raise the
//! StatisticsError-equivalent (`ValueError`) on empty / invalid input, so the
//! calls route through [`FnLower::throwing_call`] (via `call_ret(.., true)`) to
//! stay catchable (the "raising runtime calls need throwing_call" rule);
//! `multimode`, which never raises, uses a plain call.
//!
//! `multimode` / `quantiles` return a freshly-built `list[float64]`;
//! `linear_regression` assembles a `(slope, intercept)` tuple from two component
//! calls (the `math.modf`/`frexp` build pattern).

use zapc_syntax::ast;

use super::support::call_ret;
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "statistics",
    lower,
};

/// Lower a numeric-list argument to `(list_ptr, is_float_flag)`: the handle
/// value and an `i64` 1/0 selecting the element read. A non-list type (ruled
/// out by sema) defaults to the integer read.
fn list_and_flag(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Value), LowerError> {
    let (v, t) = lowerer.expr(&arg.value)?;
    let is_float = matches!(t, Ty::List(Elem::F64));
    let flag = lowerer.iconst(Ty::I64, if is_float { 1 } else { 0 });
    Ok((v, flag))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        // Means + order statistics: `zap_stats_<method>(list, is_float) -> f64`.
        "mean" | "fmean" | "geometric_mean" | "harmonic_mean" | "median" | "median_low"
        | "median_high" | "mode" => {
            let arg = args
                .first()
                .ok_or_else(|| err(format!("statistics.{method} missing its list argument")))?;
            let (v, flag) = list_and_flag(lowerer, arg)?;
            Ok(Some(call_ret(
                lowerer,
                &format!("zap_stats_{method}"),
                vec![v, flag],
                Ty::F64,
                true,
            )))
        }
        // `multimode(xs)` -> list[float64]; never raises (empty -> []).
        "multimode" => {
            let arg = args
                .first()
                .ok_or_else(|| err("statistics.multimode missing its list argument"))?;
            let (v, flag) = list_and_flag(lowerer, arg)?;
            Ok(Some(call_ret(
                lowerer,
                "zap_stats_multimode",
                vec![v, flag],
                Ty::List(Elem::F64),
                false,
            )))
        }
        // Spread with an optional centre: `(list, is_float, has_centre, centre)`.
        "variance" | "pvariance" | "stdev" | "pstdev" => {
            let arg = args
                .first()
                .ok_or_else(|| err(format!("statistics.{method} missing its list argument")))?;
            let (v, flag) = list_and_flag(lowerer, arg)?;
            let (has_c, centre) = match args.get(1) {
                Some(a) => {
                    let (cv, ct) = lowerer.expr(&a.value)?;
                    (lowerer.iconst(Ty::I64, 1), lowerer.coerce(cv, ct, Ty::F64))
                }
                None => (lowerer.iconst(Ty::I64, 0), lowerer.fconst(Ty::F64, 0.0)),
            };
            Ok(Some(call_ret(
                lowerer,
                &format!("zap_stats_{method}"),
                vec![v, flag, has_c, centre],
                Ty::F64,
                true,
            )))
        }
        // `quantiles(xs[, n][, method])` -> list[float64].
        "quantiles" => {
            let arg = args
                .first()
                .ok_or_else(|| err("statistics.quantiles missing its list argument"))?;
            let (v, flag) = list_and_flag(lowerer, arg)?;
            let nq = match args.get(1) {
                Some(a) => {
                    let (nv, nt) = lowerer.expr(&a.value)?;
                    lowerer.coerce(nv, nt, Ty::I64)
                }
                None => lowerer.iconst(Ty::I64, 4),
            };
            let method_v = match args.get(2) {
                Some(a) => lowerer.expr(&a.value)?.0,
                None => {
                    let d = lowerer.new_value(Ty::Ptr);
                    lowerer.emit(Inst::StrConst {
                        dest: d,
                        bytes: b"exclusive".to_vec(),
                    });
                    d
                }
            };
            Ok(Some(call_ret(
                lowerer,
                "zap_stats_quantiles",
                vec![v, flag, nq, method_v],
                Ty::List(Elem::F64),
                true,
            )))
        }
        // Two-list relationships: `(x, fx, y, fy) -> f64`.
        "covariance" | "correlation" => {
            let (vx, fx) = list_and_flag(
                lowerer,
                args.first()
                    .ok_or_else(|| err(format!("statistics.{method} missing its x argument")))?,
            )?;
            let (vy, fy) = list_and_flag(
                lowerer,
                args.get(1)
                    .ok_or_else(|| err(format!("statistics.{method} missing its y argument")))?,
            )?;
            Ok(Some(call_ret(
                lowerer,
                &format!("zap_stats_{method}"),
                vec![vx, fx, vy, fy],
                Ty::F64,
                true,
            )))
        }
        // `linear_regression(xs, ys[, proportional])` -> (slope, intercept).
        "linear_regression" => {
            let (vx, fx) = list_and_flag(
                lowerer,
                args.first()
                    .ok_or_else(|| err("statistics.linear_regression missing its x argument"))?,
            )?;
            let (vy, fy) = list_and_flag(
                lowerer,
                args.get(1)
                    .ok_or_else(|| err("statistics.linear_regression missing its y argument"))?,
            )?;
            let prop = match args.get(2) {
                Some(a) => {
                    let (pv, pt) = lowerer.expr(&a.value)?;
                    lowerer.coerce(pv, pt, Ty::I64)
                }
                None => lowerer.iconst(Ty::I64, 0),
            };
            let slope = lowerer.new_value(Ty::F64);
            lowerer.throwing_call(
                Some(slope),
                "zap_stats_linreg_slope",
                vec![vx, fx, vy, fy, prop],
            );
            let intercept = lowerer.new_value(Ty::F64);
            lowerer.throwing_call(
                Some(intercept),
                "zap_stats_linreg_intercept",
                vec![vx, fx, vy, fy, prop],
            );
            Ok(Some(build_pair(
                lowerer,
                slope,
                Ty::F64,
                intercept,
                Ty::F64,
            )))
        }
        _ => Ok(None),
    }
}

/// Assemble a two-field tuple value `(a: at, b: bt)` — the interned-tuple struct
/// shape a sema `Ty::Tuple` lowers to (mirrors `math`'s `build_pair`).
fn build_pair(lowerer: &mut FnLower, a: Value, at: Ty, b: Value, bt: Ty) -> (Value, Ty) {
    let sid = lowerer.ctx.tuples.borrow_mut().intern(vec![at, bt]);
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: a,
    });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 1,
        val: b,
    });
    (dest, Ty::Struct(sid))
}
