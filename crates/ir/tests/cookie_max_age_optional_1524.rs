//! `http.parse_set_cookie(...).max_age` is `Optional[int64]` (#1524).
//!
//! The parser used to return the int64 `-1` BOTH for an absent `Max-Age` and for
//! a wire `Max-Age=-1`, so a caller could not tell a session cookie (defer to
//! `Expires`) apart from an immediate-deletion cookie (RFC 6265 §5.2.2). The fix
//! makes the typed `Cookie.max_age` field `Optional[int64]`: `None` for an absent
//! or malformed attribute, `Some(n)` for a parsed value — including `Some(0)` and
//! `Some(-1)`.
//!
//! These are host-only lowering checks (no LLVM): the native five-case runtime
//! behaviour (`omitted`/`malformed`/`-1`/`0`/positive, plus the Max-Age-vs-Expires
//! precedence note) is pinned by the `http_cookie_max_age` golden on CI, since the
//! absent-vs-parsed distinction is only observable once the collector runs. Here
//! we pin that the lowering really threads the C `_present` flag into an Optional
//! `Some`/`None` construction rather than storing a bare `-1` sentinel into the
//! field.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

/// Parse + lower `src`, asserting no lex/parse/lower errors. Returns the ZIR
/// module text (`Display`) for structural assertions.
fn lower_ok(src: &str) -> String {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
    format!("{module}")
}

fn count(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}

/// The repro from the issue: one absent-Max-Age parse and one `Max-Age=-1` parse.
const REPRO: &str = "def main():\n\
    \x20   auto session_cookie = http.parse_set_cookie(\"id=one; Path=/\")\n\
    \x20   auto expired_cookie = http.parse_set_cookie(\"id=one; Path=/; Max-Age=-1\")\n\
    \x20   print(session_cookie.max_age.is_none())\n\
    \x20   print(expired_cookie.max_age.unwrap())\n";

#[test]
fn max_age_lowering_consults_the_present_flag() {
    // The crux: before the fix the lowering never asked whether a Max-Age was
    // actually present — it stored the raw getter result straight into the field.
    // Now each parse must call the `_present` discriminator so `None` (absent)
    // and `Some(-1)` (a valid -1) take different branches.
    let zir = lower_ok(REPRO);
    assert_eq!(
        count(&zir, "zap_http_cookie_max_age_present"),
        2,
        "each of the two parses must consult the present flag, got:\n{zir}"
    );
    assert_eq!(
        count(&zir, "@zap_http_cookie_max_age("),
        2,
        "each parse still reads the signed value getter, got:\n{zir}"
    );
}

#[test]
fn max_age_lowering_wraps_some_and_none_through_a_branch() {
    // The present flag must drive a real conditional that builds either a `Some`
    // (payload stored) or a `None` — not an unconditional field store. One
    // `br_if` per parse, and the Optional is materialised through a slot
    // (`alloca`/`store`/`load`), the same shape the JSON-decode Optional path and
    // the scalar-wrap fix use.
    let zir = lower_ok(REPRO);
    assert!(
        count(&zir, "br_if") >= 2,
        "the present/absent split must be a conditional branch, got:\n{zir}"
    );
    assert!(
        zir.contains("is_none") || zir.contains("isnull") || zir.contains("icmp"),
        "expected a discriminant test for the Optional, got:\n{zir}"
    );
}

#[test]
fn consuming_max_age_as_optional_typechecks_and_lowers() {
    // `Optional[int64]` ergonomics on the result: `is None`, `.unwrap()`,
    // `.unwrap_or(...)`, and a `match` all lower cleanly, proving the field really
    // carries the Optional type (not a bare int64) end to end through lowering.
    lower_ok(
        "def classify(str header) -> str:\n\
        \x20   auto ck = http.parse_set_cookie(header)\n\
        \x20   if ck.max_age is None:\n\
        \x20       return \"session\"\n\
        \x20   auto secs = ck.max_age.unwrap()\n\
        \x20   if secs <= 0:\n\
        \x20       return \"delete\"\n\
        \x20   return \"expire\"\n\
        def main():\n\
        \x20   print(classify(\"id=one; Path=/\"))\n\
        \x20   print(classify(\"id=one; Max-Age=-1\"))\n\
        \x20   print(classify(\"id=one; Max-Age=0\"))\n\
        \x20   print(classify(\"id=one; Max-Age=3600\"))\n",
    );
}

#[test]
fn unwrap_or_default_on_absent_lowers() {
    // A caller that collapses absent to a default still lowers — `unwrap_or`
    // takes the Optional, confirming the field type.
    lower_ok(
        "def main():\n\
        \x20   auto ck = http.parse_set_cookie(\"id=one; Path=/\")\n\
        \x20   print(ck.max_age.unwrap_or(-9))\n",
    );
}
