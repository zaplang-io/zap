//! `crypto` module — lowering facet (ticket #105, D-0056).
//!
//! Emits the backing `zap_crypto_*` runtime calls (implemented in `zaprt.c`).
//! `str` and `bytes` share the `ZapStr*` ABI, but actual byte-buffer results
//! retain `Ty::Bytes` for binary operations (#644, #647). Text results and opaque
//! `_new` context handles remain `Ty::Ptr`; handles are not digest buffers.
//! Count arguments (`random_bytes`/`token_hex`) coerce to `i64`. Sema has already
//! type-checked these calls.

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void};
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

/// Central result classification for every crypto runtime entry point. Missing
/// registrations fail lowering instead of silently defaulting a byte buffer to
/// Ptr. Opaque hash handles deliberately differ from actual bytes buffers.
fn result_type(func: &str) -> Result<Ty, LowerError> {
    Ok(match func {
        "zap_crypto_digest"
        | "zap_crypto_hmac_sha256_raw"
        | "zap_crypto_rsa_sha1_sign"
        | "zap_crypto_rsa_sha256_sign"
        | "zap_crypto_rsa_public_key_pem"
        | "zap_crypto_p256_public_key_pem"
        | "zap_crypto_ed25519_public_key_pem"
        | "zap_crypto_p256_sha256_sign"
        | "zap_crypto_ed25519_sign"
        | "zap_crypto_random_bytes" => Ty::Bytes,
        "zap_crypto_sha256"
        | "zap_crypto_sha512"
        | "zap_crypto_sha1"
        | "zap_crypto_md5"
        | "zap_crypto_hexdigest"
        | "zap_crypto_hmac_sha256"
        | "zap_crypto_hmac_sha512"
        | "zap_crypto_token_hex"
        | "zap_crypto_key_kind"
        | "zap_crypto_key_curve"
        | "zap_crypto_new" => Ty::Ptr,
        "zap_crypto_rsa_sha256_verify"
        | "zap_crypto_p256_sha256_verify"
        | "zap_crypto_ed25519_verify"
        | "zap_crypto_constant_time_eq" => Ty::I1,
        "zap_crypto_key_bits" | "zap_crypto_key_is_private" => Ty::I64,
        "zap_crypto_update" => Ty::Void,
        _ => return Err(err("unknown crypto runtime symbol in result-type table")),
    })
}

/// All crypto calls share result typing and emission, after the caller has
/// lowered arguments (including count coercion). Keep RSA errors catchable;
/// normalize the verifier's C int64 boolean and the update expression's void.
fn call_crypto(
    lowerer: &mut FnLower,
    func: &str,
    args: Vec<Value>,
) -> Result<(Value, Ty), LowerError> {
    let throwing = matches!(
        func,
        "zap_crypto_rsa_sha1_sign"
            | "zap_crypto_rsa_sha256_sign"
            | "zap_crypto_rsa_public_key_pem"
            | "zap_crypto_p256_public_key_pem"
            | "zap_crypto_ed25519_public_key_pem"
            | "zap_crypto_p256_sha256_sign"
            | "zap_crypto_ed25519_sign"
            | "zap_crypto_key_kind"
            | "zap_crypto_key_bits"
            | "zap_crypto_key_curve"
            | "zap_crypto_key_is_private"
    );
    Ok(match result_type(func)? {
        Ty::I1 => call_bool(lowerer, func, args),
        Ty::Void => call_void(lowerer, func, args, throwing),
        ty => call_ret(lowerer, func, args, ty, throwing),
    })
}

fn call_result1(
    lowerer: &mut FnLower,
    func: &str,
    arg: &ast::Arg,
) -> Result<(Value, Ty), LowerError> {
    let (v, _) = lowerer.expr(&arg.value)?;
    call_crypto(lowerer, func, vec![v])
}

/// Build a `KeyInfo` value struct (#643, D-0126) from the validated-key getters,
/// in the prelude field order `kind, bits, curve, is_private`. The `kind` getter
/// is the validating gate: it is emitted first and is throwing, so an
/// unparseable / unsupported / encrypted PEM raises a catchable `ValueError`
/// before any other field is read. The remaining getters run only once the key
/// has validated, so they never panic for a supported key.
fn lower_key_info(lowerer: &mut FnLower, arg: &ast::Arg) -> Result<(Value, Ty), LowerError> {
    let sid = *lowerer
        .ctx
        .ids
        .get("KeyInfo")
        .ok_or_else(|| err("KeyInfo struct unavailable (built-in prelude missing)"))?;
    let (pem, _) = lowerer.expr(&arg.value)?;
    let dest = lowerer.new_value(Ty::Struct(sid));
    lowerer.emit(Inst::StructNew { dest, sid });

    // Field 0: kind (str) — the validating gate, throwing.
    let kind = lowerer.new_value(Ty::Ptr);
    lowerer.throwing_call(Some(kind), "zap_crypto_key_kind", vec![pem]);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 0,
        val: kind,
    });

    // Field 1: bits (int64).
    let bits = lowerer.new_value(Ty::I64);
    lowerer.emit(Inst::Call {
        dest: Some(bits),
        func: "zap_crypto_key_bits".to_string(),
        args: vec![pem],
    });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 1,
        val: bits,
    });

    // Field 2: curve (str).
    let curve = lowerer.new_value(Ty::Ptr);
    lowerer.emit(Inst::Call {
        dest: Some(curve),
        func: "zap_crypto_key_curve".to_string(),
        args: vec![pem],
    });
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 2,
        val: curve,
    });

    // Field 3: is_private (bool) — normalize the C int64 0/1 to an i1.
    let (is_private, _) = call_bool(lowerer, "zap_crypto_key_is_private", vec![pem]);
    lowerer.emit(Inst::FieldSet {
        base: dest,
        sid,
        idx: 3,
        val: is_private,
    });

    Ok((dest, Ty::Struct(sid)))
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
        Ok(Some(call_result1(m, func, &args[0])?))
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
            Ok(Some(call_crypto(lowerer, "zap_crypto_new", vec![a])?))
        }
        // crypto.update(handle, data): mutate the handle in place (no result).
        "update" => {
            if args.len() != 2 {
                return Err(err("crypto.update takes exactly two arguments"));
            }
            let (h, _) = lowerer.expr(&args[0].value)?;
            let (data, _) = lowerer.expr(&args[1].value)?;
            Ok(Some(call_crypto(
                lowerer,
                "zap_crypto_update",
                vec![h, data],
            )?))
        }
        "hexdigest" => {
            if args.len() != 1 {
                return Err(err("crypto.hexdigest takes exactly one argument"));
            }
            Ok(Some(call_result1(
                lowerer,
                "zap_crypto_hexdigest",
                &args[0],
            )?))
        }
        "digest" => {
            if args.len() != 1 {
                return Err(err("crypto.digest takes exactly one argument"));
            }
            Ok(Some(call_result1(lowerer, "zap_crypto_digest", &args[0])?))
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
            Ok(Some(call_crypto(lowerer, func, vec![key, msg])?))
        }
        // crypto.hmac_sha256_raw(key, msg) -> bytes: the raw 32-byte digest (#153).
        "hmac_sha256_raw" => {
            if args.len() != 2 {
                return Err(err("crypto.hmac_sha256_raw takes exactly two arguments"));
            }
            let (key, _) = lowerer.expr(&args[0].value)?;
            let (msg, _) = lowerer.expr(&args[1].value)?;
            Ok(Some(call_crypto(
                lowerer,
                "zap_crypto_hmac_sha256_raw",
                vec![key, msg],
            )?))
        }
        // crypto.constant_time_eq(a, b) -> bool (#729, D-0131). The runtime
        // returns i64 (0/1); call_crypto normalizes to i1. Not throwing — a
        // length mismatch returns false, never a panic.
        "constant_time_eq" => {
            if args.len() != 2 {
                return Err(err("crypto.constant_time_eq takes exactly two arguments"));
            }
            let (a, _) = lowerer.expr(&args[0].value)?;
            let (b, _) = lowerer.expr(&args[1].value)?;
            Ok(Some(call_crypto(
                lowerer,
                "zap_crypto_constant_time_eq",
                vec![a, b],
            )?))
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
            Ok(Some(call_crypto(lowerer, func, vec![key, data])?))
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
            Ok(Some(call_crypto(
                lowerer,
                "zap_crypto_rsa_sha256_verify",
                vec![key, data, sig],
            )?))
        }
        // crypto.key_info(pem) -> KeyInfo (#643): build the metadata value struct.
        "key_info" => {
            if args.len() != 1 {
                return Err(err("crypto.key_info takes exactly one argument"));
            }
            Ok(Some(lower_key_info(lowerer, &args[0])?))
        }
        // crypto.{rsa,p256}_public_key_pem(a, b) -> bytes (#643): validated public
        // key import from JWK integer/octet components.
        "rsa_public_key_pem" | "p256_public_key_pem" => {
            if args.len() != 2 {
                return Err(err("crypto.*_public_key_pem takes exactly two arguments"));
            }
            let func = if method == "rsa_public_key_pem" {
                "zap_crypto_rsa_public_key_pem"
            } else {
                "zap_crypto_p256_public_key_pem"
            };
            let (a, _) = lowerer.expr(&args[0].value)?;
            let (b, _) = lowerer.expr(&args[1].value)?;
            Ok(Some(call_crypto(lowerer, func, vec![a, b])?))
        }
        // crypto.ed25519_public_key_pem(x_bytes) -> bytes (#643).
        "ed25519_public_key_pem" => {
            if args.len() != 1 {
                return Err(err(
                    "crypto.ed25519_public_key_pem takes exactly one argument",
                ));
            }
            Ok(Some(call_result1(
                lowerer,
                "zap_crypto_ed25519_public_key_pem",
                &args[0],
            )?))
        }
        // crypto.p256_sha256_sign / ed25519_sign(private_pem, msg) -> bytes (#643).
        "p256_sha256_sign" | "ed25519_sign" => {
            if args.len() != 2 {
                return Err(err(
                    "crypto.{p256_sha256,ed25519}_sign takes exactly two arguments",
                ));
            }
            let func = if method == "p256_sha256_sign" {
                "zap_crypto_p256_sha256_sign"
            } else {
                "zap_crypto_ed25519_sign"
            };
            let (key, _) = lowerer.expr(&args[0].value)?;
            let (msg, _) = lowerer.expr(&args[1].value)?;
            Ok(Some(call_crypto(lowerer, func, vec![key, msg])?))
        }
        // crypto.p256_sha256_verify / ed25519_verify(public_pem, msg, sig) -> bool
        // (#643). The runtime returns i64 (0/1); call_crypto normalizes to i1.
        "p256_sha256_verify" | "ed25519_verify" => {
            if args.len() != 3 {
                return Err(err(
                    "crypto.{p256_sha256,ed25519}_verify takes exactly three arguments",
                ));
            }
            let func = if method == "p256_sha256_verify" {
                "zap_crypto_p256_sha256_verify"
            } else {
                "zap_crypto_ed25519_verify"
            };
            let (key, _) = lowerer.expr(&args[0].value)?;
            let (msg, _) = lowerer.expr(&args[1].value)?;
            let (sig, _) = lowerer.expr(&args[2].value)?;
            Ok(Some(call_crypto(lowerer, func, vec![key, msg, sig])?))
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
            Ok(Some(call_crypto(lowerer, func, vec![n])?))
        }
        _ => Ok(None),
    }
}
