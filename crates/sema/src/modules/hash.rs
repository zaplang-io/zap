//! `hash` module — sema facet (I#139 Tier 2, #962, D-0199).
//!
//! The portable, friendly checksum wrapper sitting on top of the raw `arch.*`
//! intrinsics (D-0079 §1/§6.8):
//!   `hash.crc32c(data: bytes|str) -> uint32`
//!
//! `crc32c` is CRC32C / Castagnoli (poly 0x1EDC6F41, the SSE4.2 / aarch64-CRC
//! instruction's polynomial) — **not** the ISO/zlib CRC32 (`zlib.crc32`, gzip).
//! Unlike the raw `arch.*` intrinsics it is portable: it compiles and runs on
//! every target, using the hardware path where available and a bit-identical
//! software CRC32C otherwise (the one place a software fallback is correct,
//! because the result is the same either way). The name is always `crc32c`,
//! never a bare `crc32`.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "hash",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        "crc32c" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("hash.crc32c() takes 1 argument, found {}", args.len()),
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
                            "hash.crc32c() expects a str or bytes argument, found {}",
                            t.display()
                        ),
                        expr_span(&args[0].value),
                    );
                }
            }
            Some(Ty::Uint32)
        }
        _ => None,
    }
}
