//! Doctests (`zap test --doc`, issue #234, D-0074).
//!
//! A *doctest* is a runnable example embedded in a docstring (D-0041). This
//! module extracts fenced ```` ```zap ```` blocks from a module's
//! [`zapc_doc::DocModel`] docstrings, runs each through the REPL [`Session`]
//! engine (I#159), and asserts the snippet behaves as the example claims — so a
//! documented example has *identical* semantics to compiled code, and stale
//! examples fail the build.
//!
//! The pieces split along the LLVM boundary, like the `@test` framework:
//!
//! - Extraction + the expectation model + matching ([`extract`], [`Expectation`],
//!   [`check`]) are pure and host-testable — no backend.
//! - [`run`] (codegen only) drives a [`Session`] and captures each snippet's
//!   stdout at the file-descriptor level (the JIT's C runtime prints through
//!   buffered `stdout`, which the in-memory [`crate::repl::EvalOutcome`] does not
//!   surface), then renders results through the shared [`super::report`] model.
//!
//! ## The example contract (D-0074)
//!
//! Each ```` ```zap ```` block in a docstring is **one example** — run as a
//! single [`Session::eval`], in a fresh (reset) session, so examples are
//! self-contained and isolated. Expectations are `#` comment directives, so the
//! block stays plain, Python-readable Zap:
//!
//! - `# => <text>` — one expected **stdout** line (repeat, in order). A trailing
//!   bare expression auto-prints, so `x` then `# => 42` asserts the value.
//! - `# raises: <substr>` — the snippet must raise an uncaught runtime exception
//!   whose message contains `<substr>`.
//! - `# error: ZAPnnnn` (or `# error[ZAPnnnn]`) — the snippet must fail to
//!   compile with that stable diagnostic code.
//! - no directive — the snippet must compile and run clean (stdout unchecked).
//!
//! `# =>` and `# raises:` may combine; `# error:` is exclusive. A block that
//! produces diagnostics with no `# error:` directive is a failure (a broken
//! example).
//!
//! **First-cut limitation (documented).** The session is a fresh REPL; it does
//! *not* load the enclosing module's declarations (cross-entry/module `def`/type
//! persistence is deferred, I#252). So an example cannot yet call the item it
//! documents by name — it must be self-contained (literals, builtins, stdlib
//! imports, inline logic). Injecting the module's own items into the doctest
//! session is the obvious follow-up.

use zapc_doc::{DocItem, DocItemBody, DocModel, DocString};

/// What an example claims about its own behaviour.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expectation {
    /// Expected stdout, one entry per `# =>` directive (trailing whitespace
    /// trimmed). `None` means stdout is not checked.
    pub stdout: Option<Vec<String>>,
    /// Expected uncaught-runtime-error substring (`# raises:`).
    pub raises: Option<String>,
    /// Expected compile-error code (`# error: ZAPnnnn`).
    pub error_code: Option<String>,
}

/// One extracted example, with the documented item it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocExample {
    /// The owning item's qualified name (or the module name for a module-level
    /// docstring).
    pub name: String,
    /// 1-based source line of the owning declaration (the module docstring
    /// reports line 1). Examples carry no span of their own, so this locates the
    /// item, paired with [`index`](Self::index).
    pub line: u32,
    /// 1-based index of this example among the item's examples.
    pub index: usize,
    /// The snippet source to run (directive comments removed).
    pub code: String,
    pub expect: Expectation,
}

// ----------------------------------------------------------------------------
// Extraction
// ----------------------------------------------------------------------------

/// Pull every ```` ```zap ```` example out of a module's docstrings, in
/// declaration order (module docstring first, then each item and its nested
/// methods).
pub fn extract(model: &DocModel) -> Vec<DocExample> {
    let mut out = Vec::new();
    let module_name = model
        .module
        .name
        .clone()
        .unwrap_or_else(|| "<module>".into());
    push_examples(&mut out, &module_name, 1, model.module.doc.as_ref());
    for item in &model.items {
        extract_item(&mut out, item);
    }
    out
}

fn extract_item(out: &mut Vec<DocExample>, item: &DocItem) {
    push_examples(out, &item.qualified_name, item.line, item.doc.as_ref());
    // Types carry their methods as nested items, which have their own docstrings.
    match &item.body {
        DocItemBody::Struct(s) => {
            for m in &s.methods {
                extract_item(out, m);
            }
        }
        DocItemBody::Enum(e) => {
            for m in &e.methods {
                extract_item(out, m);
            }
        }
        DocItemBody::Interface(i) => {
            for m in &i.methods {
                extract_item(out, m);
            }
        }
        _ => {}
    }
}

fn push_examples(out: &mut Vec<DocExample>, name: &str, line: u32, doc: Option<&DocString>) {
    let Some(doc) = doc else { return };
    for (i, (expect, code)) in parse_blocks(&doc.raw).into_iter().enumerate() {
        out.push(DocExample {
            name: name.to_string(),
            line,
            index: i + 1,
            code,
            expect,
        });
    }
}

/// Find the ```` ```zap ```` fenced blocks in a raw docstring and split each into
/// its expectation + runnable code.
fn parse_blocks(raw: &str) -> Vec<(Expectation, String)> {
    let text = cleandoc(raw);
    let mut blocks = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.trim_end() != "```zap" {
            continue;
        }
        let fence_indent = line.len() - trimmed.len();
        let mut body = Vec::new();
        for inner in lines.by_ref() {
            if inner.trim_start().trim_end() == "```" {
                break;
            }
            // Strip the fence's own indentation from each code line.
            body.push(dedent_by(inner, fence_indent));
        }
        blocks.push(split_block(&body));
    }
    blocks
}

/// Split a fenced block's lines into its expectation directives and its code.
fn split_block(body: &[String]) -> (Expectation, String) {
    let mut expect = Expectation::default();
    let mut code = String::new();
    for line in body {
        match directive(line) {
            Some(Directive::Out(s)) => expect.stdout.get_or_insert_with(Vec::new).push(s),
            Some(Directive::Raises(s)) => expect.raises = Some(s),
            Some(Directive::Error(code)) => expect.error_code = Some(code),
            None => {
                code.push_str(line);
                code.push('\n');
            }
        }
    }
    (expect, code)
}

enum Directive {
    /// `# => <text>`: one expected stdout line.
    Out(String),
    /// `# raises: <substr>`.
    Raises(String),
    /// `# error: ZAPnnnn`.
    Error(String),
}

/// If `line` is an expectation directive, parse it; otherwise it is code.
fn directive(line: &str) -> Option<Directive> {
    let body = line.trim_start().strip_prefix('#')?.trim_start();
    if let Some(rest) = body.strip_prefix("=>") {
        return Some(Directive::Out(
            rest.strip_prefix(' ')
                .unwrap_or(rest)
                .trim_end()
                .to_string(),
        ));
    }
    if let Some(rest) = body.strip_prefix("raises:") {
        return Some(Directive::Raises(rest.trim().to_string()));
    }
    if body.starts_with("error:") || body.starts_with("error[") {
        return zap_code(body).map(Directive::Error);
    }
    None
}

/// Extract a `ZAPnnnn` token from a directive body (`error: ZAP4012`,
/// `error[ZAP4012]`).
fn zap_code(s: &str) -> Option<String> {
    let start = s.find("ZAP")?;
    let digits: String = s[start + 3..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        Some(format!("ZAP{digits}"))
    }
}

/// Remove up to `n` leading spaces from `line` (fence-relative dedent).
fn dedent_by(line: &str, n: usize) -> String {
    let strip = line.chars().take(n).take_while(|c| *c == ' ').count();
    line[strip..].to_string()
}

/// `inspect.cleandoc`-style normalization (mirrors `zapc_doc`'s own): drop the
/// common leading indent of the continuation lines so fence detection and code
/// dedent are stable regardless of where the docstring sat in the source.
fn cleandoc(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut min_indent: Option<usize> = None;
    for line in &lines[1..] {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        min_indent = Some(min_indent.map_or(indent, |m| m.min(indent)));
    }
    let strip = min_indent.unwrap_or(0);
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.push(lines[0].trim_start().to_string());
    for line in &lines[1..] {
        if line.len() >= strip {
            out.push(line[strip..].to_string());
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

// ----------------------------------------------------------------------------
// Matching
// ----------------------------------------------------------------------------

/// Compare an example's [`Expectation`] against the outcome of running it.
/// Returns `Ok(())` on a pass, or `Err(message)` with an actionable diff.
///
/// `diagnostics` are the formatted compile-time diagnostics
/// (`error[ZAPnnnn]: ...`), `runtime_error` the caught uncaught-exception
/// message (if any), and `stdout` the snippet's captured standard output.
pub fn check(
    expect: &Expectation,
    diagnostics: &[String],
    runtime_error: Option<&str>,
    stdout: &str,
) -> Result<(), String> {
    // A compile-error example: the snippet must not compile, with the named code.
    if let Some(code) = &expect.error_code {
        if diagnostics.is_empty() {
            return Err(format!(
                "expected compile error {code}, but the snippet compiled{}",
                run_tail(runtime_error, stdout)
            ));
        }
        if diagnostics.iter().any(|d| d.contains(&format!("[{code}]"))) {
            return Ok(());
        }
        return Err(format!(
            "expected compile error {code}, but got:\n{}",
            indent_block(&diagnostics.join("\n"))
        ));
    }

    // Otherwise the snippet must compile.
    if !diagnostics.is_empty() {
        return Err(format!(
            "unexpected compile error(s):\n{}",
            indent_block(&diagnostics.join("\n"))
        ));
    }

    // Runtime-error expectation.
    match (&expect.raises, runtime_error) {
        (Some(want), Some(got)) if got.contains(want.as_str()) => {}
        (Some(want), Some(got)) => {
            return Err(format!(
                "expected a runtime error containing {want:?}, but got: {got:?}"
            ));
        }
        (Some(want), None) => {
            return Err(format!(
                "expected a runtime error containing {want:?}, but the snippet ran clean"
            ));
        }
        (None, Some(got)) => {
            return Err(format!("unexpected runtime error: {got}"));
        }
        (None, None) => {}
    }

    // Stdout expectation.
    if let Some(want) = &expect.stdout {
        let got: Vec<String> = stdout.lines().map(|l| l.trim_end().to_string()).collect();
        let want: Vec<String> = want.iter().map(|l| l.trim_end().to_string()).collect();
        if got != want {
            return Err(format!(
                "output mismatch\n  expected:\n{}\n  actual:\n{}",
                indent_block(&want.join("\n")),
                indent_block(&got.join("\n"))
            ));
        }
    }

    Ok(())
}

/// A short tail describing what a snippet did when it was expected *not* to
/// compile (for the error message).
fn run_tail(runtime_error: Option<&str>, stdout: &str) -> String {
    if let Some(e) = runtime_error {
        format!(" and raised: {e}")
    } else if !stdout.trim().is_empty() {
        format!(" and printed: {:?}", stdout.trim_end())
    } else {
        " and ran clean".to_string()
    }
}

/// Indent a (possibly multi-line) block for a failure message.
fn indent_block(s: &str) -> String {
    if s.is_empty() {
        return "    (empty)".to_string();
    }
    s.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ----------------------------------------------------------------------------
// The runner (codegen only)
// ----------------------------------------------------------------------------

#[cfg(feature = "codegen")]
pub use run_impl::run;

#[cfg(feature = "codegen")]
mod run_impl {
    use super::{check, extract, DocExample};
    use crate::repl::Session;
    use crate::testing::report::{self, Status, TestResult};
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    /// `zap test --doc [path] [--json]`: extract and run every docstring example
    /// under `path`, reporting pass/fail and exiting non-zero on any failure.
    pub fn run(path: Option<String>, json: bool) -> ExitCode {
        let files = crate::testing::discover::discover_all(path.as_deref());
        if files.is_empty() {
            eprintln!("zap test --doc: no .zap files found");
            return ExitCode::from(1);
        }

        let mut session = match Session::new() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("zap test --doc: {e}");
                return ExitCode::from(1);
            }
        };

        let mut results = Vec::new();
        for file in &files {
            run_file(&mut session, file, &mut results);
        }

        if json {
            print!("{}", report::format_json(&results));
            return exit_code(report::Summary::of(&results).ok());
        }
        let (text, ok) = report::format_human(&results);
        print!("{text}");
        exit_code(ok)
    }

    fn run_file(session: &mut Session, path: &Path, results: &mut Vec<TestResult>) {
        let display = path.display().to_string();
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                results.push(file_error(&display, format!("cannot read file: {e}")));
                return;
            }
        };
        let stem = path.file_stem().and_then(|s| s.to_str());
        // Doctests exercise the whole documented surface, private items included.
        let opts = zapc_doc::DocOptions {
            include_private: true,
        };
        let model = zapc_doc::DocModel::from_source(&src, stem, &opts);

        for ex in extract(&model) {
            results.push(run_example(session, &display, &ex));
        }
    }

    /// Run one example in a fresh (reset) session and turn the outcome into a
    /// [`TestResult`].
    fn run_example(session: &mut Session, file: &str, ex: &DocExample) -> TestResult {
        session.reset();
        let code = ex.code.clone();
        let (outcome, stdout) = capture_stdout(|| session.eval(&code));
        let runtime_error = outcome.runtime_error.as_deref();
        let (status, message) =
            match check(&ex.expect, &outcome.diagnostics, runtime_error, &stdout) {
                Ok(()) => (Status::Pass, String::new()),
                Err(msg) => (Status::Fail, msg),
            };
        TestResult {
            file: format!("{file}:{}", ex.line),
            name: format!("{} #{}", ex.name, ex.index),
            status,
            message,
            output: stdout,
        }
    }

    fn file_error(file: &str, message: String) -> TestResult {
        TestResult {
            file: file.to_string(),
            name: "<file>".to_string(),
            status: Status::Fail,
            message,
            output: String::new(),
        }
    }

    fn exit_code(ok: bool) -> ExitCode {
        if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    }

    /// Run `f`, returning its value plus everything it wrote to the process's
    /// standard output (fd 1). The JIT'd snippet prints through the C runtime's
    /// buffered `stdout`, so we redirect the file descriptor to a temp file,
    /// flush the C streams, then restore and read it back. POSIX-only, which
    /// matches the JIT (`zap repl` is not on Windows yet). If the OS calls fail,
    /// `f` still runs and the capture degrades to an empty string.
    fn capture_stdout<T, F: FnOnce() -> T>(f: F) -> (T, String) {
        use std::io::{Read, Seek, SeekFrom, Write};
        use std::os::unix::io::AsRawFd;

        // Flush Rust's own stdout so nothing buffered there leaks into the file.
        let _ = std::io::stdout().flush();

        let tmp_path = capture_tmp_path();
        let mut tmp = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
        {
            Ok(f) => f,
            Err(_) => return (f(), String::new()),
        };

        // SAFETY: plain POSIX fd juggling. `dup` the real stdout aside, point fd
        // 1 at the temp file for the duration of `f`, then restore it. `fflush`
        // with a null stream flushes every C output stream (the JIT runtime's
        // `stdout` among them) so buffered output lands in the file.
        let saved = unsafe {
            libc::fflush(std::ptr::null_mut());
            let saved = libc::dup(1);
            if saved < 0 {
                return (f(), String::new());
            }
            if libc::dup2(tmp.as_raw_fd(), 1) < 0 {
                libc::close(saved);
                return (f(), String::new());
            }
            saved
        };

        let value = f();

        let out = unsafe {
            libc::fflush(std::ptr::null_mut());
            libc::dup2(saved, 1);
            libc::close(saved);
            let mut s = String::new();
            let _ = tmp.seek(SeekFrom::Start(0));
            let _ = tmp.read_to_string(&mut s);
            s
        };
        drop(tmp);
        let _ = std::fs::remove_file(&tmp_path);
        (value, out)
    }

    /// A unique temp path for one capture.
    fn capture_tmp_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("zap-doctest-{}-{n}", std::process::id()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(raw: &str) -> Vec<(Expectation, String)> {
        parse_blocks(raw)
    }

    #[test]
    fn extracts_only_zap_fences() {
        let raw = "Summary.\n\n```text\nnot runnable\n```\n\n```zap\nprint(1)\n# => 1\n```\n";
        let bs = blocks(raw);
        assert_eq!(bs.len(), 1, "only the zap fence counts: {bs:?}");
        assert_eq!(bs[0].1, "print(1)\n");
        assert_eq!(bs[0].0.stdout.as_deref(), Some(&["1".to_string()][..]));
    }

    #[test]
    fn directives_parse_and_strip() {
        let raw = "```zap\nauto x = 6 * 7\nx\n# => 42\n```\n";
        let (expect, code) = blocks(raw).remove(0);
        assert_eq!(code, "auto x = 6 * 7\nx\n");
        assert_eq!(expect.stdout.unwrap(), vec!["42".to_string()]);
    }

    #[test]
    fn error_and_raises_directives() {
        let err = blocks("```zap\nauto x = 1 + \"a\"\n# error: ZAP4012\n```\n").remove(0);
        assert_eq!(err.0.error_code.as_deref(), Some("ZAP4012"));
        let err2 = blocks("```zap\nx\n# error[ZAP3001]\n```\n").remove(0);
        assert_eq!(err2.0.error_code.as_deref(), Some("ZAP3001"));
        let raises = blocks("```zap\nraise \"boom\"\n# raises: boom\n```\n").remove(0);
        assert_eq!(raises.0.raises.as_deref(), Some("boom"));
    }

    #[test]
    fn multiple_output_lines_in_order() {
        let (expect, _) = blocks("```zap\nprint(1)\nprint(2)\n# => 1\n# => 2\n```\n").remove(0);
        assert_eq!(
            expect.stdout.unwrap(),
            vec!["1".to_string(), "2".to_string()]
        );
    }

    #[test]
    fn indented_fence_is_dedented() {
        // A docstring whose body is indented (common under `Examples:`).
        let raw = "Does a thing.\n\n    ```zap\n    print(1)\n    # => 1\n    ```\n";
        let (expect, code) = blocks(raw).remove(0);
        assert_eq!(code, "print(1)\n");
        assert_eq!(expect.stdout.unwrap(), vec!["1".to_string()]);
    }

    #[test]
    fn check_passes_matching_output() {
        let e = Expectation {
            stdout: Some(vec!["42".into()]),
            ..Default::default()
        };
        assert!(check(&e, &[], None, "42\n").is_ok());
    }

    #[test]
    fn check_reports_output_mismatch() {
        let e = Expectation {
            stdout: Some(vec!["42".into()]),
            ..Default::default()
        };
        let err = check(&e, &[], None, "43\n").unwrap_err();
        assert!(err.contains("output mismatch"), "{err}");
        assert!(err.contains("42") && err.contains("43"), "{err}");
    }

    #[test]
    fn check_matches_error_code() {
        let e = Expectation {
            error_code: Some("ZAP4012".into()),
            ..Default::default()
        };
        assert!(check(&e, &["error[ZAP4012]: arithmetic".into()], None, "").is_ok());
        // Compiled clean when an error was expected → fail.
        let err = check(&e, &[], None, "").unwrap_err();
        assert!(err.contains("ZAP4012"), "{err}");
        // Wrong code → fail, and the message shows what we got.
        let err2 = check(&e, &["error[ZAP3001]: unknown name".into()], None, "").unwrap_err();
        assert!(err2.contains("ZAP3001"), "{err2}");
    }

    #[test]
    fn check_matches_runtime_error() {
        let e = Expectation {
            raises: Some("boom".into()),
            ..Default::default()
        };
        assert!(check(&e, &[], Some("boom: it happened"), "").is_ok());
        let err = check(&e, &[], None, "").unwrap_err();
        assert!(err.contains("ran clean"), "{err}");
    }

    #[test]
    fn check_flags_unexpected_compile_error() {
        let err = check(
            &Expectation::default(),
            &["error[ZAP3001]: x".into()],
            None,
            "",
        )
        .unwrap_err();
        assert!(err.contains("unexpected compile error"), "{err}");
    }

    #[test]
    fn clean_expectation_passes_when_silent() {
        assert!(check(&Expectation::default(), &[], None, "").is_ok());
    }

    #[test]
    fn extract_walks_module_and_items() {
        let src = "\"\"\"Mod doc.\n\n```zap\nprint(1)\n# => 1\n```\n\"\"\"\n\n\
                   def f():\n    \"\"\"Doc.\n\n    ```zap\n    print(2)\n    # => 2\n    ```\n    \"\"\"\n    pass\n";
        let model = DocModel::from_source(src, Some("m"), &zapc_doc::DocOptions::default());
        let examples = extract(&model);
        assert_eq!(examples.len(), 2, "module + f: {examples:?}");
        assert_eq!(examples[0].name, "m");
        assert_eq!(examples[1].name, "m.f");
        assert_eq!(examples[1].index, 1);
    }

    #[test]
    fn item_without_examples_yields_nothing() {
        let src = "def f():\n    \"Just prose, no fenced zap block.\"\n    pass\n";
        let model = DocModel::from_source(src, Some("m"), &zapc_doc::DocOptions::default());
        assert!(extract(&model).is_empty());
    }
}
