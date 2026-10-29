//! `hex` module — sema facet (ticket #337, D-0056).
//!
//! Hexadecimal encoding over `bytes`/`str`:
//!   `hex.encode(data) -> str`   (lowercase hex; `data` is `str`/`bytes`)
//!   `hex.decode(s) -> bytes`    (parses `[0-9a-fA-F]`, even length; faults
//!                                at runtime on malformed input)

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "hex", check };

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    let result = match method {
        "encode" => Ty::Str,
        "decode" => Ty::Bytes,
        _ => return None,
    };
    if args.len() != 1 {
        checker.error(
            ErrorCode::ZAP4025,
            format!("hex.{method}() takes 1 argument, found {}", args.len()),
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
                    "hex.{method}() expects a str or bytes argument, found {}",
                    t.display()
                ),
                expr_span(&args[0].value),
            );
        }
    }
    Some(result)
}
