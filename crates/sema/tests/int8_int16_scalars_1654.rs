//! `int8`/`int16` as first-class signed narrow-integer scalar value types (#1654).
//!
//! These scalars are the signed mirrors of the shipped `uint8`/`uint16`. Before
//! #1654 a scalar annotation `int8 x` resolved to `Ty::Unknown` (they existed
//! only as array dtypes / ZIR `I8`/`I16`), so sema neither type-checked them nor
//! range-checked their literals. These host-only checks pin the front-end type
//! system: resolution of the annotation in every position, the `int8(...)` /
//! `int16(...)` value conversions, the width-stable literal range-check
//! (`ZAP4016`), the implicit signed-tower widening, the explicit-narrow rule, no
//! signed<->unsigned crossing (`ZAP4015`), and use as container keys/values,
//! struct fields and generic bindings. The runtime *values* (the `int8(300) ==
//! 44` wrap, arithmetic that wraps at the narrow width per #1677) are pinned by
//! the `int8_int16_scalars` golden on CI (the arithmetic wrap itself is #1677).

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
        "expected a {code} error, got: {errs:?}"
    );
}

// ---- annotations in every position -----------------------------------------

#[test]
fn int8_int16_local_bindings_resolve() {
    assert_ok("def main():\n    int8 a = 100\n    int16 b = 1000\n    print(a)\n    print(b)\n");
    // Short `iN` spellings resolve to the same types.
    assert_ok("def main():\n    i8 a = 1\n    i16 b = 2\n    print(a)\n    print(b)\n");
    // Full negative range accepted.
    assert_ok("def main():\n    int8 lo = -128\n    int8 hi = 127\n    int16 m = -32768\n    print(lo)\n    print(hi)\n    print(m)\n");
}

#[test]
fn int8_int16_params_and_return_resolve() {
    assert_ok(
        "def clamp(int8 lo, int8 hi, int8 x) -> int8:\n    return x\n\
         def widen(int16 n) -> int16:\n    return n\n\
         def main():\n    int8 r = clamp(int8(0), int8(10), int8(5))\n    print(r)\n",
    );
}

#[test]
fn int8_int16_struct_fields_resolve() {
    assert_ok(
        "struct Pixel:\n    int8 dx\n    int16 dy\n\
         def main():\n    auto p = Pixel(int8(1), int16(2))\n    print(p.dx)\n    print(p.dy)\n",
    );
}

// ---- conversions ------------------------------------------------------------

#[test]
fn int8_int16_conversions_accept_any_number() {
    // The explicit cast is the wrap escape hatch: it accepts an out-of-range
    // argument (which wraps at runtime) — unlike a bare literal binding.
    assert_ok("def main():\n    int8 a = int8(300)\n    int16 b = int16(70000)\n    print(a)\n    print(b)\n");
    assert_ok(
        "def main():\n    auto a = int8(-5)\n    auto b = int16(5)\n    print(a)\n    print(b)\n",
    );
    // From a wider int, a float, and a bigint.
    assert_ok("def main():\n    int64 n = 5\n    int8 a = int8(n)\n    int8 c = int8(3.9)\n    print(a)\n    print(c)\n");
}

// ---- the width-stable literal range-check (ZAP4016) ------------------------

#[test]
fn out_of_range_literal_into_int8_is_zap4016() {
    assert_code("def main():\n    int8 x = 300\n    print(x)\n", "ZAP4016");
    assert_code("def main():\n    int8 x = -129\n    print(x)\n", "ZAP4016");
}

#[test]
fn out_of_range_literal_into_int16_is_zap4016() {
    assert_code(
        "def main():\n    int16 x = 70000\n    print(x)\n",
        "ZAP4016",
    );
    assert_code(
        "def main():\n    int16 x = -40000\n    print(x)\n",
        "ZAP4016",
    );
}

#[test]
fn in_range_literals_are_accepted() {
    // The exact boundaries are representable (so NOT ZAP4016).
    assert_ok("def main():\n    int8 lo = -128\n    int8 hi = 127\n    int16 wl = -32768\n    int16 wh = 32767\n    print(lo + hi)\n    print(wl + wh)\n");
}

// ---- width-stable arithmetic: a literal adopts the narrow operand width -----

#[test]
fn literal_in_narrow_arithmetic_must_fit() {
    // `int8_v + 300` — the literal infers to the int8 context and is out of
    // range, so ZAP4016 (width-stable: the result is int8, not int64).
    assert_code(
        "def main():\n    int8 x = 10\n    auto y = x + 300\n    print(y)\n",
        "ZAP4016",
    );
    // A fitting literal is fine.
    assert_ok("def main():\n    int8 x = 10\n    auto y = x + 5\n    print(y)\n");
    // The counter idiom keeps the narrow width (the += literal fits).
    assert_ok("def main():\n    int8 c = 0\n    c += 1\n    print(c)\n");
}

// ---- implicit widening up the signed tower; narrowing stays explicit --------

#[test]
fn signed_tower_widening_is_implicit() {
    assert_ok("def main():\n    int8 a = 1\n    int16 b = a\n    int32 c = a\n    int64 d = b\n    print(c)\n    print(d)\n");
}

#[test]
fn signed_narrowing_requires_an_explicit_cast() {
    // int64 -> int8 is not implicit; assigning a wider typed value into a
    // narrower binding is rejected (ZAP4001), the explicit `int8(...)` is the fix.
    assert_code(
        "def main():\n    int64 n = 5\n    int8 a = n\n    print(a)\n",
        "ZAP4001",
    );
    assert_code(
        "def main():\n    int16 m = 5\n    int8 a = m\n    print(a)\n",
        "ZAP4001",
    );
}

// ---- no implicit signed<->unsigned crossing (ZAP4015), unchanged by #1654 ---

#[test]
fn int8_uint8_mixing_is_rejected() {
    assert_code(
        "def main():\n    int8 a = 1\n    uint8 b = 2\n    auto c = a + b\n    print(c)\n",
        "ZAP4015",
    );
}

// ---- containers: keys, values, elements ------------------------------------

#[test]
fn int8_int16_as_container_key_value_element() {
    assert_ok(
        "def main():\n    dict[int8, str] table = {}\n    table[int8(1)] = \"a\"\n    print(table[int8(1)])\n",
    );
    assert_ok("def main():\n    set[int16] seen = set[int16]()\n    seen.add(int16(3))\n    print(int16(3) in seen)\n");
    assert_ok("def main():\n    list[int8] xs = [int8(1), int8(2)]\n    print(xs[0])\n");
    // As a dict value too.
    assert_ok(
        "def main():\n    dict[str, int16] m = {}\n    m[\"k\"] = int16(5)\n    print(m[\"k\"])\n",
    );
}

// ---- generics: a type parameter bound to the narrow scalar ------------------

#[test]
fn int8_int16_bind_a_generic_parameter() {
    assert_ok(
        "def id[T](T x) -> T:\n    return x\n\
         def main():\n    int8 a = id(int8(7))\n    int16 b = id(int16(9))\n    print(a)\n    print(b)\n",
    );
}

// ---- Display / f-strings ----------------------------------------------------

#[test]
fn int8_int16_render_in_fstring() {
    assert_ok(
        "def main():\n    int8 a = int8(5)\n    int16 b = int16(6)\n    print(f\"{a} {b}\")\n",
    );
}

// ---- they are reserved type keywords (not usable as identifiers) ------------

#[test]
fn int8_is_a_reserved_type_keyword() {
    // Using `int8`/`int16` as an identifier (here a function name) is a parse
    // error — they are hard type keywords, exactly like `uint8`/`int32`.
    let (_t, perrs, _l) = check_str("def int8():\n    return 0\n");
    assert!(
        !perrs.is_empty(),
        "expected a parse error using `int8` as an identifier"
    );
    let (_t2, perrs2, _l2) = check_str("def int16():\n    return 0\n");
    assert!(
        !perrs2.is_empty(),
        "expected a parse error using `int16` as an identifier"
    );
}
