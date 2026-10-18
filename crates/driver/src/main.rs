//! `zapc` — the Zap compiler driver.
//!
//! `zapc run <file.zap>`   lex → parse → type-check → codegen → link → run.
//! `zapc build <file.zap>` same, but stops after producing the executable.
//!
//! The codegen/link/run steps require the `llvm` feature (and an LLVM toolchain);
//! without it the front-end still runs and reports errors.

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("zapc {}", zapc_syntax::VERSION);
            ExitCode::SUCCESS
        }
        Some("build") => pipeline(&args[2..], false),
        Some("run") => pipeline(&args[2..], true),
        // Bare `zapc file.zap` defaults to `run`.
        Some(p) if p.ends_with(".zap") => pipeline(&args[1..], true),
        _ => {
            eprintln!("zapc {} — the Zap compiler", zapc_syntax::VERSION);
            eprintln!("usage:");
            eprintln!("  zapc run <file.zap>");
            eprintln!("  zapc build <file.zap> [-o <out>]");
            ExitCode::from(2)
        }
    }
}

fn pipeline(args: &[String], run_after: bool) -> ExitCode {
    let mut file: Option<String> = None;
    let mut out: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                out = args.get(i + 1).cloned();
                i += 2;
            }
            f => {
                file = Some(f.to_string());
                i += 1;
            }
        }
    }
    let Some(file) = file else {
        eprintln!("zapc: no input file");
        return ExitCode::from(2);
    };

    let src = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("zapc: cannot read {file}: {e}");
            return ExitCode::from(1);
        }
    };

    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(&src);
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
        return ExitCode::from(1);
    }

    backend(&module, &file, out, run_after)
}

#[cfg(feature = "llvm")]
fn backend(
    module: &zapc_syntax::ast::Module,
    file: &str,
    out: Option<String>,
    run_after: bool,
) -> ExitCode {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("a")
        .to_string();
    let exe: PathBuf = match out {
        Some(o) => PathBuf::from(o),
        None if run_after => env::temp_dir().join(format!("zap-{stem}")),
        None => PathBuf::from(stem),
    };

    if let Err(e) = zapc_codegen::build_executable(module, &exe) {
        eprintln!("{file}: codegen error: {e}");
        return ExitCode::from(1);
    }

    if run_after {
        match Command::new(&exe).status() {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(e) => {
                eprintln!("zapc: failed to run {}: {e}", exe.display());
                ExitCode::from(1)
            }
        }
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(not(feature = "llvm"))]
fn backend(
    _module: &zapc_syntax::ast::Module,
    file: &str,
    _out: Option<String>,
    _run_after: bool,
) -> ExitCode {
    eprintln!("{file}: front-end OK, but this zapc was built without codegen");
    eprintln!("(rebuild with --features llvm, which requires an LLVM toolchain)");
    ExitCode::from(1)
}
