//! `argparse` module — lowering facet (ticket #101, D-0056).
//!
//! Each method lowers to a backing `zap_argparse_*` runtime call over the global
//! singleton parser. `str`/`int64` arguments and the `list[str]` argv flow as
//! their word (ptr / i64) via [`lower_args`]; `parse` and the typed getters are
//! fallible (catchable `zap_panic` on a parse error / unknown argument), so they
//! are emitted as `throwing` calls.

use zapc_syntax::ast;

use super::support::{call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "argparse",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // `get_flag` returns a C int64 0/1, normalized to an `i1`; it is fallible
    // (unknown argument), so it is a throwing call rather than `call_bool`.
    if method == "get_flag" {
        let a = lower_args(lowerer, args)?;
        let word = lowerer.new_value(Ty::I64);
        lowerer.throwing_call(Some(word), "zap_argparse_get_flag", a);
        let zero = lowerer.iconst(Ty::I64, 0);
        let d = lowerer.new_value(Ty::I1);
        lowerer.emit(Inst::ICmp {
            dest: d,
            pred: CmpPred::Ne,
            lhs: word,
            rhs: zero,
        });
        return Ok(Some((d, Ty::I1)));
    }
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "set_prog" => call_void(lowerer, "zap_argparse_set_prog", a, false),
        "set_description" => call_void(lowerer, "zap_argparse_set_description", a, false),
        "add_flag" => call_void(lowerer, "zap_argparse_add_flag", a, false),
        "add_positional" => call_void(lowerer, "zap_argparse_add_positional", a, false),
        "add_option" => call_void(lowerer, "zap_argparse_add_option", a, false),
        "add_int" => call_void(lowerer, "zap_argparse_add_int", a, false),
        "parse" => call_void(lowerer, "zap_argparse_parse", a, true),
        "get" => call_ret(lowerer, "zap_argparse_get", a, Ty::Ptr, true),
        "get_int" => call_ret(lowerer, "zap_argparse_get_int", a, Ty::I64, true),
        "format_help" => call_ret(lowerer, "zap_argparse_format_help", a, Ty::Ptr, false),
        "help_requested" => {
            // Infallible C int64 0/1 -> i1.
            let word = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(word),
                func: "zap_argparse_help_requested".to_string(),
                args: a,
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
        "reset" => call_void(lowerer, "zap_argparse_reset", a, false),
        _ => return Ok(None),
    }))
}
