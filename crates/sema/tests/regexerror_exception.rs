//! `RegexError` — sema acceptance for the #1582 dedicated regex error type
//! (decisions/lang/1582-regex-error.md), a subclass of `ValueError`.
//!
//! Host-only (parse + sema, no LLVM): the runtime hierarchy / catch behaviour is
//! pinned by the `regexerror_hierarchy`, `regexerror_catchable`, and
//! `regexerror_uncaught` goldens on the full CI matrix. These checks pin the
//! *compile-time* half of the wiring — that `RegexError` is recognized as a
//! built-in exception type, so it is raiseable and nameable in an `except`
//! clause exactly like the other built-in errors (and like its `ValueError`
//! parent) — which a host `cargo test` can verify without a backend.

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

// ---------------------------------------------------------------------------
// RegexError is a raiseable built-in exception.
// ---------------------------------------------------------------------------

#[test]
fn regex_error_is_raiseable() {
    assert_ok("def f():\n    raise RegexError(\"bad pattern\")\n");
}

// ---------------------------------------------------------------------------
// RegexError is nameable in `except` clauses, including the hierarchy
// relationships (RegexError ⊂ ValueError ⊂ Exception) used by the goldens.
// ---------------------------------------------------------------------------

#[test]
fn regex_error_except_by_name() {
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise RegexError(\"bad pattern\")\n    \
         except RegexError as e:\n        \
         print(e)\n",
    );
}

#[test]
fn regex_error_catchable_as_value_error() {
    // The whole point of #1582: because RegexError subclasses ValueError, an
    // existing `except ValueError` keeps catching regex failures — no migration.
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise RegexError(\"bad pattern\")\n    \
         except ValueError as e:\n        \
         print(e)\n",
    );
}

#[test]
fn regex_error_catchable_as_exception() {
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise RegexError(\"bad pattern\")\n    \
         except Exception as e:\n        \
         print(e)\n",
    );
}

#[test]
fn ordered_clauses_child_then_parent() {
    // The real catch shape from the goldens: a specific RegexError clause ahead
    // of its ValueError parent, both ahead of Exception — all must sema-check.
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise RegexError(\"bad pattern\")\n    \
         except RegexError:\n        \
         print(\"re\")\n    \
         except ValueError:\n        \
         print(\"val\")\n    \
         except Exception:\n        \
         print(\"exc\")\n",
    );
}

// ---------------------------------------------------------------------------
// The regex operations that produce it at runtime still type-check when wrapped
// in the typed handler callers are meant to write.
// ---------------------------------------------------------------------------

#[test]
fn regex_match_with_typed_handler() {
    assert_ok(
        "def f():\n    \
         str s = \"x\"\n    \
         try:\n        \
         auto m = s =~ m/(/\n    \
         except RegexError:\n        \
         print(\"bad regex\")\n",
    );
}
