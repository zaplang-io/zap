//! `process` module — lowering facet (ticket #97, D-0056; subprocess
//! ergonomics #976, D-0237).
//!
//! `run` / `check_run` / `run_shell` each lower to a backing `zap_process_*`
//! runtime entry point that spawns, feeds optional stdin, captures (into temp
//! files, so there is no pipe-buffer deadlock), waits (with an optional timeout
//! that SIGKILLs on expiry), and returns a GC `ZapProc` handle; three infallible
//! getters unpack its fields into a `(int64, str, str)` tuple (the same aggregate
//! path as a tuple literal). The spawn entry points may raise (spawn failure,
//! timeout `TimeoutError`, and `check_run`'s non-zero `RuntimeError`), so they
//! are emitted with `throwing_call`.
//!
//! The internal `_popen_*` primitives (prelude `Popen` only) lower to the
//! `zap_popen_*` streaming runtime: non-blocking pipe reads/writes park on the
//! netpoller. The ones that can raise (spawn, write, read, wait, returncode) use
//! `throwing_call`.

use zapc_syntax::ast;
use zapc_syntax::span::Span;

use super::support::{call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "process",
    lower,
};

/// The declared option arity of `run` / `check_run` / `run_shell`:
/// `cmd, env, cwd, timeout, input, capture_output, merge_stderr`.
const RUN_ARITY: usize = 7;

/// Build the default argument expression for option slot `i` of a `run`-family
/// call when the caller omitted it entirely (a *trailing* omission the seam
/// binder leaves absent — interior holes are already filled by the binder). The
/// values mirror the `SEAM_METHOD_DEFAULTS` rows so an omitted and an explicitly
/// passed default lower identically. Slot 0 (`cmd`) is required and never
/// defaulted here.
fn run_default_arg(i: usize, span: Span) -> ast::Arg {
    let empty_str = || ast::Expr::Str {
        value: String::new(),
        raw: false,
        bytes: false,
        fstring: false,
        span,
        fstring_fields: Vec::new(),
    };
    let value = match i {
        1 => ast::Expr::Dict(Vec::new(), span),         // env = {}
        2 => empty_str(),                               // cwd = ""
        3 => ast::Expr::Float("0.0".to_string(), span), // timeout = 0.0
        4 => empty_str(),                               // input = ""
        5 => ast::Expr::Bool(true, span),               // capture_output = True
        6 => ast::Expr::Bool(false, span),              // merge_stderr = False
        _ => unreachable!("run option slot {i} out of range"),
    };
    ast::Arg {
        name: None,
        value,
        spread: ast::ArgSpread::None,
        span,
    }
}

/// Unpack a `ZapProc` handle into a `(int64, str, str)` tuple via the three
/// infallible field getters and the tuple aggregate path (intern + StructNew +
/// FieldSet) — shared by `run` / `check_run` / `run_shell`.
fn unpack_proc_tuple(lowerer: &mut FnLower, h: Value) -> (Value, Ty) {
    let code = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Call {
        dest: Some(code),
        func: "zap_process_result_code".to_string(),
        args: vec![h],
    });
    let out = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(out),
        func: "zap_process_result_stdout".to_string(),
        args: vec![h],
    });
    let errv = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(errv),
        func: "zap_process_result_stderr".to_string(),
        args: vec![h],
    });

    let sid = lowerer
        .ctx
        .tuples
        .borrow_mut()
        .intern(vec![Ty::I64, Ty::Ptr, Ty::Ptr]);
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });
    for (idx, val) in [code, out, errv].into_iter().enumerate() {
        lowerer.emit(Inst::FieldSet {
            base: dest,
            sid,
            idx: idx as u32,
            val,
        });
    }
    (dest, Ty::Struct(sid))
}

/// Lower a `run` / `check_run` / `run_shell` call to `func`. The caller's
/// (already seam-reordered) argument list is padded to the full option arity
/// with synthesized defaults for any trailing omission, then lowered with the
/// shared `lower_args` coercion (bool → `i64`, `float`/pointer pass through).
fn lower_run(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
) -> Result<(Value, Ty), LowerError> {
    if args.is_empty() {
        return Err(err(
            "process.run family takes at least the command argument",
        ));
    }
    let span = args[0].span;
    let mut full: Vec<ast::Arg> = Vec::with_capacity(RUN_ARITY);
    full.extend(args.iter().cloned());
    for i in args.len()..RUN_ARITY {
        full.push(run_default_arg(i, span));
    }
    let vals = lower_args(lowerer, &full)?;
    // The spawn may raise (spawn failure, timeout, check_run's non-zero exit).
    let h = lowerer.new_value(Ty::Ptr);
    lowerer.throwing_call(Some(h), func, vals);
    Ok(unpack_proc_tuple(lowerer, h))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    Ok(Some(match method {
        "run" => lower_run(lowerer, "zap_process_run", args)?,
        "check_run" => lower_run(lowerer, "zap_process_check_run", args)?,
        "run_shell" => lower_run(lowerer, "zap_process_run_shell", args)?,

        // --- streaming primitives (prelude `Popen`) ------------------------
        // _popen_spawn(argv, env, cwd, flags) -> int64 handle (a stable non-GC
        // address). Raises on spawn failure (and on Windows, unsupported).
        "_popen_spawn" => {
            let a = lower_args(lowerer, args)?;
            call_ret(lowerer, "zap_popen_spawn", a, Ty::I64, true)
        }
        // _popen_write(h, data) -> int64 bytes written. May raise (broken pipe).
        "_popen_write" => {
            let a = lower_args(lowerer, args)?;
            call_ret(lowerer, "zap_popen_write", a, Ty::I64, true)
        }
        // _popen_close_stdin(h).
        "_popen_close_stdin" => {
            let a = lower_args(lowerer, args)?;
            call_void(lowerer, "zap_popen_close_stdin", a, false)
        }
        // _popen_read(h, which, maxn) -> bytes. May raise (read error / cancel).
        // Subprocess output is binary `bytes` (sema agrees), so the IR must carry
        // `Ty::Bytes`, not a bare `str` `Ty::Ptr` — otherwise `len()` on the read
        // would count UTF-8 codepoints, not bytes (the #1074 class, D-0241).
        "_popen_read" => {
            let a = lower_args(lowerer, args)?;
            call_ret(lowerer, "zap_popen_read", a, Ty::Bytes, true)
        }
        // _popen_read_all(h, which) -> bytes. May raise.
        "_popen_read_all" => {
            let a = lower_args(lowerer, args)?;
            call_ret(lowerer, "zap_popen_read_all", a, Ty::Bytes, true)
        }
        // _popen_poll(h) -> int64 (exit status, or -256 sentinel). Infallible.
        "_popen_poll" => {
            let a = lower_args(lowerer, args)?;
            call_ret(lowerer, "zap_popen_poll", a, Ty::I64, false)
        }
        // _popen_wait(h) -> int64. May raise (cancellation).
        "_popen_wait" => {
            let a = lower_args(lowerer, args)?;
            call_ret(lowerer, "zap_popen_wait", a, Ty::I64, true)
        }
        // _popen_returncode(h) -> int64. Raises RuntimeError if still running.
        "_popen_returncode" => {
            let a = lower_args(lowerer, args)?;
            call_ret(lowerer, "zap_popen_returncode", a, Ty::I64, true)
        }
        // _popen_pid(h) -> int64. Infallible.
        "_popen_pid" => {
            let a = lower_args(lowerer, args)?;
            call_ret(lowerer, "zap_popen_pid", a, Ty::I64, false)
        }
        // _popen_signal(h, sig) — terminate(15) / kill(9).
        "_popen_signal" => {
            let a = lower_args(lowerer, args)?;
            call_void(lowerer, "zap_popen_signal", a, false)
        }
        _ => return Ok(None),
    }))
}
