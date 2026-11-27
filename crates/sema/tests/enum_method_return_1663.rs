//! Binding a generic *enum* method's return type into the call result (#1663) —
//! the enum counterpart of #1643 (which did this for struct methods).
//!
//! The enum instance-method path (#1642) checks argument consistency but left the
//! method's return type erased (`-> T` resolves to `Unknown`), so a mismatched
//! binding target on a generic enum-method call was not flagged — while the
//! identical struct form is rejected after #1643. The enum path now uses the
//! substituted return type `check_generic_arg_consistency` computes, under the same
//! rule as #1643: only when the method is generic and the binding produced a
//! concrete type.
//!
//! Host-only sema checks (no LLVM). Additive: a matching target, numeric widening,
//! and a non-generic enum method result are unchanged.

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
fn assert_rejected(src: &str) {
    let errs = sema_errors(src);
    assert!(!errs.is_empty(), "expected some sema error, got none");
}

// ---- the issue repro ------------------------------------------------------

#[test]
fn enum_generic_method_return_rejects_mismatched_target() {
    assert_rejected(
        "enum Color:\n    Red\n    Green\n\
         \n    def make[T](self, Type[T] kind, T default) -> T:\n        return default\n\
         def main():\n    auto c = Color.Red\n    str s = c.make(int64, 1)\n    print(s)\n",
    );
}

#[test]
fn enum_generic_method_return_binds_matching_target() {
    assert_ok(
        "enum Color:\n    Red\n    Green\n\
         \n    def make[T](self, Type[T] kind, T default) -> T:\n        return default\n\
         def main():\n    auto c = Color.Red\n    int64 n = c.make(int64, 1)\n    print(n)\n",
    );
}

// ---- bare `T` argument binds the result ----------------------------------

#[test]
fn enum_generic_method_return_from_bare_t_argument() {
    assert_ok(
        "enum Box:\n    Empty\n    Full(int64 v)\n\
         \n    def ident[T](self, T x) -> T:\n        return x\n\
         def main():\n    auto b = Box.Empty\n    int64 n = b.ident(5)\n    print(n)\n",
    );
    assert_rejected(
        "enum Box:\n    Empty\n    Full(int64 v)\n\
         \n    def ident[T](self, T x) -> T:\n        return x\n\
         def main():\n    auto b = Box.Empty\n    str s = b.ident(5)\n    print(s)\n",
    );
}

#[test]
fn enum_generic_method_return_numeric_widening_ok() {
    assert_ok(
        "enum Box:\n    Empty\n    Full(int64 v)\n\
         \n    def ident[T](self, T x) -> T:\n        return x\n\
         def main():\n    auto b = Box.Empty\n    int32 x = 7\n    int64 n = b.ident(x)\n    print(n)\n",
    );
}

// ---- non-generic enum method result is unchanged -------------------------

#[test]
fn non_generic_enum_method_result_unchanged() {
    assert_ok(
        "enum Color:\n    Red\n    Green\n\
         \n    def plain(self, int64 x) -> int64:\n        return x\n\
         def main():\n    auto c = Color.Red\n    int64 n = c.plain(3)\n    print(n)\n",
    );
    assert_rejected(
        "enum Color:\n    Red\n    Green\n\
         \n    def plain(self, int64 x) -> int64:\n        return x\n\
         def main():\n    auto c = Color.Red\n    str t = c.plain(3)\n    print(t)\n",
    );
}
