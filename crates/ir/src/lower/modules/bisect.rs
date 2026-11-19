//! `bisect` module — lowering facet (ticket #102, generalized #983).
//!
//! Emits the element-typed `zap_bisect_*_{i64,ptr,f64}` runtime calls over a
//! sorted `ZapList[T]` handle. The list argument is a `Ptr`; the element kind
//! (read off the list's `Ty::List(elem)`) selects the typed entry point and the
//! runtime comparator (integer / byte-wise `str` / IEEE-float total order),
//! mirroring `heap`'s `_i64` / `_ptr` / `_f64` suffixing. `insort_*` mutate the
//! list in place (no result). Sema has checked the element is `Comparable`.

use zapc_syntax::ast;

use super::support::{call_ret, call_void};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "bisect",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    if !matches!(
        method,
        "bisect_left" | "bisect_right" | "insort_left" | "insort_right"
    ) {
        return Ok(None);
    }
    if args.len() != 2 {
        return Err(err(format!("bisect.{method}(xs, x) takes 2 arguments")));
    }
    let (list_val, list_ty) = lowerer.expr(&args[0].value)?;
    let Ty::List(elem) = list_ty else {
        return Err(err(format!(
            "bisect.{method}() expects a list argument, found {list_ty:?}"
        )));
    };
    // `str` compares byte-wise (a `ptr` word), `f64` by total order, integer
    // widths/`bool`/`char`/enum as plain `i64` words. Aggregate elements have no
    // runtime ordering (follow-up).
    let suffix = match elem {
        Elem::I64 | Elem::Bool | Elem::Char | Elem::Enum(_) => "i64",
        Elem::F64 => "f64",
        // `str` and `bytes` are both byte-comparable `ZapStr` words (#1074).
        Elem::Ptr | Elem::Bytes => "ptr",
        _ => {
            return Err(err(
                "bisect over a list of this element type is not supported \
                 (only integers, float64, str, or char)",
            ))
        }
    };
    let (v, vt) = lowerer.expr(&args[1].value)?;
    let v = lowerer.coerce(v, vt, elem.ty());
    let cargs = vec![list_val, v];
    Ok(Some(match method {
        "bisect_left" => call_ret(
            lowerer,
            &format!("zap_bisect_left_{suffix}"),
            cargs,
            Ty::I64,
            false,
        ),
        "bisect_right" => call_ret(
            lowerer,
            &format!("zap_bisect_right_{suffix}"),
            cargs,
            Ty::I64,
            false,
        ),
        "insort_left" => call_void(
            lowerer,
            &format!("zap_bisect_insort_left_{suffix}"),
            cargs,
            false,
        ),
        // insort_right
        _ => call_void(
            lowerer,
            &format!("zap_bisect_insort_right_{suffix}"),
            cargs,
            false,
        ),
    }))
}
