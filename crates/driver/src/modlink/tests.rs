//! Unit tests for the module linker (I#283/D-0081). These build a temporary
//! project tree on disk, run [`load_program`], and assert on the merged AST or
//! the linker diagnostics — no LLVM needed.

use super::*;
use std::io::Write;

/// A throwaway project directory under the OS temp dir, cleaned on drop.
struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "zap-modlink-{tag}-{}-{}",
            std::process::id(),
            fastrand_u64()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Project { dir }
    }

    fn write(&self, rel: &str, src: &str) -> PathBuf {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(src.as_bytes()).unwrap();
        path
    }

    fn entry(&self, rel: &str) -> String {
        self.dir.join(rel).display().to_string()
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// A tiny non-crypto pseudo-random for unique temp names (avoids a dep).
fn fastrand_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

fn target() -> Target {
    Target::host()
}

/// Collect every function name present in a merged module.
fn fn_names(m: &Module) -> Vec<String> {
    m.items
        .iter()
        .filter_map(|it| match it {
            Item::Function(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn single_file_no_imports_is_unchanged() {
    let p = Project::new("single");
    p.write("src/main.zap", "def main():\n    print(1)\n");
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert_eq!(fn_names(&m), vec!["main".to_string()]);
}

#[test]
fn imported_function_is_mangled_and_call_rewritten() {
    // The #283 repro: a.zap defines helper, main imports + calls it.
    let p = Project::new("repro");
    p.write("src/a.zap", "def helper() -> int64:\n    return 7\n");
    p.write(
        "src/main.zap",
        "import a\n\ndef main():\n    print(a.helper())\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    let names = fn_names(&m);
    // main keeps its name; helper is mangled under a's prefix.
    assert!(names.contains(&"main".to_string()));
    assert!(
        names.iter().any(|n| n.contains("helper") && n != "helper"),
        "helper should be mangled, got {names:?}"
    );
    assert!(
        !names.contains(&"helper".to_string()),
        "the bare name must not survive: {names:?}"
    );
    // The call site now references the mangled symbol, not `a.helper`.
    let mangled = names.iter().find(|n| n.contains("helper")).unwrap();
    let main_fn = m
        .items
        .iter()
        .find_map(|it| match it {
            Item::Function(f) if f.name == "main" => Some(f),
            _ => None,
        })
        .unwrap();
    let body = format!("{main_fn:?}");
    assert!(body.contains(mangled), "call not rewritten: {body}");
}

#[test]
fn private_member_access_is_rejected() {
    let p = Project::new("priv");
    p.write("src/a.zap", "def _secret() -> int64:\n    return 1\n");
    p.write(
        "src/main.zap",
        "import a\n\ndef main():\n    print(a._secret())\n",
    );
    let err = load_program(&p.entry("src/main.zap"), &target()).unwrap_err();
    assert!(
        err.iter().any(|d| d.code == ErrorCode::ZAP3012),
        "expected ZAP3012, got {:?}",
        err.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

#[test]
fn missing_module_is_reported() {
    let p = Project::new("missing");
    p.write("src/main.zap", "import nope\n\ndef main():\n    print(1)\n");
    let err = load_program(&p.entry("src/main.zap"), &target()).unwrap_err();
    assert!(err.iter().any(|d| d.code == ErrorCode::ZAP3010));
}

#[test]
fn circular_import_is_rejected() {
    let p = Project::new("cycle");
    p.write(
        "src/a.zap",
        "import b\n\ndef fa() -> int64:\n    return 1\n",
    );
    p.write(
        "src/b.zap",
        "import a\n\ndef fb() -> int64:\n    return 2\n",
    );
    p.write(
        "src/main.zap",
        "import a\nimport b\n\ndef main():\n    print(a.fa())\n",
    );
    let err = load_program(&p.entry("src/main.zap"), &target()).unwrap_err();
    assert!(
        err.iter().any(|d| d.code == ErrorCode::ZAP3011),
        "expected ZAP3011, got {:?}",
        err.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

#[test]
fn top_level_statement_in_imported_module_is_rejected() {
    let p = Project::new("declonly");
    p.write(
        "src/a.zap",
        "print(1)\n\ndef helper() -> int64:\n    return 1\n",
    );
    p.write(
        "src/main.zap",
        "import a\n\ndef main():\n    print(a.helper())\n",
    );
    let err = load_program(&p.entry("src/main.zap"), &target()).unwrap_err();
    assert!(err.iter().any(|d| d.code == ErrorCode::ZAP3014));
}

#[test]
fn top_level_binding_before_decorated_def_is_diagnosed_not_ice() {
    // The exact ticket #459 repro: a module-level binding before a decorated
    // `def`. This used to hit `unreachable!("entry statements handled via merge
    // list")` in the rewrite. It must now be a clean diagnostic (ZAP3016), not
    // a panic: a compiled program runs from `def main()`, so the top-level
    // `auto app = App(0)` would never run.
    let p = Project::new("entrystmt_ice");
    p.write(
        "src/main.zap",
        "auto app = App(0)\n@app.get(\"/x\")\ndef h() -> int64:\n    return 0\n",
    );
    let err = load_program(&p.entry("src/main.zap"), &target()).unwrap_err();
    assert!(
        err.iter().any(|d| d.code == ErrorCode::ZAP3016),
        "expected ZAP3016, got {err:?}"
    );
}

#[test]
fn top_level_executable_statement_in_entry_is_rejected() {
    // A bare executable statement at entry-module scope (no decorator involved)
    // is rejected the same way — it would silently never run otherwise.
    let p = Project::new("entrystmt_plain");
    p.write("src/main.zap", "print(1)\n");
    let err = load_program(&p.entry("src/main.zap"), &target()).unwrap_err();
    assert!(err.iter().any(|d| d.code == ErrorCode::ZAP3016));
}

#[test]
fn top_level_statement_between_defs_in_entry_is_rejected() {
    // Interleaving places too: a statement sandwiched between two `def`s.
    let p = Project::new("entrystmt_between");
    p.write(
        "src/main.zap",
        "def a() -> int64:\n    return 1\nauto x = 2\ndef main() -> int64:\n    return a()\n",
    );
    let err = load_program(&p.entry("src/main.zap"), &target()).unwrap_err();
    assert!(err.iter().any(|d| d.code == ErrorCode::ZAP3016));
}

#[test]
fn module_docstring_is_allowed_in_entry() {
    // A leading bare-string module docstring (D-0041) carries no behaviour, so
    // it is allowed at module scope even though an executable statement is not.
    let p = Project::new("entry_doc");
    p.write(
        "src/main.zap",
        "\"\"\"My entry module.\"\"\"\ndef main() -> int64:\n    return 0\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(fn_names(&m).contains(&"main".to_string()));
}

#[test]
fn module_docstring_is_allowed_in_imported_module() {
    // The docstring exemption applies to imported modules too: a declaration-
    // only module may still carry a leading docstring.
    let p = Project::new("imported_doc");
    p.write(
        "src/a.zap",
        "\"\"\"Module a.\"\"\"\ndef helper() -> int64:\n    return 1\n",
    );
    p.write(
        "src/main.zap",
        "import a\n\ndef main():\n    print(a.helper())\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(fn_names(&m).iter().any(|n| n.contains("helper")));
}

#[test]
fn local_shadowing_an_import_is_not_treated_as_module_access() {
    // A local variable named like the import: `a.foo` is a value attr, not a
    // module access, so nothing is rewritten and no privacy error fires.
    let p = Project::new("shadow");
    p.write("src/a.zap", "def foo() -> int64:\n    return 1\n");
    p.write(
        "src/main.zap",
        "import a\n\ndef main():\n    a = 3\n    print(a)\n",
    );
    // Links cleanly; `a` as a value is left alone (no ZAP3015 because it is a
    // local, not a bare module name).
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(fn_names(&m).contains(&"main".to_string()));
}

// --- Stage B: cross-package dependency imports (via explicit pkg roots) ------

#[test]
fn dependency_import_resolves_and_mangles() {
    // A dependency `aws_core` materialized at <pkg>/aws_core/src/aws_core.zap.
    let p = Project::new("dep");
    p.write(
        "pkg/aws_core/src/aws_core.zap",
        "def sign(int64 n) -> int64:\n    return n + 1\n",
    );
    p.write(
        "proj/main.zap",
        "import aws_core\n\ndef main():\n    print(aws_core.sign(5))\n",
    );
    let m = load_program_with(&p.entry("proj/main.zap"), &target(), vec![p.path("pkg")])
        .expect("links against the dependency");
    let names = fn_names(&m);
    assert!(names.contains(&"main".to_string()));
    assert!(
        names.iter().any(|n| n.contains("sign") && n != "sign"),
        "dependency symbol should be mangled, got {names:?}"
    );
}

#[test]
fn dependency_submodule_resolves() {
    // `import aws_core.sigv4` -> <pkg>/aws_core/src/sigv4.zap.
    let p = Project::new("depsub");
    p.write(
        "pkg/aws_core/src/sigv4.zap",
        "def canon(int64 n) -> int64:\n    return n * 2\n",
    );
    p.write(
        "proj/main.zap",
        "import aws_core.sigv4\n\ndef main():\n    print(sigv4.canon(3))\n",
    );
    let m = load_program_with(&p.entry("proj/main.zap"), &target(), vec![p.path("pkg")])
        .expect("links the submodule");
    assert!(fn_names(&m).iter().any(|n| n.contains("canon")));
}

#[test]
fn service_depends_on_core_transitively() {
    // main -> aws_s3 (dep) -> aws_core (dep): a service package whose own module
    // imports the shared core, mirroring D-0011 §8.
    let p = Project::new("svc");
    p.write(
        "pkg/aws_core/src/aws_core.zap",
        "def sign(int64 n) -> int64:\n    return n + 1\n",
    );
    p.write(
        "pkg/aws_s3/src/aws_s3.zap",
        "import aws_core\n\ndef put(int64 n) -> int64:\n    return aws_core.sign(n) + 10\n",
    );
    p.write(
        "proj/main.zap",
        "import aws_s3\n\ndef main():\n    print(aws_s3.put(5))\n",
    );
    let m = load_program_with(&p.entry("proj/main.zap"), &target(), vec![p.path("pkg")])
        .expect("links the service + core");
    // All three modules contributed functions; each dep symbol is mangled.
    let names = fn_names(&m);
    assert!(names.contains(&"main".to_string()));
    assert!(names.iter().any(|n| n.contains("put")));
    assert!(names.iter().any(|n| n.contains("sign")));
}

#[test]
fn missing_dependency_module_reports() {
    // The dependency dir exists but the requested module file does not.
    let p = Project::new("depmissing");
    p.write(
        "pkg/aws_core/src/aws_core.zap",
        "def sign() -> int64:\n    return 1\n",
    );
    p.write(
        "proj/main.zap",
        "import aws_core.nope\n\ndef main():\n    print(1)\n",
    );
    let err =
        load_program_with(&p.entry("proj/main.zap"), &target(), vec![p.path("pkg")]).unwrap_err();
    assert!(err.iter().any(|d| d.code == ErrorCode::ZAP3010));
}
