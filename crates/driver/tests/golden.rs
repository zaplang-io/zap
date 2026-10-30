//! Golden-test harness.
//!
//! Golden programs live in `<repo>/tests/golden/*.zap`, each paired with a
//! `*.expected` file holding the program's expected stdout. Two optional
//! sibling files cover programs that are meant to fail (e.g. an uncaught
//! exception, ticket #22):
//!
//! - `*.exit` — the expected process exit code (default `0`, i.e. success).
//! - `*.stderr` — a substring that must appear in the program's stderr.
//!
//! The end-to-end run (`golden_programs_match`) requires the `llvm` feature
//! (and an LLVM toolchain); run it in the dev container with
//! `--features zapc/llvm`. The `fixtures_are_paired` check runs everywhere.

use std::path::PathBuf;

/// `<repo>/tests/golden` — this crate lives at `<repo>/crates/driver`.
fn golden_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/driver has a two-level parent")
        .join("tests")
        .join("golden")
}

fn zap_fixtures() -> Vec<PathBuf> {
    let dir = golden_dir();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("golden dir should exist") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "zap") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// `<repo>/tests/golden_projects` — multi-file project fixtures (I#283/D-0081).
/// Each subdirectory is a project with an entry `main.zap`, its sibling module
/// files, and an `expected` file (plus optional `exit`/`stderr` sidecars).
fn golden_projects_dir() -> PathBuf {
    golden_dir()
        .parent()
        .expect("golden dir has a parent")
        .join("golden_projects")
}

/// The entry `main.zap` of each project fixture.
fn project_fixtures() -> Vec<PathBuf> {
    let dir = golden_projects_dir();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd {
            let p = entry.expect("dir entry").path();
            let main = p.join("main.zap");
            if p.is_dir() && main.exists() {
                out.push(main);
            }
        }
    }
    out.sort();
    out
}

/// Every project fixture has an `expected` file. Runs everywhere.
#[test]
fn project_fixtures_are_paired() {
    for main in project_fixtures() {
        let expected = main.parent().expect("project dir").join("expected");
        assert!(
            expected.exists(),
            "missing `expected` for project {}",
            main.display()
        );
    }
}

/// Every `.zap` fixture has a matching `.expected` file. Runs everywhere.
#[test]
fn fixtures_are_paired() {
    let fixtures = zap_fixtures();
    assert!(
        !fixtures.is_empty(),
        "expected at least one golden fixture in {}",
        golden_dir().display()
    );
    for zap in fixtures {
        let expected = zap.with_extension("expected");
        assert!(
            expected.exists(),
            "missing expected output for {}",
            zap.display()
        );
    }
}

/// Run `cmd`, capturing stdout/stderr, but FAIL FAST if it does not finish
/// within `timeout` — a hung golden (e.g. a wait on a signal/channel that never
/// fires) then fails this test deterministically instead of stalling the whole
/// CI run until the job's global timeout. stdout/stderr are drained on threads
/// so the child never blocks on a full pipe buffer.
#[cfg(feature = "codegen")]
fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    label: &str,
) -> std::process::Output {
    use std::io::Read;
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn zapc");
    let mut out = child.stdout.take().expect("stdout pipe");
    let mut err = child.stderr.take().expect("stderr pipe");
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "golden {label} did not finish within {timeout:?} \
                         (likely a deadlock/hang in the compiled program)"
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    std::process::Output {
        status,
        stdout,
        stderr,
    }
}

/// Optional `*.skip_arch` sidecar: whitespace/newline-separated architecture
/// names (matched against `std::env::consts::ARCH`; `#` starts a comment). A
/// fixture whose sidecar names the architecture this test binary runs on is
/// SKIPPED by the end-to-end runner below, with a loud notice. Used to
/// quarantine a golden that hits a known, tracked, architecture-specific bug so
/// CI stays deterministically green while the ticket stays open — currently the
/// concurrent stress goldens on aarch64, which intermittently hang on the parked
/// GC reclamation race #223 (collector-agnostic: fails under Boehm too). Delete
/// the sidecar when #223 is fixed, or once continuous aarch64 infra re-enables
/// these, to restore coverage.
#[cfg(feature = "codegen")]
fn skipped_on_this_arch(zap: &std::path::Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(zap.with_extension("skip_arch")) else {
        return false;
    };
    let arch = std::env::consts::ARCH;
    contents.lines().any(|line| {
        line.split('#')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .any(|a| a == arch)
    })
}

/// Compile + run each fixture and compare stdout. Requires the `llvm` feature.
#[cfg(feature = "codegen")]
#[test]
fn golden_programs_match() {
    use std::process::Command;
    // Generous per-fixture ceiling: a fixture compiles (cc + LLVM) and runs a
    // small program in a second or two; 60s only trips on a true hang, matching
    // GitHub's own "running for over 60 seconds" warning so a deadlock fails
    // this test fast rather than burning the job's budget.
    let per_test = std::time::Duration::from_secs(60);
    let bin = env!("CARGO_BIN_EXE_zapc");
    for zap in zap_fixtures() {
        // A fixture quarantined on this architecture (see `skipped_on_this_arch`)
        // is skipped loudly rather than run — e.g. the concurrent stress goldens
        // on aarch64, pending the parked #223 GC race.
        if skipped_on_this_arch(&zap) {
            eprintln!(
                "golden: SKIP {} on arch {} — quarantined via .skip_arch sidecar (see #223)",
                zap.display(),
                std::env::consts::ARCH
            );
            continue;
        }
        let expected = std::fs::read_to_string(zap.with_extension("expected"))
            .expect("expected output present");
        // Optional expected exit code (default 0) and expected-stderr substring.
        let expected_exit: i32 = std::fs::read_to_string(zap.with_extension("exit"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let expected_stderr = std::fs::read_to_string(zap.with_extension("stderr")).ok();

        let mut cmd = Command::new(bin);
        cmd.arg("run").arg(&zap);
        // Optional `*.env` sidecar: `KEY=VALUE` lines set on the child's
        // environment. Used to make a scheduler knob load-bearing in a fixture —
        // e.g. `MAXPROCS=1` so the preemption golden forces all coroutines onto a
        // single worker M (ticket #172).
        if let Ok(env) = std::fs::read_to_string(zap.with_extension("env")) {
            for line in env.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    cmd.env(k.trim(), v.trim());
                }
            }
        }
        let output = run_with_timeout(cmd, per_test, &zap.display().to_string());

        let got = String::from_utf8_lossy(&output.stdout);
        let got_err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "exit code mismatch for {} (stderr:\n{got_err})",
            zap.display()
        );
        assert_eq!(
            got.trim_end(),
            expected.trim_end(),
            "stdout mismatch for {} (stderr:\n{got_err})",
            zap.display()
        );
        if let Some(want_err) = expected_stderr {
            let want_err = want_err.trim_end();
            assert!(
                got_err.contains(want_err),
                "stderr for {} should contain {want_err:?}, got:\n{got_err}",
                zap.display()
            );
        }
    }
}

/// Compile + run each multi-file project fixture (I#283/D-0081) by its entry
/// `main.zap` and compare stdout. Requires the `llvm` feature.
#[cfg(feature = "codegen")]
#[test]
fn golden_projects_match() {
    use std::process::Command;
    let per_test = std::time::Duration::from_secs(60);
    let bin = env!("CARGO_BIN_EXE_zapc");
    for main in project_fixtures() {
        let proj = main.parent().expect("project dir");
        let expected =
            std::fs::read_to_string(proj.join("expected")).expect("expected output present");
        let expected_exit: i32 = std::fs::read_to_string(proj.join("exit"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let expected_stderr = std::fs::read_to_string(proj.join("stderr")).ok();

        let mut cmd = Command::new(bin);
        cmd.arg("run").arg(&main);
        // Optional `env` sidecar: `KEY=VALUE` lines set on the child's
        // environment, with `$PROJDIR` expanded to the project's absolute path.
        // Used by the cross-package fixture to point `ZAP_ZENV` at a prebuilt
        // `zenv/` holding materialized dependency packages (I#283/D-0081 Stage B).
        if let Ok(env) = std::fs::read_to_string(proj.join("env")) {
            let projdir = proj.display().to_string();
            for line in env.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    cmd.env(k.trim(), v.trim().replace("$PROJDIR", &projdir));
                }
            }
        }
        let output = run_with_timeout(cmd, per_test, &main.display().to_string());

        let got = String::from_utf8_lossy(&output.stdout);
        let got_err = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "exit code mismatch for {} (stderr:\n{got_err})",
            main.display()
        );
        assert_eq!(
            got.trim_end(),
            expected.trim_end(),
            "stdout mismatch for {} (stderr:\n{got_err})",
            main.display()
        );
        if let Some(want_err) = expected_stderr {
            let want_err = want_err.trim_end();
            assert!(
                got_err.contains(want_err),
                "stderr for {} should contain {want_err:?}, got:\n{got_err}",
                main.display()
            );
        }
    }
}
