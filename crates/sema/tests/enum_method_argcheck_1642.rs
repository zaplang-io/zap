//! Argument type-checking for enum *instance*-method calls (#1642) — the enum
//! follow-up to #1622 (generic method consistency), which deferred enums because
//! they are typed through a separate, fully loose receiver path (`struct_info`
//! deliberately excludes enums). Before this change an enum instance-method call
//! inferred its arguments with **no** type check at all, so both a plain argument
//! type mismatch and a generic type-parameter inconsistency were silently
//! accepted (the latter monomorphizing, e.g., a string into an `i64` slot).
//!
//! The enum instance-method call path now resolves the method signature from
//! `EnumInfo.methods`, checks each argument against its declared parameter
//! (`coercible`, with the implicit `self` dropped so positional arguments line up
//! by index), and runs `check_generic_arg_consistency` keyed `"Enum.method"`.
//! Both a plain mismatch and a generic inconsistency are `ZAP4021`.
//!
//! Host-only sema checks (no LLVM). The change is additive: a consistent call and
//! a call that never binds a parameter are unchanged, and arity stays loose.

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

// ---- plain (non-generic) argument type checking --------------------------

#[test]
fn enum_plain_method_rejects_arg_type_mismatch() {
    // `plain(self, int64 n)` — a string argument lands in an `i64` slot. Enum
    // instance-method arguments were previously unchecked, so this compiled.
    assert_code(
        "enum Color:\n    Red\n    Green\n\
         \n    def plain(self, int64 n) -> int64:\n        return n\n\
         def main():\n    auto c = Color.Red\n    auto w = c.plain(\"not an int\")\n    print(w)\n",
        "ZAP4021",
    );
}

#[test]
fn enum_plain_method_accepts_matching_arg() {
    assert_ok(
        "enum Color:\n    Red\n    Green\n\
         \n    def plain(self, int64 n) -> int64:\n        return n\n\
         def main():\n    auto c = Color.Red\n    auto w = c.plain(7)\n    print(w)\n",
    );
}

#[test]
fn enum_plain_method_multi_arg_mix() {
    // Two parameters past `self`: a good first argument and a bad second one is
    // still rejected (the positional-to-parameter mapping skips `self`).
    assert_code(
        "enum Color:\n    Red\n    Green\n\
         \n    def pair(self, int64 n, str s) -> int64:\n        return n\n\
         def main():\n    auto c = Color.Red\n    auto w = c.pair(1, 2)\n    print(w)\n",
        "ZAP4021",
    );
    assert_ok(
        "enum Color:\n    Red\n    Green\n\
         \n    def pair(self, int64 n, str s) -> int64:\n        return n\n\
         def main():\n    auto c = Color.Red\n    auto w = c.pair(1, \"ok\")\n    print(w)\n",
    );
}

#[test]
fn enum_plain_method_numeric_widening_ok() {
    // `int32` into an `int64` parameter is a legal widening, not a mismatch.
    assert_ok(
        "enum Color:\n    Red\n    Green\n\
         \n    def plain(self, int64 n) -> int64:\n        return n\n\
         def main():\n    auto c = Color.Red\n    int32 x = 3\n    auto w = c.plain(x)\n    print(w)\n",
    );
}

#[test]
fn enum_plain_method_keyword_arg_mismatch_rejected() {
    // A keyword argument is checked against its named parameter too.
    assert_rejected(
        "enum Color:\n    Red\n    Green\n\
         \n    def plain(self, int64 n) -> int64:\n        return n\n\
         def main():\n    auto c = Color.Red\n    auto w = c.plain(n=\"nope\")\n    print(w)\n",
    );
}

// ---- generic method argument consistency (#1622 for enums) ---------------

#[test]
fn enum_generic_method_rejects_inconsistent_args() {
    // `take[T](self, T x, T y)` — the first argument binds `T = int`; the second
    // must be consistent. Previously this lowered a string into an `i64` slot.
    assert_code(
        "enum Color:\n    Red\n    Green\n\
         \n    def take[T](self, T x, T y) -> T:\n        return x\n\
         def main():\n    auto c = Color.Red\n    auto z = c.take(1, \"hello\")\n    print(z)\n",
        "ZAP4021",
    );
}

#[test]
fn enum_generic_method_accepts_consistent_args() {
    assert_ok(
        "enum Color:\n    Red\n    Green\n\
         \n    def take[T](self, T x, T y) -> T:\n        return x\n\
         def main():\n    auto c = Color.Red\n    auto a = c.take(1, 2)\n    auto b = c.take(\"a\", \"b\")\n    print(a)\n",
    );
}

#[test]
fn enum_generic_method_type_value_binds_then_rejects_default() {
    // `make[T](self, Type[T] kind, T default)`: the type value binds `T`, a
    // mismatched second argument is rejected.
    assert_code(
        "enum Color:\n    Red\n    Green\n\
         \n    def make[T](self, Type[T] kind, T default) -> T:\n        return default\n\
         def main():\n    auto c = Color.Red\n    auto v = c.make(int64, \"oops\")\n    print(v)\n",
        "ZAP4021",
    );
}

#[test]
fn enum_generic_method_type_value_accepts_matching_default() {
    assert_ok(
        "enum Color:\n    Red\n    Green\n\
         \n    def make[T](self, Type[T] kind, T default) -> T:\n        return default\n\
         def main():\n    auto c = Color.Red\n    auto v = c.make(int64, 1)\n    print(v)\n",
    );
}

#[test]
fn enum_generic_method_numeric_widening_ok() {
    assert_ok(
        "enum Color:\n    Red\n    Green\n\
         \n    def take[T](self, T x, T y) -> T:\n        return x\n\
         def main():\n    auto c = Color.Red\n    int32 a = 1\n    int64 b = 2\n    \
         auto p = c.take(a, b)\n    auto q = c.take(b, a)\n    print(p)\n",
    );
}

// ---- a payload-carrying (sum) enum behaves the same ----------------------

#[test]
fn sum_enum_generic_method_rejects_inconsistent_args() {
    assert_code(
        "enum Box:\n    Empty\n    Full(int64 v)\n\
         \n    def take[T](self, T x, T y) -> T:\n        return x\n\
         def main():\n    auto b = Box.Empty\n    auto z = b.take(1, \"hello\")\n    print(z)\n",
        "ZAP4021",
    );
}

#[track_caller]
fn assert_rejected(src: &str) {
    let errs = sema_errors(src);
    assert!(!errs.is_empty(), "expected some sema error, got none");
}
