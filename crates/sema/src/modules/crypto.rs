//! `crypto` module — sema facet (ticket #105, D-0056).
//!
//! Hashing (`sha256`/`sha512`/`sha1`/`md5`), HMAC and secure random / secrets.
//! TLS is explicitly deferred (late-v1). Surface:
//!
//! One-shot hashing over `str`/`bytes`, returning the lowercase hex digest:
//!   `crypto.sha256(data) -> str`, and `sha512`/`sha1`/`md5` likewise.
//!
//! Streaming hashing — a `_new` returns an opaque `bytes` handle whose buffer
//! holds the hash context; feed it incrementally and finalize (non-destructive,
//! so a handle may be digested more than once):
//!   `crypto.sha256_new() -> bytes` (and `sha512`/`sha1`/`md5`)
//!   `crypto.update(h, data)` ; `crypto.hexdigest(h) -> str` ; `crypto.digest(h) -> bytes`
//!
//! HMAC over (key, message), returning the lowercase hex digest:
//!   `crypto.hmac_sha256(key, msg) -> str` ; `crypto.hmac_sha512(key, msg) -> str`
//! The raw-bytes HMAC-SHA256 digest (for chaining, e.g. AWS SigV4, #153):
//!   `crypto.hmac_sha256_raw(key, msg) -> bytes`
//!
//! Constant-time byte comparison (#729, D-0131) — timing independent of the
//! byte *contents* for equal-length inputs; a length difference is observable:
//!   `crypto.constant_time_eq(a, b) -> bool`
//!
//! RSA PKCS#1 v1.5 signing over a PEM-encoded private key (PKCS#1 or PKCS#8),
//! returning the RAW signature bytes (for CloudFront signed URLs, #628):
//!   `crypto.rsa_sha1_sign(pem_key, data) -> bytes`
//!   `crypto.rsa_sha256_sign(pem_key, data) -> bytes`
//! Verification against a PEM-encoded public key:
//!   `crypto.rsa_sha256_verify(pem_pubkey, data, sig) -> bool`
//!
//! Secure random from the OS CSPRNG (getentropy / /dev/urandom):
//!   `crypto.random_bytes(n) -> bytes` ; `crypto.token_hex(n) -> str`
//!
//! Validated asymmetric keys — metadata, JWK-component import, and P-256 ECDSA
//! (ES256) / Ed25519 sign+verify (#643, D-0126). All free functions over PEM /
//! raw components; no raw OpenSSL state is exposed:
//!   `crypto.key_info(pem) -> KeyInfo` — the built-in `KeyInfo` value struct
//!     `{ str kind; int64 bits; str curve; bool is_private }`. `kind` is
//!     `"RSA"`/`"EC"`/`"Ed25519"`; `bits` is the RSA modulus size (or 256 for
//!     P-256/Ed25519); `curve` is `"P-256"` for EC (else `""`). Raises
//!     `ValueError` on an unparseable / unsupported / encrypted key.
//!   `crypto.rsa_public_key_pem(n, e) -> bytes`,
//!   `crypto.p256_public_key_pem(x, y) -> bytes`,
//!   `crypto.ed25519_public_key_pem(x) -> bytes` — validated SubjectPublicKeyInfo
//!     PEM from JWK integer/octet components (P-256 coords are 32 bytes and must
//!     lie on the curve; Ed25519 keys are 32 bytes).
//!   `crypto.p256_sha256_sign(private_pem, msg) -> bytes` /
//!   `crypto.p256_sha256_verify(public_pem, msg, sig) -> bool` — ES256; the
//!     signature is the JOSE fixed-width 64-byte `r||s` (DER is never exposed).
//!   `crypto.ed25519_sign(private_pem, msg) -> bytes` /
//!   `crypto.ed25519_verify(public_pem, msg, sig) -> bool` — pure Ed25519 over
//!     the original message (RFC 8032); 64-byte signatures.
//! Signing requires a private key of the exact family (a public key / wrong
//! family / wrong curve raises `ValueError`); verification returns `false` on any
//! mismatch or tamper, never a panic.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "crypto",
    check,
};

/// A `str`/`bytes` value is required for data/key/message arguments (both share
/// the `ZapStr` runtime layout). Infers the argument so it is still type-checked.
fn expect_data(checker: &mut Checker, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
        checker.error(
            ErrorCode::ZAP4024,
            format!(
                "{func}() expects a str or bytes argument, found {}",
                t.display()
            ),
            expr_span(&arg.value),
        );
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // One-shot hashing: crypto.<algo>(data: str|bytes) -> str (hex digest).
        "sha256" | "sha512" | "sha1" | "md5" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(data) takes 1 argument, found {}",
                        args.len()
                    ),
                    span,
                );
            } else {
                expect_data(checker, method, &args[0]);
            }
            Some(Ty::Str)
        }
        // Streaming: crypto.<algo>_new() -> bytes (an opaque context handle).
        "sha256_new" | "sha512_new" | "sha1_new" | "md5_new" => {
            expect_argc(checker, "crypto", method, args, 0, span);
            Some(Ty::Bytes)
        }
        // crypto.update(handle, data): feed more input into a streaming handle.
        "update" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.update(h, data) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                checker.infer(&args[0].value);
                expect_data(checker, "update", &args[1]);
            }
            Some(Ty::Unit)
        }
        // crypto.hexdigest(handle) -> str: finalize to a lowercase hex digest.
        "hexdigest" => {
            expect_argc(checker, "crypto", "hexdigest", args, 1, span);
            Some(Ty::Str)
        }
        // crypto.digest(handle) -> bytes: finalize to the raw digest bytes.
        "digest" => {
            expect_argc(checker, "crypto", "digest", args, 1, span);
            Some(Ty::Bytes)
        }
        // HMAC: crypto.hmac_<algo>(key, msg) -> str (hex digest).
        "hmac_sha256" | "hmac_sha512" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(key, msg) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Str)
        }
        // Raw-bytes HMAC-SHA256: crypto.hmac_sha256_raw(key, msg) -> bytes (the
        // 32-byte digest). Lets a digest be fed as the next step's key without a
        // lossy hex round-trip (AWS SigV4 signing-key derivation, #153).
        "hmac_sha256_raw" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(key, msg) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bytes)
        }
        // crypto.constant_time_eq(a, b) -> bool (#729, D-0131): true iff the two
        // byte sequences are equal, compared in time independent of their byte
        // contents (OpenSSL CRYPTO_memcmp) for equal-length inputs. A length
        // difference returns false and is observable; no branch is taken on byte
        // contents. Both args are str|bytes; empty inputs are valid.
        "constant_time_eq" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(a, b) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bool)
        }
        // RSA PKCS#1 v1.5 signing: crypto.rsa_<hash>_sign(pem_key, data) -> bytes
        // (the raw signature over the hash of `data`). The PEM private key may be
        // PKCS#1 (`BEGIN RSA PRIVATE KEY`) or PKCS#8 (`BEGIN PRIVATE KEY`). The
        // CloudFront base64 variant is applied by the caller, not here (#628).
        "rsa_sha1_sign" | "rsa_sha256_sign" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(pem_key, data) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bytes)
        }
        // crypto.rsa_sha256_verify(pem_pubkey, data, sig) -> bool: true iff `sig`
        // is a valid RSA-SHA256 PKCS#1 v1.5 signature over `data` (#628).
        "rsa_sha256_verify" => {
            if args.len() != 3 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(pem_pubkey, data, sig) takes 3 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
                expect_data(checker, method, &args[2]);
            }
            Some(Ty::Bool)
        }
        // crypto.key_info(pem) -> KeyInfo (#643, D-0126): validated metadata for a
        // supported PEM/DER asymmetric key. Raises ValueError on an unparseable,
        // unsupported, or encrypted key.
        "key_info" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.key_info(pem) takes 1 argument, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
            }
            Some(Ty::Named("KeyInfo".to_string()))
        }
        // crypto.rsa_public_key_pem(n_bytes, e_bytes) -> bytes (#643): build a
        // validated SPKI PEM public key from the RSA JWK integer components.
        // crypto.p256_public_key_pem(x_bytes, y_bytes) -> bytes: P-256 affine
        // coordinates (32 bytes each), validated to lie on the curve.
        "rsa_public_key_pem" | "p256_public_key_pem" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(a, b) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bytes)
        }
        // crypto.ed25519_public_key_pem(x_bytes) -> bytes (#643): build a
        // validated SPKI PEM public key from the 32-byte Ed25519 public key.
        "ed25519_public_key_pem" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(x_bytes) takes 1 argument, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
            }
            Some(Ty::Bytes)
        }
        // crypto.p256_sha256_sign(private_pem, msg) -> bytes (#643, ES256): the
        // 64-byte JOSE r||s signature. crypto.ed25519_sign(private_pem, msg) ->
        // bytes: the 64-byte pure-Ed25519 signature over the original message.
        "p256_sha256_sign" | "ed25519_sign" => {
            if args.len() != 2 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(private_pem, msg) takes 2 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
            }
            Some(Ty::Bytes)
        }
        // crypto.p256_sha256_verify(public_pem, msg, sig) -> bool (#643): true iff
        // `sig` is a valid 64-byte JOSE ES256 signature. crypto.ed25519_verify
        // (public_pem, msg, sig) -> bool: pure-Ed25519 verification. A bad key or
        // any verification failure is a plain `false`, never a panic.
        "p256_sha256_verify" | "ed25519_verify" => {
            if args.len() != 3 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "crypto.{method}(public_pem, msg, sig) takes 3 arguments, found {}",
                        args.len()
                    ),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, method, &args[0]);
                expect_data(checker, method, &args[1]);
                expect_data(checker, method, &args[2]);
            }
            Some(Ty::Bool)
        }
        // crypto.random_bytes(n) -> bytes: n cryptographically secure random bytes.
        "random_bytes" => {
            expect_argc(checker, "crypto", "random_bytes", args, 1, span);
            Some(Ty::Bytes)
        }
        // crypto.token_hex(n) -> str: n secure random bytes as 2n lowercase hex chars.
        "token_hex" => {
            expect_argc(checker, "crypto", "token_hex", args, 1, span);
            Some(Ty::Str)
        }
        _ => None,
    }
}
