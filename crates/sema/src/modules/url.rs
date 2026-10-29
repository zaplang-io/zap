//! `url` module — sema facet (ticket #335, D-0056).
//!
//! `encoding/url`: RFC-3986 percent-encoding and decoding over raw UTF-8 bytes,
//! plus the AWS-query canonicalization variant used by SigV4. A `str`'s bytes
//! are its UTF-8 encoding, so encoding proceeds byte-by-byte and is
//! automatically multibyte-correct. The surface:
//!
//! - `url.quote(s) -> str` — strict RFC-3986: unreserved (`A-Za-z0-9-._~`) pass
//!   through, every other byte (incl. `/` and space, which becomes `%20`) is
//!   `%XX` with UPPERCASE hex. This is exactly SigV4's URI-encode, so it is the
//!   primitive the signer calls for query keys/values and single path segments.
//! - `url.quote_path(s) -> str` — like `quote` but leaves `/` unescaped (for an
//!   already-structured path). SigV4 CanonicalURI single-encode; the non-S3
//!   services double-encode, i.e. `url.quote_path(url.quote_path(path))`.
//! - `url.unquote(s) -> str` — inverse of `quote`: decode `%XX` (hex
//!   case-insensitive); a `+` stays literal; a malformed/truncated `%` escape
//!   passes through unchanged.
//! - `url.unquote_plus(s) -> str` — like `unquote` but also `+` -> space, for
//!   `application/x-www-form-urlencoded` query values.
//! - `url.encode_query(d: dict[str,str]) -> str` — build `k=v&...` with every
//!   key and value `quote`-encoded, in the dict's iteration order (unspecified,
//!   i.e. hash order; use `canonical_query` when a deterministic order matters).
//! - `url.canonical_query(d: dict[str,str]) -> str` — the SigV4
//!   CanonicalQueryString: every key/value `quote`-encoded, pairs sorted by
//!   encoded key then encoded value, joined with `&`.
//!
//! `import url` is accepted but not required.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{assignable, expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck { name: "url", check };

/// Require exactly one `str` argument, inferring it either way so the argument
/// expression is still type-checked on an arity mismatch.
fn expect_str(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    expect_argc(checker, "url", method, args, 1, span);
    if let Some(arg) = args.first() {
        let t = checker.infer(&arg.value);
        if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "url.{method}() expects a str argument, found {}",
                    t.display()
                ),
                expr_span(&arg.value),
            );
        }
    }
}

/// Require exactly one `dict[str,str]` argument (the query map).
fn expect_str_dict(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    expect_argc(checker, "url", method, args, 1, span);
    if let Some(arg) = args.first() {
        let t = checker.infer(&arg.value);
        let want = Ty::Dict(Box::new(Ty::Str), Box::new(Ty::Str));
        if !assignable(&t, &want) {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "url.{method}() expects a dict[str, str] argument, found {}",
                    t.display()
                ),
                expr_span(&arg.value),
            );
        }
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // Byte-level percent codec.
        "quote" | "quote_path" | "unquote" | "unquote_plus" => {
            expect_str(checker, method, args, span);
            Ty::Str
        }
        // Query builders over a dict[str,str].
        "encode_query" | "canonical_query" => {
            expect_str_dict(checker, method, args, span);
            Ty::Str
        }
        _ => return None,
    })
}
