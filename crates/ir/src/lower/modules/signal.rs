//! `signal` module — lowering facet (ticket #77, D-0058).

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "signal",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        // signal.notify(Signal) -> channel[Signal]. The argument is a tier-1
        // `Signal`, i.e. its backing i64 = the OS signal number; the channel
        // delivers that number (recv yields i64, matching Signal's backing).
        "notify" => {
            if args.len() != 1 {
                return Err(err("signal.notify(sig) takes exactly one argument"));
            }
            let (v, vt) = lowerer.expr(&args[0].value)?;
            let signo = lowerer.coerce(v, vt, Ty::I64);
            let ch = lowerer.new_value(Ty::Chan(Elem::I64));
            lowerer.emit(Inst::Call {
                dest: Some(ch),
                func: "zap_signal_notify".to_string(),
                args: vec![signo],
            });
            Ok(Some((ch, Ty::Chan(Elem::I64))))
        }
        _ => Ok(None),
    }
}
