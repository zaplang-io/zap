//! The Zap toolchain library.
//!
//! This crate ships two binaries that share the pipeline implemented here:
//!
//! - **`zap`** — the user-facing CLI (D-0053). See [`cli`].
//! - **`zapc`** — the internal compiler the `zap` driver invokes.
//!
//! The compile pipeline is `lex → parse → type-check → (lower → codegen → link
//! → run)`. The codegen/link/run steps require the `llvm` feature (and an LLVM
//! toolchain); without it the front-end still runs and reports errors.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Module;
use zapc_syntax::cfg::Target;

mod behavioral_decorator;
pub mod cli;
mod decorator_desugar;
mod modlink;
// The JSON codec now lives in its own leaf crate (`zapc_json`, I#236) so the
// lower-level `zapc_pkg` can share it; re-exported here as `crate::json` so the
// MCP/LSP/playground call sites are unchanged.
pub use zapc_json as json;
pub mod lsp;
pub mod mcp;
pub mod playground;
pub mod repl;
pub mod testing;

/// The toolchain version (shared by both binaries and `zap version`).
pub const VERSION: &str = zapc_syntax::VERSION;

/// The build target `when target.*` conditional compilation is evaluated
/// against (I#139, D-0079). In Phase 1 the architecture/OS come from the host
/// the compiler runs on (Zap builds natively); compile-time features are taken
/// from `--target-feature` flags so `target.has(.f)` is reachable without
/// cross-compilation.
fn build_target(features: &[String]) -> Target {
    let mut t = Target::host();
    t.features = features.to_vec();
    t
}

/// One diagnostic in the uniform shape the shared renderer consumes, so every
/// front-end render site (`modlink`, decorator desugar, sema, lowering) prints
/// through one path and stays consistent (#571). `span` is `None` for a
/// diagnostic that carries no source location (e.g. a lowering error).
struct Report {
    file: String,
    code: ErrorCode,
    message: String,
    span: Option<(u32, u32)>,
}

/// Lazily reads and caches source files so a [`Report`] can be rendered with a
/// `file:line:column` location and a source snippet. Reading the file afresh at
/// diagnostic time is safe: spans are byte offsets into the file as it was
/// parsed, and the error path is cold. A file that cannot be read yields
/// `None`, and rendering degrades to the location-free header.
#[derive(Default)]
struct SourceMap {
    cache: HashMap<String, Option<String>>,
}

impl SourceMap {
    fn source(&mut self, file: &str) -> Option<&str> {
        self.cache
            .entry(file.to_string())
            .or_insert_with(|| std::fs::read_to_string(file).ok())
            .as_deref()
    }
}

/// De-duplicate the reports (order-preserving, keyed on file/code/span/message,
/// so one cause reported twice prints once) and render each through the shared
/// renderer. Returned lines are what [`report`] prints to stderr; split out so
/// it is testable without capturing stderr.
fn format_reports(reports: Vec<Report>) -> Vec<String> {
    let reports = zapc_diag::render::dedup_preserving(reports, |r| {
        (r.file.clone(), r.code, r.span, r.message.clone())
    });
    let mut sources = SourceMap::default();
    reports
        .into_iter()
        .map(|r| {
            let src = if r.span.is_some() {
                sources.source(&r.file)
            } else {
                None
            };
            zapc_diag::render::render_diagnostic(src, &r.file, r.code, &r.message, r.span)
        })
        .collect()
}

/// Build [`Report`]s for a merged-program pass whose diagnostics carry a span
/// but no file of their own (sema and decorator desugar). Their spans are byte
/// offsets into the entry file, so they are attributed to `entry`.
fn sema_reports(entry: &str, errs: Vec<zapc_sema::TypeError>) -> Vec<Report> {
    errs.into_iter()
        .map(|e| Report {
            file: entry.to_string(),
            code: e.code,
            message: e.message,
            span: Some((e.span.start, e.span.end)),
        })
        .collect()
}

/// Render and print a batch of diagnostics to stderr (see [`format_reports`]).
fn report(reports: Vec<Report>) {
    for line in format_reports(reports) {
        eprintln!("{line}");
    }
}

/// Like [`format_reports`] but renders each diagnostic at warning severity
/// (`warning[ZAPnnnn]`), for the non-fatal sema warnings surfaced by the build
/// pipeline (I#472/D-0109).
fn format_warnings(reports: Vec<Report>) -> Vec<String> {
    let reports = zapc_diag::render::dedup_preserving(reports, |r| {
        (r.file.clone(), r.code, r.span, r.message.clone())
    });
    let mut sources = SourceMap::default();
    reports
        .into_iter()
        .map(|r| {
            let src = if r.span.is_some() {
                sources.source(&r.file)
            } else {
                None
            };
            zapc_diag::render::render_diagnostic_sev(
                zapc_diag::render::Severity::Warning,
                src,
                &r.file,
                r.code,
                &r.message,
                r.span,
            )
        })
        .collect()
}

/// Render and print a batch of warnings to stderr.
fn report_warnings(reports: Vec<Report>) {
    for line in format_warnings(reports) {
        eprintln!("{line}");
    }
}

/// Run the whole-program front end: discover and link the local module import
/// graph (I#283/D-0081) into one merged, qualified module, then type-check it.
/// A single file with no local imports links to just itself, so the single-file
/// path is unchanged. `when target.*` is resolved per file inside the loader.
fn front_end_program(entry: &str, target: &Target) -> Result<Module, ExitCode> {
    let mut module = match modlink::load_program(entry, target) {
        Ok(m) => m,
        Err(diags) => {
            report(
                diags
                    .into_iter()
                    .map(|d| Report {
                        file: d.file,
                        code: d.code,
                        message: d.message,
                        span: Some((d.span.start, d.span.end)),
                    })
                    .collect(),
            );
            return Err(ExitCode::from(1));
        }
    };
    // Desugar registering (dotted) function decorators — `@app.get("/x")` — into
    // the method-call registration the receiver already accepts, before sema, so
    // the synthesized call is type-checked normally (I#457, D-0087).
    // `@app.get(...)` + `Depends(...)` adapter synthesis (I#457/D-0087,
    // I#466/D-0092); any invalid-injection diagnostics are reported as sema-
    // style errors before type-checking.
    let deco_diags = decorator_desugar::desugar_registering_decorators(&mut module);
    if !deco_diags.is_empty() {
        report(sema_reports(entry, deco_diags));
        return Err(ExitCode::from(1));
    }
    // Expand user-defined behavioral decorators (`before`/`after`, D-0036; epic
    // #651) into wrapper functions, before sema, so the generated code is
    // type-checked and `zap expand`-visible like any other.
    let beh_diags = behavioral_decorator::expand_behavioral_decorators(&mut module);
    if !beh_diags.is_empty() {
        report(sema_reports(entry, beh_diags));
        return Err(ExitCode::from(1));
    }
    let (errs, warns) = zapc_sema::analyze(&module);
    // Non-fatal warnings (e.g. a call to an `@deprecated` function, I#472/D-0109;
    // the `Eq`-without-`Hash` dict/set-key warning, D-0047) are surfaced here but
    // never fail the build. They print before the error batch so an error, if
    // any, stays the last thing on stderr.
    if !warns.is_empty() {
        report_warnings(sema_reports(entry, warns));
    }
    if !errs.is_empty() {
        report(sema_reports(entry, errs));
        return Err(ExitCode::from(1));
    }
    Ok(module)
}

/// Front-end + AST→ZIR lowering. Host-safe (no LLVM needed). Used by both
/// `zap ir` and the `zapc ir` subcommand.
pub fn emit_ir(file: &str) -> ExitCode {
    emit_ir_for(file, &[])
}

/// Like [`emit_ir`], but with compile-time `--target-feature`s for `when`
/// evaluation (I#139).
pub fn emit_ir_for(file: &str, features: &[String]) -> ExitCode {
    let mut module = match front_end_program(file, &build_target(features)) {
        Ok(m) => m,
        Err(code) => return code,
    };
    // `@test` functions are compiled only under `zap test`, never into a normal
    // build or diagnostic (D-0054).
    testing::strip_test_items(&mut module);
    let (zir, errs) = zapc_ir::lower_module(&module);
    if !errs.is_empty() {
        // Lowering errors carry no span, so they render as the location-free
        // header (unchanged), but still through the one shared path (#571).
        report(
            errs.into_iter()
                .map(|e| Report {
                    file: file.to_string(),
                    code: e.code,
                    message: e.message,
                    span: None,
                })
                .collect(),
        );
        return ExitCode::from(1);
    }
    print!("{zir}");
    ExitCode::SUCCESS
}

/// `zap expand` — show decorator/`@derive`-generated code (D-0036).
///
/// The expansion machinery lives in AST→ZIR lowering (`@derive(Eq, Repr)`
/// becomes real ZIR methods, etc.), so the expanded program is rendered as its
/// lowered ZIR. A note is printed to stderr so the output's nature is clear.
pub fn emit_expand(file: &str) -> ExitCode {
    eprintln!(
        "zap expand: showing the lowered ZIR, which includes decorator/@derive-generated code (D-0036)."
    );
    emit_ir(file)
}

/// Compile `file`, then (when `run_after`) execute the produced binary.
///
/// `out` is the output path; when `None`, a default is chosen (a temp path for
/// `run`, the file stem for `build`).
pub fn compile(file: &str, out: Option<String>, run_after: bool) -> ExitCode {
    compile_for(file, out, run_after, &[], false)
}

/// Like [`compile`], but with compile-time `--target-feature`s for `when`
/// evaluation (I#139) and the `portable` link profile (I#287). Phase 1 still
/// generates code for the host; the features only steer which
/// `when target.has(.f)` arm is kept. `portable` selects the self-contained
/// static link (see [`zapc_codegen::build_executable`]).
pub fn compile_for(
    file: &str,
    out: Option<String>,
    run_after: bool,
    features: &[String],
    portable: bool,
) -> ExitCode {
    let mut module = match front_end_program(file, &build_target(features)) {
        Ok(m) => m,
        Err(code) => return code,
    };
    // `@test` functions are excluded from release binaries (D-0054).
    testing::strip_test_items(&mut module);
    backend(&module, file, out, run_after, portable)
}

/// Default output path for a compile when `-o` is not given.
///
/// For a plain `zap build` the binary lands at `./<stem>` in the working
/// directory (deterministic, user-visible). For `zap run` the binary is a
/// throwaway: it goes inside a *unique* temp directory ([`unique_run_dir`]) so
/// two concurrent `zap run` invocations never resolve to the same path and
/// clobber each other (#634). The caller ([`backend`]) creates that directory
/// and removes it after the run.
fn default_out(file: &str, run_after: bool) -> PathBuf {
    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("a")
        .to_string();
    if run_after {
        unique_run_dir().join(format!("zap-{stem}"))
    } else {
        PathBuf::from(stem)
    }
}

/// A fresh, process-unique temp directory path for one `zap run`'s throwaway
/// binary, under the system temp dir. The name combines the process id with a
/// process-global counter (and a nanosecond clock reading), so every call —
/// across concurrent processes and repeated calls within one process, even for
/// the same entry file — yields a distinct path. Keyed this way rather than on
/// the entry stem alone, the old `$TMPDIR/zap-<stem>` collision (#634) cannot
/// happen. This only computes the path; it does not create the directory.
fn unique_run_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    env::temp_dir().join(format!("zap-run-{}-{nanos}-{seq}", std::process::id()))
}

#[cfg(feature = "codegen")]
fn backend(
    module: &Module,
    file: &str,
    out: Option<String>,
    run_after: bool,
    portable: bool,
) -> ExitCode {
    use std::process::Command;

    // The binary path, plus the throwaway temp dir we own and must remove (only
    // for `zap run` without an explicit `-o`; see [`default_out`]).
    let (exe, temp_dir): (PathBuf, Option<PathBuf>) = match out {
        Some(o) => (PathBuf::from(o), None),
        None => {
            let exe = default_out(file, run_after);
            // For a run, `exe` lives in a fresh unique dir we created here; keep
            // its parent so it can be cleaned up afterwards.
            let temp_dir = if run_after {
                exe.parent().map(Path::to_path_buf)
            } else {
                None
            };
            (exe, temp_dir)
        }
    };

    if let Some(dir) = &temp_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("zap: failed to create temp dir {}: {e}", dir.display());
            return ExitCode::from(1);
        }
    }

    // Best-effort removal of the throwaway dir on every exit path below.
    let cleanup = || {
        if let Some(dir) = &temp_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    };

    if let Err(e) = zapc_codegen::build_executable(module, &exe, portable) {
        eprintln!("{file}: codegen error: {e}");
        cleanup();
        return ExitCode::from(1);
    }

    if run_after {
        let code = match Command::new(&exe).status() {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(e) => {
                eprintln!("zap: failed to run {}: {e}", exe.display());
                ExitCode::from(1)
            }
        };
        cleanup();
        code
    } else {
        cleanup();
        ExitCode::SUCCESS
    }
}

#[cfg(not(feature = "codegen"))]
fn backend(
    _module: &Module,
    file: &str,
    _out: Option<String>,
    _run_after: bool,
    _portable: bool,
) -> ExitCode {
    // Keep the default-output logic exercised so it does not warn as dead code
    // on the no-LLVM host build.
    let _ = default_out(file, _run_after);
    eprintln!("{file}: front-end OK, but this build has no codegen");
    eprintln!("(rebuild with --features zapc/llvm, which requires an LLVM toolchain)");
    ExitCode::from(1)
}

#[cfg(test)]
mod render_tests {
    use super::{format_reports, Report};
    use zapc_diag::ErrorCode;

    /// A throwaway source file under the OS temp dir, removed on drop.
    struct TmpFile {
        path: std::path::PathBuf,
    }

    impl TmpFile {
        fn new(src: &str) -> TmpFile {
            use std::time::{SystemTime, UNIX_EPOCH};
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("zap-diag-{}-{}.zap", std::process::id(), nonce));
            std::fs::write(&path, src).unwrap();
            TmpFile { path }
        }

        fn name(&self) -> String {
            self.path.display().to_string()
        }
    }

    impl Drop for TmpFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn renders_line_col_and_caret_from_the_source_file() {
        let f = TmpFile::new("def main():\n    print(x)\n");
        // `x` is byte 22..23 — line 2, column 11.
        let out = format_reports(vec![Report {
            file: f.name(),
            code: ErrorCode::ZAP3001,
            message: "unknown name `x`".into(),
            span: Some((22, 23)),
        }]);
        assert_eq!(
            out,
            vec![format!(
                "{}:2:11: error[ZAP3001]: unknown name `x`\n    print(x)\n          ^",
                f.name()
            )]
        );
    }

    #[test]
    fn duplicate_diagnostics_are_emitted_once() {
        let f = TmpFile::new("def main():\n    print(x)\n");
        let one = || Report {
            file: f.name(),
            code: ErrorCode::ZAP3001,
            message: "unknown name `x`".into(),
            span: Some((22, 23)),
        };
        // The same cause reported three times collapses to a single line.
        let out = format_reports(vec![one(), one(), one()]);
        assert_eq!(out.len(), 1);
        assert!(out[0].contains(":2:11: error[ZAP3001]: unknown name `x`"));
    }

    #[test]
    fn distinct_diagnostics_are_all_kept_in_order() {
        let f = TmpFile::new("def main():\n    print(x)\n");
        let out = format_reports(vec![
            Report {
                file: f.name(),
                code: ErrorCode::ZAP3001,
                message: "unknown name `x`".into(),
                span: Some((22, 23)),
            },
            Report {
                file: f.name(),
                code: ErrorCode::ZAP3001,
                message: "unknown name `y`".into(),
                span: Some((22, 23)),
            },
        ]);
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("unknown name `x`"));
        assert!(out[1].contains("unknown name `y`"));
    }

    #[test]
    fn missing_source_file_degrades_to_header_only() {
        let out = format_reports(vec![Report {
            file: "/no/such/file.zap".into(),
            code: ErrorCode::ZAP6001,
            message: "lowering failed".into(),
            span: Some((0, 1)),
        }]);
        assert_eq!(
            out,
            vec!["/no/such/file.zap: error[ZAP6001]: lowering failed"]
        );
    }
}

#[cfg(test)]
mod out_path_tests {
    use super::default_out;
    use std::path::Path;

    /// `zap build` (no run) keeps the deterministic `./<stem>` path so the
    /// built artifact is predictable and user-visible.
    #[test]
    fn build_output_is_the_bare_stem_in_the_cwd() {
        assert_eq!(default_out("foo/bar/main.zap", false), Path::new("main"));
    }

    /// `zap run` output paths must never collide: two different entries, and —
    /// crucially for concurrent runs of the same program — two calls for the
    /// *same* entry, each resolve to a distinct throwaway path under the system
    /// temp dir (#634).
    #[test]
    fn run_output_paths_are_unique_and_under_temp_dir() {
        let a = default_out("alpha/main.zap", true);
        let b = default_out("beta/main.zap", true);
        let same1 = default_out("alpha/main.zap", true);
        let same2 = default_out("alpha/main.zap", true);

        // Different entries never share a path, and neither do repeated calls
        // for one entry (the collision that bit concurrent `zap run`).
        assert_ne!(a, b);
        assert_ne!(same1, same2);
        assert_ne!(a, same1);

        // Each binary sits inside its own fresh dir under the system temp dir.
        let temp = std::env::temp_dir();
        for p in [&a, &b, &same1, &same2] {
            let dir = p.parent().expect("run binary has a parent temp dir");
            assert!(
                dir.starts_with(&temp),
                "{} should be under the system temp dir {}",
                p.display(),
                temp.display()
            );
            assert_ne!(dir, temp, "the binary must live in its own subdirectory");
        }
        // The per-run directories themselves are distinct.
        assert_ne!(same1.parent(), same2.parent());
    }
}
