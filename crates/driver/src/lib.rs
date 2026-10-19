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

pub mod cli;
pub mod testing;

/// The toolchain version (shared by both binaries and `zap version`).
pub const VERSION: &str = zapc_syntax::VERSION;

/// Read a source file, printing a diagnostic to stderr on failure.
fn read_source(file: &str) -> Result<String, ExitCode> {
    std::fs::read_to_string(file).map_err(|e| {
        eprintln!("zap: cannot read {file}: {e}");
        ExitCode::from(1)
    })
}

/// Run the front-end (lex → parse → type-check), printing diagnostics to
/// stderr. Returns the parsed [`Module`] when the program is well-formed.
fn front_end(file: &str, src: &str) -> Result<Module, ExitCode> {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    let mut failed = false;
    for e in &lerrs {
        eprintln!(
            "{file}: lex error: {} [{}..{}]",
            e.message, e.span.start, e.span.end
        );
        failed = true;
    }
    for e in &perrs {
        eprintln!(
            "{file}: parse error: {} [{}..{}]",
            e.message, e.span.start, e.span.end
        );
        failed = true;
    }
    if !failed {
        for e in zapc_sema::check(&module) {
            eprintln!(
                "{file}: type error: {} [{}..{}]",
                e.message, e.span.start, e.span.end
            );
            failed = true;
        }
    }
    if failed {
        return Err(ExitCode::from(1));
    }
    Ok(module)
}

/// Front-end + AST→ZIR lowering. Host-safe (no LLVM needed). Used by both
/// `zap ir` and the `zapc ir` subcommand.
pub fn emit_ir(file: &str) -> ExitCode {
    let src = match read_source(file) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let mut module = match front_end(file, &src) {
        Ok(m) => m,
        Err(code) => return code,
    };
    // `@test` functions are compiled only under `zap test`, never into a normal
    // build or diagnostic (D-0054).
    testing::strip_test_items(&mut module);
    let (zir, errs) = zapc_ir::lower_module(&module);
    if !errs.is_empty() {
        for e in &errs {
            eprintln!("{file}: lowering error: {}", e.message);
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
    let src = match read_source(file) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let mut module = match front_end(file, &src) {
        Ok(m) => m,
        Err(code) => return code,
    };
    // `@test` functions are excluded from release binaries (D-0054).
    testing::strip_test_items(&mut module);
    backend(&module, file, out, run_after)
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

#[cfg(feature = "llvm")]
fn backend(module: &Module, file: &str, out: Option<String>, run_after: bool) -> ExitCode {
    use std::process::Command;

    let exe: PathBuf = match out {
        Some(o) => PathBuf::from(o),
        None => default_out(file, run_after),
    };

    if let Err(e) = zapc_codegen::build_executable(module, &exe) {
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

#[cfg(not(feature = "llvm"))]
fn backend(_module: &Module, file: &str, _out: Option<String>, _run_after: bool) -> ExitCode {
    // Keep the default-output logic exercised so it does not warn as dead code
    // on the no-LLVM host build.
    let _ = default_out(file, _run_after);
    eprintln!("{file}: front-end OK, but this build has no codegen");
    eprintln!("(rebuild with --features zapc/llvm, which requires an LLVM toolchain)");
    ExitCode::from(1)
}
