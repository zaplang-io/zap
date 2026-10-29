//! CPython differential-testing harness (I#375, epic #374).
//!
//! Zap aims to be "Python-readable": over the subset the two languages share,
//! Zap intends CPython's exact runtime semantics. CPython is therefore a free,
//! authoritative oracle — we run the same program under `zapc run` and under
//! `python3` and diff stdout + exit status. Any difference is either a bug we
//! want to catch the day it lands, or a *deliberate* divergence we want pinned
//! down and documented rather than silently hidden.
//!
//! # What runs where
//!
//! * The pure-Rust pieces — the Zap→Python source transform ([`zap_to_python`])
//!   and the seeded program generator ([`gen::Generator`]) — are unit-tested on
//!   every host (`cargo test`, no LLVM), so a regression in the harness itself
//!   fails fast and cheap.
//! * The end-to-end diff tests ([`curated_corpus_matches_cpython`],
//!   [`random_programs_match_cpython`]) compile and run real programs, so they
//!   require the `codegen` feature and an LLVM toolchain *and* a `python3` on
//!   PATH. They ride the existing `cargo test --workspace --features
//!   zapc/gc-immix` / `gc-boehm` steps in CI's `full` and `arm64` jobs (where
//!   `python3` is installed) exactly like the golden suite — no separate LLVM
//!   build. See `tests/diff/README.md`.
//!
//! # The shared subset and the transform
//!
//! Expression and statement *semantics* are shared widely; the surface syntax
//! differs for **declarations** (`auto x = …`, `int64 x = …` vs Python's bare
//! `x = …`), **function signatures** (type-first params, `-> ret`) and **program
//! entry** (Zap auto-runs `main`, Python does not). [`zap_to_python`] is a small,
//! auditable, line-oriented transform that bridges exactly those three gaps and
//! passes everything else through unchanged. The corpus is written as real Zap
//! (one file per case — the thing we actually ship and test); the CPython side
//! is derived from it, so adding a case is dropping in one `.zap` file.

use std::path::PathBuf;
#[cfg(feature = "codegen")]
use std::process::Command;
#[cfg(feature = "codegen")]
use std::time::Duration;

// ---------------------------------------------------------------------------
// Zap -> CPython source transform
// ---------------------------------------------------------------------------

/// Translate a Zap program into the equivalent CPython 3 program.
///
/// The transform is deliberately minimal and line-oriented so it stays
/// auditable (a bug in the translator would manufacture false divergences):
///
/// 1. **Declarations** — a leading `const`/`auto`/type prefix is stripped, so
///    `auto x = 0`, `int64 x = 0`, `list[int64] xs = []` and
///    `const f64 PI = 3.14` all become plain Python assignments. Reassignments
///    (`x = 1`, `x, y = (y, x)`, `x += 1`) already match Python and pass through.
/// 2. **Function signatures** — `def f(int64 a, str b = "x") -> int64:` becomes
///    `def f(a, b="x"):`; a variadic `int64... xs` becomes `*xs`; the `-> ret`
///    annotation is dropped.
/// 3. **Entry point** — if the program defines a top-level `main`, a trailing
///    `main()` call is appended so CPython runs it, mirroring Zap's auto-run.
///
/// Everything else — control flow, `print`, comprehensions, lambdas, operators,
/// f-strings, `try`/`except`/`finally` — is already valid Python and is emitted
/// verbatim.
pub fn zap_to_python(src: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut defines_main = false;
    for line in src.lines() {
        let indent_len = line.len() - line.trim_start().len();
        let (indent, rest) = line.split_at(indent_len);
        if indent.is_empty() && rest.starts_with("def main(") {
            defines_main = true;
        }
        out.push(translate_line(indent, rest));
    }
    let mut text = out.join("\n");
    if defines_main {
        // Zap auto-invokes `main`; CPython does not, so call it explicitly. The
        // corpus never writes its own top-level `main()` call (that would make
        // Zap run main twice), so appending unconditionally is safe.
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("main()\n");
    }
    text
}

fn translate_line(indent: &str, rest: &str) -> String {
    if rest.is_empty() || rest.starts_with('#') {
        return format!("{indent}{rest}");
    }
    if let Some(sig) = translate_def(rest) {
        return format!("{indent}{sig}");
    }
    if let Some(stripped) = strip_declaration(rest) {
        return format!("{indent}{stripped}");
    }
    format!("{indent}{rest}")
}

/// Length in bytes of a leading identifier (`[A-Za-z_][A-Za-z0-9_]*`), or 0.
fn ident_len(s: &str) -> usize {
    let b = s.as_bytes();
    if b.is_empty() || !(b[0].is_ascii_alphabetic() || b[0] == b'_') {
        return 0;
    }
    let mut i = 1;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    i
}

/// Length of a leading Zap type token: an identifier optionally followed by a
/// balanced `[ … ]` generic group (e.g. `int64`, `list[int64]`,
/// `dict[str, int64]`, `list[fn() -> int64]`). `None` if `s` does not start with
/// an identifier or the brackets are unbalanced.
fn type_token_len(s: &str) -> Option<usize> {
    let id = ident_len(s);
    if id == 0 {
        return None;
    }
    let b = s.as_bytes();
    let mut i = id;
    if i < b.len() && b[i] == b'[' {
        let mut depth = 0i32;
        while i < b.len() {
            match b[i] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    i += 1;
                    if depth == 0 {
                        return Some(i);
                    }
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
        return None; // unbalanced
    }
    Some(id)
}

/// True if `s` is a declaration target list followed by a single `=`:
/// `name =`, `a, b, c =` — but not `==` (comparison) or anything else.
fn decl_targets_then_eq(s: &str) -> bool {
    let mut rest = s.trim_start();
    loop {
        let id = ident_len(rest);
        if id == 0 {
            return false;
        }
        rest = rest[id..].trim_start();
        if let Some(r) = rest.strip_prefix(',') {
            rest = r.trim_start();
            continue;
        }
        return match rest.strip_prefix('=') {
            Some(after) => !after.starts_with('='),
            None => false,
        };
    }
}

/// Strip a leading declaration prefix (`const`? (`auto` | type) target-list `=`),
/// returning the Python assignment. `None` if `rest` is not a declaration.
fn strip_declaration(rest: &str) -> Option<String> {
    let body = rest
        .strip_prefix("const ")
        .map(str::trim_start)
        .unwrap_or(rest);
    let t_len = type_token_len(body)?;
    let after_type = &body[t_len..];
    // A real declaration separates the type token from the name with whitespace.
    if !after_type.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let after = after_type.trim_start();
    if decl_targets_then_eq(after) {
        Some(after.to_string())
    } else {
        None
    }
}

/// Index of the `)` that closes the parameter list, given the slice *after* the
/// opening `(`. Tracks `()[]{}` nesting.
fn matching_paren(inner: &str) -> Option<usize> {
    let b = inner.as_bytes();
    let mut depth = 1i32;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split `s` on `delim` at bracket-nesting depth 0.
fn split_top_level(s: &str, delim: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if c == delim && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// The Python parameter name for a Zap type-first parameter declaration:
/// `int64 n` -> `n`, `list[int64] acc` -> `acc`, `int64... xs` -> `*xs`.
fn param_name(decl: &str) -> String {
    let variadic = decl.contains("...");
    let name = decl.split_whitespace().last().unwrap_or("").trim();
    if variadic {
        format!("*{name}")
    } else {
        name.to_string()
    }
}

fn translate_param(p: &str) -> String {
    let p = p.trim();
    if p.is_empty() {
        return String::new();
    }
    let parts = split_top_level(p, '=');
    if parts.len() >= 2 {
        // `TYPE name = default` -> `name=default`. Param types never contain a
        // top-level `=`, so the first split is the default separator.
        let decl = parts[0].trim();
        let default = parts[1..].join("=");
        format!("{}={}", param_name(decl), default.trim())
    } else {
        param_name(p)
    }
}

/// Rewrite a Zap `def` header to its Python form, or `None` if `rest` is not a
/// function definition.
fn translate_def(rest: &str) -> Option<String> {
    let after_kw = rest.strip_prefix("def ")?;
    let name_len = ident_len(after_kw);
    if name_len == 0 {
        return None;
    }
    let name = &after_kw[..name_len];
    let after_name = after_kw[name_len..].trim_start();
    let inner = after_name.strip_prefix('(')?;
    let close = matching_paren(inner)?;
    let params = &inner[..close];
    let py_params = split_top_level(params, ',')
        .into_iter()
        .map(translate_param)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    // The `-> ret` annotation and the trailing `:` are rebuilt uniformly.
    Some(format!("def {name}({py_params}):"))
}

// ---------------------------------------------------------------------------
// Directive parsing
// ---------------------------------------------------------------------------

/// Expected cross-language outcome for a curated case, declared by a
/// `# diff: …` directive on the program's first line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expect {
    /// Zap and CPython must produce identical stdout and exit status.
    Match,
    /// Zap and CPython are expected to differ; `reason` documents why (a locked
    /// decision like `D-0038`, or a filed `BUG`). If they ever stop differing,
    /// the case fails loudly so it can be promoted to `Match`.
    Diverge { reason: String },
}

/// Parse the `# diff: match` / `# diff: diverge <reason>` directive. Absent
/// directive defaults to `Match`.
fn parse_directive(src: &str) -> Expect {
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix('#') {
            let rest = rest.trim();
            if let Some(d) = rest.strip_prefix("diff:") {
                let d = d.trim();
                if let Some(reason) = d.strip_prefix("diverge") {
                    return Expect::Diverge {
                        reason: reason.trim().to_string(),
                    };
                } else if d.starts_with("match") {
                    return Expect::Match;
                }
            }
            // other comments: keep scanning (the directive may be below them)
            continue;
        }
        break; // first non-comment line: stop looking
    }
    Expect::Match
}

// ---------------------------------------------------------------------------
// Running + diffing (codegen feature only)
// ---------------------------------------------------------------------------

/// Captured result of running a program.
#[cfg(feature = "codegen")]
#[derive(Debug)]
struct Run {
    stdout: String,
    exit: Option<i32>,
    stderr: String,
}

#[cfg(feature = "codegen")]
impl Run {
    fn matches(&self, other: &Run) -> bool {
        self.stdout.trim_end() == other.stdout.trim_end() && self.exit == other.exit
    }
}

/// Run `cmd`, draining stdout/stderr on threads, failing fast on a hang.
#[cfg(feature = "codegen")]
fn run_with_timeout(mut cmd: Command, timeout: Duration, label: &str) -> Run {
    use std::io::Read;
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {label}: {e}"));
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
                    panic!("{label} did not finish within {timeout:?} (hang?)");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    Run {
        stdout: String::from_utf8_lossy(&out_t.join().unwrap_or_default()).into_owned(),
        exit: status.code(),
        stderr: String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned(),
    }
}

/// The `python3` interpreter: `$ZAP_DIFF_PYTHON`, else `python3`. Isolated mode
/// (`-I`) so site/user config and the program's own directory can't perturb it.
#[cfg(feature = "codegen")]
fn python_cmd(file: &std::path::Path) -> Command {
    let exe = std::env::var("ZAP_DIFF_PYTHON").unwrap_or_else(|_| "python3".to_string());
    let mut c = Command::new(exe);
    c.arg("-I").arg(file);
    c
}

#[cfg(feature = "codegen")]
fn run_zap(zap_file: &std::path::Path) -> Run {
    let bin = env!("CARGO_BIN_EXE_zapc");
    let mut c = Command::new(bin);
    c.arg("run").arg(zap_file);
    run_with_timeout(
        c,
        Duration::from_secs(60),
        &format!("zap {}", zap_file.display()),
    )
}

#[cfg(feature = "codegen")]
fn run_python(py_file: &std::path::Path) -> Run {
    run_with_timeout(
        python_cmd(py_file),
        Duration::from_secs(60),
        &format!("python3 {}", py_file.display()),
    )
}

/// A minimal, dependency-free unified-diff-style line report.
#[cfg(feature = "codegen")]
fn line_diff(expected: &str, got: &str) -> String {
    let a: Vec<&str> = expected.lines().collect();
    let b: Vec<&str> = got.lines().collect();
    let mut s = String::new();
    let n = a.len().max(b.len());
    for i in 0..n {
        match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) if x == y => s.push_str(&format!("  {x}\n")),
            (Some(x), Some(y)) => {
                s.push_str(&format!("- {x}\n+ {y}\n"));
            }
            (Some(x), None) => s.push_str(&format!("- {x}\n")),
            (None, Some(y)) => s.push_str(&format!("+ {y}\n")),
            (None, None) => {}
        }
    }
    s
}

#[cfg(feature = "codegen")]
fn report(kind: &str, name: &str, src: &str, py: &str, cpython: &Run, zap: &Run) -> String {
    format!(
        "\n================ {kind}: {name} ================\n\
         ---- zap source ----\n{src}\n\
         ---- derived python ----\n{py}\n\
         ---- cpython: exit={:?} ----\n{}\n\
         ---- zap:     exit={:?} ----\n{}\n\
         ---- stdout diff (- cpython, + zap) ----\n{}\
         ---- zap stderr ----\n{}\n",
        cpython.exit,
        cpython.stdout,
        zap.exit,
        zap.stdout,
        line_diff(&cpython.stdout, &zap.stdout),
        zap.stderr,
    )
}

// ---------------------------------------------------------------------------
// Corpus locations
// ---------------------------------------------------------------------------

/// `<repo>/tests/diff` — this crate lives at `<repo>/crates/driver`.
fn diff_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/driver has a two-level parent")
        .join("tests")
        .join("diff")
}

fn curated_fixtures() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(diff_dir()).expect("tests/diff should exist") {
        let p = entry.expect("dir entry").path();
        if p.extension().is_some_and(|e| e == "zap") {
            out.push(p);
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Host-side tests (no LLVM): translator, directives, generator
// ---------------------------------------------------------------------------

#[test]
fn translator_strips_declarations() {
    assert_eq!(zap_to_python("auto x = 0"), "x = 0");
    assert_eq!(zap_to_python("int64 count = 0"), "count = 0");
    assert_eq!(
        zap_to_python("list[int64] xs = [1, 2, 3]"),
        "xs = [1, 2, 3]"
    );
    assert_eq!(zap_to_python("dict[str, int64] d = {}"), "d = {}");
    assert_eq!(zap_to_python("const f64 PI = 3.14"), "PI = 3.14");
    assert_eq!(
        zap_to_python("auto a, b, c = (1, 2, 3)"),
        "a, b, c = (1, 2, 3)"
    );
    assert_eq!(
        zap_to_python("auto inc = lambda x: x + 1"),
        "inc = lambda x: x + 1"
    );
    // inline comment preserved
    assert_eq!(zap_to_python("int64 x = 5  # note"), "x = 5  # note");
}

#[test]
fn translator_leaves_statements_and_reassignment_alone() {
    for line in [
        "x = 1",
        "x, y = (y, x)",
        "x += 1",
        "count = count + 1",
        "print(x)",
        "return fib(n - 1) + fib(n - 2)",
        "for i in range(0, 10):",
        "if x > 0:",
        "while n > 0:",
        "    acc.append(x)",
        "x == y",
    ] {
        assert_eq!(
            zap_to_python(line),
            line,
            "line should be unchanged: {line:?}"
        );
    }
}

#[test]
fn translator_rewrites_def_signatures() {
    assert_eq!(zap_to_python("def fib(int64 n) -> int64:"), "def fib(n):");
    assert_eq!(
        zap_to_python("def greet(str name) -> str:"),
        "def greet(name):"
    );
    assert_eq!(zap_to_python("def setup():"), "def setup():");
    assert_eq!(
        zap_to_python("def app(int64 x, list[int64] acc = []) -> int64:"),
        "def app(x, acc=[]):"
    );
    assert_eq!(
        zap_to_python("def connect(str host, int64 port = 8080, bool tls = False) -> Conn:"),
        "def connect(host, port=8080, tls=False):"
    );
    assert_eq!(
        zap_to_python("def sum_all(int64... nums) -> int64:"),
        "def sum_all(*nums):"
    );
    assert_eq!(
        zap_to_python("def make_adder(int64 n) -> fn(int64) -> int64:"),
        "def make_adder(n):"
    );
}

#[test]
fn translator_appends_main_call() {
    let src = "def main():\n    print(1)\n";
    assert_eq!(zap_to_python(src), "def main():\n    print(1)\nmain()\n");
    // A program with no main is left without a trailing call.
    let src2 = "def helper() -> int64:\n    return 1\n";
    assert!(!zap_to_python(src2).contains("\nmain()"));
}

#[test]
fn directive_parsing() {
    assert_eq!(parse_directive("# diff: match\ndef main():"), Expect::Match);
    assert_eq!(parse_directive("def main():"), Expect::Match); // default
    match parse_directive("# diff: diverge D-0038: defaults per call\ndef main():") {
        Expect::Diverge { reason } => assert!(reason.starts_with("D-0038:")),
        _ => panic!("expected diverge"),
    }
}

#[test]
fn every_curated_fixture_has_a_valid_directive_and_translates() {
    let fixtures = curated_fixtures();
    assert!(
        !fixtures.is_empty(),
        "expected curated fixtures in {}",
        diff_dir().display()
    );
    for f in fixtures {
        let src = std::fs::read_to_string(&f).expect("read fixture");
        // Directive parses to one of the two kinds (default Match is fine).
        let _ = parse_directive(&src);
        // Translation is total and non-empty for every fixture.
        let py = zap_to_python(&src);
        assert!(
            !py.trim().is_empty(),
            "translation produced nothing for {}",
            f.display()
        );
        // Every curated program defines `main`, so the python side must call it.
        assert!(
            py.contains("\nmain()"),
            "{} defines no top-level main (corpus convention)",
            f.display()
        );
    }
}

#[test]
fn generator_is_deterministic_and_bounded() {
    let a = gen::Generator::new(12345).programs(20);
    let b = gen::Generator::new(12345).programs(20);
    assert_eq!(a, b, "same seed must yield identical programs");
    let c = gen::Generator::new(999).programs(20);
    assert_ne!(a, c, "different seeds should (almost surely) differ");
    for p in &a {
        assert!(
            p.contains("def main():"),
            "generated program wraps a main: {p}"
        );
        assert!(p.contains("print("), "generated program prints: {p}");
        // Generated programs are pure prints: translation only adds the main call.
        let py = zap_to_python(p);
        assert_eq!(py, format!("{}\nmain()\n", p.trim_end()));
    }
}

// ---------------------------------------------------------------------------
// End-to-end differential tests (codegen + python3 required)
// ---------------------------------------------------------------------------

/// Run every curated `.zap` under `zapc` and `python3` and enforce its
/// directive: `match` cases must agree, `diverge` cases must differ (and are
/// flagged if they ever stop differing).
#[cfg(feature = "codegen")]
#[test]
fn curated_corpus_matches_cpython() {
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let mut match_cases = 0usize;
    let mut diverge_cases = 0usize;
    let mut failures = String::new();

    for zap_file in curated_fixtures() {
        let name = zap_file.file_stem().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(&zap_file).expect("read fixture");
        let expect = parse_directive(&src);

        let py_src = zap_to_python(&src);
        let py_file = tmp.join(format!("{name}.py"));
        std::fs::write(&py_file, &py_src).expect("write python");

        let cpython = run_python(&py_file);
        let zap = run_zap(&zap_file);
        let agree = cpython.matches(&zap);

        match &expect {
            Expect::Match => {
                match_cases += 1;
                if !agree {
                    failures.push_str(&report(
                        "MATCH-FAILED",
                        &name,
                        &src,
                        &py_src,
                        &cpython,
                        &zap,
                    ));
                }
            }
            Expect::Diverge { reason } => {
                diverge_cases += 1;
                if agree {
                    failures.push_str(&format!(
                        "\n================ DIVERGENCE-GONE: {name} ================\n\
                         expected to diverge ({reason}) but zap and cpython now AGREE.\n\
                         If the gap was fixed, promote this case to `# diff: match`.\n{}",
                        report("(now-agreeing)", &name, &src, &py_src, &cpython, &zap),
                    ));
                }
            }
        }
    }

    eprintln!(
        "diff_cpython curated: {match_cases} match-cases, {diverge_cases} documented-divergence cases"
    );
    assert!(failures.is_empty(), "{failures}");
}

/// Generate a seeded corpus of pure-`print` programs over the safe shared
/// grammar and require byte-identical stdout + exit under both engines. The
/// grammar deliberately excludes the known divergences (int64 overflow, true
/// division's float repr, non-bool `and`/`or` values, chained comparison, dict
/// iteration order), so a failure here is a genuine, newly-found divergence.
///
/// Deterministic in CI via a fixed seed and a modest count; override with
/// `ZAP_DIFF_SEED` / `ZAP_DIFF_COUNT` to fuzz harder locally.
#[cfg(feature = "codegen")]
#[test]
fn random_programs_match_cpython() {
    let seed: u64 = std::env::var("ZAP_DIFF_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5A50_D1FF_C0DE_0375);
    let count: usize = std::env::var("ZAP_DIFF_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(32);

    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let programs = gen::Generator::new(seed).programs(count);
    let mut failures = String::new();

    for (i, prog) in programs.iter().enumerate() {
        let name = format!("gen_{seed:016x}_{i:03}");
        let zap_file = tmp.join(format!("{name}.zap"));
        let py_file = tmp.join(format!("{name}.py"));
        std::fs::write(&zap_file, prog).expect("write zap");
        std::fs::write(&py_file, zap_to_python(prog)).expect("write python");

        let cpython = run_python(&py_file);
        let zap = run_zap(&zap_file);
        if !cpython.matches(&zap) {
            failures.push_str(&report(
                "GENERATED-DIVERGENCE",
                &name,
                prog,
                &zap_to_python(prog),
                &cpython,
                &zap,
            ));
            failures.push_str(&format!(
                "reproduce: ZAP_DIFF_SEED={seed} ZAP_DIFF_COUNT={count} (program #{i})\n"
            ));
        }
    }

    eprintln!("diff_cpython random: {count} generated programs (seed {seed:#018x})");
    assert!(failures.is_empty(), "{failures}");
}

// ---------------------------------------------------------------------------
// Seeded program generator
// ---------------------------------------------------------------------------

/// A bounded, deterministic generator of programs over the subset where Zap and
/// CPython are *required* to agree. Every program is a `def main():` wrapping a
/// handful of `print(...)` statements; every expression is fully parenthesized
/// so operator precedence/associativity can never be the source of a
/// difference. See [`random_programs_match_cpython`].
pub mod gen {
    /// SplitMix64 — a tiny, dependency-free, fully reproducible PRNG.
    pub struct Generator {
        state: u64,
    }

    impl Generator {
        pub fn new(seed: u64) -> Self {
            Generator { state: seed }
        }

        fn next(&mut self) -> u64 {
            self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        /// Generate `count` independent programs.
        pub fn programs(&mut self, count: usize) -> Vec<String> {
            (0..count).map(|_| self.one_program()).collect()
        }

        fn one_program(&mut self) -> String {
            let n_stmts = 3 + self.below(4) as usize; // 3..=6 statements
            let mut body = String::from("def main():\n");
            for _ in 0..n_stmts {
                body.push_str("    ");
                body.push_str(&self.statement());
                body.push('\n');
            }
            body
        }

        /// One `print(...)` statement drawn from the safe kinds. The grammar is
        /// deliberately restricted to constructs Zap and CPython agree on *and*
        /// Zap codegen fully supports today: integer `+`/`-`/`*` with unary
        /// minus, boolean logic over boolean operands, single comparisons,
        /// string concat/len, ternaries, and a comprehension count. It avoids
        /// every known divergence/gap this PR documents — int64 overflow, true
        /// division's float repr, non-bool `and`/`or` values, chained
        /// comparison, `%` sign on negatives, dict iteration order, and the
        /// unsupported list slice/`in`/negative-index — so a failure here is a
        /// genuine, newly-found divergence.
        fn statement(&mut self) -> String {
            match self.below(5) {
                0 => format!("print({})", self.int_expr(3)),
                1 => format!("print(str({}))", self.bool_expr(2)),
                2 => format!("print({})", self.str_expr(2)),
                3 => format!("print(len({}))", self.str_expr(2)),
                _ => {
                    // A ternary ("if") or a comprehension count ("for"/"if"),
                    // chosen to exercise control-flow-flavored expressions.
                    if self.below(2) == 0 {
                        format!(
                            "print(({} if {} else {}))",
                            self.int_expr(2),
                            self.bool_expr(1),
                            self.int_expr(2)
                        )
                    } else {
                        // `i` ranges over 0..k and the divisor `m` is positive,
                        // so the `%` here is always non-negative — the sign bug
                        // documented in mod_negatives.zap is never triggered.
                        let k = 1 + self.below(5);
                        let m = 1 + self.below(4);
                        let r = self.below(m);
                        format!("print(len([i for i in range({k}) if (i % {m} == {r})]))")
                    }
                }
            }
        }

        /// Fully-parenthesized integer expression; leaves are 0..=9 with an
        /// explicit unary-minus node. Only `+`/`-`/`*` — division (`/` float
        /// repr, `//`/`%` sign) is excluded because it is a documented
        /// divergence point (see division.zap, mod_negatives.zap). Bounded depth
        /// keeps results well inside int64, so the two engines must agree.
        fn int_expr(&mut self, depth: u32) -> String {
            if depth == 0 || self.below(3) == 0 {
                return self.below(10).to_string();
            }
            match self.below(4) {
                0 => format!(
                    "({} + {})",
                    self.int_expr(depth - 1),
                    self.int_expr(depth - 1)
                ),
                1 => format!(
                    "({} - {})",
                    self.int_expr(depth - 1),
                    self.int_expr(depth - 1)
                ),
                2 => format!(
                    "({} * {})",
                    self.int_expr(depth - 1),
                    self.int_expr(depth - 1)
                ),
                _ => format!("(-{})", self.int_expr(depth - 1)),
            }
        }

        /// Fully-parenthesized boolean expression. Operands of `and`/`or`/`not`
        /// are always boolean (comparisons or bool literals), so the result
        /// value is a plain bool in both languages — Zap's strict-bool `and`/`or`
        /// (D-0022) only diverges from CPython for *non-bool* operands, which
        /// this generator never produces. Comparisons are never chained.
        fn bool_expr(&mut self, depth: u32) -> String {
            if depth == 0 || self.below(3) == 0 {
                return self.comparison();
            }
            match self.below(4) {
                0 => format!(
                    "({} and {})",
                    self.bool_expr(depth - 1),
                    self.bool_expr(depth - 1)
                ),
                1 => format!(
                    "({} or {})",
                    self.bool_expr(depth - 1),
                    self.bool_expr(depth - 1)
                ),
                2 => format!("(not {})", self.bool_expr(depth - 1)),
                _ => self.comparison(),
            }
        }

        fn comparison(&mut self) -> String {
            let op = *["<", "<=", ">", ">=", "==", "!="]
                .get(self.below(6) as usize)
                .unwrap();
            format!("({} {} {})", self.int_expr(2), op, self.int_expr(2))
        }

        /// Fully-parenthesized string expression over a small literal pool.
        fn str_expr(&mut self, depth: u32) -> String {
            if depth == 0 || self.below(3) == 0 {
                let pool = ["", "a", "bc", "Zap", "xy", "123"];
                return format!("\"{}\"", pool[self.below(pool.len() as u64) as usize]);
            }
            format!(
                "({} + {})",
                self.str_expr(depth - 1),
                self.str_expr(depth - 1)
            )
        }
    }
}
