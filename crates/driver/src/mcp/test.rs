//! Targeted testing (D-0055 surface 4).
//!
//! Bridges the `zap_test` tool to the I#73 test runner (`crate::testing`),
//! running only the changed/affected paths an agent names and returning the
//! runner's structured `--json` report verbatim.
//!
//! Running tests compiles and executes them, which needs codegen: this surface
//! is therefore only functional in an `llvm`-featured build. Without it the
//! tool returns a clear, structured error rather than silently doing nothing.

use std::path::Path;

use super::ToolOutcome;

/// Run tests for `paths` (relative to `root`); empty means the whole project.
#[cfg(feature = "codegen")]
pub(super) fn run(root: &Path, paths: &[String]) -> ToolOutcome {
    let joined: Vec<std::path::PathBuf> = paths.iter().map(|p| root.join(p)).collect();
    let (json, _ok) = crate::testing::run_json(&joined);
    // The JSON itself carries `summary.ok`; a failing suite is a successful
    // tool call that *reports* failures, so `isError` stays false.
    ToolOutcome::ok(json)
}

/// Without a backend the runner cannot compile and execute tests.
#[cfg(not(feature = "codegen"))]
pub(super) fn run(_root: &Path, _paths: &[String]) -> ToolOutcome {
    ToolOutcome::err(
        "zap_test needs a codegen-enabled build (rebuild with --features \
         zapc/llvm, which requires an LLVM toolchain); this build cannot \
         compile and run tests"
            .to_string(),
    )
}
