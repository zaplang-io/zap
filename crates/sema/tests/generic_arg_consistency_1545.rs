//! Generic argument consistency after a type parameter is bound (#1545).
//!
//! Binding a type parameter `T` from one argument must constrain every other
//! argument declared with that parameter. The #1527 ADR deferred this: with
//! `def option[T](Type[T] kind, T default)`, the `Type[T]` argument establishes
//! `T = int64`, yet a mismatched `T default` was silently accepted (and lowered
//! to an `i64` slot holding a string constant). This closes that gap — a property
//! of generic checking in general, not of the decorator desugar, so it is proven
//! here on plain generic functions.
//!
//! These are host-only sema checks (no LLVM). The rule is additive: before this
//! change every type parameter erased to `Unknown`, so no previously-accepted
//! *consistent* call changes; only genuinely inconsistent calls are now rejected.

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

// ---- the issue's reproduction: Type[T] binds T, then T default is checked ----

#[test]
fn type_arg_binds_t_then_rejects_incompatible_default() {
    // The exact repro: `Type[int64]` establishes `T = int64`; a `str` default is
    // not assignable to `int64`.
    assert_code(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    int64 n = option(int64, default=\"oops\")\n    print(n)\n",
        "ZAP4021",
    );
}

#[test]
fn type_arg_binds_t_then_accepts_matching_default() {
    // A matching integer default continues to compile.
    assert_ok(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    int64 n = option(int64, default=1)\n    print(n)\n",
    );
}

// ---- a second primitive -------------------------------------------------------

#[test]
fn second_primitive_str_rejects_int_default() {
    assert_code(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    auto s = option(str, default=5)\n    print(s)\n",
        "ZAP4021",
    );
}

#[test]
fn second_primitive_str_accepts_str_default() {
    assert_ok(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    auto s = option(str, default=\"hi\")\n    print(s)\n",
    );
}

#[test]
fn bool_type_arg_rejects_str_default() {
    assert_code(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    auto b = option(bool, default=\"x\")\n    print(b)\n",
        "ZAP4021",
    );
}

#[test]
fn bool_type_arg_accepts_bool_default() {
    assert_ok(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    auto b = option(bool, default=False)\n    print(b)\n",
    );
}

// ---- keyword ordering is irrelevant to the binding ---------------------------

#[test]
fn keyword_kind_and_default_mismatch_is_rejected() {
    // `kind` passed by keyword, default mismatched — the binding is by parameter,
    // not by textual argument order.
    assert_code(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    auto n = option(kind=int64, default=\"oops\")\n    print(n)\n",
        "ZAP4021",
    );
}

#[test]
fn reordered_keywords_default_before_kind_mismatch_is_rejected() {
    // The type value appears textually *after* the value it must constrain; the
    // establish/check split makes the verdict order-independent.
    assert_code(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    auto n = option(default=\"oops\", kind=int64)\n    print(n)\n",
        "ZAP4021",
    );
}

#[test]
fn reordered_keywords_matching_is_accepted() {
    assert_ok(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    auto n = option(default=1, kind=int64)\n    print(n)\n",
    );
}

// ---- repeated ordinary T parameters (distinguishes this from the decorator) ---

#[test]
fn repeated_ordinary_t_params_reject_mismatch() {
    // `take[T](T x, T y)` — the ADR's canonical loose case. The first argument
    // binds `T`; the second must be consistent. No type value and no decorator
    // involved, so this proves the rule lives in generic checking.
    assert_code(
        "def take[T](T x, T y) -> T:\n    return x\n\
         def main():\n    auto z = take(1, \"hello\")\n    print(z)\n",
        "ZAP4021",
    );
}

#[test]
fn repeated_ordinary_t_params_accept_same_type() {
    assert_ok(
        "def take[T](T x, T y) -> T:\n    return x\n\
         def main():\n    auto a = take(1, 2)\n    auto b = take(\"a\", \"b\")\n    print(a)\n",
    );
}

#[test]
fn repeated_ordinary_t_params_numeric_widening_ok() {
    // A narrower integer then a wider one is consistent (the bound widens to the
    // least upper bound), independent of which argument comes first.
    assert_ok(
        "def take[T](T x, T y) -> T:\n    return x\n\
         def main():\n    int32 a = 1\n    int64 b = 2\n    \
         auto p = take(a, b)\n    auto q = take(b, a)\n    print(p)\n",
    );
}

#[test]
fn three_t_params_one_mismatch_rejected() {
    assert_code(
        "def pick[T](Type[T] kind, T a, T b) -> T:\n    return a\n\
         def main():\n    auto r = pick(int64, 1, \"two\")\n    print(r)\n",
        "ZAP4021",
    );
}

#[test]
fn three_t_params_all_consistent_accepted() {
    assert_ok(
        "def pick[T](Type[T] kind, T a, T b) -> T:\n    return a\n\
         def main():\n    auto r = pick(int64, 1, 2)\n    print(r)\n",
    );
}

// ---- the enforcement must not regress #1527 ----------------------------------

#[test]
fn single_t_param_type_value_still_accepted() {
    // `opt[T](T kind)` with one occurrence of `T` has nothing to be inconsistent
    // with — the #1527 surface is unchanged.
    assert_ok(
        "def opt[T](T kind) -> int64:\n    return 1\n\
         def main():\n    print(opt(int64))\n",
    );
}

#[test]
fn concrete_type_arg_binding_still_accepts_matching_default() {
    // A concrete (non-generic) `Type[int64]`/`int64` pair is checked by ordinary
    // type checking; the generic pass does not interfere.
    assert_ok(
        "def opt(Type[int64] kind, int64 default) -> int64:\n    return default\n\
         def main():\n    print(opt(int64, 1))\n",
    );
}

// ---- Unknown / dynamic arguments never trigger a false positive --------------

#[test]
fn unknown_typed_argument_does_not_force_a_binding() {
    // A `py` (dynamic) value is a wildcard: it neither binds `T` nor conflicts, so
    // a mixed call through `py` stays accepted (no spurious rejection).
    assert_ok(
        "def take[T](T x, T y) -> T:\n    return x\n\
         def main():\n    py d = 1\n    auto z = take(d, \"hello\")\n    print(z)\n",
    );
}
