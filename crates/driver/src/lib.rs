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

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use zapc_syntax::ast::Module;
use zapc_syntax::cfg::Target;

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

/// Run the whole-program front end: discover and link the local module import
/// graph (I#283/D-0081) into one merged, qualified module, then type-check it.
/// A single file with no local imports links to just itself, so the single-file
/// path is unchanged. `when target.*` is resolved per file inside the loader.
fn front_end_program(entry: &str, target: &Target) -> Result<Module, ExitCode> {
    let mut module = match modlink::load_program(entry, target) {
        Ok(m) => m,
        Err(diags) => {
            for d in &diags {
                eprintln!(
                    "{}: error[{}]: {} [{}..{}]",
                    d.file, d.code, d.message, d.span.start, d.span.end
                );
            }
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
        for e in &deco_diags {
            eprintln!(
                "{entry}: error[{}]: {} [{}..{}]",
                e.code, e.message, e.span.start, e.span.end
            );
        }
        return Err(ExitCode::from(1));
    }
    let errs = zapc_sema::check(&module);
    if !errs.is_empty() {
        for e in &errs {
            eprintln!(
                "{entry}: error[{}]: {} [{}..{}]",
                e.code, e.message, e.span.start, e.span.end
            );
        }
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
        for e in &errs {
            eprintln!("{file}: error[{}]: {}", e.code, e.message);
        }
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
fn default_out(file: &str, run_after: bool) -> PathBuf {
    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("a")
        .to_string();
    if run_after {
        env::temp_dir().join(format!("zap-{stem}"))
    } else {
        PathBuf::from(stem)
    }
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

    let exe: PathBuf = match out {
        Some(o) => PathBuf::from(o),
        None => default_out(file, run_after),
    };

    if let Err(e) = zapc_codegen::build_executable(module, &exe, portable) {
        eprintln!("{file}: codegen error: {e}");
        return ExitCode::from(1);
    }

    if run_after {
        match Command::new(&exe).status() {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(e) => {
                eprintln!("zap: failed to run {}: {e}", exe.display());
                ExitCode::from(1)
            }
        }
    } else {
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
