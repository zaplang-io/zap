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
//! RSA PKCS#1 v1.5 signing over a PEM-encoded private key (PKCS#1 or PKCS#8),
//! returning the RAW signature bytes (for CloudFront signed URLs, #628):
//!   `crypto.rsa_sha1_sign(pem_key, data) -> bytes`
//!   `crypto.rsa_sha256_sign(pem_key, data) -> bytes`
//! Verification against a PEM-encoded public key:
//!   `crypto.rsa_sha256_verify(pem_pubkey, data, sig) -> bool`
//!
//! Secure random from the OS CSPRNG (getentropy / /dev/urandom):
//!   `crypto.random_bytes(n) -> bytes` ; `crypto.token_hex(n) -> str`

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
