//! A `bigint` value must not be silently accepted into a fixed-width numeric
//! `Optional[T]` payload (or struct field) and mis-compiled (#1379).
//!
//! A `bigint` is never implicitly assignable to an `int*`/`float*`: sema rejects
//! `int64 x = <bigint>` / `float64 f = <bigint>` with `ZAP4001`. But sema erases
//! the `Optional` payload (tracks `Optional[T]` as opaque `Named("Optional")`),
//! so the #1265 bare-payload -> `Optional[T]` widening is payload-agnostic, and
//! struct-constructor fields are not sema-type-checked at all. So a `bigint`
//! reached IR lowering, where `coerce` has no `bigint -> int*` arm and silently
//! stored the `bigint` heap pointer into the scalar slot — a memory-unsafe
//! miscompile: a later `.unwrap()` / field read reinterprets the pointer bits as
//! the value (or, for a tagged `Optional`, reads it as `None` and raises). The
//! literal `9223372036854775808` (= 2^63) overflows `int64` and so is a
//! `bigint`, which is how the issue's `-9223372036854775808` (i64::MIN magnitude)
//! triggers it.
//!
//! Lowering now rejects a `bigint` flowing into a fixed-width numeric payload
//! with the same `ZAP4001` assignability error, at the shared `expr_coerced`
//! chokepoint (constructor args/fields, call args, returns, typed bindings) and
//! the bare-payload `Optional`-wrap arm (plain reassignment). A genuine
//! `Optional[bigint]`, and any in-range integer payload, are unaffected.
//!
//! Host-only (`lower_module`): the rejection is in lowering, so it surfaces here.

use zapc_ir::lower_module;
use zapc_syntax::parser::parse_str;

fn lower_ok(src: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(errs.is_empty(), "lower errors: {errs:?}");
}

fn lower_err_contains(src: &str, needle: &str) {
    let (ast, perrs, lerrs) = parse_str(src);
    assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "parse errors: {perrs:?}");
    let (_module, errs) = lower_module(&ast);
    assert!(!errs.is_empty(), "expected a lowering error, got none");
    let joined = errs
        .iter()
        .map(|e| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains(needle),
        "expected a lowering error containing {needle:?}, got:\n{joined}"
    );
}

// ----- Rejected: a bigint literal in a fixed-width-numeric Optional payload -----

#[test]
fn explicit_some_bigint_into_optional_int64_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   Optional[int64] x = Optional.Some(9223372036854775808)\n\
        \x20   print(x.is_some())\n",
        "bigint",
    );
}

#[test]
fn issue_min_magnitude_literal_rejected() {
    // The issue's exact value: i64::MIN magnitude, a bigint.
    lower_err_contains(
        "def main():\n\
        \x20   Optional[int64] x = Optional.Some(-9223372036854775808)\n\
        \x20   print(x.is_some())\n",
        "bigint",
    );
}

#[test]
fn reassign_bigint_into_optional_int64_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   Optional[int64] b = None\n\
        \x20   b = 9223372036854775808\n\
        \x20   print(b.unwrap())\n",
        "bigint",
    );
}

#[test]
fn call_arg_bigint_into_optional_int64_rejected() {
    lower_err_contains(
        "def f(Optional[int64] o):\n\
        \x20   print(o.is_some())\n\
        def main():\n\
        \x20   f(9223372036854775808)\n",
        "bigint",
    );
}

#[test]
fn struct_optional_field_bigint_rejected() {
    lower_err_contains(
        "struct Box:\n\
        \x20   Optional[int64] f = None\n\
        def main():\n\
        \x20   Box b = Box(9223372036854775808)\n\
        \x20   print(b.f.is_some())\n",
        "bigint",
    );
}

#[test]
fn struct_scalar_int_field_bigint_rejected() {
    // The same raw-store bug affects a plain (non-Optional) scalar field, which
    // sema also does not type-check; the shared chokepoint catches it too.
    lower_err_contains(
        "struct S:\n\
        \x20   int64 x\n\
        def main():\n\
        \x20   S s = S(9223372036854775808)\n\
        \x20   print(s.x)\n",
        "bigint",
    );
}

#[test]
fn optional_float64_bigint_rejected() {
    lower_err_contains(
        "def main():\n\
        \x20   Optional[float64] x = Optional.Some(9223372036854775808)\n\
        \x20   print(x.is_some())\n",
        "bigint",
    );
}

#[test]
fn optional_int32_bigint_rejected() {
    // A narrower fixed-width payload is rejected the same way.
    lower_err_contains(
        "def main():\n\
        \x20   Optional[int32] x = Optional.Some(9223372036854775808)\n\
        \x20   print(x.is_some())\n",
        "bigint",
    );
}

// ----- Accepted: genuine Optional[bigint] and in-range integer payloads -----

#[test]
fn optional_bigint_payload_accepted() {
    // A genuine `Optional[bigint]` with a `bigint` value must still lower.
    lower_ok(
        "def main():\n\
        \x20   bigint g = 9223372036854775808\n\
        \x20   Optional[bigint] x = Optional.Some(g)\n\
        \x20   print(x.is_some())\n",
    );
}

#[test]
fn optional_int64_in_range_payload_accepted() {
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] x = Optional.Some(100)\n\
        \x20   print(x.unwrap())\n",
    );
}

#[test]
fn reassign_in_range_payload_accepted() {
    lower_ok(
        "def main():\n\
        \x20   Optional[int64] b = None\n\
        \x20   b = 5\n\
        \x20   print(b.unwrap())\n",
    );
}

#[test]
fn optional_float64_float_payload_accepted() {
    lower_ok(
        "def main():\n\
        \x20   Optional[float64] x = Optional.Some(1.5)\n\
        \x20   print(x.unwrap())\n",
    );
}

#[test]
fn large_valid_int64_payload_accepted() {
    // The valid int64 minimum + 1 fits int64 and must box normally.
    lower_ok(
        "def main():\n\
        \x20   int64 lo = -9223372036854775807\n\
        \x20   Optional[int64] x = None\n\
        \x20   x = lo\n\
        \x20   print(x.unwrap())\n",
    );
}
