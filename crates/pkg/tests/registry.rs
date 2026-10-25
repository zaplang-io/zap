//! End-to-end registry read-path tests against a local static-file fixture
//! index (design/0027-registry.md §7, I#236).
//!
//! Every test builds a throwaway index tree on disk — `config.json`, per-package
//! line-delimited-JSON files at their Cargo-sparse fan-out paths, and
//! deterministic archives — then points a project's `[registries]` table at it
//! over `file://`. No network, no hosting. This exercises the real parse +
//! fetch + MVS + download + verify + cache + materialize path, not a mock.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use zapc_pkg::archive::{self, Codec};
use zapc_pkg::checksum::checksum_tree;
use zapc_pkg::index::{self, Bucketing};
use zapc_pkg::name::RegistryName;
use zapc_pkg::ops;
use zapc_pkg::zenv;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zapc_reg-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Point the content cache at a per-process temp root so tests never touch the
/// real `$HOME/.zap`. Content-addressing + unique package names keep a shared
/// root parallel-safe. Set once.
fn isolate_cache() {
    use std::sync::OnceLock;
    static CACHE: OnceLock<PathBuf> = OnceLock::new();
    let dir = CACHE.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("zapc_reg_cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    });
    std::env::set_var(zenv::ZAP_CACHE, dir);
}

/// A package version to publish into the fixture index.
struct Pkg {
    name: &'static str,
    vers: &'static str,
    /// `(dep registry name, min req)` pairs.
    deps: Vec<(&'static str, &'static str)>,
    yanked: bool,
    /// Record an `acksum`? (The archive-bytes pre-check.)
    with_acksum: bool,
    /// A deliberately-wrong `zap1:` checksum to force a mismatch, else None.
    bad_cksum: Option<&'static str>,
    /// Corrupt the archive bytes after building (transfer tamper).
    tamper_archive: bool,
}

impl Pkg {
    fn new(name: &'static str, vers: &'static str) -> Pkg {
        Pkg {
            name,
            vers,
            deps: vec![],
            yanked: false,
            with_acksum: true,
            bad_cksum: None,
            tamper_archive: false,
        }
    }
    fn dep(mut self, name: &'static str, req: &'static str) -> Pkg {
        self.deps.push((name, req));
        self
    }
    fn yanked(mut self) -> Pkg {
        self.yanked = true;
        self
    }
}

/// A fixture index being assembled on disk.
struct Fixture {
    root: PathBuf,
    index: PathBuf,
    archives: PathBuf,
    lines: BTreeMap<String, Vec<String>>,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let root = unique(tag);
        let index = root.join("index");
        let archives = root.join("archives");
        fs::create_dir_all(&index).unwrap();
        fs::create_dir_all(&archives).unwrap();
        Fixture {
            root,
            index,
            archives,
            lines: BTreeMap::new(),
        }
    }

    /// `file://` URL of the index base, for a `[registries]` entry.
    fn index_url(&self) -> String {
        zapc_pkg::registry::file_url(&self.index)
    }

    /// Build a package's source tree (unique content per name+vers), archive it,
    /// and append its index line.
    fn publish(&mut self, pkg: &Pkg) {
        let rn = RegistryName::parse(pkg.name).unwrap();
        // A small, unique source tree.
        let src = self
            .root
            .join("src")
            .join(format!("{}-{}", pkg.name, pkg.vers));
        fs::create_dir_all(src.join("src")).unwrap();
        fs::write(
            src.join("zap.toml"),
            format!(
                "[package]\nname = \"{}\"\nversion = \"{}\"\n",
                pkg.name, pkg.vers
            ),
        )
        .unwrap();
        fs::write(
            src.join("src/lib.zap"),
            format!("# {} {}\ndef f(): pass\n", pkg.name, pkg.vers),
        )
        .unwrap();

        let real_cksum = checksum_tree(&src).unwrap();
        let mut bytes = archive::build(&src, Codec::Store).unwrap();
        let acksum = archive::digest(&bytes);
        if pkg.tamper_archive {
            // Flip a byte in the file-data region so the content hash changes.
            let n = bytes.len();
            bytes[n - 1024] ^= 0xff;
        }
        // dl template: <archives>/<name>/<name>-<vers>.zap.tar
        let adir = self.archives.join(rn.normalized());
        fs::create_dir_all(&adir).unwrap();
        fs::write(
            adir.join(format!(
                "{}-{}.{}",
                rn.normalized(),
                pkg.vers,
                Codec::Store.extension()
            )),
            &bytes,
        )
        .unwrap();

        let cksum = pkg.bad_cksum.map(str::to_string).unwrap_or(real_cksum);
        let mut obj = vec![
            ("name", json_str(&rn.normalized())),
            ("vers", json_str(pkg.vers)),
            ("deps", deps_json(&pkg.deps)),
            ("cksum", json_str(&cksum)),
            ("yanked", zapc_json::Json::Bool(pkg.yanked)),
        ];
        if pkg.with_acksum {
            obj.push(("acksum", json_str(&acksum)));
        }
        let line = zapc_json::Json::obj(obj).to_string();
        self.lines.entry(rn.normalized()).or_default().push(line);
    }

    /// Write `config.json` and every package index file at its fan-out path.
    fn finish(&self) {
        let dl = format!(
            "{}/{{name}}/{{name}}-{{vers}}.{}",
            zapc_pkg::registry::file_url(&self.archives),
            Codec::Store.extension()
        );
        let config = zapc_json::Json::obj(vec![
            ("index_version", zapc_json::Json::int(1)),
            ("dl", json_str(&dl)),
            ("bucketing", json_str("cargo-sparse")),
        ]);
        fs::write(self.index.join("config.json"), config.to_string()).unwrap();
        for (name, lines) in &self.lines {
            let rn = RegistryName::parse(name).unwrap();
            let rel = index::index_path(&rn, Bucketing::CargoSparse);
            let path = self.index.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
        }
    }
}

fn json_str(s: &str) -> zapc_json::Json {
    zapc_json::Json::str(s)
}

fn deps_json(deps: &[(&str, &str)]) -> zapc_json::Json {
    let items = deps
        .iter()
        .map(|(n, req)| {
            let major = semver::Version::parse(req).map(|v| v.major).unwrap_or(0);
            zapc_json::Json::obj(vec![
                (
                    "name",
                    json_str(&RegistryName::parse(n).unwrap().normalized()),
                ),
                ("req", json_str(req)),
                ("major", zapc_json::Json::int(major as i64)),
            ])
        })
        .collect();
    zapc_json::Json::Arr(items)
}

/// Write a project `zap.toml` whose single `test` index points at `fx`.
fn project(root: &Path, fx: &Fixture, deps_body: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(
        ops::manifest_path(root),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [registries]\ntest = {{ index = \"{}\" }}\n\n\
             [dependencies]\n{deps_body}",
            fx.index_url()
        ),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------

#[test]
fn resolves_registry_graph_and_writes_lock() {
    isolate_cache();
    let mut fx = Fixture::new("resolve");
    fx.publish(&Pkg::new("bytesx", "0.3.0"));
    fx.publish(&Pkg::new("jsonlib", "1.4.0").dep("bytesx", "0.3.0"));
    fx.finish();

    let app = unique("resolve-app");
    project(
        &app,
        &fx,
        "json = { registry = \"jsonlib\", version = \"1.4.0\", index = \"test\" }\n",
    );

    let (lock, warnings) = ops::relock_reporting(&app).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    // Both the root (under its registry name) and the transitive dep are locked.
    let names: Vec<_> = lock.modules.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"jsonlib"), "{names:?}");
    assert!(names.contains(&"bytesx"), "{names:?}");

    let jsonlib = lock.modules.iter().find(|m| m.name == "jsonlib").unwrap();
    assert_eq!(jsonlib.source, "registry+test#jsonlib");
    assert!(jsonlib.checksum.as_deref().unwrap().starts_with("zap1:"));
    assert_eq!(jsonlib.version, "1.4.0");

    // The lock round-trips through serialize/parse.
    let text = lock.to_toml().unwrap();
    let back = zapc_pkg::lockfile::Lockfile::parse(&text).unwrap();
    assert_eq!(lock, back);
}

#[test]
fn mixes_registry_and_path_in_one_graph() {
    isolate_cache();
    let mut fx = Fixture::new("mixed");
    fx.publish(&Pkg::new("jsonlib", "1.4.0"));
    fx.finish();

    let base = unique("mixed-base");
    let app = base.join("app");
    let localdep = base.join("localdep");
    fs::create_dir_all(localdep.join("src")).unwrap();
    fs::write(
        ops::manifest_path(&localdep),
        "[package]\nname = \"localdep\"\nversion = \"0.2.0\"\n",
    )
    .unwrap();
    fs::write(localdep.join("src/lib.zap"), "def g(): pass\n").unwrap();

    project(
        &app,
        &fx,
        "json = { registry = \"jsonlib\", version = \"1.4.0\", index = \"test\" }\n\
         localdep = { path = \"../localdep\" }\n",
    );

    let (lock, _) = ops::relock_reporting(&app).unwrap();
    let json = lock.modules.iter().find(|m| m.name == "jsonlib").unwrap();
    let local = lock.modules.iter().find(|m| m.name == "localdep").unwrap();
    assert_eq!(json.source, "registry+test#jsonlib");
    assert!(local.source.starts_with("path+"));
    assert!(local.checksum.as_deref().unwrap().starts_with("zap1:"));
}

#[test]
fn yanked_skipped_for_fresh_floor_but_resolvable_when_pinned() {
    isolate_cache();
    let mut fx = Fixture::new("yank");
    fx.publish(&Pkg::new("libz", "1.0.0"));
    fx.publish(&Pkg::new("libz", "1.1.0").yanked());
    fx.finish();

    // No explicit version -> the floor skips the yanked 1.1.0 and picks 1.0.0.
    let fresh = unique("yank-fresh");
    project(
        &fresh,
        &fx,
        "z = { registry = \"libz\", index = \"test\" }\n",
    );
    let (lock, warnings) = ops::relock_reporting(&fresh).unwrap();
    let m = lock.modules.iter().find(|m| m.name == "libz").unwrap();
    assert_eq!(
        m.version, "1.0.0",
        "yanked version must not be the fresh floor"
    );
    assert!(
        warnings.is_empty(),
        "no warning when a non-yanked floor exists"
    );

    // Pinning the yanked version still resolves, with a ZAP6102 warning.
    let pinned = unique("yank-pinned");
    project(
        &pinned,
        &fx,
        "z = { registry = \"libz\", version = \"1.1.0\", index = \"test\" }\n",
    );
    let (lock, warnings) = ops::relock_reporting(&pinned).unwrap();
    let m = lock.modules.iter().find(|m| m.name == "libz").unwrap();
    assert_eq!(m.version, "1.1.0");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, zapc_pkg::error::ErrorCode::ZAP6102);
}

#[test]
fn download_unpack_verify_round_trips_into_zenv() {
    isolate_cache();
    let mut fx = Fixture::new("materialize");
    fx.publish(&Pkg::new("bytesx", "0.3.0"));
    fx.publish(&Pkg::new("jsonlib", "1.4.0").dep("bytesx", "0.3.0"));
    fx.finish();

    let app = unique("materialize-app");
    project(
        &app,
        &fx,
        "json = { registry = \"jsonlib\", version = \"1.4.0\", index = \"test\" }\n",
    );

    let report = ops::zenv_create(&app, true).unwrap();
    assert_eq!(report.registry_modules_materialized, 2, "jsonlib + bytesx");
    // The materialized tree is present and hashes to the locked checksum.
    let pkgdir = zenv::pkg_dir(&app).join("jsonlib");
    assert!(
        pkgdir.join("src/lib.zap").exists(),
        "jsonlib not materialized"
    );
    assert!(zenv::pkg_dir(&app).join("bytesx").join("zap.toml").exists());
}

#[test]
fn tampered_archive_is_rejected_pre_unpack() {
    isolate_cache();
    let mut fx = Fixture::new("tamper");
    let mut p = Pkg::new("tamperpkg", "1.0.0");
    p.tamper_archive = true; // acksum recorded from the pristine bytes
    fx.publish(&p);
    fx.finish();

    let app = unique("tamper-app");
    project(
        &app,
        &fx,
        "t = { registry = \"tamperpkg\", version = \"1.0.0\", index = \"test\" }\n",
    );
    // Relock (TOFU-trusts the index cksum) succeeds; the archive-digest check
    // fires at materialize.
    ops::relock_reporting(&app).unwrap();
    let err = ops::zenv_create(&app, true).unwrap_err();
    match err {
        zapc_pkg::Error::Registry { code, .. } => {
            assert_eq!(code, zapc_pkg::error::ErrorCode::ZAP6101);
        }
        other => panic!("expected ZAP6101, got {other}"),
    }
}

#[test]
fn wrong_index_checksum_is_rejected_on_content_hash() {
    isolate_cache();
    let mut fx = Fixture::new("badsum");
    let mut p = Pkg::new("badpkg", "1.0.0");
    p.bad_cksum = Some("zap1:deadbeef"); // index lies about the tree hash
    p.with_acksum = false; // skip the pre-check so the tree-hash check runs
    fx.publish(&p);
    fx.finish();

    let app = unique("badsum-app");
    project(
        &app,
        &fx,
        "b = { registry = \"badpkg\", version = \"1.0.0\", index = \"test\" }\n",
    );
    ops::relock_reporting(&app).unwrap();
    let err = ops::zenv_create(&app, true).unwrap_err();
    match err {
        zapc_pkg::Error::Registry { code, .. } => {
            assert_eq!(code, zapc_pkg::error::ErrorCode::ZAP6100);
        }
        other => panic!("expected ZAP6100, got {other}"),
    }
}

#[test]
fn missing_version_in_index_is_a_clear_error() {
    isolate_cache();
    let mut fx = Fixture::new("absent");
    fx.publish(&Pkg::new("libq", "1.0.0"));
    fx.finish();

    let app = unique("absent-app");
    project(
        &app,
        &fx,
        "q = { registry = \"libq\", version = \"9.9.9\", index = \"test\" }\n",
    );
    let err = ops::relock_reporting(&app).unwrap_err();
    match err {
        zapc_pkg::Error::Registry { code, .. } => {
            assert_eq!(code, zapc_pkg::error::ErrorCode::ZAP6105);
        }
        other => panic!("expected ZAP6105, got {other}"),
    }
}
