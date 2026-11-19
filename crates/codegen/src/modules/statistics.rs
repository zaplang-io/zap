//! `statistics` module — runtime-symbol declarations (#980, D-0243, epic #966).
//!
//! The ergonomic scalar-list descriptive-statistics surface (Python
//! `statistics`). Every kernel takes a `ZapList[T]` handle (`ptr`) plus an
//! `is_float` `i64` flag selecting how each element word is read as a `double`
//! (0 = int64 word, 1 = f64 bits); results are `f64`, except `multimode` /
//! `quantiles` which return a freshly-built `list[float64]` (`ptr`). The spread
//! functions carry an optional centre (`has_xbar`/`has_mu` + value); the
//! two-list relationships take a per-list `is_float` flag. See the "statistics"
//! section of `runtime/statistics.inc.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let f = cg.ctx.f64_type();
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    // Single-list `(list, is_float) -> f64`: the means + order statistics.
    for name in [
        "zap_stats_mean",
        "zap_stats_fmean",
        "zap_stats_geometric_mean",
        "zap_stats_harmonic_mean",
        "zap_stats_median",
        "zap_stats_median_low",
        "zap_stats_median_high",
        "zap_stats_mode",
    ] {
        cg.declare_rt(name, &[p.into(), i.into()], Some(f.into()));
    }
    // `multimode(list, is_float) -> list[f64]` (ptr).
    cg.declare_rt("zap_stats_multimode", &[p.into(), i.into()], Some(p.into()));
    // Spread: `(list, is_float, has_centre, centre) -> f64`.
    for name in [
        "zap_stats_variance",
        "zap_stats_pvariance",
        "zap_stats_stdev",
        "zap_stats_pstdev",
    ] {
        cg.declare_rt(
            name,
            &[p.into(), i.into(), i.into(), f.into()],
            Some(f.into()),
        );
    }
    // `quantiles(list, is_float, n, method_str) -> list[f64]` (ptr).
    cg.declare_rt(
        "zap_stats_quantiles",
        &[p.into(), i.into(), i.into(), p.into()],
        Some(p.into()),
    );
    // Two-list relationships: `(x, fx, y, fy) -> f64`.
    for name in ["zap_stats_covariance", "zap_stats_correlation"] {
        cg.declare_rt(
            name,
            &[p.into(), i.into(), p.into(), i.into()],
            Some(f.into()),
        );
    }
    // `linear_regression` slope / intercept: `(x, fx, y, fy, proportional) -> f64`.
    for name in ["zap_stats_linreg_slope", "zap_stats_linreg_intercept"] {
        cg.declare_rt(
            name,
            &[p.into(), i.into(), p.into(), i.into(), i.into()],
            Some(f.into()),
        );
    }
}
