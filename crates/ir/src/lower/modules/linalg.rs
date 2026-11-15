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
        "cholesky" => {
            if args.len() != 1 {
                return Err(err("linalg.cholesky expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let out = Ty::Array(Dtype::F64, 2);
            let d = lowerer.new_value(out);
            lowerer.throwing_call(Some(d), "zap_linalg_cholesky", vec![a]);
            Ok(Some((d, out)))
        }
        // qr / svd return a tuple of arrays — one runtime call per output, then
        // assemble the tuple (the where()/nonzero pattern). f64 throughout.
        "qr" => {
            if args.len() != 1 {
                return Err(err("linalg.qr expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let elems = vec![Ty::Array(Dtype::F64, 2), Ty::Array(Dtype::F64, 2)];
            let fns = ["zap_linalg_qr_q", "zap_linalg_qr_r"];
            Ok(Some(tuple_of_arrays(lowerer, a, &elems, &fns)))
        }
        "svd" => {
            if args.len() != 1 {
                return Err(err("linalg.svd expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let elems = vec![
                Ty::Array(Dtype::F64, 2), // U
                Ty::Array(Dtype::F64, 1), // S
                Ty::Array(Dtype::F64, 2), // Vt
            ];
            let fns = ["zap_linalg_svd_u", "zap_linalg_svd_s", "zap_linalg_svd_vt"];
            Ok(Some(tuple_of_arrays(lowerer, a, &elems, &fns)))
        }
        _ => Ok(None),
    }
}

/// Build a tuple whose fields are the results of calling each `fns[i]` on the
/// same matrix `a` (each a throwing LAPACK hand-off). The element types `elems`
/// give the tuple's struct layout. Mirrors `where`/nonzero's tuple construction.
fn tuple_of_arrays(lowerer: &mut FnLower, a: Value, elems: &[Ty], fns: &[&str]) -> (Value, Ty) {
    let mut vals = Vec::with_capacity(fns.len());
    for (i, f) in fns.iter().enumerate() {
        let d = lowerer.new_value(elems[i]);
        lowerer.throwing_call(Some(d), f, vec![a]);
        vals.push(d);
    }
    let sid = lowerer.ctx.tuples.borrow_mut().intern(elems.to_vec());
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
    (dest, Ty::Struct(sid))
}
