//! `base64` module — sema facet (ticket #337, D-0056).
//!
//! Base64 transfer encoding over `bytes`/`str`. Both the standard (`+`/`/`) and
//! URL-safe (`-`/`_`) alphabets, each with and without `=` padding:
//!   `base64.encode(data) -> str`              (standard, padded)
//!   `base64.encode_nopad(data) -> str`        (standard, no padding)
//!   `base64.urlsafe_encode(data) -> str`      (URL-safe, padded)
//!   `base64.urlsafe_encode_nopad(data) -> str`(URL-safe, no padding)
//!   `base64.decode(s) -> bytes`               (standard; padding optional)
//!   `base64.urlsafe_decode(s) -> bytes`       (URL-safe; padding optional)
//!
//! `data`/`s` accept `str` or `bytes` (both are the `ZapStr` runtime layout).

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "base64",
    check,
};

/// A one-argument codec call whose argument must be `str`/`bytes`. Reports a
/// bad arity or argument type, then returns `result`.
fn codec1(checker: &mut Checker, method: &str, args: &[Arg], span: Span, result: Ty) -> Option<Ty> {
    if args.len() != 1 {
        checker.error(
            ErrorCode::ZAP4025,
            format!("base64.{method}() takes 1 argument, found {}", args.len()),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
    } else {
        let t = checker.infer(&args[0].value);
        if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "base64.{method}() expects a str or bytes argument, found {}",
                    t.display()
                ),
                expr_span(&args[0].value),
            );
        }
    }
    Some(result)
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        "encode" | "encode_nopad" | "urlsafe_encode" | "urlsafe_encode_nopad" => {
            codec1(checker, method, args, span, Ty::Str)
        }
        "decode" | "urlsafe_decode" => codec1(checker, method, args, span, Ty::Bytes),
        _ => None,
    }
}
