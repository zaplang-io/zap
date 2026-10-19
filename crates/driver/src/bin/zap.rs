//! `zap` — the user-facing Zap CLI (D-0053).
//!
//! One predictable tool that drives the internal `zapc` compiler. The command
//! surface and dispatch live in `zapc::cli`; this binary is just the entry
//! point.

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    zapc::cli::run(&args)
}
