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
//! - `url.unquote(s[, errors]) -> str` — inverse of `quote`: decode `%XX` (hex
//!   case-insensitive); a `+` stays literal; a malformed/truncated `%` escape
//!   passes through unchanged. The optional `errors` str ("strict" | "replace",
//!   default "strict", #1242) controls UTF-8 handling of the decoded octets:
//!   "strict" raises a catchable error on invalid UTF-8 (like `str(bytes)`),
//!   "replace" substitutes U+FFFD for each maximal invalid subpart.
//! - `url.unquote_plus(s[, errors]) -> str` — like `unquote` but also `+` ->
//!   space, for `application/x-www-form-urlencoded` query values.
//! - `url.encode_query(d: dict[str,str]) -> str` — build `k=v&...` with every
//!   key and value `quote`-encoded, in the dict's iteration order (unspecified,
//!   i.e. hash order; use `canonical_query` when a deterministic order matters).
//! - `url.canonical_query(d: dict[str,str]) -> str` — the SigV4
//!   CanonicalQueryString: every key/value `quote`-encoded, pairs sorted by
//!   encoded key then encoded value, joined with `&`.
//!
//! The parse/build half (#970, D-0209) takes a URL apart and reassembles it:
//!
//! - `url.split(s) -> UrlParts` — RFC-3986 decomposition into the prelude value
//!   struct `UrlParts` (`scheme`, `has_authority`, `userinfo`, `host`, `port`,
//!   `path`, `query`, `fragment`). Lenient like Python `urlsplit`; a present but
//!   non-numeric / out-of-range `port` raises a catchable `ValueError`. The
//!   IPv6 `host` is stored WITHOUT its brackets (Python `.hostname` form), and
//!   `port` is `-1` when absent.
//! - `url.build(parts: UrlParts) -> str` — the inverse of `split` (Python
//!   `urlunsplit`); round-trips a normalized URL (`build(split(s)) == s`). A
//!   host containing `:` (an IPv6 literal) is re-wrapped in `[...]`.
//! - `url.parse_query(q: str) -> dict[str, list[str]]` — read `k=v&k=v` into a
//!   multimap (Python `parse_qs`), honoring `%XX`, `+`->space and repeated keys;
//!   a blank value / a key with no `=` is kept with an empty value (Go
//!   `url.ParseQuery` form). A malformed `%XX` is left literal (like `unquote`).
//! - `url.parse_query_first(q: str) -> dict[str, str]` — the first value per key
//!   (a scalar convenience over `parse_query`).
//! - `url.join(base: str, ref: str) -> str` — resolve a relative reference
//!   against a base per RFC-3986 §5 (Python `urljoin`; `.`/`..` removal,
//!   absolute / scheme-relative / query-only / fragment-only references).
//! - `url.encode_query_multi(d: dict[str, list[str]]) -> str` — the repeated-key
//!   query encoder (companion to `encode_query`, which is `dict[str,str]`).
//!
//! `import url` is accepted but not required.

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, expect_argc_range, ModuleCheck};
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

/// Type-check `url.unquote` / `url.unquote_plus` (#1242): a required `str`/`bytes`
/// first argument and an optional `errors` `str` (the seam binder has already
/// reordered any keyword and stripped its name, so a supplied `errors` lands in
/// `args[1]`). Each argument is inferred either way so it is still checked on an
/// arity mismatch.
fn expect_unquote(checker: &mut Checker, method: &str, args: &[Arg], span: Span) {
    expect_argc_range(checker, "url", method, args, 1, 2, span);
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
    if let Some(arg) = args.get(1) {
        let t = checker.infer(&arg.value);
        if !assignable(&t, &Ty::Str) {
            checker.error(
                ErrorCode::ZAP4024,
                format!(
                    "url.{method}() expects a str `errors` argument, found {}",
                    t.display()
                ),
                expr_span(&arg.value),
            );
        }
    }
}

/// Require exactly one argument assignable to `want` (a query map), with a
/// human-readable `desc` for the diagnostic.
fn expect_dict(
    checker: &mut Checker,
    method: &str,
    args: &[Arg],
    span: Span,
    want: Ty,
    desc: &str,
) {
    expect_argc(checker, "url", method, args, 1, span);
    if let Some(arg) = args.first() {
        let t = checker.infer(&arg.value);
        if !assignable(&t, &want) {
            checker.error(
                ErrorCode::ZAP4024,
                format!("url.{method}() expects {desc}, found {}", t.display()),
                expr_span(&arg.value),
            );
        }
    }
}

/// The prelude value-struct name `url.split` returns / `url.build` consumes.
/// Kept in step with `zapc_syntax::parser::PRELUDE_SRC`.
const URL_PARTS: &str = "UrlParts";

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // Byte-level percent encoders: str -> str.
        "quote" | "quote_path" => {
            expect_str(checker, method, args, span);
            Ty::Str
        }
        // Percent decoders: `(s: str[, errors: str]) -> str` (#1242). `errors`
        // ("strict" | "replace") selects how invalid UTF-8 in the decoded octets
        // is handled; it is optional and defaults to strict. The value set is not
        // statically constrained (an unknown name raises at runtime, like
        // CPython's codec lookup), so `errors` only needs to be a `str`.
        "unquote" | "unquote_plus" => {
            expect_unquote(checker, method, args, span);
            Ty::Str
        }
        // Query builders over a dict[str,str].
        "encode_query" | "canonical_query" => {
            expect_dict(
                checker,
                method,
                args,
                span,
                Ty::Dict(Box::new(Ty::Str), Box::new(Ty::Str)),
                "a dict[str, str] argument",
            );
            Ty::Str
        }
        // Repeated-key query encoder over a dict[str, list[str]] (#970).
        "encode_query_multi" => {
            expect_dict(
                checker,
                method,
                args,
                span,
                Ty::Dict(Box::new(Ty::Str), Box::new(Ty::List(Box::new(Ty::Str)))),
                "a dict[str, list[str]] argument",
            );
            Ty::Str
        }
        // Decompose a URL into the prelude `UrlParts` value struct (#970).
        "split" => {
            expect_str(checker, method, args, span);
            Ty::Named(URL_PARTS.to_string())
        }
        // Reassemble a URL from a `UrlParts` value struct (#970).
        "build" => {
            expect_argc(checker, "url", method, args, 1, span);
            if let Some(arg) = args.first() {
                let t = checker.infer(&arg.value);
                if !assignable(&t, &Ty::Named(URL_PARTS.to_string())) && !matches!(t, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "url.{method}() expects a UrlParts argument, found {}",
                            t.display()
                        ),
                        expr_span(&arg.value),
                    );
                }
            }
            Ty::Str
        }
        // Query-string readers (#970): str -> dict.
        "parse_query" => {
            expect_str(checker, method, args, span);
            Ty::Dict(Box::new(Ty::Str), Box::new(Ty::List(Box::new(Ty::Str))))
        }
        "parse_query_first" => {
            expect_str(checker, method, args, span);
            Ty::Dict(Box::new(Ty::Str), Box::new(Ty::Str))
        }
        // RFC-3986 reference resolution (#970): (base, ref) -> str.
        "join" => {
            expect_argc(checker, "url", method, args, 2, span);
            for arg in args {
                let t = checker.infer(&arg.value);
                if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "url.{method}() expects str arguments, found {}",
                            t.display()
                        ),
                        expr_span(&arg.value),
                    );
                }
            }
            Ty::Str
        }
        _ => return None,
    })
}
