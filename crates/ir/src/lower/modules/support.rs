//! Shared lowering helpers for the #97 system/I/O/process modules.
//!
//! These modules all lower to a backing `zap_*` runtime call over `str`/`int64`
//! values. A fallible call (one whose C implementation may `zap_panic`) is
//! emitted with [`FnLower::throwing_call`] so it becomes an `Invoke` inside a
//! `try` (catchable) and a plain `Call` otherwise; an infallible one is a plain
//! `Call`.

use zapc_syntax::ast;

use crate::ir::*;
use crate::lower::{FnLower, LowerError};

/// Lower each argument expression: integer arguments (fds, counts) are coerced
/// to `i64`; reference arguments (`str`, `list`) pass through as their pointer
/// value. The backing runtime symbols take `i64`/`ptr` words accordingly.
pub(super) fn lower_args(
    lowerer: &mut FnLower,
    args: &[ast::Arg],
) -> Result<Vec<Value>, LowerError> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let (v, vt) = lowerer.expr(&a.value)?;
        let v = match vt {
            Ty::I1 | Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 => lowerer.coerce(v, vt, Ty::I64),
            _ => v,
        };
        out.push(v);
    }
    Ok(out)
}

/// Emit a value-returning runtime call (`throwing` when it may raise).
pub(super) fn call_ret(
    lowerer: &mut FnLower,
    func: &str,
    args: Vec<Value>,
    ret: Ty,
    throwing: bool,
) -> (Value, Ty) {
    let d = lowerer.new_value(ret);
    if throwing {
        lowerer.throwing_call(Some(d), func, args);
    } else {
        lowerer.emit(Inst::Call {
            dest: Some(d),
            func: func.to_string(),
            args,
        });
    }
    (d, ret)
}

/// Emit an infallible runtime call returning a C `int64_t` boolean, normalized
/// to an `i1` (`word != 0`) — the same shape `in`/membership lowering uses.
pub(super) fn call_bool(lowerer: &mut FnLower, func: &str, args: Vec<Value>) -> (Value, Ty) {
    let word = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Call {
        dest: Some(word),
        func: func.to_string(),
        args,
    });
    let zero = lowerer.iconst(Ty::I64, 0);
    let d = lowerer.new_value(Ty::I1);
    lowerer.emit(Inst::ICmp {
        dest: d,
        pred: CmpPred::Ne,
        lhs: word,
        rhs: zero,
    });
    (d, Ty::I1)
}

/// Emit a statement-position runtime call (no result), yielding a throwaway
/// `i32 0` so the call expression still has a value.
pub(super) fn call_void(
    lowerer: &mut FnLower,
    func: &str,
    args: Vec<Value>,
    throwing: bool,
) -> (Value, Ty) {
    if throwing {
        lowerer.throwing_call(None, func, args);
    } else {
        lowerer.emit(Inst::Call {
            dest: None,
            func: func.to_string(),
            args,
        });
    }
    (lowerer.iconst(Ty::I32, 0), Ty::I32)
}
