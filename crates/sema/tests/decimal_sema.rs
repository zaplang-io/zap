//! `decimal` module — sema (type-check) behaviour (#981, #981).
//!
//! Host-only (parse + sema, no LLVM). Locks the type surface of the exact
//! base-10 `Decimal`: construction from `str`/`int`, the rejection of a `float`
//! argument (the core correctness guarantee — a silent float hop would reinstate
//! the binary-rounding hazard the type removes), the explicit `from_float`
//! opt-in, and the prelude-struct method surface (`+ - * /`, `quantize`, `div`,
//! comparisons, `to_int`, keyword/default arguments, `list[Decimal]`).

use zapc_sema::check_str;

#[track_caller]
fn errors(src: &str) -> Vec<(String, String)> {
    let (terrs, perrs, lerrs) = check_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    terrs
        .into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect()
}

#[track_caller]
fn ok(src: &str) {
    let e = errors(src);
    assert!(e.is_empty(), "expected no sema errors, got: {e:?}");
}

#[track_caller]
fn code(src: &str, want: &str) {
    let e = errors(src);
    assert!(
        e.iter().any(|(c, _)| c == want),
        "expected a {want} error, got: {e:?}",
    );
}

const IMP: &str = "import decimal\n";

// --- construction from str / int type-checks to a Decimal value ----------

#[test]
fn construct_from_str_and_int() {
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    return decimal.Decimal(\"1.50\")\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    return decimal.Decimal(42)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    return decimal.Decimal(-7)\n"
    ));
    // the result is a `Decimal` usable where a `Decimal` is expected
    ok(&format!(
        "{IMP}def g(Decimal d) -> Decimal:\n    return d\n\ndef f() -> Decimal:\n    return g(decimal.Decimal(\"1\"))\n"
    ));
}

// --- the headline guarantee: a float argument is a compile error ----------

#[test]
fn float_argument_is_rejected() {
    code(
        &format!("{IMP}def f():\n    auto a = decimal.Decimal(1.5)\n"),
        "ZAP4024",
    );
    // a typed float64 variable is rejected too, not just a literal
    code(
        &format!("{IMP}def f():\n    float64 x = 1.5\n    auto a = decimal.Decimal(x)\n"),
        "ZAP4024",
    );
}

#[test]
fn float_rejection_message_points_to_from_float() {
    let e = errors(&format!(
        "{IMP}def f():\n    auto a = decimal.Decimal(1.5)\n"
    ));
    assert!(
        e.iter()
            .any(|(_, m)| m.contains("from_float") && m.contains("float")),
        "message should steer to decimal.from_float, got: {e:?}",
    );
}

// --- from_float is the explicit opt-in; it wants a float -------------------

#[test]
fn from_float_accepts_float_rejects_others() {
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    return decimal.from_float(1.25)\n"
    ));
    code(
        &format!("{IMP}def f():\n    auto a = decimal.from_float(\"1.25\")\n"),
        "ZAP4024",
    );
    code(
        &format!("{IMP}def f():\n    auto a = decimal.from_float(1)\n"),
        "ZAP4024",
    );
}

// --- arity ----------------------------------------------------------------

#[test]
fn arity_is_enforced() {
    code(
        &format!("{IMP}def f():\n    auto a = decimal.Decimal()\n"),
        "ZAP4025",
    );
    code(
        &format!("{IMP}def f():\n    auto a = decimal.Decimal(\"1\", \"2\")\n"),
        "ZAP4025",
    );
    code(
        &format!("{IMP}def f():\n    auto a = decimal.from_float()\n"),
        "ZAP4025",
    );
}

// --- prelude-struct method + operator surface type-checks ------------------

#[test]
fn arithmetic_and_comparison_operators() {
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    auto a = decimal.Decimal(\"0.1\")\n    auto b = decimal.Decimal(\"0.2\")\n    return a + b - a * b\n"
    ));
    ok(&format!(
        "{IMP}def f() -> bool:\n    auto a = decimal.Decimal(\"0.1\")\n    auto b = decimal.Decimal(\"0.3\")\n    return a < b and a <= b and b > a and b >= a and a == a and a != b\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    return decimal.Decimal(\"1\") / decimal.Decimal(\"3\")\n"
    ));
}

#[test]
fn methods_with_keyword_and_default_arguments() {
    // div: prec + rounding both defaulted, partially given, fully given + reordered
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    auto a = decimal.Decimal(\"1\")\n    auto b = decimal.Decimal(\"3\")\n    return a.div(b)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    auto a = decimal.Decimal(\"1\")\n    auto b = decimal.Decimal(\"3\")\n    return a.div(b, prec=10)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    auto a = decimal.Decimal(\"1\")\n    auto b = decimal.Decimal(\"3\")\n    return a.div(b, rounding=\"ROUND_DOWN\", prec=5)\n"
    ));
    // quantize: exemplar + optional rounding
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    auto a = decimal.Decimal(\"1.005\")\n    return a.quantize(decimal.Decimal(\"0.01\"))\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Decimal:\n    auto a = decimal.Decimal(\"1.005\")\n    return a.quantize(decimal.Decimal(\"0.01\"), rounding=\"ROUND_HALF_UP\")\n"
    ));
}

#[test]
fn conversions_and_introspection() {
    ok(&format!(
        "{IMP}def f() -> int64:\n    return decimal.Decimal(\"1.9\").to_int()\n"
    ));
    ok(&format!(
        "{IMP}def f() -> int64:\n    return decimal.Decimal(\"1.50\").exponent()\n"
    ));
    ok(&format!(
        "{IMP}def f() -> bool:\n    return decimal.Decimal(\"0\").is_zero()\n"
    ));
    ok(&format!(
        "{IMP}def f() -> str:\n    return str(decimal.Decimal(\"1.50\"))\n"
    ));
}

// --- the money use case: a list of line items totalled exactly -------------

#[test]
fn list_of_decimals_totals() {
    ok(&format!(
        "{IMP}def total(list[Decimal] xs) -> Decimal:\n    Decimal acc = decimal.Decimal(0)\n    for x in xs:\n        acc = acc + x\n    return acc\n"
    ));
}
