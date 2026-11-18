//! `datetime` timezone surface — sema shape/arity/type checks (#1045, D-0225).
//!
//! Exhaustive type-checking for the timezone database surface: `zone`,
//! `fixed_offset`, `astimezone`, `isoformat`/`rfc3339`, and `rfc3339_parse`,
//! including their return types (the prelude `TimeZone`/`DateTime` value
//! structs) and the argument type/arity diagnostics. Host-only (parse + sema),
//! no LLVM.

use zapc_sema::check_str;

#[track_caller]
fn sema_errors(src: &str) -> Vec<(String, String)> {
    let (terrs, perrs, lerrs) = check_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    terrs
        .into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn assert_ok(src: &str) {
    let errs = sema_errors(src);
    assert!(errs.is_empty(), "expected no sema errors, got: {errs:?}");
}

#[track_caller]
fn assert_code(src: &str, code: &str) {
    let errs = sema_errors(src);
    assert!(
        errs.iter().any(|(c, _)| c == code),
        "expected a {code} error, got: {errs:?}",
    );
}

fn prog(body: &str) -> String {
    format!("def f():\n{body}\n")
}

// ---------------------------------------------------------------------------
// Return types — zone/fixed_offset -> TimeZone; astimezone/rfc3339_parse ->
// DateTime; isoformat/rfc3339 -> str. Annotated locals force the check.
// ---------------------------------------------------------------------------

#[test]
fn zone_and_fixed_offset_return_timezone() {
    assert_ok(&prog("    TimeZone z = datetime.zone(\"Asia/Kolkata\")\n"));
    assert_ok(&prog("    TimeZone z = datetime.fixed_offset(330)\n"));
    assert_ok(&prog("    TimeZone z = datetime.fixed_offset(-300)\n"));
}

#[test]
fn astimezone_returns_datetime() {
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    \
         TimeZone z = datetime.fixed_offset(330)\n    \
         DateTime a = datetime.astimezone(t, z)\n",
    ));
    // The result is itself a DateTime, so it flows into the #974 helpers.
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    \
         auto z = datetime.zone(\"UTC\")\n    \
         auto a = datetime.astimezone(t, z)\n    \
         auto w = datetime.weekday(a)\n",
    ));
}

#[test]
fn isoformat_rfc3339_return_str() {
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    str s = datetime.isoformat(t)\n",
    ));
    assert_ok(&prog(
        "    auto t = datetime.from_timestamp(0)\n    str s = datetime.rfc3339(t)\n",
    ));
}

#[test]
fn rfc3339_parse_returns_datetime() {
    assert_ok(&prog(
        "    DateTime d = datetime.rfc3339_parse(\"2021-06-01T05:30:00+05:30\")\n",
    ));
    assert_ok(&prog(
        "    auto d = datetime.rfc3339_parse(\"2021-06-01T00:00:00Z\")\n    \
         auto ts = datetime.to_timestamp(d)\n",
    ));
}

// ---------------------------------------------------------------------------
// Argument type diagnostics (ZAP4024).
// ---------------------------------------------------------------------------

#[test]
fn zone_rejects_non_str() {
    assert_code(&prog("    auto z = datetime.zone(42)\n"), "ZAP4024");
}

#[test]
fn fixed_offset_rejects_non_int() {
    assert_code(
        &prog("    auto z = datetime.fixed_offset(\"nope\")\n"),
        "ZAP4024",
    );
}

#[test]
fn astimezone_rejects_bad_operands() {
    // First arg must be a DateTime.
    assert_code(
        &prog(
            "    auto z = datetime.fixed_offset(0)\n    auto a = datetime.astimezone(\"x\", z)\n",
        ),
        "ZAP4024",
    );
    // Second arg must be a TimeZone (a DateTime is not one).
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto a = datetime.astimezone(t, t)\n"),
        "ZAP4024",
    );
}

#[test]
fn isoformat_rejects_non_datetime() {
    assert_code(&prog("    auto s = datetime.isoformat(\"x\")\n"), "ZAP4024");
    assert_code(&prog("    auto s = datetime.rfc3339(5)\n"), "ZAP4024");
}

#[test]
fn rfc3339_parse_rejects_non_str() {
    assert_code(&prog("    auto d = datetime.rfc3339_parse(5)\n"), "ZAP4024");
}

// ---------------------------------------------------------------------------
// Arity diagnostics (ZAP4025).
// ---------------------------------------------------------------------------

#[test]
fn arity_errors() {
    assert_code(&prog("    auto z = datetime.zone()\n"), "ZAP4025");
    assert_code(
        &prog("    auto z = datetime.zone(\"UTC\", \"extra\")\n"),
        "ZAP4025",
    );
    assert_code(&prog("    auto z = datetime.fixed_offset()\n"), "ZAP4025");
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto a = datetime.astimezone(t)\n"),
        "ZAP4025",
    );
    assert_code(
        &prog(
            "    auto t = datetime.from_timestamp(0)\n    \
             auto z = datetime.fixed_offset(0)\n    \
             auto a = datetime.astimezone(t, z, z)\n",
        ),
        "ZAP4025",
    );
    assert_code(
        &prog("    auto t = datetime.from_timestamp(0)\n    auto s = datetime.isoformat(t, t)\n"),
        "ZAP4025",
    );
}
