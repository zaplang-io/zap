//! `fs` module — lowering facet (ticket #97, D-0056).

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower { name: "fs", lower };

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        "read_file" => call_ret(lowerer, "zap_fs_read_file", a, Ty::Ptr, true),
        "write_file" => call_void(lowerer, "zap_fs_write_file", a, true),
        "exists" => call_bool(lowerer, "zap_fs_exists", a),
        "is_dir" => call_bool(lowerer, "zap_fs_is_dir", a),
        "is_file" => call_bool(lowerer, "zap_fs_is_file", a),
        "size" => call_ret(lowerer, "zap_fs_size", a, Ty::I64, true),
        "listdir" => call_ret(lowerer, "zap_os_listdir", a, Ty::List(Elem::Ptr), true),
        "mkdtemp" => call_ret(lowerer, "zap_fs_mkdtemp", a, Ty::Ptr, true),
        _ => return Ok(None),
    }))
}
