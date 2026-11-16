//! `select` statement sema (D-0031, #1002): CSP channel multiplexing.
//!
//! Each case is a single channel op whose operand must be a channel. A receive
//! binds fresh case-local name(s) (the value, and an optional comma-ok `bool`);
//! a send checks the value against the element type. At least one case; at most
//! one `default`. Host-only (parse + sema); lowering is a separate stage.

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

/// `def f():` with an `int64` channel `ch` in scope, then `body` (already
/// indented to 4 spaces).
fn prog(body: &str) -> String {
    format!("def f():\n    auto ch = channel[int64]()\n{body}")
}

// ---------------------------------------------------------------------------
// Positive: the three case forms bind/typecheck.
// ---------------------------------------------------------------------------

#[test]
fn receive_binds_value_and_default() {
    // The received value is the channel element type (`int64`); a `default`
    // case is accepted (non-blocking poll).
    assert_ok(&prog(
        "    select:\n        case v = <-ch:\n            auto y = v\n        default:\n            auto z = 0\n",
    ));
}

#[test]
fn comma_ok_receive_binds_bool() {
    // The comma-ok second target is a `bool` usable in a condition.
    assert_ok(&prog(
        "    select:\n        case v, ok = <-ch:\n            if ok:\n                auto y = v\n",
    ));
}

#[test]
fn send_case_typechecks() {
    assert_ok(&prog(
        "    select:\n        case ch <- 5:\n            auto n = 1\n",
    ));
}

#[test]
fn case_binding_is_scoped_to_its_case() {
    // `v` is visible only inside its own case body, not a sibling's.
    assert_code(
        &prog(
            "    select:\n        case v = <-ch:\n            auto y = v\n        default:\n            auto z = v\n",
        ),
        "ZAP3001",
    );
}

// ---------------------------------------------------------------------------
// Negative: operand must be a channel; send type must match; structural rules.
// ---------------------------------------------------------------------------

#[test]
fn receive_on_non_channel_is_rejected() {
    assert_code(
        "def f():\n    auto x = 1\n    select:\n        case v = <-x:\n            auto y = 0\n",
        "ZAP4030",
    );
}

#[test]
fn send_on_non_channel_is_rejected() {
    assert_code(
        "def f():\n    auto x = 1\n    select:\n        case x <- 1:\n            auto y = 0\n",
        "ZAP4031",
    );
}

#[test]
fn send_type_mismatch_is_rejected() {
    assert_code(
        &prog("    select:\n        case ch <- \"s\":\n            auto n = 1\n"),
        "ZAP4032",
    );
}

#[test]
fn two_default_cases_are_rejected() {
    assert_code(
        &prog(
            "    select:\n        case v = <-ch:\n            auto y = v\n        default:\n            auto a = 0\n        default:\n            auto b = 0\n",
        ),
        "ZAP4035",
    );
}

#[test]
fn underscore_receive_target_is_rejected() {
    assert_code(
        &prog("    select:\n        case _ = <-ch:\n            auto y = 0\n"),
        "ZAP4035",
    );
}
