//! Conditional optional-backend link assertions (#853).
//!
//! WHY THIS EXISTS. Every compiled Zap binary used to eagerly hard-link the
//! optional C backends used by parts of the stdlib — PCRE2 (`libpcre2-8`), zlib
//! (`libz`), and OpenSSL (`libssl`/`libcrypto`) — as `NEEDED` dynamic
//! dependencies even when the program used none of regex, compression, or
//! crypto/TLS/https. `ld.so` mapped and relocated all of them at every process
//! start, and the binary refused to run without them present (#853; the sibling
//! #852 did the same for libpython). The compiler knows the whole program at
//! build time, so `build_executable` now links each backend's library ONLY when
//! the lowered program references one of that backend's runtime seam symbols:
//!   * `libpcre2-8`  ⟺ a `zap_regex_*` call,
//!   * `libz`        ⟺ a `zap_zlib_*` / `zap_gzip_*` call,
//!   * `libssl`/`libcrypto` ⟺ a `zap_tls_*`, `zap_http_*`, or OpenSSL-backed
//!     `zap_crypto_*` call — but NOT the self-contained `zap_crypto_sha*` /
//!     `md5` / `hmac_*` / CSPRNG family, which is pure C.
//!
//! Otherwise that backend's `zaprt.c` code is compiled out behind
//! `-DZAP_NO_PCRE2` / `-DZAP_NO_ZLIB` / `-DZAP_NO_OPENSSL` and no library links.
//!
//! These tests are the walled garden around that gate (AGENTS.md: exhaustive,
//! intent-documenting). Via `readelf -d` they assert the `NEEDED` set is exactly
//! what each program's seam usage implies — in particular:
//!   (a) a program that touches NONE of the backends links none of them (the
//!       defect this issue fixes), and still runs;
//!   (b) each backend is present when — and only when — its seam is reached, so
//!       a regex program does not drag in zlib/OpenSSL, etc.;
//!   (c) a program that only uses the SELF-CONTAINED hashes (`crypto.sha256`)
//!       links no OpenSSL — guarding the exclusion set against regressions;
//!   (d) OpenSSL-backed crypto (`crypto.blake2b`), TLS (`tls.connect`), and the
//!       http client (`http.get`, conservatively — its inline `https://` path
//!       embeds OpenSSL) each DO link `libssl`/`libcrypto`.
//!
//! The assertions are ELF/Linux-specific and need `readelf`; on a non-Linux host
//! or without `readelf` they SKIP (macOS dev hosts emit Mach-O and have no
//! `readelf`). A case whose `-dev` package is missing on the host SKIPS that
//! case rather than failing. CI's Linux leg has binutils + the `-dev` packages,
//! so the gate is exercised there.

#![cfg(feature = "codegen")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A program touching no optional backend.
const NO_BACKEND_SRC: &str = "\
def main():
    print(0)
";

/// Regex (PCRE2) only.
const REGEX_SRC: &str = "\
def main():
    auto m = \"abc123\" =~ m/[a-z]+/
    if m:
        print(m.group(0))
";

/// zlib/gzip only.
const ZLIB_SRC: &str = "\
def main():
    auto c = zlib.compress(\"hello world\")
    print(len(zlib.decompress(c)))
";

/// Self-contained hashing only (`crypto.sha256` is pure C) — must NOT pull
/// OpenSSL. This is the regression guard for the OpenSSL exclusion set.
const CRYPTO_SELFCONTAINED_SRC: &str = "\
def main():
    print(len(crypto.sha256(\"x\")))
";

/// OpenSSL-backed crypto (`crypto.blake2b` routes through OpenSSL EVP).
const CRYPTO_OPENSSL_SRC: &str = "\
def main():
    print(len(crypto.blake2b(\"x\")))
";

/// TLS client — references the `zap_tls_*` seam (OpenSSL). Built and inspected,
/// not run (it would open a network connection).
const TLS_SRC: &str = "\
def main():
    auto c = tls.connect(\"localhost\", 443)
    print(0)
";

/// http client — references the `zap_http_*` seam; the http client embeds the
/// `https://` path inline, so any http use is treated as needing OpenSSL
/// (conservative, #853). Built and inspected, not run.
const HTTP_SRC: &str = "\
def main():
    auto r = http.get(\"http://localhost/\")
    print(0)
";

/// The optional libraries this gate governs, by the `readelf` `NEEDED` token
/// prefix. `libpython` is the sibling #852 gate, asserted absent from a
/// no-backend program here too (it must never come back).
const OPTIONAL_LIBS: &[&str] = &["libpython", "libpcre2", "libz.so", "libssl", "libcrypto"];

/// Locate `readelf`. `None` => skip the ELF-inspection tests.
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

/// Build `src` to a binary; returns `(path, output)` so the caller decides
/// whether a failure is a skip (missing `-dev` package) or a hard failure.
fn try_build(dir: &Path, name: &str, src: &str) -> (PathBuf, std::process::Output) {
    std::fs::create_dir_all(dir).unwrap();
    let srcf = dir.join(format!("{name}.zap"));
    std::fs::write(&srcf, src).unwrap();
    let bin = dir.join(name);
    let out = Command::new(env!("CARGO_BIN_EXE_zapc"))
        .arg("build")
        .arg(&srcf)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("spawn zapc build");
    (bin, out)
}

/// The set of governed optional libs present in `bin`'s `NEEDED` entries.
fn needed_optional_libs(readelf: &str, bin: &Path) -> BTreeSet<String> {
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
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    OPTIONAL_LIBS
        .iter()
        .copied()
        .filter(|lib| {
            // Only count a hit on a NEEDED line (not a RUNPATH/comment).
            text.lines()
                .any(|l| l.contains("needed") && l.contains(*lib))
        })
        .map(|lib| lib.to_string())
        .collect()
}

fn scratch(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zap-link-opt-{}-{}", std::process::id(), tag))
}

/// Shared skip guards; returns the `readelf` tool or `None` (already logged).
fn readelf_or_skip(test: &str) -> Option<String> {
    if !cfg!(target_os = "linux") {
        eprintln!("SKIP {test}: not Linux/ELF (readelf NEEDED inspection is ELF-specific)");
        return None;
    }
    match find_readelf() {
        Some(r) => Some(r),
        None => {
            eprintln!("SKIP {test}: no readelf on PATH (CI's Linux leg installs binutils)");
            None
        }
    }
}

/// Build `src`, returning the governed-optional-lib `NEEDED` set, or `None` to
/// SKIP (the build failed for want of a `-dev` package / static lib on this
/// host — a backend program cannot link there by design).
fn needed_or_skip(readelf: &str, test: &str, tag: &str, src: &str) -> Option<BTreeSet<String>> {
    let dir = scratch(tag);
    let (bin, out) = try_build(&dir, tag, src);
    if !out.status.success() {
        eprintln!(
            "SKIP {test} [{tag}]: `zapc build` failed (likely a missing backend `-dev` package \
             on this host):\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    }
    let libs = needed_optional_libs(readelf, &bin);
    let _ = std::fs::remove_dir_all(&dir);
    Some(libs)
}

/// (a) A program touching no optional backend links NONE of them — the defect
/// #853 (+ sibling #852) fixes — and still runs.
#[test]
fn no_backend_program_links_no_optional_libs() {
    let Some(readelf) = readelf_or_skip("no_backend_program_links_no_optional_libs") else {
        return;
    };
    let dir = scratch("none");
    let (bin, out) = try_build(&dir, "none", NO_BACKEND_SRC);
    assert!(
        out.status.success(),
        "no-backend build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let libs = needed_optional_libs(&readelf, &bin);
    assert!(
        libs.is_empty(),
        "a program using no regex/zlib/crypto/tls/http/python must link NONE of \
         {OPTIONAL_LIBS:?}, but its NEEDED set has: {libs:?}"
    );
    let run = Command::new(&bin).output().expect("run no-backend binary");
    assert!(run.status.success(), "no-backend binary did not run");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim_end(), "0");
    let _ = std::fs::remove_dir_all(&dir);
}

/// (b) Regex links ONLY libpcre2 — not zlib/OpenSSL.
#[test]
fn regex_links_only_pcre2() {
    let Some(readelf) = readelf_or_skip("regex_links_only_pcre2") else {
        return;
    };
    let Some(libs) = needed_or_skip(&readelf, "regex_links_only_pcre2", "regex", REGEX_SRC) else {
        return;
    };
    assert!(
        libs.contains("libpcre2"),
        "a regex program must link libpcre2; NEEDED: {libs:?}"
    );
    for forbidden in ["libz.so", "libssl", "libcrypto", "libpython"] {
        assert!(
            !libs.contains(forbidden),
            "a regex-only program must NOT link {forbidden}; NEEDED: {libs:?}"
        );
    }
}

/// (b) zlib links ONLY libz — not pcre2/OpenSSL.
#[test]
fn zlib_links_only_libz() {
    let Some(readelf) = readelf_or_skip("zlib_links_only_libz") else {
        return;
    };
    let Some(libs) = needed_or_skip(&readelf, "zlib_links_only_libz", "zlib", ZLIB_SRC) else {
        return;
    };
    assert!(
        libs.contains("libz.so"),
        "a zlib program must link libz; NEEDED: {libs:?}"
    );
    for forbidden in ["libpcre2", "libssl", "libcrypto", "libpython"] {
        assert!(
            !libs.contains(forbidden),
            "a zlib-only program must NOT link {forbidden}; NEEDED: {libs:?}"
        );
    }
}

/// (c) Self-contained hashing (`crypto.sha256`) links NO OpenSSL — the exclusion
/// set regression guard.
#[test]
fn self_contained_crypto_links_no_openssl() {
    let Some(readelf) = readelf_or_skip("self_contained_crypto_links_no_openssl") else {
        return;
    };
    let Some(libs) = needed_or_skip(
        &readelf,
        "self_contained_crypto_links_no_openssl",
        "sha256",
        CRYPTO_SELFCONTAINED_SRC,
    ) else {
        return;
    };
    for forbidden in ["libssl", "libcrypto"] {
        assert!(
            !libs.contains(forbidden),
            "the self-contained `crypto.sha256` is pure C and must NOT link {forbidden}; \
             NEEDED: {libs:?}"
        );
    }
}

/// (d) OpenSSL-backed crypto (`crypto.blake2b`) links libssl+libcrypto, and runs.
#[test]
fn openssl_crypto_links_openssl() {
    let Some(readelf) = readelf_or_skip("openssl_crypto_links_openssl") else {
        return;
    };
    let dir = scratch("blake2b");
    let (bin, out) = try_build(&dir, "blake2b", CRYPTO_OPENSSL_SRC);
    if !out.status.success() {
        eprintln!(
            "SKIP openssl_crypto_links_openssl: build failed (missing libssl-dev?):\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let libs = needed_optional_libs(&readelf, &bin);
    for needed in ["libssl", "libcrypto"] {
        assert!(
            libs.contains(needed),
            "an OpenSSL-backed crypto program must link {needed}; NEEDED: {libs:?}"
        );
    }
    let run = Command::new(&bin).output().expect("run blake2b binary");
    assert!(
        run.status.success(),
        "OpenSSL crypto binary did not run: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// (d) TLS and the http client each link libssl+libcrypto (built + inspected,
/// not run — they would open a network connection).
#[test]
fn tls_and_http_link_openssl() {
    let Some(readelf) = readelf_or_skip("tls_and_http_link_openssl") else {
        return;
    };
    for (tag, src) in [("tls", TLS_SRC), ("http", HTTP_SRC)] {
        let Some(libs) = needed_or_skip(&readelf, "tls_and_http_link_openssl", tag, src) else {
            continue;
        };
        for needed in ["libssl", "libcrypto"] {
            assert!(
                libs.contains(needed),
                "the {tag} program must link {needed}; NEEDED: {libs:?}"
            );
        }
    }
}
