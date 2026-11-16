//! `list[T]()` / `dict[K, V]()` empty-collection constructors (#1004).
//!
//! The construction synonyms for the `[]` / `{}` empty literals, parity with
//! `set[T]()`. Zero-arg only; a populated collection uses a literal. Host-only
//! (parse + sema). A type-parameter element (`list[T]()` in a generic body) is
//! accepted by sema but its end-to-end codegen is a follow-up (the monomorphizer
//! does not yet substitute a collection constructor's type-argument subscript —
//! the same limitation `set[T]()` has), so it is not asserted here.

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

#[test]
fn list_ctor_is_accepted() {
    assert_ok("def f():\n    auto a = list[int64]()\n    print(len(a))\n");
    assert_ok("def f():\n    auto a = list[str]()\n    a.append(\"x\")\n");
}

#[test]
fn dict_ctor_is_accepted() {
    assert_ok("def f():\n    auto d = dict[str, int64]()\n    print(len(d))\n");
    assert_ok("def f():\n    auto d = dict[str, int64]()\n    d[\"k\"] = 1\n");
}

#[test]
fn list_ctor_equals_empty_literal_type() {
    // `list[int64]()` binds the same `list[int64]` an annotated `[]` does, so a
    // later typed reassignment is accepted.
    assert_ok("def f():\n    auto a = list[int64]()\n    list[int64] b = []\n    a = b\n");
}

#[test]
fn list_ctor_rejects_arguments() {
    assert_code("def f():\n    auto a = list[int64](1)\n", "ZAP4025");
}

#[test]
fn dict_ctor_rejects_arguments() {
    assert_code("def f():\n    auto d = dict[str, int64](1)\n", "ZAP4025");
}

#[test]
fn dict_ctor_requires_two_type_args() {
    assert_code("def f():\n    auto d = dict[int64]()\n", "ZAP4025");
}
