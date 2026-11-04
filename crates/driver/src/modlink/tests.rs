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

/// Collect module-level `const` binding names from a merged module, in merged
/// (import-topological + source) order (D-0112).
fn module_const_names(m: &Module) -> Vec<String> {
    m.items
        .iter()
        .filter_map(|it| match it {
            Item::Stmt(Stmt::Let {
                is_const: true,
                name,
                ..
            }) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Collect every module-level binding name (const and mutable alike) from a
/// merged module, in merged (import-topological + source) order (D-0112).
fn module_binding_names(m: &Module) -> Vec<String> {
    m.items
        .iter()
        .filter_map(|it| match it {
            Item::Stmt(Stmt::Let { name, .. }) => Some(name.clone()),
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
fn top_level_mutable_binding_before_def_links_not_ice() {
    // The ticket #459 shape: a module-level binding before a `def`. This used to
    // hit `unreachable!("entry statements handled via merge list")` in the
    // rewrite. A module-level mutable declaration (`int64 counter = 0`) is now a
    // persistent mutable global (D-0112, #662 Phase 2), so it links cleanly —
    // the regression guard is that it neither panics nor is rejected.
    let p = Project::new("entrystmt_ice");
    p.write(
        "src/main.zap",
        "int64 counter = 0\n\ndef main() -> int64:\n    counter += 1\n    return counter\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(fn_names(&m).contains(&"main".to_string()));
    assert!(
        module_binding_names(&m).iter().any(|n| n == "counter"),
        "the mutable global is a module member"
    );
}

#[test]
fn module_level_tuple_destructuring_is_rejected() {
    // A module-level tuple-destructuring declaration (`int64 a, b = ...`) is NOT
    // a single-name binding, so it is not admitted at module scope — each
    // binding must be declared on its own line (ZAP3018, D-0112). This is the
    // one declaration-with-initializer form still rejected after #662.
    let p = Project::new("entry_tuple_decl");
    p.write(
        "src/main.zap",
        "auto a, b = (1, 2)\n\ndef main() -> int64:\n    return a + b\n",
    );
    let err = load_program(&p.entry("src/main.zap"), &target()).unwrap_err();
    assert!(
        err.iter().any(|d| d.code == ErrorCode::ZAP3018),
        "expected ZAP3018, got {err:?}"
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
fn top_level_executable_statement_between_defs_in_entry_is_rejected() {
    // Interleaving places too: a bare executable statement sandwiched between two
    // `def`s is still rejected (ZAP3016) — it would never run. A module-level
    // declaration (`auto x = 2`) IS now admitted here (D-0112, #662), so the
    // rejected case uses a genuine executable statement.
    let p = Project::new("entrystmt_between");
    p.write(
        "src/main.zap",
        "def a() -> int64:\n    return 1\nprint(1)\ndef main() -> int64:\n    return a()\n",
    );
    let err = load_program(&p.entry("src/main.zap"), &target()).unwrap_err();
    assert!(err.iter().any(|d| d.code == ErrorCode::ZAP3016));
}

#[test]
fn module_mutable_binding_between_defs_in_entry_is_allowed() {
    // A module-level mutable binding may sit anywhere among the top-level items
    // (D-0112, #662) — it is a declaration, not an executable statement.
    let p = Project::new("mutglobal_between");
    p.write(
        "src/main.zap",
        "def a() -> int64:\n    return 1\nauto x = 2\ndef main() -> int64:\n    return a() + x\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(module_binding_names(&m).iter().any(|n| n == "x"));
}

#[test]
fn module_const_is_allowed_in_entry() {
    // A module-level `const` (D-0112, #658 Phase 1) is persistent immutable
    // state, not a rejected top-level statement: the program links.
    let p = Project::new("entry_const");
    p.write(
        "src/main.zap",
        "const int64 LIMIT = 100\n\ndef main() -> int64:\n    return LIMIT\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(fn_names(&m).contains(&"main".to_string()));
}

#[test]
fn module_const_is_allowed_and_reachable_across_modules() {
    // A `const` declared in an imported module is a module member: it is
    // reachable as `alias.NAME` and mangled with the module prefix so it does
    // not collide with a same-named const in the importer.
    let p = Project::new("const_xmod");
    p.write(
        "src/conf.zap",
        "const int64 LIMIT = 100\ndef cap() -> int64:\n    return LIMIT\n",
    );
    p.write(
        "src/main.zap",
        "import conf\n\nconst int64 LIMIT = 5\n\ndef main() -> int64:\n    return conf.LIMIT + LIMIT\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(fn_names(&m).contains(&"main".to_string()));
    // Both consts survive the merge under distinct (mangled) names: the entry's
    // bare `LIMIT` and the dependency's prefixed one.
    let consts = module_const_names(&m);
    assert!(
        consts.iter().any(|n| n == "LIMIT"),
        "entry const kept its bare name, got {consts:?}"
    );
    assert!(
        consts.iter().any(|n| n != "LIMIT" && n.ends_with("LIMIT")),
        "imported const mangled with module prefix, got {consts:?}"
    );
    // Import-topological order: the dependency's const initializes before the
    // entry's (its slot is declared first in the merged item list).
    let dep = consts
        .iter()
        .position(|n| n != "LIMIT" && n.ends_with("LIMIT"));
    let own = consts.iter().position(|n| n == "LIMIT");
    assert!(
        dep < own,
        "dependency const must precede entry const: {consts:?}"
    );
}

#[test]
fn module_mutable_binding_in_entry_is_allowed() {
    // A module-level mutable binding (D-0112, #662 Phase 2) is persistent
    // mutable state, not a rejected top-level statement: the program links and
    // the binding is a module member.
    let p = Project::new("entry_mutglobal");
    p.write(
        "src/main.zap",
        "int64 counter = 0\n\ndef main() -> int64:\n    counter = counter + 1\n    return counter\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(fn_names(&m).contains(&"main".to_string()));
    assert!(module_binding_names(&m).iter().any(|n| n == "counter"));
}

#[test]
fn module_mutable_binding_is_allowed_and_reachable_across_modules() {
    // A mutable global declared in an imported module is a module member, like a
    // `const`: reachable as `alias.NAME`, mangled with the module prefix so it
    // never collides with a same-named global in the importer, and ordered
    // dependency-first for eager init (D-0112, #662).
    let p = Project::new("mutglobal_xmod");
    p.write(
        "src/state.zap",
        "int64 counter = 0\ndef tick() -> int64:\n    counter += 1\n    return counter\n",
    );
    p.write(
        "src/main.zap",
        "import state\n\nint64 counter = 100\n\ndef main() -> int64:\n    return state.tick() + counter\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(fn_names(&m).contains(&"main".to_string()));
    let names = module_binding_names(&m);
    assert!(
        names.iter().any(|n| n == "counter"),
        "entry global kept its bare name, got {names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n != "counter" && n.ends_with("counter")),
        "imported global mangled with module prefix, got {names:?}"
    );
    // Import-topological order: the dependency's global initializes before the
    // entry's (its slot is declared first in the merged item list).
    let dep = names
        .iter()
        .position(|n| n != "counter" && n.ends_with("counter"));
    let own = names.iter().position(|n| n == "counter");
    assert!(
        dep < own,
        "dependency global must precede entry global: {names:?}"
    );
}

#[test]
fn module_const_is_allowed_in_imported_module() {
    // A declaration-only imported module may carry a `const` (it is a
    // declaration, not executable code).
    let p = Project::new("imported_const");
    p.write(
        "src/a.zap",
        "const int64 K = 3\ndef helper() -> int64:\n    return K\n",
    );
    p.write(
        "src/main.zap",
        "import a\n\ndef main() -> int64:\n    return a.helper()\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert!(fn_names(&m).iter().any(|n| n.contains("helper")));
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
