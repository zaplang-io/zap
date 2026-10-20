//! `math` module — runtime-symbol declarations (ticket #98, D-0056).
//!
//! Thin libm wrappers: the unary/binary functions take and return `f64`;
//! `isnan`/`isinf` return an `i64` 0/1. `math.pow` reuses the existing
//! `zap_powf` (declared with the scalar numeric helpers in `lib.rs`), so it is
//! intentionally absent here. See the "math + random" section of `zaprt.c`.

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let f = cg.ctx.f64_type();
    let i = cg.ctx.i64_type();
    for name in [
        "zap_math_sqrt",
        "zap_math_exp",
        "zap_math_log",
        "zap_math_log2",
        "zap_math_log10",
        "zap_math_sin",
        "zap_math_cos",
        "zap_math_tan",
        "zap_math_asin",
        "zap_math_acos",
        "zap_math_atan",
        "zap_math_floor",
        "zap_math_ceil",
        "zap_math_round",
        "zap_math_trunc",
        "zap_math_fabs",
    ] {
        cg.declare_rt(name, &[f.into()], Some(f.into()));
    }
    for name in ["zap_math_atan2", "zap_math_hypot", "zap_math_fmod"] {
        cg.declare_rt(name, &[f.into(), f.into()], Some(f.into()));
    }
    cg.declare_rt("zap_math_isnan", &[f.into()], Some(i.into()));
    cg.declare_rt("zap_math_isinf", &[f.into()], Some(i.into()));
}
