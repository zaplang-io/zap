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
/// SHA-384 / SHA-224 (#765): truncated SHA-512 / SHA-256 streaming handles.
const ALGO_SHA384: i64 = 4;
const ALGO_SHA224: i64 = 5;

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
        | "zap_crypto_pbkdf2_hmac"
        | "zap_crypto_hkdf"
        | "zap_crypto_scrypt"
        | "zap_crypto_aes256gcm_encrypt"
        | "zap_crypto_aes256gcm_decrypt"
        | "zap_crypto_chacha20poly1305_encrypt"
        | "zap_crypto_chacha20poly1305_decrypt"
        | "zap_crypto_hmac_digest"
        | "zap_crypto_rsa_sha384_sign"
        | "zap_crypto_rsa_sha512_sign"
        | "zap_crypto_rsa_pss_sha256_sign"
        | "zap_crypto_rsa_pss_sha384_sign"
        | "zap_crypto_rsa_pss_sha512_sign"
        | "zap_crypto_p384_public_key_pem"
        | "zap_crypto_p521_public_key_pem"
        | "zap_crypto_rsa_private_key_pem"
        | "zap_crypto_p256_private_key_pem"
        | "zap_crypto_ed25519_private_key_pem"
        | "zap_crypto_p384_sha384_sign"
        | "zap_crypto_p521_sha512_sign"
        | "zap_hex_decode"
        | "zap_base64_decode"
        | "zap_crypto_random_bytes" => Ty::Bytes,
        "zap_crypto_sha256"
        | "zap_crypto_sha512"
        | "zap_crypto_sha1"
        | "zap_crypto_md5"
        | "zap_crypto_sha384"
        | "zap_crypto_sha224"
        | "zap_crypto_sha3_256"
        | "zap_crypto_sha3_512"
        | "zap_crypto_blake2b"
        | "zap_crypto_blake2s"
        | "zap_crypto_hexdigest"
        | "zap_crypto_hmac_sha256"
        | "zap_crypto_hmac_sha512"
        | "zap_crypto_hmac_new"
        | "zap_crypto_hmac_hexdigest"
        | "zap_crypto_token_hex"
        | "zap_crypto_token_urlsafe"
        | "zap_crypto_key_kind"
        | "zap_crypto_key_curve"
        | "zap_hex_encode"
        | "zap_base64_encode"
        | "zap_crypto_new" => Ty::Ptr,
        "zap_crypto_rsa_sha256_verify"
        | "zap_crypto_rsa_sha384_verify"
        | "zap_crypto_rsa_sha512_verify"
        | "zap_crypto_rsa_pss_sha256_verify"
        | "zap_crypto_rsa_pss_sha384_verify"
        | "zap_crypto_rsa_pss_sha512_verify"
        | "zap_crypto_p256_sha256_verify"
        | "zap_crypto_p384_sha384_verify"
        | "zap_crypto_p521_sha512_verify"
        | "zap_crypto_ed25519_verify"
        | "zap_crypto_constant_time_eq" => Ty::I1,
        "zap_crypto_key_bits" | "zap_crypto_key_is_private" | "zap_crypto_randbelow" => Ty::I64,
        "zap_crypto_update" | "zap_crypto_hmac_update" => Ty::Void,
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
            | "zap_crypto_pbkdf2_hmac"
            | "zap_crypto_hkdf"
            | "zap_crypto_scrypt"
            | "zap_crypto_aes256gcm_encrypt"
            | "zap_crypto_aes256gcm_decrypt"
            | "zap_crypto_chacha20poly1305_encrypt"
            | "zap_crypto_chacha20poly1305_decrypt"
            | "zap_crypto_token_urlsafe"
            | "zap_crypto_hmac_new"
            | "zap_crypto_randbelow"
            | "zap_crypto_rsa_sha384_sign"
            | "zap_crypto_rsa_sha512_sign"
            | "zap_crypto_rsa_pss_sha256_sign"
            | "zap_crypto_rsa_pss_sha384_sign"
            | "zap_crypto_rsa_pss_sha512_sign"
            | "zap_crypto_p384_public_key_pem"
            | "zap_crypto_p521_public_key_pem"
            | "zap_crypto_p384_sha384_sign"
            | "zap_crypto_p521_sha512_sign"
            | "zap_crypto_rsa_private_key_pem"
            | "zap_crypto_p256_private_key_pem"
            | "zap_crypto_ed25519_private_key_pem"
            | "zap_hex_decode"
            | "zap_base64_decode"
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
    // Lower exactly `n` reference (str/bytes/handle) arguments and call `func`.
    let refs = |m: &mut FnLower, func: &str, n: usize| -> Result<Option<(Value, Ty)>, LowerError> {
        if args.len() != n {
            return Err(err("crypto call has the wrong number of arguments"));
        }
        let mut vals = Vec::with_capacity(n);
        for a in args {
            vals.push(m.expr(&a.value)?.0);
        }
        Ok(Some(call_crypto(m, func, vals)?))
    };
    match method {
        "sha256" => one_shot(lowerer, "zap_crypto_sha256"),
        "sha512" => one_shot(lowerer, "zap_crypto_sha512"),
        "sha1" => one_shot(lowerer, "zap_crypto_sha1"),
        "md5" => one_shot(lowerer, "zap_crypto_md5"),
        // SHA-384 / SHA-224 one-shot hex (#765): same shape as sha256/sha512.
        "sha384" => one_shot(lowerer, "zap_crypto_sha384"),
        "sha224" => one_shot(lowerer, "zap_crypto_sha224"),
        // crypto.<algo>_new() -> bytes handle: zap_crypto_new(algo).
        "sha256_new" | "sha512_new" | "sha1_new" | "md5_new" | "sha384_new" | "sha224_new" => {
            let algo = match method {
                "sha256_new" => ALGO_SHA256,
                "sha512_new" => ALGO_SHA512,
                "sha1_new" => ALGO_SHA1,
                "sha384_new" => ALGO_SHA384,
                "sha224_new" => ALGO_SHA224,
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
        // crypto.pbkdf2_hmac(hash_name, password, salt, iterations, dklen=0)
        // -> bytes (#762). `iterations`/`dklen` coerce to i64; an omitted
        // `dklen` becomes 0 (the hash's native digest length). The runtime
        // raises a catchable ValueError on bad arguments or an unknown hash.
        "pbkdf2_hmac" => {
            if args.len() != 4 && args.len() != 5 {
                return Err(err(
                    "crypto.pbkdf2_hmac takes 4 or 5 arguments (hash_name, password, salt, iterations, dklen=0)",
                ));
            }
            let (name, _) = lowerer.expr(&args[0].value)?;
            let (password, _) = lowerer.expr(&args[1].value)?;
            let (salt, _) = lowerer.expr(&args[2].value)?;
            let (iter_v, iter_t) = lowerer.expr(&args[3].value)?;
            let iterations = lowerer.coerce(iter_v, iter_t, Ty::I64);
            let dklen = if args.len() == 5 {
                let (dk_v, dk_t) = lowerer.expr(&args[4].value)?;
                lowerer.coerce(dk_v, dk_t, Ty::I64)
            } else {
                lowerer.iconst(Ty::I64, 0)
            };
            Ok(Some(call_crypto(
                lowerer,
                "zap_crypto_pbkdf2_hmac",
                vec![name, password, salt, iterations, dklen],
            )?))
        }
        // crypto.scrypt(password, salt, n, r, p, dklen=64) -> bytes (#775,
        // D-0134). n/r/p coerce to i64; an omitted dklen becomes 64 (Python's
        // default). The runtime raises a catchable ValueError on bad parameters.
        "scrypt" => {
            if args.len() != 5 && args.len() != 6 {
                return Err(err(
                    "crypto.scrypt takes 5 or 6 arguments (password, salt, n, r, p, dklen=64)",
                ));
            }
            let (password, _) = lowerer.expr(&args[0].value)?;
            let (salt, _) = lowerer.expr(&args[1].value)?;
            let (n_v, n_t) = lowerer.expr(&args[2].value)?;
            let n = lowerer.coerce(n_v, n_t, Ty::I64);
            let (r_v, r_t) = lowerer.expr(&args[3].value)?;
            let r = lowerer.coerce(r_v, r_t, Ty::I64);
            let (p_v, p_t) = lowerer.expr(&args[4].value)?;
            let p = lowerer.coerce(p_v, p_t, Ty::I64);
            let dklen = if args.len() == 6 {
                let (dk_v, dk_t) = lowerer.expr(&args[5].value)?;
                lowerer.coerce(dk_v, dk_t, Ty::I64)
            } else {
                lowerer.iconst(Ty::I64, 64)
            };
            Ok(Some(call_crypto(
                lowerer,
                "zap_crypto_scrypt",
                vec![password, salt, n, r, p, dklen],
            )?))
        }
        // AEAD (#772, D-0135): <alg>_encrypt/_decrypt(key, nonce, data, aad="").
        // An omitted `aad` defaults to the empty string; the runtime validates
        // key/nonce lengths and (on decrypt) the authentication tag.
        "aes256gcm_encrypt"
        | "aes256gcm_decrypt"
        | "chacha20poly1305_encrypt"
        | "chacha20poly1305_decrypt" => {
            if args.len() != 3 && args.len() != 4 {
                return Err(err(
                    "crypto AEAD takes 3 or 4 arguments (key, nonce, data, aad=\"\")",
                ));
            }
            let (key, _) = lowerer.expr(&args[0].value)?;
            let (nonce, _) = lowerer.expr(&args[1].value)?;
            let (data, _) = lowerer.expr(&args[2].value)?;
            let aad = if args.len() == 4 {
                lowerer.expr(&args[3].value)?.0
            } else {
                let d = lowerer.new_value(Ty::Ptr);
                lowerer.emit(Inst::StrConst {
                    dest: d,
                    bytes: Vec::new(),
                });
                d
            };
            let func = match method {
                "aes256gcm_encrypt" => "zap_crypto_aes256gcm_encrypt",
                "aes256gcm_decrypt" => "zap_crypto_aes256gcm_decrypt",
                "chacha20poly1305_encrypt" => "zap_crypto_chacha20poly1305_encrypt",
                _ => "zap_crypto_chacha20poly1305_decrypt",
            };
            Ok(Some(call_crypto(
                lowerer,
                func,
                vec![key, nonce, data, aad],
            )?))
        }
        // crypto.hkdf(hash_name, ikm, length, salt="", info="") -> bytes (#763,
        // RFC 5869). `length` coerces to i64; omitted `salt`/`info` default to the
        // empty string. The runtime raises ValueError on bad length / unknown hash.
        "hkdf" => {
            if !(3..=5).contains(&args.len()) {
                return Err(err(
                    "crypto.hkdf takes 3 to 5 arguments (hash_name, ikm, length, salt=\"\", info=\"\")",
                ));
            }
            let (name, _) = lowerer.expr(&args[0].value)?;
            let (ikm, _) = lowerer.expr(&args[1].value)?;
            let (len_v, len_t) = lowerer.expr(&args[2].value)?;
            let length = lowerer.coerce(len_v, len_t, Ty::I64);
            let empty_str = |m: &mut FnLower| -> Value {
                let d = m.new_value(Ty::Ptr);
                m.emit(Inst::StrConst {
                    dest: d,
                    bytes: Vec::new(),
                });
                d
            };
            let salt = if args.len() >= 4 {
                lowerer.expr(&args[3].value)?.0
            } else {
                empty_str(lowerer)
            };
            let info = if args.len() == 5 {
                lowerer.expr(&args[4].value)?.0
            } else {
                empty_str(lowerer)
            };
            Ok(Some(call_crypto(
                lowerer,
                "zap_crypto_hkdf",
                vec![name, ikm, length, salt, info],
            )?))
        }
        // crypto.token_urlsafe(nbytes) -> str (#764): count coerces to i64; a
        // negative count raises a catchable ValueError in the runtime.
        "token_urlsafe" => {
            if args.len() != 1 {
                return Err(err("crypto.token_urlsafe takes exactly one argument"));
            }
            let (v, vt) = lowerer.expr(&args[0].value)?;
            let n = lowerer.coerce(v, vt, Ty::I64);
            Ok(Some(call_crypto(
                lowerer,
                "zap_crypto_token_urlsafe",
                vec![n],
            )?))
        }
        // SHA-3 / BLAKE2 one-shot hex (#770), OpenSSL-backed.
        "sha3_256" => one_shot(lowerer, "zap_crypto_sha3_256"),
        "sha3_512" => one_shot(lowerer, "zap_crypto_sha3_512"),
        "blake2b" => one_shot(lowerer, "zap_crypto_blake2b"),
        "blake2s" => one_shot(lowerer, "zap_crypto_blake2s"),
        // Encoding helpers (#771): thin aliases over the existing hex/base64
        // runtime. hex/b64url_encode never throw; unhex/b64url_decode raise a
        // catchable error on malformed input.
        "hex" => one_shot(lowerer, "zap_hex_encode"),
        "unhex" => one_shot(lowerer, "zap_hex_decode"),
        // b64url_encode(data) -> zap_base64_encode(data, urlsafe=1, pad=0).
        "b64url_encode" => {
            if args.len() != 1 {
                return Err(err("crypto.b64url_encode takes exactly one argument"));
            }
            let (data, _) = lowerer.expr(&args[0].value)?;
            let urlsafe = lowerer.iconst(Ty::I64, 1);
            let pad = lowerer.iconst(Ty::I64, 0);
            Ok(Some(call_crypto(
                lowerer,
                "zap_base64_encode",
                vec![data, urlsafe, pad],
            )?))
        }
        // b64url_decode(s) -> zap_base64_decode(s, urlsafe=1).
        "b64url_decode" => {
            if args.len() != 1 {
                return Err(err("crypto.b64url_decode takes exactly one argument"));
            }
            let (data, _) = lowerer.expr(&args[0].value)?;
            let urlsafe = lowerer.iconst(Ty::I64, 1);
            Ok(Some(call_crypto(
                lowerer,
                "zap_base64_decode",
                vec![data, urlsafe],
            )?))
        }
        // Streaming HMAC (#766): hmac_new(hash_name, key) -> handle; update(h,
        // data); hexdigest/digest(h). Mirrors the streaming-hash handle surface.
        "hmac_new" => refs(lowerer, "zap_crypto_hmac_new", 2),
        "hmac_update" => refs(lowerer, "zap_crypto_hmac_update", 2),
        "hmac_hexdigest" => refs(lowerer, "zap_crypto_hmac_hexdigest", 1),
        "hmac_digest" => refs(lowerer, "zap_crypto_hmac_digest", 1),
        // crypto.randbelow(n) -> int (#767): count coerces to i64; the runtime
        // raises on n <= 0.
        "randbelow" => {
            if args.len() != 1 {
                return Err(err("crypto.randbelow takes exactly one argument"));
            }
            let (v, vt) = lowerer.expr(&args[0].value)?;
            let n = lowerer.coerce(v, vt, Ty::I64);
            Ok(Some(call_crypto(lowerer, "zap_crypto_randbelow", vec![n])?))
        }
        // crypto.choice(seq) -> a uniformly random element (#767), CSPRNG-backed.
        // zap_crypto_list_index draws the index (raising a catchable ValueError
        // on an empty list — unlike random.choice, which aborts); the list's
        // typed element getter then reads it, mirroring how `l[i]` and
        // random.choice lower.
        "choice" => {
            if args.len() != 1 {
                return Err(err("crypto.choice takes exactly one argument"));
            }
            let (lst, lty) = lowerer.expr(&args[0].value)?;
            let elem = match lty {
                Ty::List(elem) => elem,
                _ => return Err(err("crypto.choice expects a list")),
            };
            let idx = lowerer.new_value(Ty::I64);
            lowerer.throwing_call(Some(idx), "zap_crypto_list_index", vec![lst]);
            let d = lowerer.new_value(elem.ty());
            lowerer.emit(Inst::Call {
                dest: Some(d),
                func: format!("zap_list_get_{}", elem.val_suffix()),
                args: vec![lst, idx],
            });
            Ok(Some((d, elem.ty())))
        }
        // RSA PKCS#1 v1.5 SHA-384/512 and RSA-PSS (PS256/384/512) sign/verify
        // (#768). Sign takes (pem, data) -> bytes; verify (pem, data, sig) -> bool.
        "rsa_sha384_sign" => refs(lowerer, "zap_crypto_rsa_sha384_sign", 2),
        "rsa_sha512_sign" => refs(lowerer, "zap_crypto_rsa_sha512_sign", 2),
        "rsa_sha384_verify" => refs(lowerer, "zap_crypto_rsa_sha384_verify", 3),
        "rsa_sha512_verify" => refs(lowerer, "zap_crypto_rsa_sha512_verify", 3),
        "rsa_pss_sha256_sign" => refs(lowerer, "zap_crypto_rsa_pss_sha256_sign", 2),
        "rsa_pss_sha384_sign" => refs(lowerer, "zap_crypto_rsa_pss_sha384_sign", 2),
        "rsa_pss_sha512_sign" => refs(lowerer, "zap_crypto_rsa_pss_sha512_sign", 2),
        "rsa_pss_sha256_verify" => refs(lowerer, "zap_crypto_rsa_pss_sha256_verify", 3),
        "rsa_pss_sha384_verify" => refs(lowerer, "zap_crypto_rsa_pss_sha384_verify", 3),
        "rsa_pss_sha512_verify" => refs(lowerer, "zap_crypto_rsa_pss_sha512_verify", 3),
        // Additional JOSE ECDSA curves P-384 (ES384) / P-521 (ES512) (#769).
        "p384_public_key_pem" => refs(lowerer, "zap_crypto_p384_public_key_pem", 2),
        "p521_public_key_pem" => refs(lowerer, "zap_crypto_p521_public_key_pem", 2),
        "p384_sha384_sign" => refs(lowerer, "zap_crypto_p384_sha384_sign", 2),
        "p384_sha384_verify" => refs(lowerer, "zap_crypto_p384_sha384_verify", 3),
        "p521_sha512_sign" => refs(lowerer, "zap_crypto_p521_sha512_sign", 2),
        "p521_sha512_verify" => refs(lowerer, "zap_crypto_p521_sha512_verify", 3),
        // Private-key PEM builders from private JWK components (#778, D-0136).
        "rsa_private_key_pem" => refs(lowerer, "zap_crypto_rsa_private_key_pem", 8),
        "p256_private_key_pem" => refs(lowerer, "zap_crypto_p256_private_key_pem", 1),
        "ed25519_private_key_pem" => refs(lowerer, "zap_crypto_ed25519_private_key_pem", 1),
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
