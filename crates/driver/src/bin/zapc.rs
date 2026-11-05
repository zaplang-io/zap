//! `zapc` — the internal Zap compiler, driven by the user-facing `zap` CLI.
//!
//! `zapc run <file.zap>`   lex → parse → type-check → codegen → link → run.
//! `zapc build <file.zap>` same, but stops after producing the executable.
//! `zapc ir <file.zap>`    front-end + lowering; prints ZIR (host-safe).
//!
//! The codegen/link/run steps require the `llvm` feature (and an LLVM
//! toolchain); without it the front-end still runs and reports errors.
//!
//! This binary is intentionally thin: the pipeline lives in the `zapc` library
//! (`crate::lib`), shared with the `zap` driver.

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("zapc {}", zapc::VERSION);
            ExitCode::SUCCESS
        }
        Some("build") => compile_args(&args[2..], false),
        Some("run") => compile_args(&args[2..], true),
        Some("ir") => ir_args(&args[2..]),
        // Bare `zapc file.zap` defaults to `run`.
        Some(p) if p.ends_with(".zap") => compile_args(&args[1..], true),
        _ => {
            eprintln!(
                "zapc {} — the Zap compiler (internal; use `zap`)",
                zapc::VERSION
            );
            eprintln!("usage:");
            eprintln!("  zapc run <file.zap>");
            eprintln!("  zapc build <file.zap> [-o <out>]");
            eprintln!("  zapc ir <file.zap>              # print ZIR");
            ExitCode::from(2)
        }
    }
}

/// Parse `[-o <out>] [--race] <file>` and compile.
fn compile_args(args: &[String], run_after: bool) -> ExitCode {
    let mut file: Option<String> = None;
    let mut out: Option<String> = None;
    // `--race` (alias `-race`): enable the runtime data-race detector (#665,
    // D-0114). The thin `zapc` bin must thread this through exactly like the
    // user-facing `zap` CLI, or `zapc run --race` would silently compile WITHOUT
    // instrumentation (the detector's goldens drive `zapc`, so a dropped flag
    // makes a racy program run clean — #678).
    let mut race = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                out = args.get(i + 1).cloned();
                i += 2;
            }
            "--race" | "-race" => {
                race = true;
                i += 1;
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
    zapc::compile_for(&file, out, run_after, &[], false, false, race)
}

/// Parse the first positional argument as the file and dump ZIR. `--race`
/// (alias `-race`) shows the data-race instrumentation hooks (#665, D-0114).
fn ir_args(args: &[String]) -> ExitCode {
    let Some(file) = args.iter().find(|a| !a.starts_with('-')).cloned() else {
        eprintln!("zapc: no input file");
        return ExitCode::from(2);
    };
    let race = args.iter().any(|a| a == "--race" || a == "-race");
    zapc::emit_ir_for(&file, &[], false, race)
}
