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
    // A malformed `as` pattern capture (#603, D11): `as` must be followed by a
    // single plain binding name, never `_`.
    assert_code(
        "def main():\n    match 1:\n        case 1 as _:\n            pass\n",
        "ZAP2021",
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
    // `@inject` stays reclassified/declined for core — dependency injection is
    // the `Depends(...)` adapter, not a core decorator — rejected with ZAP5055
    // (I#472, D-0109). (`@cache` is no longer here: it ships in I#666/D-0115,
    // expanded before sema; its ZAP5056 signature checks run in the driver pass,
    // which `check_source` does not, so they are covered by the expansion pass's
    // own unit tests instead.)
    assert_code(
        "struct S:\n    @inject\n    int64 x\n\ndef main():\n    pass\n",
        "ZAP5055",
    );
    // `@cache` is a free-function decorator: misapplied to a struct it is a clean
    // require-target error (ZAP5047), which sema — and so `check_source` — sees.
    assert_code(
        "@cache\nstruct S:\n    int64 x\n\ndef main():\n    pass\n",
        "ZAP5047",
    );
    // `@deprecated` on the wrong target (a struct) — it is a function/method
    // decorator only (I#472, D-0109).
    assert_code(
        "@deprecated\nstruct S:\n    int64 x\n\ndef main():\n    pass\n",
        "ZAP5047",
    );
    // An unknown decorator.
    assert_code("@notathing\ndef main():\n    pass\n", "ZAP5032");
    // A `**` keyword-collector that is not the last parameter (#601).
    assert_code(
        "def f(int64 **kw, int64 a) -> int64:\n    return a\n\ndef main():\n    pass\n",
        "ZAP5121",
    );
    // A `*`-spread of an unknown-length value into a non-variadic callee (#597).
    assert_code(
        "def f(int64 a, int64 b) -> int64:\n    return a + b\n\
         def main():\n    auto xs = [1, 2]\n    auto y = f(*xs)\n",
        "ZAP5122",
    );
    // A `**`-merge of a non-dict operand in a dict display (#598).
    assert_code(
        "def main():\n    list[int64] xs = [1]\n    auto d = {\"a\": 1, **xs}\n",
        "ZAP5123",
    );
    // A `@scoped` struct missing its context-manager methods (#876, D-0159).
    assert_code(
        "@scoped\nstruct Arena:\n    int64 id = 0\n\ndef main():\n    pass\n",
        "ZAP5127",
    );
    // A detached construction of a `@scoped` value — not bound by `with` (#876).
    assert_code(
        "@scoped\nstruct Arena:\n    int64 id = 0\n    \
         def __enter__(self) -> Arena:\n        return self\n    \
         def __exit__(self):\n        pass\n\
         def main():\n    auto a = Arena()\n",
        "ZAP5128",
    );
    // A `with`-bound `@scoped` value escaping its scope (returned) (#876).
    assert_code(
        "@scoped\nstruct Arena:\n    int64 id = 0\n    \
         def __enter__(self) -> Arena:\n        return self\n    \
         def __exit__(self):\n        pass\n\
         def make() -> Arena:\n    with Arena() as a:\n        return a\n\
         def main():\n    pass\n",
        "ZAP5129",
    );
}

#[test]
fn eq_none_on_value_type_is_rejected() {
    // `== None` / `!= None` is consistent with `is None` (SPEC §5, #374/B3): a
    // never-None value type can never be None, so this is a ZAP4074 error rather
    // than a silent sema pass that then fails in lowering.
    assert_code(
        "def main():\n    int64 n = 5\n    print(n == None)\n",
        "ZAP4074",
    );
    assert_code(
        "def main():\n    int64 n = 5\n    print(n != None)\n",
        "ZAP4074",
    );
}

#[test]
fn augmented_assign_to_const_is_rejected() {
    // A `const` binding cannot be reassigned, including via an augmented
    // assignment (D-0003; #374 walled garden, operators).
    assert_code(
        "def main():\n    const int64 c = 5\n    c += 1\n",
        "ZAP3017",
    );
}

#[test]
fn call_argument_matrix_is_rejected() {
    // The compile-time argument-checking matrix (#374 walled garden, §B-6).
    // Each mistake is caught at compile time with a stable code, so a call that
    // does not match its callee's signature never reaches codegen.
    //
    // Too many / too few positional arguments (arity) -> ZAP4020.
    assert_code(
        "def f(int64 a) -> int64:\n    return a\n\ndef main():\n    print(f(1, 2))\n",
        "ZAP4020",
    );
    assert_code(
        "def f(int64 a, int64 b) -> int64:\n    return a\n\ndef main():\n    print(f(1))\n",
        "ZAP4020",
    );
    // An unexpected keyword argument -> ZAP4023.
    assert_code(
        "def f(int64 a) -> int64:\n    return a\n\ndef main():\n    print(f(z=1))\n",
        "ZAP4023",
    );
    // A keyword-only parameter (after a bare `*`) supplied positionally ->
    // ZAP4029; omitting the required keyword-only param -> ZAP4020 (I#496, D-0095).
    assert_code(
        "def f(str h, *, int64 m) -> int64:\n    return m\n\ndef main():\n    print(f(\"a\", 80))\n",
        "ZAP4029",
    );
    assert_code(
        "def f(str h, *, int64 m) -> int64:\n    return m\n\ndef main():\n    print(f(\"a\"))\n",
        "ZAP4020",
    );
    // A positional-only parameter (before a `/`) supplied by name -> ZAP4034
    // (#602, D10).
    assert_code(
        "def f(int64 a, /) -> int64:\n    return a\n\ndef main():\n    print(f(a=1))\n",
        "ZAP4034",
    );
}

#[test]
fn str_method_limits_are_rejected() {
    // The str method surface is deliberately a strict subset of CPython's
    // (#284, #374 walled garden). These cases pin the limitations as intent:
    // a program that relies on an unsupported form fails at compile time with a
    // stable code, rather than silently mis-behaving.
    //
    // strip()/lstrip()/rstrip() take NO chars argument.
    assert_code("def main():\n    print(\"xxhi\".strip(\"x\"))\n", "ZAP4025");
    // split() takes exactly one separator — no maxsplit, no no-arg whitespace
    // split.
    assert_code(
        "def main():\n    auto p = \"a b c\".split(\" \", 1)\n",
        "ZAP4025",
    );
    assert_code("def main():\n    auto p = \"a b\".split()\n", "ZAP4025");
    // replace() takes exactly (old, new) — no count.
    assert_code(
        "def main():\n    print(\"aaa\".replace(\"a\", \"b\", 1))\n",
        "ZAP4025",
    );
    // Absent methods are a clean ZAP4080 "str has no method", not a crash.
    assert_code("def main():\n    print(\"aaa\".count(\"a\"))\n", "ZAP4080");
    assert_code("def main():\n    print(\"ab cd\".title())\n", "ZAP4080");
    assert_code("def main():\n    print(\"abc\".rfind(\"b\"))\n", "ZAP4080");
}

#[test]
fn str_times_int_is_rejected() {
    // `str * int` repetition is a FEATURE-GAP (not in SPEC §8 yet, #374): it is
    // rejected at compile time rather than silently supported.
    assert_code("def main():\n    print(\"ab\" * 3)\n", "ZAP4012");
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
