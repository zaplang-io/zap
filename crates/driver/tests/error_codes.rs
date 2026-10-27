//! Pins representative error inputs to their stable `ZAPnnnn` codes (I#157/F1,
//! issue #239), exercised through the MCP structured-check surface
//! (`check_source`) that coding agents consume.
//!
//! These tests guard the *codes*, not the message text: the whole point of the
//! scheme is that a code is stable while prose may change. A renumbering or an
//! accidental code change at an emission site breaks exactly here, which is the
//! intended regression signal. (Message text is deliberately not asserted.)

use zapc::mcp::check::{check_source, result_to_json};

/// Assert that checking `src` surfaces a diagnostic with `code` among its
/// diagnostics. We assert *presence*, not exclusivity, because recovery often
/// emits more than one diagnostic for one mistake.
#[track_caller]
fn assert_code(src: &str, code: &str) {
    let r = check_source(src);
    let got: Vec<String> = r.diagnostics.iter().map(|d| d.code.to_string()).collect();
    assert!(
        got.iter().any(|c| c == code),
        "expected {code} for source {src:?}, got {got:?}"
    );
}

#[test]
fn lexical_codes() {
    // Unterminated string literal.
    assert_code("def main():\n    x = \"oops\n", "ZAP1001");
    // Tab in indentation.
    assert_code("def main():\n\ty = 1\n", "ZAP1008");
}

#[test]
fn syntax_codes() {
    // A dangling binary operator — an expression was required.
    assert_code("def main():\n    x = 1 +\n", "ZAP2014");
    // A malformed `when target.*` predicate (I#139, D-0079).
    assert_code(
        "when target.cpu == .x86_64:\n    def f():\n        pass\n",
        "ZAP2015",
    );
    // An unknown architecture in a `when target.arch` predicate.
    assert_code(
        "when target.arch == .sparc:\n    def f():\n        pass\n",
        "ZAP2016",
    );
}

#[test]
fn name_codes() {
    // Reference to an undefined name.
    assert_code("def main():\n    x = y\n", "ZAP3001");
    // Call to an undefined function.
    assert_code("def main():\n    nope()\n", "ZAP3003");
}

#[test]
fn type_codes() {
    // `int + str` — arithmetic on non-numbers.
    assert_code("def main():\n    x = 1 + \"a\"\n", "ZAP4012");
    // Division on non-numbers.
    assert_code("def main():\n    x = 1 / \"a\"\n", "ZAP4013");
    // Bitwise on non-integers.
    assert_code("def main():\n    x = 1 & \"a\"\n", "ZAP4014");
    // Returning the wrong type.
    assert_code(
        "def f() -> int32:\n    return \"x\"\n\ndef main():\n    pass\n",
        "ZAP4003",
    );
}

#[test]
fn semantic_codes() {
    // `@cache` takes no arguments.
    assert_code(
        "@cache(1)\ndef f():\n    pass\n\ndef main():\n    pass\n",
        "ZAP5041",
    );
    // An unknown decorator.
    assert_code("@notathing\ndef main():\n    pass\n", "ZAP5032");
}

#[test]
fn json_payload_carries_the_code() {
    // The structured surface agents consume must expose the code as a field.
    let r = check_source("def main():\n    x = 1 +\n");
    let json = result_to_json(&r).to_string();
    assert!(
        json.contains("\"code\":\"ZAP2014\""),
        "JSON payload is missing the code field: {json}"
    );
}

#[test]
fn clean_program_has_no_diagnostics() {
    let r = check_source("def main():\n    print(1)\n");
    assert!(r.ok, "unexpected diagnostics: {:?}", r.diagnostics);
}
