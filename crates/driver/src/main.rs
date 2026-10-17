//! `zapc` — the Zap compiler driver.
//!
//! Phase 0 scaffold: this wires up the CLI shape and depends on the frozen
//! `zapc_syntax` data structures. The lexer, parser, type checker and codegen
//! are implemented in later phases (see `ROADMAP.md`); until then this prints a
//! clear "not implemented yet" notice.

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    if args.len() == 2 && (args[1] == "--version" || args[1] == "-V") {
        println!("zapc {}", zapc_syntax::VERSION);
        return ExitCode::SUCCESS;
    }

    if args.len() != 2 || args[1].starts_with('-') {
        eprintln!(
            "zapc {} — the Zap compiler (Phase 0 scaffold)",
            zapc_syntax::VERSION
        );
        eprintln!("usage: zapc <file.zap>");
        return ExitCode::from(2);
    }

    let path = &args[1];
    match std::fs::read_to_string(path) {
        Ok(src) => {
            eprintln!("zapc: read {} ({} bytes)", path, src.len());
            eprintln!("zapc: lexer/parser/typecheck/codegen not implemented yet (Phase 1)");
            // Exit non-zero so golden tests know there is no real output yet.
            ExitCode::from(1)
        }
        Err(err) => {
            eprintln!("zapc: cannot read {path}: {err}");
            ExitCode::from(1)
        }
    }
}
