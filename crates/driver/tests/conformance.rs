//! Spec-conformance harness (I#376, epic #374).
//!
//! The golden suite (`golden.rs`) and the CPython differential harness
//! (`diff_cpython.rs`) each test something real but partial: goldens pin the
//! happy path the feature's author implemented, and the diff harness pins
//! Zap-vs-CPython *parity* over the shared subset. Neither one maps the
//! **normative claims of [`SPEC.md`]** to executable checks, so a feature can
//! drift from the spec's stated behavior — including the places Zap
//! *deliberately* diverges from Python — without any test noticing until a
//! downstream package trips on it (epic #374).
//!
//! This harness closes that gap. Each case under `<repo>/tests/conformance/` is
//! a real Zap program that pins one or more **SPEC.md claims**, cited by a
//! stable claim id in its leading `# claim:` directive. The claim ids are
//! catalogued in [`tests/conformance/CLAIMS.md`] — the "claim index" — which
//! maps every extracted normative claim to either a conformance case
//! (`covered`) or a `TODO`, so coverage is measurable and can grow incrementally
//! (most of SPEC.md starts as `TODO`; that is expected — see the README).
//!
//! Unlike the diff harness, a conformance case pins Zap's OWN specified behavior
//! (an `.expected` file), so it also pins the **intentional** divergences from
//! Python — `and`/`or` yielding a strict `bool` (D-0022), per-call default
//! arguments (D-0038), wrapping int64 (D-0005/D-0019) — as deliberate, not as
//! accidents waiting to regress.
//!
//! # What runs where
//!
//! * The index/backbone checks ([`fixtures_are_paired`],
//!   [`claims_index_is_consistent`], [`every_fixture_claim_is_in_the_index`],
//!   [`every_covered_claim_points_at_its_case`]) are pure Rust and run on every
//!   host (`cargo test`, no LLVM), so the suite's structure is always enforced.
//! * The end-to-end check ([`conformance_programs_match`]) compiles and runs
//!   each program, so it needs the `codegen` feature and an LLVM toolchain. It
//!   rides the existing `cargo test --workspace --features zapc/gc-immix` /
//!   `gc-boehm` steps in CI's `full` and `arm64` jobs, exactly like the golden
//!   suite and the diff harness — no separate CI job.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// `<repo>/tests/conformance` — this crate lives at `<repo>/crates/driver`.
fn conformance_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/driver has a two-level parent")
        .join("tests")
        .join("conformance")
}

fn claims_index_path() -> PathBuf {
    conformance_dir().join("CLAIMS.md")
}

fn zap_fixtures() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(conformance_dir()).expect("tests/conformance should exist") {
        let p = entry.expect("dir entry").path();
        if p.extension().is_some_and(|e| e == "zap") {
            out.push(p);
        }
    }
    out.sort();
    out
}

fn fixture_stem(p: &std::path::Path) -> String {
    p.file_stem().unwrap().to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Claim directives and the claim index
// ---------------------------------------------------------------------------

/// A claim id is `AREA-N`: an uppercase area tag, a hyphen, and a number
/// (`TRUTH-1`, `NUM-4`, `DICT-2`). Kept deliberately simple so both the fixture
/// directive and the `CLAIMS.md` table can be parsed by eye and by this harness.
fn is_claim_id(s: &str) -> bool {
    let Some((area, num)) = s.split_once('-') else {
        return false;
    };
    !area.is_empty()
        && area
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && area.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
}

/// The claim ids a fixture pins, from its leading `# claim: ID[, ID...]`
/// directive. The directive must appear in the leading comment block (before the
/// first non-comment line). Every fixture must declare at least one.
fn fixture_claims(src: &str) -> Vec<String> {
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix('#') {
            if let Some(ids) = rest.trim().strip_prefix("claim:") {
                return ids
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            continue; // other comment lines: keep scanning
        }
        break; // first code line: directive must have come before it
    }
    Vec::new()
}

/// One row of the `CLAIMS.md` index table.
#[derive(Debug, Clone)]
struct ClaimRow {
    status: String, // "covered" | "todo"
    case: String,   // fixture stem for a covered row; "—"/empty for a TODO
}

/// Parse the machine-readable claim table out of `CLAIMS.md`. A data row is a
/// Markdown table row whose first cell is a claim id; the columns are
/// `| id | section | claim text | status | case |`. Header and separator rows
/// (whose first cell is not a claim id) are skipped, so prose around the table
/// is harmless.
fn parse_claims_index(md: &str) -> BTreeMap<String, ClaimRow> {
    let mut out = BTreeMap::new();
    for line in md.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            continue;
        }
        let cells: Vec<String> = t
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().to_string())
            .collect();
        if cells.len() < 5 {
            continue;
        }
        let id = &cells[0];
        if !is_claim_id(id) {
            continue;
        }
        let status = cells[3].to_ascii_lowercase();
        let case = cells[4].clone();
        assert!(
            status == "covered" || status == "todo",
            "claim {id} has unknown status {:?} (want `covered` or `TODO`)",
            cells[3]
        );
        assert!(
            out.insert(id.clone(), ClaimRow { status, case },).is_none(),
            "duplicate claim id {id} in CLAIMS.md"
        );
    }
    out
}

fn load_claims_index() -> BTreeMap<String, ClaimRow> {
    let md = std::fs::read_to_string(claims_index_path()).expect("CLAIMS.md should exist");
    parse_claims_index(&md)
}

// ---------------------------------------------------------------------------
// Host-side structural checks (no LLVM) — always run
// ---------------------------------------------------------------------------

/// Every `.zap` fixture has a matching `.expected` file.
#[test]
fn fixtures_are_paired() {
    let fixtures = zap_fixtures();
    assert!(
        !fixtures.is_empty(),
        "expected conformance fixtures in {}",
        conformance_dir().display()
    );
    for zap in fixtures {
        assert!(
            zap.with_extension("expected").exists(),
            "missing `.expected` for {}",
            zap.display()
        );
    }
}

/// The claim index parses, has no duplicate ids, and every `covered` row names a
/// real fixture pair. Reports the covered/TODO split (the coverage metric).
#[test]
fn claims_index_is_consistent() {
    let index = load_claims_index();
    assert!(!index.is_empty(), "CLAIMS.md defined no claims");

    let mut covered = 0usize;
    let mut todo = 0usize;
    for (id, row) in &index {
        match row.status.as_str() {
            "covered" => {
                covered += 1;
                assert!(
                    !row.case.is_empty() && row.case != "—",
                    "covered claim {id} must name a case in CLAIMS.md"
                );
                let dir = conformance_dir();
                assert!(
                    dir.join(format!("{}.zap", row.case)).exists(),
                    "covered claim {id} names case {:?} but {}.zap is missing",
                    row.case,
                    row.case
                );
                assert!(
                    dir.join(format!("{}.expected", row.case)).exists(),
                    "covered claim {id} names case {:?} but {}.expected is missing",
                    row.case,
                    row.case
                );
            }
            _ => todo += 1,
        }
    }
    eprintln!(
        "conformance claims: {} total — {covered} covered, {todo} TODO",
        index.len()
    );
}

/// Every claim a fixture declares exists in the index, is marked `covered`, and
/// names that fixture. This keeps a fixture from citing a claim the index does
/// not track, or one parked as `TODO`.
#[test]
fn every_fixture_claim_is_in_the_index() {
    let index = load_claims_index();
    for zap in zap_fixtures() {
        let stem = fixture_stem(&zap);
        let src = std::fs::read_to_string(&zap).expect("read fixture");
        let claims = fixture_claims(&src);
        assert!(
            !claims.is_empty(),
            "{} has no `# claim:` directive (cite the SPEC.md claim id it pins)",
            zap.display()
        );
        for id in claims {
            let row = index
                .get(&id)
                .unwrap_or_else(|| panic!("{stem} cites claim {id}, absent from CLAIMS.md"));
            assert_eq!(
                row.status, "covered",
                "{stem} cites claim {id}, but CLAIMS.md marks it {}",
                row.status
            );
            assert_eq!(
                row.case, stem,
                "{stem} cites claim {id}, but CLAIMS.md points it at case {:?}",
                row.case
            );
        }
    }
}

/// The reverse direction: every `covered` claim's named case exists and actually
/// declares that claim. Together with the check above this makes the index and
/// the fixtures' directives mirror each other exactly.
#[test]
fn every_covered_claim_points_at_its_case() {
    let index = load_claims_index();
    for (id, row) in &index {
        if row.status != "covered" {
            continue;
        }
        let zap = conformance_dir().join(format!("{}.zap", row.case));
        let src = std::fs::read_to_string(&zap)
            .unwrap_or_else(|_| panic!("covered claim {id}: cannot read {}", zap.display()));
        let declared = fixture_claims(&src);
        assert!(
            declared.iter().any(|d| d == id),
            "CLAIMS.md maps {id} to case {:?}, but that fixture's `# claim:` does not list it",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// End-to-end conformance run (codegen + LLVM required)
// ---------------------------------------------------------------------------

/// Run `cmd` with a per-case timeout, draining stdout/stderr on threads so the
/// child never blocks on a full pipe and a hung program fails fast rather than
/// stalling the whole CI job. Mirrors the golden harness helper.
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
                    panic!("conformance {label} did not finish within {timeout:?} (hang?)");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    };
    std::process::Output {
        status,
        stdout: out_t.join().unwrap_or_default(),
        stderr: err_t.join().unwrap_or_default(),
    }
}

/// Compile + run each conformance program and check its stdout, exit code
/// (optional `.exit` sidecar, default 0) and, when present, an expected-stderr
/// substring (`.stderr` sidecar) — the same contract as the golden suite, so a
/// conformance case is a golden that additionally cites the SPEC.md claim it
/// pins. Requires the `codegen` feature (LLVM).
///
/// Every case is run and every mismatch is collected before the test fails, so
/// one CI run reports the complete set of divergences (not just the first) —
/// important because the codegen path only runs in CI, not on the typical
/// no-LLVM dev host.
#[cfg(feature = "codegen")]
#[test]
fn conformance_programs_match() {
    use std::fmt::Write as _;
    use std::process::Command;
    let per_test = std::time::Duration::from_secs(60);
    let bin = env!("CARGO_BIN_EXE_zapc");
    let mut ran = 0usize;
    let mut failures = String::new();

    for zap in zap_fixtures() {
        let name = fixture_stem(&zap);
        let expected = std::fs::read_to_string(zap.with_extension("expected"))
            .expect("expected output present");
        let expected_exit: i32 = std::fs::read_to_string(zap.with_extension("exit"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let expected_stderr = std::fs::read_to_string(zap.with_extension("stderr")).ok();

        let mut cmd = Command::new(bin);
        cmd.arg("run").arg(&zap);
        let output = run_with_timeout(cmd, per_test, &zap.display().to_string());

        let got = String::from_utf8_lossy(&output.stdout);
        let got_err = String::from_utf8_lossy(&output.stderr);
        if output.status.code() != Some(expected_exit) {
            let _ = write!(
                failures,
                "\n[{name}] exit code: want {expected_exit}, got {:?}\n  stderr: {got_err}",
                output.status.code()
            );
        }
        if got.trim_end() != expected.trim_end() {
            let _ = write!(
                failures,
                "\n[{name}] stdout mismatch:\n  want: {:?}\n  got:  {:?}\n  stderr: {got_err}",
                expected.trim_end(),
                got.trim_end()
            );
        }
        if let Some(want_err) = expected_stderr {
            let want_err = want_err.trim_end();
            if !got_err.contains(want_err) {
                let _ = write!(
                    failures,
                    "\n[{name}] stderr should contain {want_err:?}, got:\n{got_err}"
                );
            }
        }
        ran += 1;
    }
    eprintln!("conformance: ran {ran} programs end-to-end");
    assert!(failures.is_empty(), "conformance mismatches:{failures}");
}

// ---------------------------------------------------------------------------
// Host-side unit tests for the parsers themselves
// ---------------------------------------------------------------------------

#[test]
fn claim_id_shape() {
    for ok in ["TRUTH-1", "NUM-4", "DICT-2", "CMP-10", "A1-2"] {
        assert!(is_claim_id(ok), "{ok} should be a claim id");
    }
    for bad in [
        "truth-1", "TRUTH", "TRUTH-", "-1", "TRUTH-1a", "TR UTH-1", "",
    ] {
        assert!(!is_claim_id(bad), "{bad} should NOT be a claim id");
    }
}

#[test]
fn fixture_claim_directive_parsing() {
    assert_eq!(fixture_claims("# claim: TRUTH-1\ndef main():"), ["TRUTH-1"]);
    assert_eq!(
        fixture_claims("# claim: TUP-1, TUP-2, TUP-3\n# more\ndef main():"),
        ["TUP-1", "TUP-2", "TUP-3"]
    );
    // A directive below another comment line is still found.
    assert_eq!(
        fixture_claims("# header\n# claim: NUM-3\ndef main():"),
        ["NUM-3"]
    );
    // No directive -> empty (the harness turns this into a failure per fixture).
    assert!(fixture_claims("def main():\n    pass").is_empty());
    // A `# claim:` that appears only after code does not count.
    assert!(fixture_claims("def main():\n# claim: X-1").is_empty());
}

#[test]
fn claims_table_parsing() {
    let md = "\
# Claims

| Claim | Section | Statement | Status | Case |
|---|---|---|---|---|
| TRUTH-1 | §5 | zero is falsy | covered | truthiness_numbers |
| STRUCT-1 | §4.4 | structs have methods | TODO | — |

prose after the table is ignored | not | a | row | here
";
    let index = parse_claims_index(md);
    assert_eq!(index.len(), 2);
    assert_eq!(index["TRUTH-1"].status, "covered");
    assert_eq!(index["TRUTH-1"].case, "truthiness_numbers");
    assert_eq!(index["STRUCT-1"].status, "todo");
    assert_eq!(index["STRUCT-1"].case, "—");
}
