//! `int64` arithmetic, comparison, bitwise and shift ops (#1250), and the
//! **numeric-parity suite** — the issue's "easy to get wrong" requirement.
//!
//! Parity means a value folded at comptime equals the same computation run at
//! runtime, bit for bit. Two things pin that here: (1) explicit hand-computed
//! constants for the runtime edge cases (two's-complement wrap, floor
//! division/modulo sign rules, the `INT64_MIN` traps), taken straight from the
//! runtime C contract (`zap_ifloordiv`/`zap_imod`/`zap_ishl`/`zap_ishr`); and
//! (2) a matrix cross-checked against an *independent* i128 reference so a
//! regression in either the evaluator or the reference is caught.

mod common;

use common::{err, ok};
use zapc_comptime::{CtErrorKind, Value};

fn i(v: &Value) -> i64 {
    v.as_int().expect("int value")
}

// ---------------------------------------------------------------------------
// + - * : two's-complement wrap parity
// ---------------------------------------------------------------------------

#[test]
fn add_sub_mul_basic() {
    assert_eq!(ok("2 + 3"), Value::Int(5));
    assert_eq!(ok("10 - 4"), Value::Int(6));
    assert_eq!(ok("6 * 7"), Value::Int(42));
    assert_eq!(ok("2 * 3 + 1"), Value::Int(7));
    assert_eq!(ok("2 + 3 * 4"), Value::Int(14));
}

#[test]
fn add_wraps_like_runtime() {
    // INT64_MAX + 1 wraps to INT64_MIN (two's complement, D-0019).
    assert_eq!(ok("9223372036854775807 + 1"), Value::Int(i64::MIN));
    // INT64_MIN - 1 wraps to INT64_MAX.
    assert_eq!(ok("-9223372036854775808 - 1"), Value::Int(i64::MAX));
    // INT64_MIN * -1 wraps back to INT64_MIN.
    assert_eq!(ok("-9223372036854775808 * -1"), Value::Int(i64::MIN));
    assert_eq!(ok("9223372036854775807 * 2"), Value::Int(-2));
}

#[test]
fn unary_neg_pos_bitnot() {
    assert_eq!(ok("-(5)"), Value::Int(-5));
    assert_eq!(ok("+(5)"), Value::Int(5));
    assert_eq!(ok("-(-3)"), Value::Int(3));
    assert_eq!(ok("~0"), Value::Int(-1));
    assert_eq!(ok("~5"), Value::Int(-6));
    // Negating INT64_MIN wraps to itself.
    assert_eq!(ok("-(-9223372036854775808)"), Value::Int(i64::MIN));
}

// ---------------------------------------------------------------------------
// // and % : floor division / floor-sign modulo parity
// ---------------------------------------------------------------------------

#[test]
fn floordiv_sign_rules() {
    assert_eq!(ok("7 // 2"), Value::Int(3));
    assert_eq!(ok("-7 // 2"), Value::Int(-4));
    assert_eq!(ok("7 // -2"), Value::Int(-4));
    assert_eq!(ok("-7 // -2"), Value::Int(3));
    assert_eq!(ok("6 // 3"), Value::Int(2));
    assert_eq!(ok("-6 // 3"), Value::Int(-2));
}

#[test]
fn mod_sign_follows_divisor() {
    assert_eq!(ok("7 % 3"), Value::Int(1));
    assert_eq!(ok("-7 % 3"), Value::Int(2));
    assert_eq!(ok("7 % -3"), Value::Int(-2));
    assert_eq!(ok("-7 % -3"), Value::Int(-1));
    assert_eq!(ok("6 % 3"), Value::Int(0));
}

#[test]
fn floordiv_mod_intmin_edge_cases() {
    // INT64_MIN // -1 would trap the hardware divide; Zap wraps to INT64_MIN.
    assert_eq!(ok("-9223372036854775808 // -1"), Value::Int(i64::MIN));
    // INT64_MIN % -1 == 0 (special-cased so it does not trap).
    assert_eq!(ok("-9223372036854775808 % -1"), Value::Int(0));
}

#[test]
fn division_by_zero_is_a_clean_error() {
    for src in ["5 // 0", "5 % 0", "0 // 0"] {
        let e = err(src);
        assert_eq!(e.kind, CtErrorKind::Value, "{src}");
        assert!(e.message.contains("by zero"), "{src}: {}", e.message);
    }
}

// ---------------------------------------------------------------------------
// shifts and bitwise
// ---------------------------------------------------------------------------

#[test]
fn shifts_parity() {
    assert_eq!(ok("1 << 3"), Value::Int(8));
    assert_eq!(ok("255 >> 4"), Value::Int(15));
    // Arithmetic right shift preserves sign.
    assert_eq!(ok("-8 >> 1"), Value::Int(-4));
    // count >= 64 saturates: 0 for <<, sign-fill for >>.
    assert_eq!(ok("1 << 64"), Value::Int(0));
    assert_eq!(ok("-1 >> 64"), Value::Int(-1));
    assert_eq!(ok("5 >> 64"), Value::Int(0));
}

#[test]
fn negative_shift_count_is_an_error() {
    let e = err("1 << -1");
    assert_eq!(e.kind, CtErrorKind::Value);
    assert!(e.message.contains("negative shift count"), "{}", e.message);
}

#[test]
fn bitwise_on_ints_and_bools() {
    assert_eq!(ok("12 & 10"), Value::Int(8));
    assert_eq!(ok("12 | 10"), Value::Int(14));
    assert_eq!(ok("12 ^ 10"), Value::Int(6));
    assert_eq!(ok("True & False"), Value::Bool(false));
    assert_eq!(ok("True | False"), Value::Bool(true));
    assert_eq!(ok("True ^ True"), Value::Bool(false));
}

// ---------------------------------------------------------------------------
// comparisons (incl. chained)
// ---------------------------------------------------------------------------

#[test]
fn integer_comparisons() {
    assert_eq!(ok("1 == 1"), Value::Bool(true));
    assert_eq!(ok("1 != 2"), Value::Bool(true));
    assert_eq!(ok("1 < 2"), Value::Bool(true));
    assert_eq!(ok("2 <= 2"), Value::Bool(true));
    assert_eq!(ok("3 > 2"), Value::Bool(true));
    assert_eq!(ok("3 >= 4"), Value::Bool(false));
}

#[test]
fn chained_comparison_short_circuits() {
    assert_eq!(ok("1 < 2 < 3"), Value::Bool(true));
    assert_eq!(ok("1 < 2 < 1"), Value::Bool(false));
    assert_eq!(ok("3 > 2 > 1"), Value::Bool(true));
    assert_eq!(ok("1 <= 1 < 5"), Value::Bool(true));
}

// ---------------------------------------------------------------------------
// out-of-fragment arithmetic
// ---------------------------------------------------------------------------

#[test]
fn true_division_is_out_of_fragment() {
    let e = err("5 / 2");
    assert!(e.is_not_available(), "{}", e.message);
    assert!(e.message.contains("not available at comptime"));
}

#[test]
fn pow_and_matmul_are_out_of_fragment() {
    assert!(err("2 ** 8").is_not_available());
    assert!(err("2 @ 3").is_not_available());
}

// ---------------------------------------------------------------------------
// parity matrix vs an independent i128 reference
// ---------------------------------------------------------------------------

/// Floor division, implemented independently (operand-sign test, i128 width) so
/// it is a genuine second opinion on the evaluator.
fn floordiv_ref(a: i64, b: i64) -> i64 {
    let (ai, bi) = (a as i128, b as i128);
    let mut q = ai / bi;
    if ai % bi != 0 && ((ai < 0) != (bi < 0)) {
        q -= 1;
    }
    q as i64
}

/// Floor-sign modulo, independent reference.
fn mod_ref(a: i64, b: i64) -> i64 {
    let (ai, bi) = (a as i128, b as i128);
    let mut r = ai % bi;
    if r != 0 && ((r < 0) != (bi < 0)) {
        r += bi;
    }
    r as i64
}

#[test]
fn parity_matrix_add_sub_mul_div_mod() {
    let operands = [
        -9223372036854775808i64,
        -9223372036854775807,
        -1000,
        -7,
        -1,
        0,
        1,
        7,
        1000,
        9223372036854775806,
        9223372036854775807,
    ];
    for &a in &operands {
        for &b in &operands {
            assert_eq!(
                i(&ok(&format!("({a}) + ({b})"))),
                a.wrapping_add(b),
                "{a} + {b}"
            );
            assert_eq!(
                i(&ok(&format!("({a}) - ({b})"))),
                a.wrapping_sub(b),
                "{a} - {b}"
            );
            assert_eq!(
                i(&ok(&format!("({a}) * ({b})"))),
                a.wrapping_mul(b),
                "{a} * {b}"
            );
            if b != 0 {
                assert_eq!(
                    i(&ok(&format!("({a}) // ({b})"))),
                    floordiv_ref(a, b),
                    "{a} // {b}"
                );
                assert_eq!(
                    i(&ok(&format!("({a}) % ({b})"))),
                    mod_ref(a, b),
                    "{a} % {b}"
                );
            }
        }
    }
}

#[test]
fn parity_matrix_comparisons() {
    let operands = [-5i64, -1, 0, 1, 5, i64::MIN, i64::MAX];
    for &a in &operands {
        for &b in &operands {
            assert_eq!(ok(&format!("({a}) == ({b})")), Value::Bool(a == b));
            assert_eq!(ok(&format!("({a}) != ({b})")), Value::Bool(a != b));
            assert_eq!(ok(&format!("({a}) < ({b})")), Value::Bool(a < b));
            assert_eq!(ok(&format!("({a}) <= ({b})")), Value::Bool(a <= b));
            assert_eq!(ok(&format!("({a}) > ({b})")), Value::Bool(a > b));
            assert_eq!(ok(&format!("({a}) >= ({b})")), Value::Bool(a >= b));
        }
    }
}
