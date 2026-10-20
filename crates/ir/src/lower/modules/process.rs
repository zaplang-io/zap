//! `process` module — lowering facet (ticket #97, D-0056).

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "process",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        // process.run(argv: list[str]) -> (int64, str, str).
        //
        // `zap_process_run` returns a GC `ZapProc` handle (which may raise on
        // spawn failure); three infallible getters unpack its fields, which are
        // then assembled into a `(int64, str, str)` tuple — the same aggregate
        // path as a tuple literal (intern + StructNew + FieldSet), so codegen
        // needs no process-specific handling.
        "run" => {
            if args.len() != 1 {
                return Err(err("process.run(argv) takes exactly one argument"));
            }
            let (argv, _) = lowerer.expr(&args[0].value)?;
            let h = lowerer.new_value(Ty::Ptr);
            lowerer.throwing_call(Some(h), "zap_process_run", vec![argv]);

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
            Ok(Some((dest, Ty::Struct(sid))))
        }
        _ => Ok(None),
    }
}
