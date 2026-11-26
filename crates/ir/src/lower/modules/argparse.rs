//! `argparse` module — lowering facet (ticket #101, D-0056).
//!
//! Each method lowers to a backing `zap_argparse_*` runtime call over the global
//! singleton parser. `str`/`int64` arguments and the `list[str]` argv flow as
//! their word (ptr / i64) via [`lower_args`]; `parse` and the typed getters are
//! fallible (catchable `zap_panic` on a parse error / unknown argument), so they
//! are emitted as `throwing` calls.

use zapc_syntax::ast;

use super::support::{call_ret, call_void, empty_str, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "argparse",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    // `argparse.ArgumentParser(prog="", description="")` — the object-model
    // constructor (#997, D-0238). Like `concurrent.Counter`, it builds the
    // prelude `struct ArgumentParser` by delegating to the prelude free function
    // `__argparse_new` (crates/syntax/src/parser.rs); omitted trailing string
    // arguments are padded with an empty `str` so the fixed-arity call always
    // gets both words. Everything else about a parser (methods, parsing) is pure
    // prelude Zap, so there is no runtime symbol to declare.
    if method == "ArgumentParser" {
        let sid = lowerer
            .ctx
            .ids
            .get("ArgumentParser")
            .copied()
            .ok_or_else(|| err("ArgumentParser unavailable (built-in prelude missing)"))?;
        let prog = match args.first() {
            Some(a) => {
                let (v, vt) = lowerer.expr(&a.value)?;
                lowerer.coerce(v, vt, Ty::Ptr)
            }
            None => empty_str(lowerer),
        };
        let desc = match args.get(1) {
            Some(a) => {
                let (v, vt) = lowerer.expr(&a.value)?;
                lowerer.coerce(v, vt, Ty::Ptr)
            }
            None => empty_str(lowerer),
        };
        let dest = lowerer.new_value(Ty::Struct(sid));
        lowerer.throwing_call(Some(dest), "__argparse_new", vec![prog, desc]);
        return Ok(Some((dest, Ty::Struct(sid))));
    }
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
        // Registration raises a catchable exception on a duplicate name or when
        // the registration limit is hit (#1566), so these lower through an
        // unwind edge to the calling frame's handler.
        "add_flag" => call_void(lowerer, "zap_argparse_add_flag", a, true),
        "add_positional" => call_void(lowerer, "zap_argparse_add_positional", a, true),
        "add_option" => call_void(lowerer, "zap_argparse_add_option", a, true),
        "add_int" => call_void(lowerer, "zap_argparse_add_int", a, true),
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
