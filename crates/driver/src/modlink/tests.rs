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
        // pid + process-global counter, no clock (#917): a `SystemTime`-nanosecond
        // name collided for two tests passing the same `tag` in the same tick on a
        // microsecond-quantised clock. Same `zap-modlink-<tag>-` prefix as before.
        let dir = crate::testutil::unique_temp_dir(&format!("zap-modlink-{tag}"));
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
fn dependency_imports_its_own_nested_submodule() {
    // #1697: a dependency ships a nested module `src/<name>/<sub>.zap` and its
    // own top module imports it as `<name>.<sub>`. This is exactly how the
    // package resolves the import when built standalone (tier 3, full dotted
    // path under its own `src/`), so it must resolve the same way when the
    // package is pulled in as a dependency — the leading `<name>` segment
    // naming the package's *own* directory must not be stripped.
    let p = Project::new("depnested");
    p.write(
        "pkg/validation/src/validation/pattern.zap",
        "def is_valid(int64 n) -> int64:\n    return n % 2\n",
    );
    p.write(
        "pkg/validation/src/validation.zap",
        "import validation.pattern\n\ndef check(int64 n) -> int64:\n    \
         return pattern.is_valid(n)\n",
    );
    p.write(
        "proj/main.zap",
        "import validation\n\ndef main():\n    print(validation.check(7))\n",
    );
    let m = load_program_with(&p.entry("proj/main.zap"), &target(), vec![p.path("pkg")])
        .expect("links the dependency and its nested submodule");
    let names = fn_names(&m);
    assert!(names.contains(&"main".to_string()));
    assert!(
        names.iter().any(|n| n.contains("check")),
        "dependency top module should contribute, got {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("is_valid")),
        "nested submodule of the dependency should contribute, got {names:?}"
    );
}

#[test]
fn deep_nested_dependency_submodule_chain_resolves() {
    // Two levels of nesting inside one dependency, each importing the next:
    // `src/pkg.zap` -> `src/pkg/a.zap` -> `src/pkg/a/b.zap`. Guards the walk
    // past a single level of nesting (#1697).
    let p = Project::new("depdeep");
    p.write(
        "pkg/pkg/src/pkg/a/b.zap",
        "def leaf(int64 n) -> int64:\n    return n + 100\n",
    );
    p.write(
        "pkg/pkg/src/pkg/a.zap",
        "import pkg.a.b\n\ndef mid(int64 n) -> int64:\n    return b.leaf(n) + 10\n",
    );
    p.write(
        "pkg/pkg/src/pkg.zap",
        "import pkg.a\n\ndef top(int64 n) -> int64:\n    return a.mid(n)\n",
    );
    p.write(
        "proj/main.zap",
        "import pkg\n\ndef main():\n    print(pkg.top(1))\n",
    );
    let m = load_program_with(&p.entry("proj/main.zap"), &target(), vec![p.path("pkg")])
        .expect("links a two-level nested submodule chain");
    let names = fn_names(&m);
    assert!(names.iter().any(|n| n.contains("top")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("mid")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("leaf")), "{names:?}");
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

// --- arch.* off-target import gating (I#139 Tier 2, #962, D-0199) ----------

fn target_arch(arch: CfgArch) -> Target {
    Target::new(Some(arch), Some(zapc_syntax::ast::CfgOs::Linux))
}

#[test]
fn arch_x86_64_import_rejected_off_target() {
    // `import arch.x86_64` on an aarch64 build is a hard error (ZAP3019): the
    // honest "no silent fallback" contract (D-0079 rule 0.3).
    let p = Project::new("archx86off");
    p.write(
        "src/main.zap",
        "import arch.x86_64 as x86\n\ndef main():\n    print(1)\n",
    );
    let err = load_program(&p.entry("src/main.zap"), &target_arch(CfgArch::Aarch64)).unwrap_err();
    assert!(
        err.iter().any(|d| d.code == ErrorCode::ZAP3019),
        "expected ZAP3019 off-target arch import, got {err:?}"
    );
}

#[test]
fn arch_aarch64_import_rejected_off_target() {
    let p = Project::new("archarmoff");
    p.write(
        "src/main.zap",
        "import arch.aarch64 as a\n\ndef main():\n    print(1)\n",
    );
    let err = load_program(&p.entry("src/main.zap"), &target_arch(CfgArch::X86_64)).unwrap_err();
    assert!(
        err.iter().any(|d| d.code == ErrorCode::ZAP3019),
        "expected ZAP3019 off-target arch import, got {err:?}"
    );
}

#[test]
fn arch_x86_64_import_ok_on_target() {
    // On an x86-64 build the import links cleanly (it is a built-in: no file,
    // no diagnostic). The call is left for sema/lowering to resolve.
    let p = Project::new("archx86on");
    p.write(
        "src/main.zap",
        "import arch.x86_64 as x86\n\ndef main():\n    print(1)\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target_arch(CfgArch::X86_64)).expect("links");
    assert!(fn_names(&m).contains(&"main".to_string()));
}

#[test]
fn arch_aarch64_import_ok_on_target() {
    let p = Project::new("archarmon");
    p.write(
        "src/main.zap",
        "import arch.aarch64 as a\n\ndef main():\n    print(1)\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target_arch(CfgArch::Aarch64)).expect("links");
    assert!(fn_names(&m).contains(&"main".to_string()));
}

#[test]
fn imported_generic_bound_interface_name_is_mangled() {
    // #1222: a generic struct `Wrapper[T, C: Codec[T]]` declared in an imported
    // module. Before the fix the struct name, the interface declaration, and the
    // concrete argument were all mangled with the module prefix, but the
    // interface name *inside the type-parameter bound* stayed unqualified, so
    // the monomorphizer could not resolve it and reported "'Codec' is not an
    // interface". The bound's interface name must be mangled exactly like the
    // interface declaration, while the bound's own type argument `T` (a sibling
    // type parameter) stays bare.
    let p = Project::new("imported_bound");
    p.write(
        "src/codec.zap",
        "interface Codec[T]:\n\
         \x20   def decode(self, bytes raw) -> T\n\
         \n\
         struct TextCodec:\n\
         \x20   def decode(self, bytes raw) -> str:\n\
         \x20       return str(raw)\n\
         \n\
         struct Wrapper[T, C: Codec[T]]:\n\
         \x20   C codec\n\
         \n\
         \x20   def decode(self, bytes raw) -> T:\n\
         \x20       return self.codec.decode(raw)\n",
    );
    p.write(
        "src/main.zap",
        "import codec\n\n\
         def main():\n\
         \x20   auto w = codec.Wrapper[str, codec.TextCodec](codec.TextCodec())\n\
         \x20   print(w.decode(b\"hi\"))\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");

    // The interface declaration is mangled to some `<prefix>Codec`.
    let iface_name = m
        .items
        .iter()
        .find_map(|it| match it {
            Item::Interface(i) if i.name.ends_with("Codec") => Some(i.name.clone()),
            _ => None,
        })
        .expect("Codec interface present in merged module");
    assert_ne!(iface_name, "Codec", "imported interface must be mangled");

    // The `Wrapper` struct's `C` bound must name that same mangled interface,
    // and the bound's type argument must still be the bare sibling param `T`.
    let wrapper = m
        .items
        .iter()
        .find_map(|it| match it {
            Item::Struct(s) if s.name.ends_with("Wrapper") => Some(s),
            _ => None,
        })
        .expect("Wrapper struct present in merged module");
    let c_param = wrapper
        .generics
        .iter()
        .find(|g| g.name == "C")
        .expect("Wrapper has a `C` type parameter");
    let bound = c_param.bound.as_ref().expect("`C` carries a bound");
    match bound {
        zapc_syntax::ast::Type::Named { name, args, .. } => {
            assert_eq!(
                name, &iface_name,
                "bound interface name must match the mangled interface declaration, got {name:?}"
            );
            assert_ne!(
                name, "Codec",
                "bound interface name must not stay unqualified"
            );
            match args.as_slice() {
                [zapc_syntax::ast::Type::Named { name: arg, .. }] => assert_eq!(
                    arg, "T",
                    "the bound's type argument is the sibling param `T`, unmangled, got {arg:?}"
                ),
                other => panic!("expected a single `T` bound argument, got {other:?}"),
            }
        }
        other => panic!("bound must be a named interface type, got {other:?}"),
    }
}

#[test]
fn imported_generic_bound_capability_name_is_not_mangled() {
    // The bound rewrite (#1222) must only touch module-own interface symbols: a
    // built-in capability bound (`Comparable`) is not a module member, so it
    // keeps its bare name across the import boundary and is still resolved
    // intrinsically by the monomorphizer.
    let p = Project::new("imported_cap_bound");
    p.write(
        "src/ds.zap",
        "struct Holder[K: Comparable]:\n\
         \x20   K key\n",
    );
    p.write(
        "src/main.zap",
        "import ds\n\n\
         def main():\n\
         \x20   auto h = ds.Holder[int64](3)\n\
         \x20   print(h.key)\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    let holder = m
        .items
        .iter()
        .find_map(|it| match it {
            Item::Struct(s) if s.name.ends_with("Holder") => Some(s),
            _ => None,
        })
        .expect("Holder struct present");
    let bound = holder.generics[0]
        .bound
        .as_ref()
        .expect("K carries a bound");
    match bound {
        zapc_syntax::ast::Type::Named { name, .. } => assert_eq!(
            name, "Comparable",
            "a built-in capability bound must keep its bare name, got {name:?}"
        ),
        other => panic!("bound must be a named type, got {other:?}"),
    }
}

// ---- #1258: struct/enum method names are not mangled across imports --------
//
// A struct or enum *method* is not a module member — it lives in its type's
// namespace and is resolved by the `{Struct}__{method}` mangling in lowering,
// built from the type's own (already-prefixed) name. The module linker must
// therefore leave a method's own name alone. It used to run methods through the
// same rewrite as a top-level function, which mangled the method name whenever
// it collided with one of the declaring module's top-level symbols (a free
// function, struct, enum, … of the same name). The definition was then emitted
// under `{Struct}__{prefix}{method}` while every call site resolved it to
// `{Struct}__{method}`, so an imported module's call to that method failed with
// the generic `ZAP6001` ("no method '…' on that value"). Standalone the prefix
// is empty, so the mangle was a no-op and the gap stayed hidden.

/// The method names declared on the first struct whose (possibly mangled) name
/// ends with `suffix`.
fn struct_method_names(m: &Module, suffix: &str) -> Vec<String> {
    m.items
        .iter()
        .find_map(|it| match it {
            Item::Struct(s) if s.name.ends_with(suffix) => {
                Some(s.methods.iter().map(|f| f.name.clone()).collect())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no struct ending in {suffix:?} in merged module"))
}

/// The method names declared on the first enum whose (possibly mangled) name
/// ends with `suffix`.
fn enum_method_names(m: &Module, suffix: &str) -> Vec<String> {
    m.items
        .iter()
        .find_map(|it| match it {
            Item::Enum(e) if e.name.ends_with(suffix) => {
                Some(e.methods.iter().map(|f| f.name.clone()).collect())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no enum ending in {suffix:?} in merged module"))
}

#[test]
fn imported_struct_method_colliding_with_free_fn_keeps_bare_name() {
    // The exact #1258 shape: a method `result` whose name also names a free
    // function `result` in the same module. The struct and the free function are
    // module members and so are mangled; the method is not, and must keep its
    // bare name.
    let p = Project::new("method_vs_free_fn");
    p.write(
        "src/provider.zap",
        "struct Provider:\n\
         \x20   int64 id\n\n\
         \x20   def result(self) -> int64:\n\
         \x20       return self.id\n\n\
         const Provider _default = Provider(42)\n\n\
         def result() -> int64:\n\
         \x20   return _default.result()\n",
    );
    p.write(
        "src/main.zap",
        "import provider\n\n\
         def main():\n\
         \x20   print(provider.result())\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");

    // The struct is a module member → mangled.
    let struct_name = m
        .items
        .iter()
        .find_map(|it| match it {
            Item::Struct(s) if s.name.ends_with("Provider") => Some(s.name.clone()),
            _ => None,
        })
        .expect("Provider struct in merged module");
    assert_ne!(struct_name, "Provider", "imported struct must be mangled");

    // The free function `result` is a module member → mangled away.
    let fns = fn_names(&m);
    assert!(
        !fns.contains(&"result".to_string()),
        "the free `result` must be mangled, got {fns:?}"
    );
    assert!(
        fns.iter().any(|n| n.ends_with("result") && n != "result"),
        "a mangled free `result` must be present, got {fns:?}"
    );

    // The method, however, keeps its bare name — this is the fix.
    assert_eq!(
        struct_method_names(&m, "Provider"),
        vec!["result".to_string()],
        "the method name must not be mangled across the import boundary"
    );
}

#[test]
fn imported_enum_method_colliding_with_free_fn_keeps_bare_name() {
    // The enum arm of the same bug: an enum `self` method whose name collides
    // with a free function.
    let p = Project::new("enum_method_vs_free_fn");
    p.write(
        "src/lib.zap",
        "enum Level:\n\
         \x20   LOW\n\
         \x20   HIGH\n\n\
         \x20   def weight(self) -> int64:\n\
         \x20       match self:\n\
         \x20           case Level.LOW:\n\
         \x20               return 1\n\
         \x20           case Level.HIGH:\n\
         \x20               return 10\n\n\
         const Level _lvl = Level.HIGH\n\n\
         def weight() -> int64:\n\
         \x20   return _lvl.weight()\n",
    );
    p.write(
        "src/main.zap",
        "import lib\n\n\
         def main():\n\
         \x20   print(lib.weight())\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");

    let fns = fn_names(&m);
    assert!(
        !fns.contains(&"weight".to_string()),
        "the free `weight` must be mangled, got {fns:?}"
    );
    assert_eq!(
        enum_method_names(&m, "Level"),
        vec!["weight".to_string()],
        "the enum method name must not be mangled across the import boundary"
    );
}

#[test]
fn imported_struct_method_colliding_with_type_name_keeps_bare_name() {
    // The collision need not be with a free function: any top-level symbol of the
    // same name triggered the mangle. Here a method is named `Tag`, which also
    // names a top-level struct in the same module. The method must still keep its
    // bare name.
    let p = Project::new("method_vs_type_name");
    p.write(
        "src/lib.zap",
        "struct Tag:\n\
         \x20   int64 v\n\n\
         struct Holder:\n\
         \x20   int64 n\n\n\
         \x20   def Tag(self) -> int64:\n\
         \x20       return self.n\n\n\
         const Holder _h = Holder(9)\n\n\
         def get() -> int64:\n\
         \x20   return _h.Tag()\n",
    );
    p.write(
        "src/main.zap",
        "import lib\n\n\
         def main():\n\
         \x20   print(lib.get())\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");

    // The `Tag` struct is mangled (a module member)…
    let tag_struct = m
        .items
        .iter()
        .find_map(|it| match it {
            Item::Struct(s) if s.name.ends_with("Tag") => Some(s.name.clone()),
            _ => None,
        })
        .expect("Tag struct present");
    assert_ne!(tag_struct, "Tag", "the Tag struct must be mangled");
    // …but `Holder`'s method named `Tag` is not.
    assert_eq!(
        struct_method_names(&m, "Holder"),
        vec!["Tag".to_string()],
        "a method whose name collides with a type name must not be mangled"
    );
}

#[test]
fn imported_struct_method_without_collision_keeps_bare_name() {
    // Consistency pin: a method whose name collides with nothing was always left
    // bare (the mangle only fires on `own` members). This guards the general
    // behavior so a future change cannot start mangling method names wholesale.
    let p = Project::new("method_no_collision");
    p.write(
        "src/lib.zap",
        "struct Box:\n\
         \x20   int64 v\n\n\
         \x20   def unwrap(self) -> int64:\n\
         \x20       return self.v\n\n\
         const Box _b = Box(5)\n\n\
         def get() -> int64:\n\
         \x20   return _b.unwrap()\n",
    );
    p.write(
        "src/main.zap",
        "import lib\n\n\
         def main():\n\
         \x20   print(lib.get())\n",
    );
    let m = load_program(&p.entry("src/main.zap"), &target()).expect("links");
    assert_eq!(
        struct_method_names(&m, "Box"),
        vec!["unwrap".to_string()],
        "a non-colliding method name stays bare"
    );
}

#[test]
fn arch_import_guarded_by_cfg_compiles_on_both_targets() {
    // The intended portable pattern: a `when target.arch` guard keeps only the
    // matching arm, so one source carries both arch paths and the off-target
    // `import` is removed before it can error.
    let src = "\
when target.arch == .x86_64:
    import arch.x86_64 as a
elif target.arch == .aarch64:
    import arch.aarch64 as a

def main():
    print(1)
";
    let p = Project::new("archcfg");
    p.write("src/main.zap", src);
    for arch in [CfgArch::X86_64, CfgArch::Aarch64] {
        let m = load_program(&p.entry("src/main.zap"), &target_arch(arch))
            .unwrap_or_else(|e| panic!("cfg-guarded arch import should link for {arch:?}: {e:?}"));
        assert!(fn_names(&m).contains(&"main".to_string()));
    }
}
