//! `FileNotFoundError` / `OSError` — sema acceptance for the #1311 filesystem
//! read-error types (decisions/lang/1311-filesystem-notfound-error.md).
//!
//! Host-only (parse + sema, no LLVM): the runtime hierarchy / catch behaviour is
//! pinned by the `fs_notfound_error` and `fs_notfound_hierarchy` goldens on the
//! full CI matrix. These checks pin the *compile-time* half of the wiring — that
//! both new names are recognized built-in exception types, so they are
//! raiseable and nameable in an `except` clause exactly like the other built-in
//! errors — which a host `cargo test` can verify without a backend.

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
// Both new types are raiseable built-in exceptions.
// ---------------------------------------------------------------------------

#[test]
fn file_not_found_error_is_raiseable() {
    assert_ok("def f():\n    raise FileNotFoundError(\"missing\")\n");
}

#[test]
fn os_error_is_raiseable() {
    assert_ok("def f():\n    raise OSError(\"io\")\n");
}

// ---------------------------------------------------------------------------
// Both are nameable in `except` clauses, including the hierarchy relationships
// (FileNotFoundError ⊂ OSError ⊂ Exception) used by the goldens.
// ---------------------------------------------------------------------------

#[test]
fn file_not_found_error_except_by_name() {
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise FileNotFoundError(\"missing\")\n    \
         except FileNotFoundError as e:\n        \
         print(e)\n",
    );
}

#[test]
fn file_not_found_error_catchable_as_os_error() {
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise FileNotFoundError(\"missing\")\n    \
         except OSError as e:\n        \
         print(e)\n",
    );
}

#[test]
fn os_error_catchable_as_exception() {
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise OSError(\"io\")\n    \
         except Exception as e:\n        \
         print(e)\n",
    );
}

#[test]
fn ordered_clauses_child_then_parent() {
    // The real catch shape from the goldens: a specific FileNotFoundError clause
    // ahead of its OSError parent, both ahead of Exception — all must sema-check.
    assert_ok(
        "def f():\n    \
         try:\n        \
         raise FileNotFoundError(\"missing\")\n    \
         except FileNotFoundError:\n        \
         print(\"nf\")\n    \
         except OSError:\n        \
         print(\"os\")\n    \
         except Exception:\n        \
         print(\"exc\")\n",
    );
}

// ---------------------------------------------------------------------------
// The filesystem read that produces these at runtime still type-checks when
// wrapped in the typed handlers callers are meant to write.
// ---------------------------------------------------------------------------

#[test]
fn fs_read_file_with_typed_handlers() {
    assert_ok(
        "import fs\n\
         def f() -> str:\n    \
         try:\n        \
         return fs.read_file(\"/etc/resolv.conf\")\n    \
         except FileNotFoundError:\n        \
         return \"defaults\"\n    \
         except OSError:\n        \
         return \"io-error\"\n",
    );
}
