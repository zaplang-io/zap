//! First-class type values `Type[T]` (#1527) — sema layer.
//!
//! A bare primitive type name in value position denotes a first-class type value
//! so a concrete type can be passed as a library-decorator argument
//! (`@cli.option("--count", int64, default=1)`). These are the host-only sema
//! checks: the type value is statically distinguishable (`Type[int64]` is not
//! `Type[str]`), a non-type argument in a `Type[T]` position is a precise error,
//! and a `Type[T]` parameter admits any concrete type value. The imported-
//! decorator flow and native codegen are covered by the driver integration tests
//! and the `type_arg_decorator` golden project.

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

// ---- a primitive type name is a value ------------------------------------

#[test]
fn concrete_type_parameter_accepts_matching_type_value() {
    // A `Type[int64]` parameter accepts the `int64` type value.
    assert_ok(
        "def opt(str name, Type[int64] kind) -> int64:\n    return 1\n\
         def main():\n    print(opt(\"--count\", int64))\n",
    );
}

#[test]
fn type_value_binds_a_bare_generic_parameter() {
    // The issue's reproduction shape: a bare generic `T` parameter receives the
    // type value (binding `T = Type[int64]`), with no concrete `Type[..]` written.
    assert_ok(
        "def opt[T](str name, T kind) -> int64:\n    return 1\n\
         def main():\n    print(opt(\"--count\", int64))\n",
    );
}

#[test]
fn every_primitive_spelling_is_a_type_value() {
    // The whole v1 primitive set — including the Python-facing `int`/`float` and
    // the short `i64`/`u8`/`f32` aliases — is usable in value position.
    for ty in [
        "int64", "int", "i64", "int32", "i32", "uint8", "u8", "uint16", "u16", "uint32", "u32",
        "uint64", "u64", "usize", "float64", "float", "f64", "float32", "f32", "bool", "str",
        "bytes", "char", "bigint", "complex",
    ] {
        assert_ok(&format!(
            "def opt[T](T kind) -> int64:\n    return 1\n\
             def main():\n    print(opt({ty}))\n"
        ));
    }
}

// ---- static distinguishability -------------------------------------------

#[test]
fn distinct_primitive_type_values_are_not_interchangeable() {
    // `Type[str]` is not assignable to `Type[int64]` — the distinguishability the
    // issue requires (reported against the exact declared type).
    assert_code(
        "def opt(Type[int64] kind) -> int64:\n    return 1\n\
         def main():\n    print(opt(str))\n",
        "ZAP4021",
    );
}

#[test]
fn type_values_are_invariant_no_numeric_widening() {
    // Type values do NOT inherit numeric widening: `Type[int32]` is rejected where
    // `Type[int64]` is expected, even though `int32` widens to `int64` as a value.
    assert_code(
        "def opt(Type[int64] kind) -> int64:\n    return 1\n\
         def main():\n    print(opt(int32))\n",
        "ZAP4021",
    );
}

// ---- a non-type value in type position is a precise error ----------------

#[test]
fn non_type_value_in_type_position_is_rejected() {
    // An ordinary value where a `Type[T]` is expected is a precise type error, not
    // a silent accept.
    assert_code(
        "def opt(Type[int64] kind) -> int64:\n    return 1\n\
         def main():\n    print(opt(5))\n",
        "ZAP4021",
    );
}

#[test]
fn string_value_in_type_position_is_rejected() {
    assert_code(
        "def opt(Type[str] kind) -> int64:\n    return 1\n\
         def main():\n    print(opt(\"nope\"))\n",
        "ZAP4021",
    );
}

// ---- concrete default checking against the type ---------------------------

#[test]
fn incompatible_default_against_a_concrete_type_is_rejected() {
    // With a concrete `Type[int64] kind` paired with an `int64 default`, a
    // mismatched default is rejected by ordinary type checking.
    assert_code(
        "def opt(Type[int64] kind, int64 default) -> int64:\n    return default\n\
         def main():\n    print(opt(int64, \"not an int\"))\n",
        "ZAP4021",
    );
}

#[test]
fn compatible_default_against_a_concrete_type_is_accepted() {
    assert_ok(
        "def opt(Type[int64] kind, int64 default) -> int64:\n    return default\n\
         def main():\n    print(opt(int64, 1))\n",
    );
}

// ---- conversions are unaffected ------------------------------------------

#[test]
fn conversion_call_forms_still_work() {
    // Making a bare `str`/`int64`/`bool` a type value must not disturb the
    // conversion *call* forms, which are matched structurally before inference.
    assert_ok(
        "def main():\n    \
         auto a = int64(3)\n    \
         auto b = str(7)\n    \
         auto c = bool(1)\n    \
         auto d = float64(2)\n    \
         print(a)\n",
    );
}
