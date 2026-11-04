//! `crypto` module — lowering facet (ticket #105, D-0056).
//!
//! Emits the backing `zap_crypto_*` runtime calls (implemented in `zaprt.c`).
//! `str` and `bytes` share the `ZapStr*` ABI, but actual byte-buffer results
//! retain `Ty::Bytes` for binary operations (#644, #647). Text results and opaque
//! `_new` context handles remain `Ty::Ptr`; handles are not digest buffers.
//! Count arguments (`random_bytes`/`token_hex`) coerce to `i64`. Sema has already
//! type-checked these calls.

use zapc_syntax::ast;

use super::ModuleLower;
use crate::ir::*;
use crate::lower::{err, FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "crypto",
    lower,
};

/// Algorithm selector shared with `zaprt.c`'s crypto section.
const ALGO_SHA256: i64 = 0;
const ALGO_SHA512: i64 = 1;
const ALGO_SHA1: i64 = 2;
const ALGO_MD5: i64 = 3;

/// Lower a one-argument call with an explicit text/bytes result type.
fn call_result1(
    lowerer: &mut FnLower,
    func: &str,
    arg: &ast::Arg,
    result_ty: Ty,
) -> Result<(Value, Ty), LowerError> {
    let (v, _) = lowerer.expr(&arg.value)?;
    let d = lowerer.new_value(result_ty);
    lowerer.emit(Inst::Call {
        dest: Some(d),
        func: func.to_string(),
        args: vec![v],
    });
    Ok((d, result_ty))
}

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let one_shot = |m: &mut FnLower, func: &str| -> Result<Option<(Value, Ty)>, LowerError> {
        if args.len() != 1 {
            return Err(err("crypto hash one-shot takes exactly one argument"));
        }
        Ok(Some(call_result1(m, func, &args[0], Ty::Ptr)?))
    };
    match method {
        "sha256" => one_shot(lowerer, "zap_crypto_sha256"),
        "sha512" => one_shot(lowerer, "zap_crypto_sha512"),
        "sha1" => one_shot(lowerer, "zap_crypto_sha1"),
        "md5" => one_shot(lowerer, "zap_crypto_md5"),
        // crypto.<algo>_new() -> bytes handle: zap_crypto_new(algo).
        "sha256_new" | "sha512_new" | "sha1_new" | "md5_new" => {
            let algo = match method {
                "sha256_new" => ALGO_SHA256,
                "sha512_new" => ALGO_SHA512,
                "sha1_new" => ALGO_SHA1,
                _ => ALGO_MD5,
            };
            let a = lowerer.iconst(Ty::I64, algo);
            let d = lowerer.new_value(Ty::Ptr);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_crypto_new".to_string(),
                args: vec![a],
            });
            Ok(Some((d, Ty::Ptr)))
        }
        // crypto.update(handle, data): mutate the handle in place (no result).
        "update" => {
            if args.len() != 2 {
                return Err(err("crypto.update takes exactly two arguments"));
            }
            let (h, _) = lowerer.expr(&args[0].value)?;
            let (data, _) = lowerer.expr(&args[1].value)?;
            lowerer.emit(Inst::Call {
                dest: None,
                func: "zap_crypto_update".to_string(),
                args: vec![h, data],
            });
            Ok(Some((lowerer.iconst(Ty::I32, 0), Ty::I32)))
        }
        "hexdigest" => {
            if args.len() != 1 {
                return Err(err("crypto.hexdigest takes exactly one argument"));
            }
            Ok(Some(call_result1(
                lowerer,
                "zap_crypto_hexdigest",
                &args[0],
                Ty::Ptr,
            )?))
        }
        "digest" => {
            if args.len() != 1 {
                return Err(err("crypto.digest takes exactly one argument"));
            }
            Ok(Some(call_result1(
                lowerer,
                "zap_crypto_digest",
                &args[0],
                Ty::Bytes,
            )?))
        }
        "hmac_sha256" | "hmac_sha512" => {
            if args.len() != 2 {
                return Err(err("crypto.hmac takes exactly two arguments"));
            }
            let func = if method == "hmac_sha256" {
                "zap_crypto_hmac_sha256"
            } else {
                "zap_crypto_hmac_sha512"
            };
            let (key, _) = lowerer.expr(&args[0].value)?;
            let (msg, _) = lowerer.expr(&args[1].value)?;
            let d = lowerer.new_value(Ty::Ptr);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: func.to_string(),
                args: vec![key, msg],
            });
            Ok(Some((d, Ty::Ptr)))
        }
        // crypto.hmac_sha256_raw(key, msg) -> bytes: the raw 32-byte digest (#153).
        "hmac_sha256_raw" => {
            if args.len() != 2 {
                return Err(err("crypto.hmac_sha256_raw takes exactly two arguments"));
            }
            let (key, _) = lowerer.expr(&args[0].value)?;
            let (msg, _) = lowerer.expr(&args[1].value)?;
            let d = lowerer.new_value(Ty::Bytes);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: "zap_crypto_hmac_sha256_raw".to_string(),
                args: vec![key, msg],
            });
            Ok(Some((d, Ty::Bytes)))
        }
        // crypto.rsa_<hash>_sign(pem_key, data) -> bytes (#628): the raw PKCS#1
        // v1.5 signature. Keep the result Ty::Bytes for byte operations (#644);
        // args retain their existing pointer ABI.
        "rsa_sha1_sign" | "rsa_sha256_sign" => {
            if args.len() != 2 {
                return Err(err("crypto.rsa_*_sign takes exactly two arguments"));
            }
            let func = if method == "rsa_sha1_sign" {
                "zap_crypto_rsa_sha1_sign"
            } else {
                "zap_crypto_rsa_sha256_sign"
            };
            let (key, _) = lowerer.expr(&args[0].value)?;
            let (data, _) = lowerer.expr(&args[1].value)?;
            let d = lowerer.new_value(Ty::Bytes);
            // Key parsing, family rejection and signing failures raise ValueError.
            // Preserve the active try handler for every signing error (#642).
            lowerer.throwing_call(Some(d), func, vec![key, data]);
            Ok(Some((d, Ty::Bytes)))
        }
        // crypto.rsa_sha256_verify(pem_pubkey, data, sig) -> bool (#628). The
        // runtime returns an i64 (0/1); the compare-to-zero yields the i1 `bool`.
        "rsa_sha256_verify" => {
            if args.len() != 3 {
                return Err(err(
                    "crypto.rsa_sha256_verify takes exactly three arguments",
                ));
            }
            let (key, _) = lowerer.expr(&args[0].value)?;
            let (data, _) = lowerer.expr(&args[1].value)?;
            let (sig, _) = lowerer.expr(&args[2].value)?;
            Ok(Some(super::support::call_bool(
                lowerer,
                "zap_crypto_rsa_sha256_verify",
                vec![key, data, sig],
            )))
        }
        // crypto.random_bytes(n) / crypto.token_hex(n): count coerces to i64.
        "random_bytes" | "token_hex" => {
            if args.len() != 1 {
                return Err(err(
                    "crypto.random_bytes/token_hex takes exactly one argument",
                ));
            }
            let (v, vt) = lowerer.expr(&args[0].value)?;
            let n = lowerer.coerce(v, vt, Ty::I64);
            let func = if method == "random_bytes" {
                "zap_crypto_random_bytes"
            } else {
                "zap_crypto_token_hex"
            };
            let result_ty = if method == "random_bytes" {
                Ty::Bytes
            } else {
                Ty::Ptr
            };
            let d = lowerer.new_value(result_ty);
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: func.to_string(),
                args: vec![n],
            });
            Ok(Some((d, result_ty)))
        }
        _ => Ok(None),
    }
}
