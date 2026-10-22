//! Signal fault-handler integration test (ticket #77, D-0058).
//!
//! The fault crash handler (SIGSEGV/SIGFPE/SIGBUS/SIGILL) prints a diagnostic
//! report — faulting signal/address, offending coroutine, backtrace — and then
//! RE-RAISES the default disposition so the process dies with the correct
//! signal status (and a core, if enabled). Because the process is killed by a
//! signal rather than exiting with a code, this cannot use the stdout/exit-code
//! golden harness (`status.code()` is `None` under signal death); it is checked
//! here instead.
//!
//! Requires the `llvm` feature and an LLVM toolchain (run in the dev container,
//! `make docker-test`).

#![cfg(feature = "codegen")]

use std::io::Write;
use std::process::Command;

/// Compile a tiny Zap program that triggers a SIGSEGV via the internal
/// `__raise_signal` test hook to a native binary, run THAT binary directly, and
/// assert the crash handler reported the fault and the process died from the
/// signal. (Running via `zapc run` would hide the signal death: it executes the
/// compiled binary as a subprocess and maps its signal death to its own exit 1.)
#[test]
fn fault_signal_prints_crash_report() {
    let src = "def main() -> int64:\n    __raise_signal(11)\n    return 0\n";
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let zap = dir.join("fault_signal.zap");
    {
        let mut f = std::fs::File::create(&zap).expect("create temp .zap");
        f.write_all(src.as_bytes()).expect("write temp .zap");
    }
    let exe = dir.join("fault_signal_bin");

    let bin = env!("CARGO_BIN_EXE_zapc");
    let build = Command::new(bin)
        .arg("build")
        .arg(&zap)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("zapc build");
    assert!(
        build.status.success(),
        "zapc build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    // Run the native binary directly so a signal death reaches us unmodified.
    let output = Command::new(&exe).output().expect("run fault binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    // The crash handler's report must name the signal.
    assert!(
        stderr.contains("zap fatal: SIGSEGV"),
        "expected a SIGSEGV crash report in stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("backtrace:"),
        "expected a backtrace in the crash report, got:\n{stderr}"
    );

    // The process must have died from the signal (re-raised default), not
    // exited with a normal code.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            output.status.signal(),
            Some(11),
            "expected death by SIGSEGV (signal 11); status = {:?}",
            output.status
        );
    }
}
