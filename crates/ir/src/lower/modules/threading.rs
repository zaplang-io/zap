//! `threading` module — lowering facet (ticket #175).
//!
//! `threading.Thread(target=..., daemon=...)` constructs the prelude `struct
//! Thread` wrapping a fresh `zap_thread_new` handle (the datetime/json pattern:
//! a module method builds a compiler-known prelude type). The target is an
//! `fn()` value (I#163) stored in the struct and invoked by the prelude
//! `__thread_entry` trampoline via `CallClosure` — so the closure is called from
//! compiled Zap, rooted through the shadow stack, with no runtime C→Zap call.
//! The flat `threading._*` methods lower to the `zap_thread_*` / threading
//! runtime entry points in the "threading" section of `zaprt.c`.

use zapc_syntax::ast;

use super::support::{call_bool, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "threading",
    lower,
};

/// Build a `Thread` struct: `{ _target: fn(), _handle: bytes, _daemon: bool }`.
/// `target` is taken from the `target=` keyword (or a lone positional); `daemon`
/// from the `daemon=` keyword, defaulting to `false`.
fn lower_thread_ctor(lowerer: &mut FnLower, args: &[ast::Arg]) -> Result<(Value, Ty), LowerError> {
    let mut target_expr: Option<&ast::Expr> = None;
    let mut daemon_expr: Option<&ast::Expr> = None;
    for a in args {
        match a.name.as_deref() {
            Some("target") => target_expr = Some(&a.value),
            Some("daemon") => daemon_expr = Some(&a.value),
            Some(_) => {} // ignore unknown kwargs (e.g. name=); transpiler folds args= into the closure
            None => {
                if target_expr.is_none() {
                    target_expr = Some(&a.value);
                } else if daemon_expr.is_none() {
                    daemon_expr = Some(&a.value);
                }
            }
        }
    }
    let target_expr = target_expr.ok_or_else(|| err("threading.Thread requires a target"))?;
    let sid = lowerer
        .ctx
        .ids
        .get("Thread")
        .copied()
        .ok_or_else(|| err("Thread unavailable (built-in prelude missing)"))?;
    // Lower the target against the `_target` field's declared `fn()` type so a
    // bare `lambda: ...` gets its function-typed context (I#163); a top-level fn
    // name or stored `fn()` value coerces through unchanged.
    let target_ty = lowerer.ctx.fields[sid as usize]
        .first()
        .map(|(_, t)| t.clone())
        .ok_or_else(|| err("Thread struct layout missing its target field"))?;
    let target_v = lowerer.expr_coerced(target_expr, target_ty)?;
    let daemon_v = match daemon_expr {
        Some(e) => lowerer.expr(e)?.0,
        None => {
            let d = lowerer.new_value(Ty::I1);
            lowerer.emit(Inst::BConst {
                dest: d,
                val: false,
            });
            d
        }
    };
    let handle = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(handle),
        func: "zap_thread_new".to_string(),
        args: vec![],
    });
    let d = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest: d, sid });
    lowerer.emit(Inst::FieldSet {
        base: d,
        sid,
        idx: 0,
        val: target_v,
    });
    lowerer.emit(Inst::FieldSet {
        base: d,
        sid,
        idx: 1,
        val: handle,
    });
    lowerer.emit(Inst::FieldSet {
        base: d,
        sid,
        idx: 2,
        val: daemon_v,
    });
    Ok((d, Ty::Struct(sid)))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // Thread is built inline (constructs a prelude struct); the rest are flat
    // runtime calls over the opaque handle.
    if method == "Thread" {
        return Ok(Some(lower_thread_ctor(lowerer, args)?));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        // Pin / unpin the calling coroutine's dedicated OS thread.
        "_pin" => call_void(lowerer, "zap_threading_pin", a, false),
        "_unpin" => call_void(lowerer, "zap_runtime_unlock_os_thread", a, false),
        // Thread lifecycle over the handle.
        "_register" => call_void(lowerer, "zap_thread_register", a, false),
        "_finish" => call_void(lowerer, "zap_thread_finish", a, false),
        "_join" => call_void(lowerer, "zap_thread_join", a, false),
        "_join_timeout" => call_bool(lowerer, "zap_thread_join_timeout", a),
        "_is_alive" => call_bool(lowerer, "zap_thread_is_alive", a),
        _ => return Ok(None),
    }))
}
