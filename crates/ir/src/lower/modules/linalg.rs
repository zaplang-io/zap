//! `linalg` module — lowering facet (#926, D-0183).
//!
//! Emits `zap_linalg_solve(a, b) -> array`, `zap_linalg_inv(a) -> array`, and
//! `zap_linalg_det(a) -> f64` (implemented in `zaprt.c`, LAPACK-backed). Each can
//! raise a catchable `ValueError` (bad shape / singular matrix / no provider), so
//! all route through `throwing_call`. The array results are typed `Ty::Array` (f64)
//! — like matmul — so indexing/`len` on them route to the array ops, not the
//! generic-pointer/string paths. Sema has type-checked these calls.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "linalg",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        "solve" => {
            if args.len() != 2 {
                return Err(err("linalg.solve expects (a, b)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let (b, bt) = lowerer.expr(&args[1].value)?;
            // Result has b's rank (x matches b: a vector for 1-D b, a matrix for
            // 2-D b), always f64. Require a statically-ranked array, as matmul does.
            let nd = match bt {
                Ty::Array(_, nd) => nd,
                _ => return Err(err("linalg.solve: b must be a 1-D or 2-D array")),
            };
            let out = Ty::Array(Dtype::F64, nd);
            let d = lowerer.new_value(out);
            lowerer.throwing_call(Some(d), "zap_linalg_solve", vec![a, b]);
            Ok(Some((d, out)))
        }
        "inv" => {
            if args.len() != 1 {
                return Err(err("linalg.inv expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let out = Ty::Array(Dtype::F64, 2);
            let d = lowerer.new_value(out);
            lowerer.throwing_call(Some(d), "zap_linalg_inv", vec![a]);
            Ok(Some((d, out)))
        }
        "det" => {
            if args.len() != 1 {
                return Err(err("linalg.det expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let d = lowerer.new_value(Ty::F64);
            lowerer.throwing_call(Some(d), "zap_linalg_det", vec![a]);
            Ok(Some((d, Ty::F64)))
        }
        _ => Ok(None),
    }
}
