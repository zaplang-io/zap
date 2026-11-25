//! Typed os/fs/io errors — sema acceptance for #1320
//! (decisions/lang/1320-os-fs-typed-errors.md), the generalization of #1311's
//! `FileNotFoundError` / `OSError` typing across the whole filesystem surface.
//!
//! Host-only (parse + sema, no LLVM): the runtime behaviour / catch semantics
//! are pinned by the `os_fs_typed_errors` and `os_fs_error_buckets` goldens on
//! the full CI matrix. These checks pin the compile-time half — that the typed
//! handlers callers are meant to write around the os/fs/io surface type-check.

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
fn fs_size_missing_with_typed_handlers() {
    assert_ok(
        "import fs\n\
         def f() -> int:\n    \
         try:\n        \
         return fs.size(\"/no/such/path\")\n    \
         except FileNotFoundError:\n        \
         return -1\n    \
         except OSError:\n        \
         return -2\n",
    );
}

#[test]
fn os_listdir_missing_catchable_as_filenotfound() {
    assert_ok(
        "import os\n\
         def f():\n    \
         try:\n        \
         auto xs = os.listdir(\"/no/such/dir\")\n    \
         except FileNotFoundError:\n        \
         print(\"missing\")\n",
    );
}

#[test]
fn read_file_directory_caught_as_oserror() {
    assert_ok(
        "import fs\n\
         def f():\n    \
         try:\n        \
         auto c = fs.read_file(\"/\")\n    \
         except OSError:\n        \
         print(\"io\")\n    \
         except Exception:\n        \
         print(\"other\")\n",
    );
}

#[test]
fn io_open_bad_mode_caught_as_valueerror() {
    assert_ok(
        "import io\n\
         def f():\n    \
         try:\n        \
         auto fd = io.open(\"/tmp/x\", \"zzz\")\n    \
         except ValueError:\n        \
         print(\"bad-mode\")\n",
    );
}

#[test]
fn all_buckets_raiseable_and_catchable_as_exception() {
    // MemoryError / OSError / ValueError are raiseable built-in exceptions and
    // each descends from Exception (the #1320 buckets).
    for name in ["MemoryError", "OSError", "ValueError"] {
        assert_ok(&format!(
            "def f():\n    \
             try:\n        \
             raise {name}(\"m\")\n    \
             except {name} as e:\n        \
             print(e)\n"
        ));
        assert_ok(&format!(
            "def f():\n    \
             try:\n        \
             raise {name}(\"m\")\n    \
             except Exception as e:\n        \
             print(e)\n"
        ));
    }
}

#[test]
fn response_too_large_error_is_oserror_subtype() {
    // #1448: `ResponseTooLargeError` is a first-class built-in exception,
    // raiseable and catchable at its own level, as its `OSError` parent, and as
    // `Exception`. The runtime raise + hierarchy walk is pinned by the
    // `http_client_response_limit_*` goldens; this pins the compile-time half —
    // that the three handler spellings a caller writes around it type-check.
    for catch in ["ResponseTooLargeError", "OSError", "Exception"] {
        assert_ok(&format!(
            "def f():\n    \
             try:\n        \
             raise ResponseTooLargeError(\"too big\")\n    \
             except {catch} as e:\n        \
             print(e)\n"
        ));
    }
}
