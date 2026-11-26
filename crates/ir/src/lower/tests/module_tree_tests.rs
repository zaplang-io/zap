//! Source-tree guard: every `.rs` file under `src/lower/` belongs to the module
//! tree (I#1583).
//!
//! Rust silently ignores a source file that no `mod` declaration names. That is
//! how `lower/arrays.rs` went dead: it was carved out of `lower.rs` (#1231), a
//! later merge (#1235) dropped `mod arrays;` and put the methods back, and the
//! crate kept building with a stale second copy on disk. These tests turn that
//! silent state into a failure.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The names `source` declares as file-backed modules (`mod name;`), with any
/// same-line attributes and visibility. Inline modules (`mod name { .. }`) and
/// commented-out declarations name no file, so they do not count.
fn declared_modules(source: &str) -> BTreeSet<String> {
    source.lines().filter_map(declared_module).collect()
}

fn declared_module(line: &str) -> Option<String> {
    let mut rest = line.trim_start();
    while rest.starts_with("#[") {
        rest = rest[rest.find(']')? + 1..].trim_start();
    }
    if let Some(after_pub) = rest.strip_prefix("pub") {
        rest = after_pub;
        if rest.starts_with('(') {
            rest = &rest[rest.find(')')? + 1..];
        } else if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        rest = rest.trim_start();
    }
    let (name, _) = rest.strip_prefix("mod ")?.split_once(';')?;
    let name = name.trim();
    let is_ident = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    is_ident.then(|| name.to_string())
}

/// The file that must declare the modules living directly in `dir`:
/// `dir/mod.rs` if present, otherwise the sibling `dir.rs`.
fn owner_of(dir: &Path) -> PathBuf {
    let mod_rs = dir.join("mod.rs");
    if mod_rs.is_file() {
        mod_rs
    } else {
        dir.with_extension("rs")
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source directory") {
        let path = entry.expect("read directory entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn lower_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lower")
}

#[test]
fn every_source_file_under_lower_is_declared_as_a_module() {
    let mut files = Vec::new();
    rust_files(&lower_dir(), &mut files);

    let mut orphans = Vec::new();
    for file in &files {
        // `x/mod.rs` is the module `x`, declared one level further up.
        let module = if file.file_name().is_some_and(|n| n == "mod.rs") {
            file.parent().expect("mod.rs has a parent").to_path_buf()
        } else {
            file.with_extension("")
        };
        let name = module
            .file_name()
            .and_then(|n| n.to_str())
            .expect("UTF-8 module name");
        let owner = owner_of(module.parent().expect("module has a parent"));
        let source =
            fs::read_to_string(&owner).unwrap_or_else(|e| panic!("read {}: {e}", owner.display()));
        if !declared_modules(&source).contains(name) {
            orphans.push(format!(
                "{} (no `mod {name};` in {})",
                file.display(),
                owner.display()
            ));
        }
    }
    orphans.sort();

    assert!(
        orphans.is_empty(),
        "source files that are never compiled:\n  {}\ndeclare each one or delete it",
        orphans.join("\n  ")
    );
}

#[test]
fn the_scan_reaches_every_level_of_the_lowering_tree() {
    // Guards the test above against passing vacuously (wrong root, or a walk
    // that stops recursing).
    let mut files = Vec::new();
    rust_files(&lower_dir(), &mut files);
    for expected in ["stmts.rs", "modules/mod.rs", "tests/mod.rs"] {
        assert!(
            files.contains(&lower_dir().join(expected)),
            "scan missed src/lower/{expected}"
        );
    }
}

#[test]
fn a_directory_is_owned_by_its_mod_rs_or_its_sibling_file() {
    assert_eq!(owner_of(&lower_dir()), lower_dir().with_extension("rs"));
    assert_eq!(
        owner_of(&lower_dir().join("tests")),
        lower_dir().join("tests/mod.rs")
    );
}

#[test]
fn file_backed_declarations_are_recognised() {
    for line in [
        "mod arrays;",
        "    mod arrays;",
        "pub mod arrays;",
        "pub(crate) mod arrays;",
        "pub(super) mod arrays;",
        "pub(in crate::lower) mod arrays;",
        "#[cfg(test)] mod arrays;",
        "mod arrays; // lowering",
    ] {
        assert_eq!(
            declared_module(line).as_deref(),
            Some("arrays"),
            "{line:?} declares `arrays`"
        );
    }
}

#[test]
fn lines_that_name_no_file_are_not_declarations() {
    for line in [
        "// mod arrays;",
        "/// mod arrays;",
        "//! `mod arrays;`",
        "mod arrays {",
        "mod arrays { fn f() {} }",
        "use arrays;",
        "pubmod arrays;",
        "modarrays;",
        "let s = \"mod arrays;\";",
        "",
    ] {
        assert_eq!(declared_module(line), None, "{line:?} declares nothing");
    }
}

#[test]
fn a_dropped_declaration_is_not_satisfied_by_a_similar_name() {
    let names = declared_modules("mod arrays_extra;\nmod nd_arrays;\n// mod arrays;\n");
    assert!(!names.contains("arrays"));
    assert_eq!(names.len(), 2);
}
