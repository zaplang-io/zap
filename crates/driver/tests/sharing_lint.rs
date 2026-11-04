//! End-to-end driver tests for the concurrency-safety sharing lint (ZAP5125)
//! and the `--warnings-as-errors` knob (#664, D-0113).
//!
//! These drive the real `zap` binary through the host-safe `ir` subcommand (no
//! LLVM needed), so they exercise the whole front-end pipeline: the sema lint,
//! the driver's warning rendering, per-site `# noqa` suppression, and the
//! promotion of warnings to a fatal exit under `--warnings-as-errors`. The lint
//! itself (every fires/silent case) is unit-tested in `zapc_sema`; this file
//! pins the *policy wiring* a team's CI depends on.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A process-unique temp path for one fixture's `.zap` source.
fn unique_zap_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "zap-share-{}-{nanos}-{seq}.zap",
        std::process::id()
    ))
}

/// Run `zap ir [flags] <file>` on `src`, returning the process output. `ir` is
/// host-safe (front-end + lowering only), so this runs on the no-LLVM CI leg.
fn run_ir(src: &str, flags: &[&str]) -> Output {
    let path = unique_zap_path();
    std::fs::write(&path, src).expect("write temp source");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zap"));
    cmd.arg("ir");
    for f in flags {
        cmd.arg(f);
    }
    cmd.arg(&path);
    let out = cmd.output().expect("spawn zap");
    let _ = std::fs::remove_file(&path);
    out
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("process exited with a code")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A bare mutable global of a non-`@shareable` type: the lint fires (a warning
/// on stderr), but the build still succeeds — the program is legal.
const RACY: &str =
    "int64 counter = 0\ndef bump():\n    counter = counter + 1\ndef main():\n    bump()\n";

#[test]
fn lint_warns_but_build_succeeds_by_default() {
    let out = run_ir(RACY, &[]);
    assert_eq!(
        code(&out),
        0,
        "default build must succeed: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("ZAP5125"),
        "expected the sharing-lint warning on stderr, got: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("warning"),
        "the diagnostic must render at warning severity, got: {}",
        stderr(&out)
    );
}

#[test]
fn warnings_as_errors_makes_the_lint_fatal() {
    // Same program, but `--warnings-as-errors` promotes the warning to a build
    // failure. Both the long flag and the `-Werror` alias do it.
    let out = run_ir(RACY, &["--warnings-as-errors"]);
    assert_eq!(
        code(&out),
        1,
        "warnings-as-errors must fail the build; stderr: {}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("ZAP5125"));

    let out = run_ir(RACY, &["-Werror"]);
    assert_eq!(code(&out), 1, "the -Werror alias must also fail the build");
}

#[test]
fn const_global_is_never_fatal_even_under_werror() {
    // A `const` global can never race, so it raises no warning — and so
    // `--warnings-as-errors` does not fail it.
    let src =
        "const int64 N = 7\ndef use() -> int64:\n    return N\ndef main():\n    print(use())\n";
    let out = run_ir(src, &["--warnings-as-errors"]);
    assert_eq!(
        code(&out),
        0,
        "a const global must not warn, so -Werror must pass; stderr: {}",
        stderr(&out)
    );
    assert!(!stderr(&out).contains("ZAP5125"));
}

#[test]
fn noqa_suppression_silences_the_site_even_under_werror() {
    // A `# noqa` on the one access line suppresses the warning entirely, so even
    // `--warnings-as-errors` sees nothing to promote and the build passes.
    let src = "int64 counter = 0\ndef bump():\n    counter = 1  # noqa\ndef main():\n    bump()\n";
    let out = run_ir(src, &["--warnings-as-errors"]);
    assert_eq!(
        code(&out),
        0,
        "a suppressed site must leave nothing for -Werror to fail on; stderr: {}",
        stderr(&out)
    );
    assert!(!stderr(&out).contains("ZAP5125"));
}
