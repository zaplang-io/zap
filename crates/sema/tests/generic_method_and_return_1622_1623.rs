//! Generic argument consistency for methods (#1622) and binding a generic return
//! type into the call's result (#1623) — the two follow-ups to #1545.
//!
//! #1545 enforced type-parameter consistency for generic *free functions* and
//! left the result type erased to `Unknown`. #1622 extends the consistency check
//! to generic *methods* (whose instance-call arguments were otherwise inferred
//! without any type check), and #1623 substitutes the bound type parameters back
//! into the return type so a mismatched binding target is rejected.
//!
//! Host-only sema checks (no LLVM). Both are additive: a consistent call and a
//! call that never binds a parameter are unchanged.

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

#[track_caller]
fn assert_rejected(src: &str) {
    let errs = sema_errors(src);
    assert!(!errs.is_empty(), "expected some sema error, got none");
}

// ---- #1622: generic method argument consistency --------------------------

#[test]
fn generic_method_rejects_inconsistent_args() {
    // `take[T](self, T x, T y)` — the first argument binds `T`; the second must
    // be consistent. Instance-method arguments were previously unchecked, so this
    // compiled and lowered a string into an `i64` slot.
    assert_code(
        "struct Util:\n    def take[T](self, T x, T y) -> T:\n        return x\n\
         def main():\n    Util u = Util()\n    auto z = u.take(1, \"hello\")\n    print(z)\n",
        "ZAP4021",
    );
}

#[test]
fn generic_method_accepts_consistent_args() {
    assert_ok(
        "struct Util:\n    def take[T](self, T x, T y) -> T:\n        return x\n\
         def main():\n    Util u = Util()\n    auto a = u.take(1, 2)\n    auto b = u.take(\"a\", \"b\")\n    print(a)\n",
    );
}

#[test]
fn generic_method_type_value_binds_then_rejects_default() {
    // The `Type[T]`/`T` method shape: the type value binds `T`, a mismatched
    // second argument is rejected.
    assert_code(
        "struct Opt:\n    def make[T](self, Type[T] kind, T default) -> T:\n        return default\n\
         def main():\n    Opt o = Opt()\n    auto v = o.make(int64, \"oops\")\n    print(v)\n",
        "ZAP4021",
    );
}

#[test]
fn generic_method_type_value_accepts_matching_default() {
    assert_ok(
        "struct Opt:\n    def make[T](self, Type[T] kind, T default) -> T:\n        return default\n\
         def main():\n    Opt o = Opt()\n    auto v = o.make(int64, 1)\n    print(v)\n",
    );
}

#[test]
fn generic_method_numeric_widening_ok() {
    assert_ok(
        "struct Util:\n    def take[T](self, T x, T y) -> T:\n        return x\n\
         def main():\n    Util u = Util()\n    int32 a = 1\n    int64 b = 2\n    \
         auto p = u.take(a, b)\n    auto q = u.take(b, a)\n    print(p)\n",
    );
}

// Enum *instance* methods are typed through a separate, fully loose receiver
// path (`struct_info` deliberately excludes enums), so generic-method consistency
// for enum instance calls is a documented follow-up, out of scope here.

// ---- #1623: binding a generic return type into the result ----------------

#[test]
fn generic_return_binds_and_rejects_mismatched_target() {
    // `option[T](Type[T] kind, T default) -> T`: binding `T = int64` makes the
    // result `int64`, so assigning it to `str` is rejected (previously the result
    // erased to `Unknown` and the assignment was accepted).
    assert_rejected(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    str s = option(int64, default=1)\n    print(s)\n",
    );
}

#[test]
fn generic_return_binds_matching_target() {
    assert_ok(
        "def option[T](Type[T] kind, T default) -> T:\n    return default\n\
         def main():\n    int64 n = option(int64, default=1)\n    print(n)\n",
    );
}

#[test]
fn generic_return_from_bare_t_argument() {
    // `ident[T](T x) -> T`: the argument binds `T`, so the result type follows the
    // argument. A matching target compiles; a mismatched one is rejected.
    assert_ok(
        "def ident[T](T x) -> T:\n    return x\n\
         def main():\n    int64 n = ident(5)\n    print(n)\n",
    );
    assert_rejected(
        "def ident[T](T x) -> T:\n    return x\n\
         def main():\n    str s = ident(5)\n    print(s)\n",
    );
}

#[test]
fn non_generic_call_result_unchanged() {
    // A non-generic function's result is unaffected by the binding pass.
    assert_ok(
        "def plain(int64 x) -> int64:\n    return x\n\
         def main():\n    int64 n = plain(3)\n    print(n)\n",
    );
}
