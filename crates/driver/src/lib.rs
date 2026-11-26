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
use zapc_ir::FpPolicy;
use zapc_syntax::ast::Module;
use zapc_syntax::cfg::Target;

mod behavioral_decorator;
mod bundled_stdlib;
pub mod cli;
mod decorator_desugar;
mod derive_synth;
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
/// Shared test-only temp-path helpers (issue #917): one pid+counter mechanism
/// for every test module, so the `SystemTime`-nanosecond collision class (#180)
/// cannot reappear piecemeal.
#[cfg(test)]
mod testutil;

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

/// Render one comptime `@derive` failure (E3d, #1293) through the #1256
/// renderers, with the entry `src` so it carries `file:line:col` and the source
/// snippet. A failure under a derivable generator becomes the consumer
/// `ZAP7003` "cannot derive `trait` for `type`" at the user's field, with the
/// `required because of …` backtrace and the root cause; a comptime failure not
/// under a derive renders as a plain comptime error at its own span. The
/// `CtError`/`DeriveBacktrace` the renderers want are reconstructed from the
/// plain data the [`DeriveErrorInfo`](zapc_ir::DeriveErrorInfo) carried up.
fn render_derive_failure(file: &str, src: Option<&str>, di: &zapc_ir::DeriveErrorInfo) -> String {
    let cause = if di.cause_is_user {
        zapc_comptime::CtError::user(di.cause_message.clone(), di.cause_span)
    } else {
        zapc_comptime::CtError::value(di.cause_message.clone(), di.cause_span)
    };
    let mut backtrace = zapc_comptime::DeriveBacktrace::new();
    for (label, span) in &di.frames {
        backtrace = backtrace.with(zapc_comptime::Frame::new(label.clone(), *span));
    }
    let src = src.unwrap_or("");
    match (&di.trait_and_type, di.cause_is_user, di.instantiation_span) {
        // An engine fault *inside* a generator (not a `comptime.error`) is a
        // library bug: cite the generator line and the user's derive site, so it
        // is a clean dual-span error, never a panic (#1293, HQ).
        (Some(_), false, Some(inst)) => {
            zapc_comptime::render_generator_fault(src, file, &cause, di.cause_span, inst)
        }
        // A library `comptime.error` abort: the consumer `ZAP7003` form, at the
        // user's field, with the backtrace and the author's message.
        (Some((trait_name, type_name)), _, _) => zapc_comptime::render_derive_error(
            src,
            file,
            trait_name,
            type_name,
            di.cause_span,
            &cause,
            &backtrace,
        ),
        // A comptime failure outside any derive: a plain comptime error.
        (None, _, _) => zapc_comptime::render_ct_error(src, file, &cause, &backtrace),
    }
}

/// Print lowering errors to stderr: a `@derive` failure through
/// [`render_derive_failure`] (with the entry source), and any ordinary lowering
/// error as the location-free `ZAP6xxx` header exactly as before (lowering errors
/// carry no span of their own). Returns whether anything was printed.
fn report_lower_errors(file: &str, errs: Vec<zapc_ir::LowerError>) {
    let mut sources = SourceMap::default();
    for e in errs {
        if let Some(di) = e.derive {
            // `render_derive_failure` only consults `src` when a span is present;
            // the derive payload always carries one (the user's field).
            let rendered = {
                let src = sources.source(file);
                render_derive_failure(file, src, &di)
            };
            eprintln!("{rendered}");
        } else {
            eprintln!(
                "{}",
                zapc_diag::render::render_diagnostic(None, file, e.code, &e.message, None)
            );
        }
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
fn front_end_program(
    entry: &str,
    target: &Target,
    warnings_as_errors: bool,
) -> Result<(Module, zapc_sema::TypeMap), ExitCode> {
    match front_end_reports(entry, target) {
        Err(errs) => {
            report(errs);
            Err(ExitCode::from(1))
        }
        Ok((module, types, warns)) => {
            // Non-fatal warnings (e.g. a call to an `@deprecated` function,
            // I#472/D-0109; the `Eq`-without-`Hash` dict/set-key warning, D-0047;
            // the shared-mutable sharing lint, #664/D-0113) are surfaced here but
            // never fail the build — unless `--warnings-as-errors` is set (off by
            // default), a team CI knob that promotes any surviving warning to a
            // build failure without changing what the language considers legal.
            // They print before the error batch (there is none on this arm) so an
            // error, if any, stays the last thing on stderr.
            if !warns.is_empty() {
                let n = warns.len();
                report_warnings(warns);
                if warnings_as_errors {
                    eprintln!("zap: {n} warning(s) treated as errors (--warnings-as-errors)");
                    return Err(ExitCode::from(1));
                }
            }
            Ok((module, types))
        }
    }
}

/// The whole-program front end, returning structured diagnostics instead of
/// printing them: link the import graph (modlink, I#283/D-0081), desugar
/// decorators, and type-check. `Ok((module, types, warnings))` carries the
/// merged, qualified module, sema's inferred-type map (#1167), and any non-fatal
/// warnings (already `# noqa`-suppressed, #664/D-0113); `Err(errors)` carries the
/// fatal diagnostics as [`Report`]s. Shared by the printing build path
/// ([`front_end_program`]) and `zap test` (which routes through the SAME module
/// resolution, D-0054/SPEC §16; #1232) so there is one resolver, not two.
fn front_end_reports(
    entry: &str,
    target: &Target,
) -> Result<(Module, zapc_sema::TypeMap, Vec<Report>), Vec<Report>> {
    let pkg_roots = zapc_pkg::zenv::module_search_paths_from_env();
    let linked = modlink::load_program_linked(entry, target, pkg_roots).map_err(modlink_reports)?;
    front_end_tail(entry, linked.module, &linked.import_prefixes)
}

/// Like [`front_end_reports`], but with explicit dependency package-search roots
/// instead of reading them from the environment. Lets tests resolve a `zenv`
/// dependency against a temporary package tree without touching the process-wide
/// `ZAP_ZENV`, mirroring [`modlink::load_program_with`]. Test-only: production
/// code always resolves roots from the environment via [`front_end_reports`].
#[cfg(test)]
fn front_end_reports_with(
    entry: &str,
    target: &Target,
    pkg_roots: Vec<PathBuf>,
) -> Result<(Module, zapc_sema::TypeMap, Vec<Report>), Vec<Report>> {
    let linked = modlink::load_program_linked(entry, target, pkg_roots).map_err(modlink_reports)?;
    front_end_tail(entry, linked.module, &linked.import_prefixes)
}

/// Turn module-linker diagnostics into the uniform [`Report`] shape.
fn modlink_reports(diags: Vec<modlink::ModDiag>) -> Vec<Report> {
    diags
        .into_iter()
        .map(|d| Report {
            file: d.file,
            code: d.code,
            message: d.message,
            span: Some((d.span.start, d.span.end)),
        })
        .collect()
}

/// The decorator-desugar + type-check tail shared by both front-end entry points
/// (the only difference between them is how the import graph's dependency roots
/// are sourced). Takes the already-linked, merged module and finishes the front
/// end: desugar decorators, then type-check.
fn front_end_tail(
    entry: &str,
    mut module: Module,
    import_prefixes: &std::collections::HashMap<String, String>,
) -> Result<(Module, zapc_sema::TypeMap, Vec<Report>), Vec<Report>> {
    // Desugar registering (dotted) function decorators — `@app.get("/x")` — into
    // the method-call registration the receiver already accepts, before sema, so
    // the synthesized call is type-checked normally (I#457, D-0087).
    // `@app.get(...)` + `Depends(...)` adapter synthesis (I#457/D-0087,
    // I#466/D-0092); any invalid-injection diagnostics are reported as sema-
    // style errors before type-checking.
    let deco_diags =
        decorator_desugar::desugar_registering_decorators(&mut module, import_prefixes);
    if !deco_diags.is_empty() {
        return Err(sema_reports(entry, deco_diags));
    }
    // Expand user-defined behavioral decorators (`before`/`after`, D-0036; epic
    // #651) into wrapper functions, before sema, so the generated code is
    // type-checked and `zap expand`-visible like any other.
    let beh_diags = behavioral_decorator::expand_behavioral_decorators(&mut module);
    if !beh_diags.is_empty() {
        return Err(sema_reports(entry, beh_diags));
    }
    // Synthesize `@derive(<library trait>)` forwarding stubs (E3b, #1291, epic
    // #1289): for each type that derives a library-registered `@derivable` trait,
    // add a thin method per the trait's `interface` that forwards to the
    // registered generic generator specialized at the type, and strip that trait
    // from the `@derive(...)` so lowering's deferral backstop no longer fires.
    // Runs before sema so the synthesized methods are type-checked and
    // conformance-checked like any hand-written method.
    // `_derive_bounds` are the inferred serde bounds of each generic `@derive`
    // (E3c, #1292); E3d (#1293) will render unsatisfied ones at the offending
    // field, so the current pipeline does not consume them yet.
    let (derive_diags, _derive_bounds) = derive_synth::expand_library_derives(&mut module);
    if !derive_diags.is_empty() {
        return Err(sema_reports(entry, derive_diags));
    }
    // Analysis mode (#1167): same diagnostics as `analyze`, plus the inferred
    // type of every expression (a `TypeMap`), which lowering hands to the
    // monomorphizer so a generic argument whose type mono's own inferer cannot
    // derive is pinned from sema's type instead (#1132/#1162). `zapc_ir` cannot
    // compute this — it does not depend on the sema checker — so the driver does.
    let (errs, warns, types) = zapc_sema::analyze_types(&module);
    if !errs.is_empty() {
        return Err(sema_reports(entry, errs));
    }
    // Per-site suppression (#664, D-0113): drop warnings whose source line
    // carries a `# noqa` pragma. The spans are byte offsets into the entry
    // file's source (sema diagnostics are attributed to `entry`), so the entry
    // source is the line reference.
    let warns = match std::fs::read_to_string(entry) {
        Ok(src) => zapc_sema::apply_suppressions(&src, warns),
        Err(_) => warns,
    };
    Ok((module, types, sema_reports(entry, warns)))
}

/// Front-end + AST→ZIR lowering. Host-safe (no LLVM needed). Used by both
/// `zap ir` and the `zapc ir` subcommand.
pub fn emit_ir(file: &str) -> ExitCode {
    emit_ir_for(file, &[], false, false)
}

/// Like [`emit_ir`], but with compile-time `--target-feature`s for `when`
/// evaluation (I#139) and the `--warnings-as-errors` knob (#664, D-0113; off by
/// default), which promotes any surviving warning to a fatal error.
pub fn emit_ir_for(
    file: &str,
    features: &[String],
    warnings_as_errors: bool,
    race: bool,
) -> ExitCode {
    let (mut module, types) =
        match front_end_program(file, &build_target(features), warnings_as_errors) {
            Ok(mt) => mt,
            Err(code) => return code,
        };
    // `@test` functions are compiled only under `zap test`, never into a normal
    // build or diagnostic (D-0054). Removing items does not shift the remaining
    // expressions' spans, so `types` still keys into them.
    testing::strip_test_items(&mut module);
    // `--race` (#677, epic #665): emit the shared-access instrumentation hooks so
    // `zap ir --race` shows them. Off → identical ZIR to today (golden-safe).
    // `types` lets the monomorphizer pin generic arguments from sema's inferred
    // types (#1167).
    let (zir, errs) = zapc_ir::lower_module_opts_with_types(&module, race, &types);
    if !errs.is_empty() {
        // A `@derive` failure renders through the #1256 renderers with the entry
        // source (E3d, #1293, `ZAP7003` at the user's field); an ordinary lowering
        // error stays the location-free header (it carries no span).
        report_lower_errors(file, errs);
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
    compile_for(
        file,
        out,
        run_after,
        &[],
        false,
        false,
        false,
        false,
        FpPolicy::default(),
    )
}

/// Like [`compile`], but with compile-time `--target-feature`s for `when`
/// evaluation (I#139) and the `portable` link profile (I#287). Phase 1 still
/// generates code for the host; the features only steer which
/// `when target.has(.f)` arm is kept. `portable` selects the self-contained
/// static link (see [`zapc_codegen::build_executable`]).
#[allow(clippy::too_many_arguments)]
pub fn compile_for(
    file: &str,
    out: Option<String>,
    run_after: bool,
    features: &[String],
    portable: bool,
    warnings_as_errors: bool,
    race: bool,
    debug: bool,
    fp: FpPolicy,
) -> ExitCode {
    let (mut module, types) =
        match front_end_program(file, &build_target(features), warnings_as_errors) {
            Ok(mt) => mt,
            Err(code) => return code,
        };
    // `@test` functions are excluded from release binaries (D-0054). Removing
    // items does not shift the remaining expressions' spans, so `types` still
    // keys into them.
    testing::strip_test_items(&mut module);
    // `--fp=fast` enables reduction reassociation (unbounded error, width-
    // dependent bits). Warn in the compiler log as the model mandates (#946,
    // D-0196) so a non-reproducible build is never silent.
    if fp == FpPolicy::Fast {
        eprintln!(
            "zap: warning: --fp=fast enables float reduction reassociation; \
             results are width-dependent and NOT reproducible across targets (#946)"
        );
    }
    backend(
        &module, &types, file, out, run_after, portable, race, debug, fp,
    )
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
#[allow(clippy::too_many_arguments)]
fn backend(
    module: &Module,
    types: &zapc_sema::TypeMap,
    file: &str,
    out: Option<String>,
    run_after: bool,
    portable: bool,
    race: bool,
    debug: bool,
    fp: FpPolicy,
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

    // Entry source for located runtime diagnostics (#1489/#1493): a null
    // dereference raises a `NullReferenceError` carrying `... at <file>:<line>`,
    // whose line is resolved against the entry source (the same entry-source
    // span convention sema diagnostics use, #571). Best-effort: an unreadable
    // entry degrades the location to `<unknown>:0`.
    let entry_src = std::fs::read_to_string(file).unwrap_or_default();
    let loc = (!entry_src.is_empty()).then_some(zapc_ir::SourceLoc {
        file,
        src: entry_src.as_str(),
    });
    if let Err(e) =
        zapc_codegen::build_executable(module, types, &exe, portable, race, debug, fp, loc)
    {
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
#[allow(clippy::too_many_arguments)]
fn backend(
    _module: &Module,
    _types: &zapc_sema::TypeMap,
    file: &str,
    _out: Option<String>,
    _run_after: bool,
    _portable: bool,
    _race: bool,
    _debug: bool,
    _fp: FpPolicy,
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
            // pid + process-global counter, no clock (#917): in-process test
            // threads reading a microsecond-quantised clock in the same tick used
            // to get the same `zap-diag-<pid>-<nanos>.zap` path. Same prefix/suffix.
            let path = crate::testutil::unique_temp_path("zap-diag", ".zap");
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

#[cfg(test)]
mod derive_render_tests {
    use super::render_derive_failure;
    use zapc_ir::DeriveErrorInfo;
    use zapc_syntax::span::Span;

    const SRC: &str = "struct User:\n    int64 id\n    Widget w\n";

    // The span of `Widget w` (3rd line) in SRC, for the `at`/cause/frame.
    fn field_span() -> Span {
        let start = SRC.find("Widget w").unwrap() as u32;
        Span::new(start, start + "Widget w".len() as u32)
    }

    #[test]
    fn a_derive_failure_renders_the_consumer_header_and_backtrace() {
        let sp = field_span();
        let di = DeriveErrorInfo {
            trait_and_type: Some(("Ser".into(), "User".into())),
            cause_message: "`Ser` is not implemented for this type".into(),
            cause_span: sp,
            cause_is_user: true,
            instantiation_span: Some(Span::new(0, 6)),
            frames: vec![("field `w` of `User`".into(), sp)],
        };
        let out = render_derive_failure("user.zap", Some(SRC), &di);
        assert!(
            out.contains("error[ZAP7003]: cannot derive `Ser` for `User`"),
            "{out}"
        );
        assert!(
            out.contains("required because of field `w` of `User`"),
            "{out}"
        );
        assert!(
            out.contains("the generator reported: `Ser` is not implemented for this type"),
            "{out}"
        );
        // Points at the user's field line, with a caret snippet.
        assert!(out.contains("user.zap:3:5"), "located at the field: {out}");
    }

    #[test]
    fn a_comptime_failure_without_a_derive_renders_as_a_plain_error() {
        // No `trait_and_type` → a plain comptime error at its own span (ZAP7002
        // for a library abort), not the "cannot derive" consumer header.
        let sp = field_span();
        let di = DeriveErrorInfo {
            trait_and_type: None,
            cause_message: "boom".into(),
            cause_span: sp,
            cause_is_user: true,
            instantiation_span: None,
            frames: vec![],
        };
        let out = render_derive_failure("user.zap", Some(SRC), &di);
        assert!(!out.contains("cannot derive"), "no consumer header: {out}");
        assert!(out.contains("ZAP7002"), "a user abort is ZAP7002: {out}");
        assert!(out.contains("boom"), "{out}");
    }

    #[test]
    fn an_engine_fault_inside_a_generator_renders_the_dual_span_form() {
        // A non-user `CtError` (an engine fault, e.g. an out-of-range
        // `reflect.field`) under a derive renders as the dual-span generator
        // fault: the generator line AND "instantiated here", never the consumer
        // "cannot derive" header (the bug is the library's, not the user's).
        let sp = field_span();
        let inst = Span::new(0, 6); // `struct` keyword of `struct User`
        let di = DeriveErrorInfo {
            trait_and_type: Some(("Ser".into(), "User".into())),
            cause_message: "comptime field index 100 is out of range".into(),
            cause_span: sp,
            cause_is_user: false,
            instantiation_span: Some(inst),
            frames: vec![("field `w` of `User`".into(), sp)],
        };
        let out = render_derive_failure("user.zap", Some(SRC), &di);
        assert!(
            !out.contains("cannot derive"),
            "an engine fault is not the consumer header: {out}"
        );
        assert!(
            out.contains("error in comptime generator"),
            "dual-span generator-fault header: {out}"
        );
        assert!(out.contains("instantiated here"), "second span: {out}");
        assert!(out.contains("out of range"), "{out}");
    }

    #[test]
    fn an_enum_derive_failure_names_the_variant_in_the_backtrace() {
        // An enum derive failure the generator points at a variant payload field
        // renders a backtrace frame naming the variant (#1616), not a frame that
        // mislabels the payload as a struct field of the enum.
        const ENUM_SRC: &str = "enum Shape:\n    Circle(Widget r)\n";
        let start = ENUM_SRC.find("Widget r").unwrap() as u32;
        let sp = Span::new(start, start + "Widget r".len() as u32);
        let di = DeriveErrorInfo {
            trait_and_type: Some(("Ser".into(), "Shape".into())),
            cause_message: "`Ser` is not implemented for this type".into(),
            cause_span: sp,
            cause_is_user: true,
            instantiation_span: Some(Span::new(0, 4)),
            frames: vec![(
                "payload field `r` of variant `Circle` of `Shape`".into(),
                sp,
            )],
        };
        let out = render_derive_failure("shape.zap", Some(ENUM_SRC), &di);
        assert!(
            out.contains("error[ZAP7003]: cannot derive `Ser` for `Shape`"),
            "{out}"
        );
        assert!(
            out.contains("required because of payload field `r` of variant `Circle` of `Shape`"),
            "{out}"
        );
    }
}
