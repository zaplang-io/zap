//! `linalg` module — lowering facet (#926 D-0183, #927 D-0184, #928 D-0185).
//!
//! Emits the LAPACK-backed `zap_linalg_*` runtime calls (implemented in
//! `zaprt.c`): solve / inv / det (#926), the decompositions cholesky / qr / svd
//! (#927), and the eigensolvers / least squares / norm / pinv / matrix_power
//! (#928). Each can raise a catchable `ValueError` (bad shape / singular /
//! non-convergent / no provider), so all route through `throwing_call`. Array
//! results are typed `Ty::Array` (f64) — like matmul — so indexing/`len` route to
//! the array ops, not the generic-pointer/string paths; `eigvals` returns an
//! `array[complex]` and the multi-output ops (eig/qr/svd/eigh/lstsq) assemble a
//! tuple.
//! Sema has type-checked these calls.

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
            Ok(Some(tuple_of_outputs(lowerer, &[a], &elems, &fns)))
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
            Ok(Some(tuple_of_outputs(lowerer, &[a], &elems, &fns)))
        }
        // eigvals (#928, migrated #928-eig): general matrix -> eigenvalues as a
        // 1-D `array[complex]` (the complex array dtype, #938).
        "eigvals" => {
            if args.len() != 1 {
                return Err(err("linalg.eigvals expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let out = Ty::Array(Dtype::Complex, 1);
            let d = lowerer.new_value(out);
            lowerer.throwing_call(Some(d), "zap_linalg_eigvals", vec![a]);
            Ok(Some((d, out)))
        }
        // eig (#928-eig): general matrix -> (w, v), the eigenvalues (1-D) and the
        // complex eigenvector matrix (2-D, columns = eigenvectors), both complex.
        "eig" => {
            if args.len() != 1 {
                return Err(err("linalg.eig expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let elems = vec![Ty::Array(Dtype::Complex, 1), Ty::Array(Dtype::Complex, 2)];
            let fns = ["zap_linalg_eig_w", "zap_linalg_eig_v"];
            Ok(Some(tuple_of_outputs(lowerer, &[a], &elems, &fns)))
        }
        // eigh (#928): symmetric matrix -> (w ascending real, v real eigenvectors).
        "eigh" => {
            if args.len() != 1 {
                return Err(err("linalg.eigh expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let elems = vec![Ty::Array(Dtype::F64, 1), Ty::Array(Dtype::F64, 2)];
            let fns = ["zap_linalg_eigh_w", "zap_linalg_eigh_v"];
            Ok(Some(tuple_of_outputs(lowerer, &[a], &elems, &fns)))
        }
        "eigvalsh" => {
            if args.len() != 1 {
                return Err(err("linalg.eigvalsh expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let out = Ty::Array(Dtype::F64, 1);
            let d = lowerer.new_value(out);
            lowerer.throwing_call(Some(d), "zap_linalg_eigh_w", vec![a]);
            Ok(Some((d, out)))
        }
        // lstsq (#928): (a, b) -> (solution, residuals, rank, singular_values) —
        // a mixed tuple (rank is an int, the rest arrays), each its own hand-off.
        "lstsq" => {
            if args.len() != 2 {
                return Err(err("linalg.lstsq expects (a, b)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let (b, bt) = lowerer.expr(&args[1].value)?;
            let x_nd = match bt {
                Ty::Array(_, nd) => nd,
                _ => return Err(err("linalg.lstsq: b must be a 1-D or 2-D array")),
            };
            let elems = vec![
                Ty::Array(Dtype::F64, x_nd), // solution (b's rank)
                Ty::Array(Dtype::F64, 1),    // residuals (1-D, possibly empty)
                Ty::I64,                     // effective rank
                Ty::Array(Dtype::F64, 1),    // singular values
            ];
            let fns = [
                "zap_linalg_lstsq_x",
                "zap_linalg_lstsq_resid",
                "zap_linalg_lstsq_rank",
                "zap_linalg_lstsq_s",
            ];
            Ok(Some(tuple_of_outputs(lowerer, &[a, b], &elems, &fns)))
        }
        // norm (#928): scalar f64. `ord` is optional; when omitted it lowers to a
        // NaN sentinel the runtime reads as "default" (2-norm / Frobenius).
        "norm" => {
            if args.is_empty() || args.len() > 2 {
                return Err(err("linalg.norm expects (a) or (a, ord)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let ord = if args.len() == 2 {
                let (o, ot) = lowerer.expr(&args[1].value)?;
                lowerer.coerce(o, ot, Ty::F64)
            } else {
                lowerer.fconst(Ty::F64, f64::NAN)
            };
            let d = lowerer.new_value(Ty::F64);
            lowerer.throwing_call(Some(d), "zap_linalg_norm", vec![a, ord]);
            Ok(Some((d, Ty::F64)))
        }
        "pinv" => {
            if args.len() != 1 {
                return Err(err("linalg.pinv expects (a)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let out = Ty::Array(Dtype::F64, 2);
            let d = lowerer.new_value(out);
            lowerer.throwing_call(Some(d), "zap_linalg_pinv", vec![a]);
            Ok(Some((d, out)))
        }
        // matrix_power (#928): a**p for an integer p (binary exponentiation over
        // matmul; p<0 inverts). f64 result this slice.
        "matrix_power" => {
            if args.len() != 2 {
                return Err(err("linalg.matrix_power expects (a, p)"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let (p, pt) = lowerer.expr(&args[1].value)?;
            let p = lowerer.coerce(p, pt, Ty::I64);
            let out = Ty::Array(Dtype::F64, 2);
            let d = lowerer.new_value(out);
            lowerer.throwing_call(Some(d), "zap_linalg_matrix_power", vec![a, p]);
            Ok(Some((d, out)))
        }
        _ => Ok(None),
    }
}

/// Build a tuple whose fields are the results of calling each `fns[i]` with the
/// same `call_args` (each a throwing LAPACK hand-off). The element types `elems`
/// give the tuple's struct layout (they need not all be arrays — `lstsq`'s rank
/// field is an `i64`). Mirrors `where`/nonzero's tuple construction.
fn tuple_of_outputs(
    lowerer: &mut FnLower,
    call_args: &[Value],
    elems: &[Ty],
    fns: &[&str],
) -> (Value, Ty) {
    let mut vals = Vec::with_capacity(fns.len());
    for (i, f) in fns.iter().enumerate() {
        let d = lowerer.new_value(elems[i]);
        lowerer.throwing_call(Some(d), f, call_args.to_vec());
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
