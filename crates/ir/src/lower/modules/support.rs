//! Shared lowering helpers for the #97 system/I/O/process modules.
//!
//! These modules all lower to a backing `zap_*` runtime call over `str`/`int64`
//! values. A fallible call (one whose C implementation may `zap_panic`) is
//! emitted with [`FnLower::throwing_call`] so it becomes an `Invoke` inside a
//! `try` (catchable) and a plain `Call` otherwise; an infallible one is a plain
//! `Call`.

use zapc_syntax::ast;

use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

/// Resolve the prelude `LineReader` struct id (injected by the parser's prelude
/// whenever the surface is used). Shared by `io.lines` / `fs.open_reader`.
fn line_reader_sid(lowerer: &FnLower) -> Result<u32, LowerError> {
    lowerer
        .ctx
        .ids
        .get("LineReader")
        .copied()
        .ok_or_else(|| err("LineReader struct unavailable (built-in prelude missing)"))
}

/// Wrap a buffered reader handle `h` in a freshly-constructed prelude
/// `LineReader` (field 0 = `_h`) — the iterable returned by `io.lines(reader)`
/// and `fs.open_reader(path)` (#1031, D-0223). The handle is rooted by the new
/// struct, which then drives the D-0048 iterator protocol via `LineReader`'s
/// `__next__` (one `io.readline` per step).
pub(super) fn line_reader_wrap(lowerer: &mut FnLower, h: Value) -> Result<(Value, Ty), LowerError> {
    let sid = line_reader_sid(lowerer)?;
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: h,
    });
    Ok((dest, Ty::Struct(sid)))
}

/// Resolve a prelude struct id by name (the user may have shadowed it, but the
/// prelude injects it whenever the triggering surface is referenced).
fn prelude_sid(lowerer: &FnLower, name: &str) -> Result<u32, LowerError> {
    lowerer.ctx.ids.get(name).copied().ok_or_else(|| {
        err(format!(
            "{name} struct unavailable (built-in prelude missing)"
        ))
    })
}

/// Lower `io.writer(fd)` (#1071, D-0239): allocate a fresh buffered-writer handle
/// via `zap_io_writer_new(fd)` and wrap it in a freshly-constructed prelude
/// `Writer` (field 0 = `_h`). The constructor only allocates — it does no I/O —
/// so it is a plain (non-throwing) call; the object's `write`/`flush`/`close`
/// methods carry the throwing runtime ops. The new struct roots the handle.
pub(super) fn writer_new_wrap(lowerer: &mut FnLower, fd: Value) -> Result<(Value, Ty), LowerError> {
    let handle = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(handle),
        func: "zap_io_writer_new".to_string(),
        args: vec![fd],
    });
    let sid = prelude_sid(lowerer, "Writer")?;
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: handle,
    });
    Ok((dest, Ty::Struct(sid)))
}

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
            Ty::I1 | Ty::I8 | Ty::I16 | Ty::I32 | Ty::Char | Ty::I64 => {
                lowerer.coerce(v, vt, Ty::I64)
            }
            _ => v,
        };
        out.push(v);
    }
    Ok(out)
}

/// Build an empty `str` constant value, used to pad an omitted optional string
/// argument so a fixed-arity runtime symbol always receives its full word list
/// (e.g. the optional `capath`/`keyfile` of the `tls` context loaders, #560).
pub(super) fn empty_str(lowerer: &mut FnLower) -> Value {
    let d = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::StrConst {
        dest: d,
        bytes: Vec::new(),
    });
    d
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
