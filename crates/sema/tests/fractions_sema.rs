//! `fractions` module — sema (type-check) behaviour (#982, epic #966).
//!
//! Host-only (parse + sema, no LLVM). Locks the type surface of the exact
//! rational `Fraction`: construction from `(int, int)` / `int` / `str`, the
//! rejection of a `float` argument (the correctness guarantee — a silent float
//! hop would reinstate the binary-rounding hazard the exact type removes), and
//! the prelude-struct method/operator surface (`+ - * /`, comparisons,
//! `numerator`/`denominator`, `limit_denominator`, `to_float`, keyword/default
//! arguments, `list[Fraction]`).

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

const IMP: &str = "import fractions\n";

// --- construction from (int,int) / int / str type-checks to a Fraction -----

#[test]
fn construct_from_pair_int_and_str() {
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(3, 4)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(-5, 10)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(42)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(\"3/4\")\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(\"0.25\")\n"
    ));
    // the result is a `Fraction` usable where a `Fraction` is expected
    ok(&format!(
        "{IMP}def g(Fraction x) -> Fraction:\n    return x\n\ndef f() -> Fraction:\n    return g(fractions.Fraction(1, 2))\n"
    ));
}

// --- the headline guarantee: a float argument is a compile error -----------

#[test]
fn float_argument_is_rejected() {
    code(
        &format!("{IMP}def f():\n    auto a = fractions.Fraction(1.5)\n"),
        "ZAP4024",
    );
    // a typed float64 variable is rejected too, not just a literal
    code(
        &format!("{IMP}def f():\n    float64 x = 1.5\n    auto a = fractions.Fraction(x)\n"),
        "ZAP4024",
    );
    // a float in either slot of the two-argument form is rejected
    code(
        &format!("{IMP}def f():\n    auto a = fractions.Fraction(1.5, 2)\n"),
        "ZAP4024",
    );
    code(
        &format!("{IMP}def f():\n    auto a = fractions.Fraction(1, 2.0)\n"),
        "ZAP4024",
    );
}

#[test]
fn float_rejection_message_points_to_str_form() {
    let e = errors(&format!(
        "{IMP}def f():\n    auto a = fractions.Fraction(1.5)\n"
    ));
    assert!(
        e.iter()
            .any(|(_, m)| m.contains("str") && m.contains("float")),
        "message should steer to the str form, got: {e:?}",
    );
}

// --- a str in the two-argument form is rejected ----------------------------

#[test]
fn str_in_pair_form_is_rejected() {
    code(
        &format!("{IMP}def f():\n    auto a = fractions.Fraction(\"3\", 4)\n"),
        "ZAP4024",
    );
}

// --- arity -----------------------------------------------------------------

#[test]
fn arity_is_enforced() {
    code(
        &format!("{IMP}def f():\n    auto a = fractions.Fraction()\n"),
        "ZAP4025",
    );
    code(
        &format!("{IMP}def f():\n    auto a = fractions.Fraction(1, 2, 3)\n"),
        "ZAP4025",
    );
}

// --- prelude-struct operator surface type-checks ---------------------------

#[test]
fn arithmetic_and_comparison_operators() {
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    auto a = fractions.Fraction(1, 3)\n    auto b = fractions.Fraction(1, 6)\n    return a + b - a * b\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(1, 2) / fractions.Fraction(3, 4)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> bool:\n    auto a = fractions.Fraction(1, 3)\n    auto b = fractions.Fraction(1, 2)\n    return a < b and a <= b and b > a and b >= a and a == a and a != b\n"
    ));
    // unary negation
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return -fractions.Fraction(1, 2)\n"
    ));
}

// --- methods with keyword/default arguments (I#481) ------------------------

#[test]
fn limit_denominator_default_and_keyword() {
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(31415926, 10000000).limit_denominator()\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(31415926, 10000000).limit_denominator(100)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(31415926, 10000000).limit_denominator(max_denominator=10)\n"
    ));
}

// --- accessors + conversions -----------------------------------------------

#[test]
fn accessors_and_conversions() {
    ok(&format!(
        "{IMP}def f() -> bigint:\n    return fractions.Fraction(3, 4).numerator()\n"
    ));
    ok(&format!(
        "{IMP}def f() -> bigint:\n    return fractions.Fraction(3, 4).denominator()\n"
    ));
    ok(&format!(
        "{IMP}def f() -> float64:\n    return fractions.Fraction(1, 2).to_float()\n"
    ));
    ok(&format!(
        "{IMP}def f() -> str:\n    return str(fractions.Fraction(3, 4))\n"
    ));
}

// --- the exact-sum use case: a list of rationals totalled exactly ----------

#[test]
fn list_of_fractions_totals() {
    ok(&format!(
        "{IMP}def total(list[Fraction] xs) -> Fraction:\n    Fraction acc = fractions.Fraction(0)\n    for x in xs:\n        acc = acc + x\n    return acc\n"
    ));
}

// --- keyword binding of the two seam constructor parameters (D-0213) -------

#[test]
fn constructor_keyword_binding() {
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(numerator=3, denominator=4)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(numerator=5)\n"
    ));
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(numerator=\"3/4\")\n"
    ));
    // reordered keywords still bind by name
    ok(&format!(
        "{IMP}def f() -> Fraction:\n    return fractions.Fraction(denominator=4, numerator=3)\n"
    ));
}
