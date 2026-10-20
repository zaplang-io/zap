//! `zlib` module — sema facet (ticket #106, D-0056/D-0060).
//!
//! DEFLATE compression with the zlib wrapper (RFC 1950): an Adler-32-checked
//! stream. Backed by the system zlib (`-lz`), which the build already links.
//! Surface (`import zlib` accepted but not required, like `crypto`):
//!
//!   `zlib.compress(data: str|bytes, level?: int) -> bytes`
//!   `zlib.decompress(data: str|bytes) -> bytes`
//!   `zlib.crc32(data: str|bytes) -> int`     (ISO CRC32, poly 0x04C11DB7)
//!   `zlib.adler32(data: str|bytes) -> int`   (Adler-32)
//!
//! `gzip` (the RFC 1952 / CRC32 framing) is the sibling module `gzip`.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "zlib",
    check,
};

/// A `str`/`bytes` value is required for the data argument (both share the
/// `ZapStr` runtime layout). Infers the argument so it is still type-checked.
pub(super) fn expect_data(checker: &mut Checker, module: &str, func: &str, arg: &Arg) {
    let t = checker.infer(&arg.value);
    if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
        checker.error(
            format!(
                "{module}.{func}() expects a str or bytes argument, found {}",
                t.display()
            ),
            expr_span(&arg.value),
        );
    }
}

/// Type-check a `module.compress(data, level?)` call: a `str`/`bytes` data
/// argument and an optional numeric compression level. Shared by `zlib` and
/// `gzip` (see [`gzip`](super::gzip)).
pub(super) fn check_compress(checker: &mut Checker, module: &str, args: &[Arg], span: Span) {
    if args.is_empty() || args.len() > 2 {
        checker.error(
            format!(
                "{module}.compress(data, level?) takes 1 or 2 arguments, found {}",
                args.len()
            ),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return;
    }
    expect_data(checker, module, "compress", &args[0]);
    if let Some(level) = args.get(1) {
        let lt = checker.infer(&level.value);
        if !lt.is_numeric() && !matches!(lt, Ty::Unknown) {
            checker.error(
                format!(
                    "{module}.compress() level must be an int, found {}",
                    lt.display()
                ),
                expr_span(&level.value),
            );
        }
    }
}

/// Type-check a `module.decompress(data)` call: a single `str`/`bytes` argument.
/// Shared by `zlib` and `gzip`.
pub(super) fn check_decompress(checker: &mut Checker, module: &str, args: &[Arg], span: Span) {
    if args.len() != 1 {
        checker.error(
            format!(
                "{module}.decompress(data) takes 1 argument, found {}",
                args.len()
            ),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return;
    }
    expect_data(checker, module, "decompress", &args[0]);
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // zlib.compress(data, level?) -> bytes (RFC 1950, Adler-32 trailer).
        "compress" => {
            check_compress(checker, "zlib", args, span);
            Some(Ty::Bytes)
        }
        // zlib.decompress(data) -> bytes.
        "decompress" => {
            check_decompress(checker, "zlib", args, span);
            Some(Ty::Bytes)
        }
        // zlib.crc32(data) / zlib.adler32(data) -> int (the running checksum of
        // `data` from the standard initial value), 0..2^32-1.
        "crc32" | "adler32" => {
            if args.len() != 1 {
                checker.error(
                    format!("zlib.{method}(data) takes 1 argument, found {}", args.len()),
                    span,
                );
                for a in args {
                    checker.infer(&a.value);
                }
            } else {
                expect_data(checker, "zlib", method, &args[0]);
            }
            Some(Ty::Int64)
        }
        _ => None,
    }
}
