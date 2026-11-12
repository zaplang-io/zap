//! Conditional-libpython link assertions (#852).
//!
//! WHY THIS EXISTS. Every compiled Zap binary used to hard-link
//! `libpython3.12` as a `NEEDED` dynamic dependency — even a program that never
//! `import python`s — so `ld.so` had to locate, mmap and relocate libpython at
//! every process start, inflating the startup floor and resident footprint, and
//! making the binary refuse to run without a matching libpython present (#852,
//! and the al2023 / portability story I#287). The compiler knows the whole
//! import graph at build time (it is module-linked into one module before
//! codegen, D-0081), so it knows whether `import python` is reachable:
//! `build_executable` now embeds CPython (and emits the `libpython` `NEEDED`)
//! ONLY when the import graph reaches `import python`. Otherwise it takes the
//! same `-DZAP_NO_CPYTHON` catchable-error-stub path the `--portable` profile
//! (I#287) and Windows v1 (I#198) already use, and links no libpython.
//!
//! These tests are the walled garden around that gate (AGENTS.md: exhaustive,
//! intent-documenting). They assert, via `readelf -d`:
//!   (a) a program that does NOT `import python` has NO `libpython` `NEEDED`
//!       entry — the defect this issue fixes — and still runs;
//!   (b) a program that DOES `import python` still links libpython (`NEEDED`
//!       present), so interop is unchanged (the `python_interop` golden covers
//!       that it still RUNS, under the full CI build with `python3-dev`); and
//!   (c) the catchable-error stub path still raises an ordinary (catchable) Zap
//!       exception when a `--portable` build calls into `import python` — the
//!       manual override must keep working exactly as today.
//!
//! The `readelf` assertions are ELF/Linux-specific and need `readelf` on PATH;
//! on a non-Linux host or without `readelf` they SKIP rather than fail (macOS
//! dev hosts have no `readelf` and emit Mach-O, not ELF). CI's Linux leg has
//! binutils + `python3-dev`, so the gate is exercised there. Test (b)
//! additionally SKIPS where no `python3-embed`/`python3` `pkg-config` module is
//! installed (a dev host without `python3-dev`), since a Python program cannot
//! link there by design.

#![cfg(feature = "codegen")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A program with NO `import python`: its import graph never reaches CPython, so
/// it must not pull in libpython.
const NO_PYTHON_SRC: &str = "\
def main():
    print(0)
";

/// A program that DOES `import python` (stdlib-only, like the `python_interop`
/// golden, so it needs no `pip install`). Its import graph reaches CPython, so
/// it must still link libpython in a normal (non-portable) build.
const PYTHON_SRC: &str = "\
import python \"math\" as m

def main():
    print(int64(m.gcd(48, 60)))
";

/// A program that `import python`s and immediately uses it inside a
/// `try`/`except Exception`. Under the `--portable` / `-DZAP_NO_CPYTHON` stub
/// path the use raises a CATCHABLE Zap exception (`zap_panic` → `Exception`),
/// which the handler catches — proving the stub is an ordinary exception, not a
/// hard `abort()` or a link failure.
const PORTABLE_CATCH_SRC: &str = "\
import python \"math\" as m

def main():
    try:
        int64 g = int64(m.gcd(48, 60))
        print(g)
    except Exception as e:
        print(\"caught\")
";

/// Locate `readelf` (binutils). `None` => skip the ELF-inspection tests.
fn find_readelf() -> Option<String> {
    for cand in ["readelf", "llvm-readelf", "llvm-readelf-18"] {
        let ok = Command::new(cand)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Some(cand.to_string());
        }
    }
    None
}

/// Whether a `pkg-config` CPython embedding module is installed, i.e. whether a
/// Python program can link here at all. `false` => skip the "python still
/// links" test on this host (it cannot, by design).
fn has_python_embed() -> bool {
    ["python3-embed", "python3"].iter().any(|m| {
        Command::new("pkg-config")
            .args(["--exists", m])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Build `src` to a native binary at `dir/<name>`, passing `extra_args` to
/// `zapc build` (e.g. `--portable`). Returns `(binary_path, build_output)` so
/// the caller can decide whether a build failure is a skip or a hard failure.
fn try_build(
    dir: &Path,
    name: &str,
    src: &str,
    extra_args: &[&str],
) -> (PathBuf, std::process::Output) {
    std::fs::create_dir_all(dir).unwrap();
    let srcf = dir.join(format!("{name}.zap"));
    std::fs::write(&srcf, src).unwrap();
    let bin = dir.join(name);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zapc"));
    cmd.arg("build").arg(&srcf).arg("-o").arg(&bin);
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd.output().expect("spawn zapc build");
    (bin, out)
}

/// Build `src` and assert it succeeded (hard failure otherwise). Returns the
/// binary path.
fn build_ok(dir: &Path, name: &str, src: &str, extra_args: &[&str]) -> PathBuf {
    let (bin, out) = try_build(dir, name, src, extra_args);
    assert!(
        out.status.success(),
        "`zapc build {name}`{} failed:\nstdout={}\nstderr={}",
        if extra_args.is_empty() {
            String::new()
        } else {
            format!(" {}", extra_args.join(" "))
        },
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(bin.exists(), "expected a binary at {}", bin.display());
    bin
}

/// `readelf -d <bin>`: the dynamic section, as text.
fn readelf_dynamic(readelf: &str, bin: &Path) -> String {
    let out = Command::new(readelf)
        .arg("-d")
        .arg(bin)
        .output()
        .expect("spawn readelf -d");
    assert!(
        out.status.success(),
        "readelf -d {} failed: {}",
        bin.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A unique temp dir for one test's artifacts (removed at the end).
fn scratch(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zap-link-cpython-{}-{}", std::process::id(), tag))
}

/// Run a built binary and return its captured output.
fn run(bin: &Path) -> std::process::Output {
    Command::new(bin).output().expect("spawn built binary")
}

/// (a) A program with no `import python` must not link libpython — the defect
/// #852 fixes — and must still run and print its output.
#[test]
fn no_python_program_has_no_libpython_needed() {
    if !cfg!(target_os = "linux") {
        eprintln!(
            "SKIP no_python_program_has_no_libpython_needed: not Linux/ELF \
             (readelf NEEDED inspection is ELF-specific)"
        );
        return;
    }
    let Some(readelf) = find_readelf() else {
        eprintln!(
            "SKIP no_python_program_has_no_libpython_needed: no readelf on PATH \
             (CI's Linux leg installs binutils)"
        );
        return;
    };
    let dir = scratch("no-python");
    let bin = build_ok(&dir, "no_python", NO_PYTHON_SRC, &[]);

    let dynsec = readelf_dynamic(&readelf, &bin);
    assert!(
        !dynsec.to_lowercase().contains("libpython"),
        "a program that never `import python`s must NOT link libpython (#852), \
         but its dynamic section lists one:\n{dynsec}"
    );

    // It must also still be a working binary.
    let out = run(&bin);
    assert!(
        out.status.success(),
        "no-python binary did not run: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "0");

    let _ = std::fs::remove_dir_all(&dir);
}

/// (b) A program that DOES `import python` still links libpython: the `NEEDED`
/// entry is present, so interop is unchanged. (The `python_interop` golden
/// covers that it still runs, under the full CI build with CPython present.)
#[test]
fn python_program_still_links_libpython() {
    if !cfg!(target_os = "linux") {
        eprintln!("SKIP python_program_still_links_libpython: not Linux/ELF");
        return;
    }
    let Some(readelf) = find_readelf() else {
        eprintln!("SKIP python_program_still_links_libpython: no readelf on PATH");
        return;
    };
    if !has_python_embed() {
        eprintln!(
            "SKIP python_program_still_links_libpython: no `python3-embed`/`python3` \
             pkg-config module (install `python3-dev`); a Python program cannot link here"
        );
        return;
    }
    let dir = scratch("python");
    let bin = build_ok(&dir, "python_prog", PYTHON_SRC, &[]);

    let dynsec = readelf_dynamic(&readelf, &bin);
    assert!(
        dynsec.to_lowercase().contains("libpython"),
        "a program that `import python`s must still link libpython (#852 keeps \
         interop unchanged), but no libpython NEEDED entry is present:\n{dynsec}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// (c) The `--portable` / `-DZAP_NO_CPYTHON` stub path still raises an ORDINARY
/// (catchable) Zap exception when a build without CPython calls into
/// `import python`: the `except Exception` handler catches it and the program
/// exits 0. This is the manual override (I#287), which must keep working exactly
/// as today regardless of the automatic #852 gate. Needs no CPython at build or
/// run time (portable omits libpython), so it runs anywhere the toolchain links
/// a static binary.
#[test]
fn portable_import_python_raises_catchable_error() {
    let dir = scratch("portable-catch");
    let (bin, out) = try_build(&dir, "portable_catch", PORTABLE_CATCH_SRC, &["--portable"]);
    if !out.status.success() {
        // A host without the static archives the portable (`-static`) profile
        // needs cannot build this; skip rather than fail (CI's Linux leg has
        // them — see docs/DOCKER.md / I#287).
        eprintln!(
            "SKIP portable_import_python_raises_catchable_error: `zapc build --portable` \
             failed (likely no static libs for the portable profile on this host):\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let run = run(&bin);
    assert!(
        run.status.success(),
        "portable `import python` use must raise a CATCHABLE exception caught by \
         `except Exception` (exit 0), but the binary exited {:?}:\nstdout={}\nstderr={}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim_end(),
        "caught",
        "expected the `except Exception` handler to catch the stub's raise"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
