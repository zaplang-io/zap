//! Test discovery (D-0054): find the `.zap` files that may hold `@test`
//! functions, using the *same* file layout the build uses (single file, or a
//! `zap.toml` project with `src/main.zap`). There is no test-only import path;
//! external-dependency resolution awaits the package manager (#69).
//!
//! A file is a candidate when it is the project entry (`src/main.zap`, which may
//! carry inline `@test` functions), ends in `_test.zap`, or lives under a
//! `tests/` directory. The runner later keeps only the candidates that actually
//! define a test, so over-inclusion here is harmless.

use std::path::{Path, PathBuf};

/// Resolve the set of test files for a `zap test` invocation.
///
/// `explicit` is a path the user named (a single file, or a directory to scan).
/// With no path, the current directory is scanned in project mode.
pub fn discover(explicit: Option<&str>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    match explicit {
        Some(p) => {
            let path = Path::new(p);
            if path.is_dir() {
                walk(path, &mut files);
            } else {
                files.push(path.to_path_buf());
            }
        }
        None => {
            let root = Path::new(".");
            // The project entry may carry inline `@test` functions.
            let entry = Path::new("src/main.zap");
            if entry.exists() {
                files.push(entry.to_path_buf());
            }
            walk(root, &mut files);
        }
    }
    files.sort();
    files.dedup();
    files
}

/// Resolve the `.zap` files for a `zap test --doc` run (#234). Doctests live in
/// the docstrings of *ordinary* source, not just `*_test.zap`, so this collects
/// **every** `.zap` file under the target (a single file is used directly; a
/// directory, or no path at all, is walked recursively), skipping build/VCS
/// directories.
pub fn discover_all(explicit: Option<&str>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let root = match explicit {
        Some(p) => {
            let path = Path::new(p);
            if !path.is_dir() {
                return vec![path.to_path_buf()];
            }
            path.to_path_buf()
        }
        None => PathBuf::from("."),
    };
    collect_zap(&root, &mut files);
    files.sort();
    files.dedup();
    files
}

/// Is `name` a candidate test file by name (`*_test.zap`)?
fn is_test_named(name: &str) -> bool {
    name.ends_with("_test.zap")
}

/// Recursively collect candidate test files under `dir`, skipping build and VCS
/// directories.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if is_skipped_dir(&name) {
                continue;
            }
            // Everything under a `tests/` directory is a candidate; elsewhere we
            // keep recursing to find `*_test.zap` files.
            if name == "tests" {
                collect_zap(&path, out);
            } else {
                walk(&path, out);
            }
        } else if is_test_named(&name) {
            out.push(path);
        }
    }
}

/// Collect every `.zap` file at or below `dir` (used for a `tests/` tree).
fn collect_zap(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            if !is_skipped_dir(&name.to_string_lossy()) {
                collect_zap(&path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("zap") {
            out.push(path);
        }
    }
}

fn is_skipped_dir(name: &str) -> bool {
    matches!(name, "target" | ".git" | ".claude" | "node_modules") || name.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_named_detection() {
        assert!(is_test_named("math_test.zap"));
        assert!(!is_test_named("math.zap"));
        assert!(!is_test_named("test_math.zap")); // Go-style suffix, not prefix
    }

    #[test]
    fn skipped_dirs() {
        assert!(is_skipped_dir("target"));
        assert!(is_skipped_dir(".git"));
        assert!(is_skipped_dir(".hidden"));
        assert!(!is_skipped_dir("tests"));
        assert!(!is_skipped_dir("src"));
    }

    #[test]
    fn explicit_file_is_used_directly() {
        let files = discover(Some("some/specific/file.zap"));
        assert_eq!(files, vec![PathBuf::from("some/specific/file.zap")]);
    }
}
