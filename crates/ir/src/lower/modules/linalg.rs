//! `linalg` module — lowering facet (#926, D-0183).
//!
//! Emits `zap_linalg_solve(a, b) -> array`, `zap_linalg_inv(a) -> array`, and
//! `zap_linalg_det(a) -> f64` (implemented in `zaprt.c`, LAPACK-backed). Each can
//! raise a catchable `ValueError` (bad shape / singular matrix / no provider), so
//! all route through `throwing_call`. Sema has type-checked these calls.

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
    let (rt, argc, ret): (&str, usize, Ty) = match method {
        "solve" => ("zap_linalg_solve", 2, Ty::Ptr),
        "inv" => ("zap_linalg_inv", 1, Ty::Ptr),
        "det" => ("zap_linalg_det", 1, Ty::F64),
        _ => return Ok(None),
    };
    if args.len() != argc {
        return Err(err("linalg call has the wrong number of arguments"));
    }
    let mut vs = Vec::with_capacity(argc);
    for a in args {
        let (v, _) = lowerer.expr(&a.value)?;
        vs.push(v);
    }
    let d = lowerer.new_value(ret);
    lowerer.throwing_call(Some(d), rt, vs);
    Ok(Some((d, ret)))
}
