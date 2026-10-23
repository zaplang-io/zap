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
