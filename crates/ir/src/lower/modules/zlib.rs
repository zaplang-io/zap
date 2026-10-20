//! `zlib` module — lowering facet (ticket #106, D-0056/D-0060).
//!
//! Emits the backing `zap_zlib_*` runtime calls (implemented in `zaprt.c` over
//! the system zlib). `str`/`bytes` values are already `Ty::Ptr` (a `ZapStr*`),
//! so the data argument passes straight through; `compress`/`decompress` results
//! are also `Ty::Ptr` (a `bytes` object). `crc32`/`adler32` return an `i64`
//! checksum. The optional compression level coerces to `i64`, defaulting to
//! `Z_DEFAULT_COMPRESSION` (-1) when omitted. Sema has already type-checked.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "zlib",
    lower,
};

/// zlib's `Z_DEFAULT_COMPRESSION`: let the library pick its default level (6).
const DEFAULT_LEVEL: i64 = -1;

/// Lower `module.compress(data, level?)` -> `func(data, level)`: the data
/// pointer plus an `i64` level (the given one, or [`DEFAULT_LEVEL`]). Shared by
/// `zlib` and `gzip`.
pub(super) fn lower_compress(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
) -> Result<(Value, Ty), LowerError> {
    if args.is_empty() || args.len() > 2 {
        return Err(err("compress takes one or two arguments"));
    }
    let (data, _) = lowerer.expr(&args[0].value)?;
    let level = match args.get(1) {
        Some(a) => {
            let (v, vt) = lowerer.expr(&a.value)?;
            lowerer.coerce(v, vt, Ty::I64)
        }
        None => lowerer.iconst(Ty::I64, DEFAULT_LEVEL),
    };
    let d = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args: vec![data, level],
    });
    Ok((d, Ty::Ptr))
}

/// Lower `module.decompress(data)` -> `func(data)`, returning a `bytes` pointer.
/// Shared by `zlib` and `gzip`.
pub(super) fn lower_decompress(
    lowerer: &mut FnLower,
    func: &str,
    args: &[ast::Arg],
) -> Result<(Value, Ty), LowerError> {
    if args.len() != 1 {
        return Err(err("decompress takes exactly one argument"));
    }
    let (data, _) = lowerer.expr(&args[0].value)?;
    let d = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args: vec![data],
    });
    Ok((d, Ty::Ptr))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    match method {
        "compress" => Ok(Some(lower_compress(lowerer, "zap_zlib_compress", args)?)),
        "decompress" => Ok(Some(lower_decompress(
            lowerer,
            "zap_zlib_decompress",
            args,
        )?)),
        // crc32/adler32(data) -> i64 checksum.
        "crc32" | "adler32" => {
            if args.len() != 1 {
                return Err(err("crc32/adler32 takes exactly one argument"));
            }
            let (data, _) = lowerer.expr(&args[0].value)?;
            let func = if method == "crc32" {
                "zap_zlib_crc32"
            } else {
                "zap_zlib_adler32"
            };
            let d = lowerer.new_value(Ty::I64);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: func.to_string(),
                args: vec![data],
            });
            Ok(Some((d, Ty::I64)))
        }
        _ => Ok(None),
    }
}
