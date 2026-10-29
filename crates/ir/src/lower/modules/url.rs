//! `url` module — lowering facet (ticket #335, D-0056).
//!
//! Each call lowers to a backing `zap_url_*` C runtime entry point in `zaprt.c`.
//! `str` and `dict` values are both pointers; every function returns a `str`
//! (pointer). None of them raise, so they are plain (infallible) calls.

use zapc_syntax::ast;

use super::support::{call_ret, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "url", lower };

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "quote" => call_ret(lowerer, "zap_url_quote", a, Ty::Ptr, false),
        "quote_path" => call_ret(lowerer, "zap_url_quote_path", a, Ty::Ptr, false),
        "unquote" => call_ret(lowerer, "zap_url_unquote", a, Ty::Ptr, false),
        "unquote_plus" => call_ret(lowerer, "zap_url_unquote_plus", a, Ty::Ptr, false),
        "encode_query" => call_ret(lowerer, "zap_url_encode_query", a, Ty::Ptr, false),
        "canonical_query" => call_ret(lowerer, "zap_url_canonical_query", a, Ty::Ptr, false),
        _ => return Ok(None),
    }))
}
