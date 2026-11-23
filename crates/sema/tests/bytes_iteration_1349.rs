//! #1349: iterating a `bytes` value in a `for` loop.
//!
//! Host-only (parse + sema, no LLVM). Before the fix `bytes` had no
//! `iterable_elem` arm, so `for b in data` was rejected with ZAP4070 ("not
//! iterable") even though `bytes` is iterable in Python and the datetime-utils
//! parser needs it (zaplang-io/datetime-utils#4). `bytes` iteration now yields
//! each raw byte as an **unsigned `int64` in 0..=255** — byte-wise, not
//! UTF-8-decoded, and distinct from `str`'s codepoint (`char`) iteration.
//!
//! These lock the *type* of the loop variable (an `int64`, never a 1-char
//! `str` — the #1074 bytes-collapse hazard): the walled garden here is that a
//! later change which regresses the element type to `str`/`char` fails these
//! tests. End-to-end byte *values* (0x00/0xFF unsigned, order, empty) are
//! covered by the `tests/golden/bytes_for_loop.zap` golden and the
//! `test_bytes_iteration_1349` native-`zap test` case.

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

// ---------------------------------------------------------------------------
// bytes is iterable (the fix)
// ---------------------------------------------------------------------------

#[test]
fn for_over_bytes_literal_type_checks() {
    // The issue's exact reduction.
    ok("def main():\n\
        \x20   auto data = b\"UTC\"\n\
        \x20   int64 total = 0\n\
        \x20   for byte in data:\n\
        \x20       total += byte\n\
        \x20   assert total == 236\n");
}

#[test]
fn for_over_bytes_typed_local_type_checks() {
    ok("def main():\n\
        \x20   bytes data = b\"abc\"\n\
        \x20   for b in data:\n\
        \x20       print(b)\n");
}

#[test]
fn for_over_bytes_param_type_checks() {
    // The datetime-utils shape: a function iterating a `bytes` argument.
    ok("def scan(bytes token) -> int64:\n\
        \x20   int64 n = 0\n\
        \x20   for b in token:\n\
        \x20       n += b\n\
        \x20   return n\n");
}

#[test]
fn for_over_bytes_constructed_from_str_type_checks() {
    ok("def main():\n\
        \x20   for b in bytes(\"hi\"):\n\
        \x20       print(b)\n");
}

// ---------------------------------------------------------------------------
// the loop variable is an `int64`, not a `str`/`char` (the #1074 collapse
// hazard the fix must avoid)
// ---------------------------------------------------------------------------

#[test]
fn loop_variable_is_an_int_usable_in_arithmetic() {
    // `byte + 1` and comparison against an int literal only type-check if the
    // element is numeric.
    ok("def main():\n\
        \x20   for byte in b\"xyz\":\n\
        \x20       int64 next = byte + 1\n\
        \x20       assert next > 0\n");
}

#[test]
fn loop_variable_is_an_int_usable_as_list_index() {
    // An index must be an `int`; a `str`/`char` here would be a type error.
    ok("def main():\n\
        \x20   list[int64] table = [0, 0, 0, 0, 0]\n\
        \x20   for b in b\"abc\":\n\
        \x20       print(table[b % 5])\n");
}

#[test]
fn loop_variable_is_not_a_str_binding_to_str_is_rejected() {
    // Binding the element to a `str` local is a type error precisely because
    // the element is an `int64`, not a 1-char `str` (the #1074 collapse hazard
    // would have made this compile). ZAP4001 is the assignment-type mismatch.
    code(
        "def main():\n\
        \x20   for b in b\"abc\":\n\
        \x20       str s = b\n",
        "ZAP4001",
    );
}

#[test]
fn bytes_element_assignable_to_int64_binding() {
    // Binding the element to an explicit `int64` local must be allowed (no
    // narrowing/widening diagnostic), confirming the element type is `int64`.
    ok("def main():\n\
        \x20   for b in b\"UTC\":\n\
        \x20       int64 copy = b\n\
        \x20       assert copy >= 0\n");
}

// ---------------------------------------------------------------------------
// contrast: `str` still iterates codepoints (`char`), not bytes
// ---------------------------------------------------------------------------

#[test]
fn for_over_str_still_yields_char_not_int() {
    // A `str` loop variable is a `char`; `char + int` arithmetic is rejected
    // (ZAP4012), proving the bytes fix did not bleed into the `str` path and
    // that `str`/`bytes` iteration stay distinct element types.
    code(
        "def main():\n\
        \x20   for c in \"abc\":\n\
        \x20       int64 bad = c + 1\n",
        "ZAP4012",
    );
}
