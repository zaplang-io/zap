//! Sema acceptance for the `NullReferenceError` built-in (I#1492, Epic I#1489,
//! decisions/lang/1489-null-reference-error.md).
//!
//! Host-only (parse + sema, no LLVM): these pin the compile-time half — that the
//! bare name `NullReferenceError` is a known built-in exception usable as a
//! `raise` target and in a typed `except` clause (the #1278 raiseability gate),
//! catchable at its own level and as its `Exception` parent. The runtime catch
//! behaviour / message round-trip is pinned by the `null_reference_error_*`
//! goldens and the `zapc_ir` message-shape unit tests.

use zapc_sema::check_str;

#[track_caller]
fn assert_ok(src: &str) {
    let (terrs, perrs, lerrs) = check_str(src);
    assert!(lerrs.is_empty(), "unexpected lex errors: {lerrs:?}");
    assert!(perrs.is_empty(), "unexpected parse errors: {perrs:?}");
    let terrs: Vec<_> = terrs
        .into_iter()
        .map(|e| (e.code.to_string(), e.message))
        .collect();
    assert!(terrs.is_empty(), "expected no sema errors, got: {terrs:?}");
}

#[test]
fn raiseable_and_catchable_at_own_level() {
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise NullReferenceError(\"null dereference at foo.zap:7\")\n    \
         except NullReferenceError as e:\n        \
         print(e)\n",
    );
}

#[test]
fn catchable_as_exception_base() {
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise NullReferenceError(\"null dereference at foo.zap:7\")\n    \
         except Exception as e:\n        \
         print(e)\n",
    );
}

#[test]
fn all_three_handler_spellings_type_check() {
    // Its own type, its `Exception` parent, and a bare re-raising handler all
    // type-check around a `NullReferenceError` raise site.
    for catch in ["NullReferenceError", "Exception"] {
        assert_ok(&format!(
            "def f():\n    \
             try:\n        \
             raise NullReferenceError(\"m\")\n    \
             except {catch} as e:\n        \
             print(e)\n"
        ));
    }
}

#[test]
fn usable_in_a_multi_clause_handler_next_to_other_builtins() {
    // A realistic handler stack mixing NullReferenceError with the other
    // first-class built-ins type-checks (ordering/shadowing is a runtime
    // concern, pinned by the goldens).
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise NullReferenceError(\"m\")\n    \
         except ValueError:\n        \
         print(\"value\")\n    \
         except NullReferenceError as e:\n        \
         print(e)\n    \
         except Exception:\n        \
         print(\"other\")\n",
    );
}

#[test]
fn bare_raise_target_type_checks() {
    // `raise NullReferenceError` (no call) is also a valid raise of the built-in.
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise NullReferenceError\n    \
         except NullReferenceError:\n        \
         print(\"caught\")\n",
    );
}
