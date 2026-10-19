//! High-level, filesystem-touching operations behind `zap add`/`remove`/`zenv`.
//!
//! The CLI arms in `crates/driver` stay thin: they parse flags and call these.
//! Pure logic (parsing, MVS, checksums, script generation) lives in the sibling
//! modules; this layer is the glue that reads/writes `zap.toml`, `zap.lock`, and
//! the `zenv/` tree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;

use crate::checksum::checksum_tree;
use crate::error::{Error, Result};
use crate::lockfile::{LockedModule, Lockfile};
use crate::manifest::{Manifest, Source};
use crate::resolver::{self, Requirement};
use crate::zenv;

/// The manifest path under a project root.
pub fn manifest_path(root: &Path) -> PathBuf {
    root.join("zap.toml")
}

/// The lockfile path under a project root.
pub fn lockfile_path(root: &Path) -> PathBuf {
    root.join("zap.lock")
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| Error::io(path.display().to_string(), e))
}

fn write(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).map_err(|e| Error::io(path.display().to_string(), e))
}

/// Read and parse the project manifest, with a helpful error if it is missing.
pub fn load_manifest(root: &Path) -> Result<Manifest> {
    let path = manifest_path(root);
    if !path.exists() {
        return Err(Error::Manifest(
            "no zap.toml in this directory; run `zap init` first".to_string(),
        ));
    }
    Manifest::parse(&read(&path)?)
}

/// `zap add <name> <source>` — insert/update the dependency and re-lock.
pub fn add(root: &Path, name: &str, source: &Source) -> Result<()> {
    let path = manifest_path(root);
    if !path.exists() {
        return Err(Error::Manifest(
            "no zap.toml in this directory; run `zap init` first".to_string(),
        ));
    }
    let text = read(&path)?;
    // Validate the current manifest before mutating it.
    Manifest::parse(&text)?;
    let new_text = crate::manifest::edit::add_dependency(&text, name, source)?;
    write(&path, &new_text)?;
    relock(root)?;
    Ok(())
}

/// `zap remove <name>` — drop the dependency and re-lock.
pub fn remove(root: &Path, name: &str) -> Result<()> {
    let path = manifest_path(root);
    let text = read(&path)?;
    let new_text = crate::manifest::edit::remove_dependency(&text, name)?;
    write(&path, &new_text)?;
    relock(root)?;
    Ok(())
}

/// Source + resolved location of a discovered module.
struct SourceInfo {
    lock_string: String,
    /// The module's directory, for a `path` source (used to checksum it).
    dir: Option<PathBuf>,
}

/// The discovered dependency graph: the MVS registry plus per-name source info.
#[derive(Default)]
struct Graph {
    sources: BTreeMap<String, SourceInfo>,
    reqs: BTreeMap<(String, Version), Vec<Requirement>>,
}

impl resolver::Registry for Graph {
    fn requirements(&self, name: &str, version: &Version) -> Result<Vec<Requirement>> {
        Ok(self
            .reqs
            .get(&(name.to_string(), version.clone()))
            .cloned()
            .unwrap_or_default())
    }
}

/// The version (and, for `path`, absolute directory) a source resolves to.
fn module_version(source: &Source, base: &Path) -> Result<(Version, Option<PathBuf>)> {
    match source {
        Source::Git { version, .. } => Ok((resolver::parse_version("<git dep>", version)?, None)),
        Source::Path { dir } => {
            let target = base.join(dir);
            let tm = load_manifest(&target)?;
            let v = resolver::parse_version(&tm.package.name, &tm.package.version)?;
            let abs = fs::canonicalize(&target).unwrap_or(target);
            Ok((v, Some(abs)))
        }
    }
}

/// Discover a module and (for `path` sources) its transitive dependencies,
/// populating `graph`. Cycles terminate via the `reqs` visited-set guard.
fn discover(name: &str, source: &Source, base: &Path, graph: &mut Graph) -> Result<()> {
    let (version, dir) = module_version(source, base)?;
    graph.sources.insert(
        name.to_string(),
        SourceInfo {
            lock_string: source.lock_string(),
            dir: dir.clone(),
        },
    );
    let key = (name.to_string(), version.clone());
    if graph.reqs.contains_key(&key) {
        return Ok(());
    }

    let mut child_reqs = Vec::new();
    let mut children: Vec<(String, Source, PathBuf)> = Vec::new();
    if let Some(ref d) = dir {
        let tm = load_manifest(d)?;
        for (cname, cdep) in &tm.dependencies {
            let csource = cdep.source(cname)?;
            let (cversion, _) = module_version(&csource, d)?;
            child_reqs.push(Requirement {
                name: cname.clone(),
                version: cversion,
            });
            children.push((cname.clone(), csource, d.clone()));
        }
    }
    graph.reqs.insert(key, child_reqs);
    for (cname, csource, cbase) in children {
        discover(&cname, &csource, &cbase, graph)?;
    }
    Ok(())
}

/// Re-resolve the manifest with MVS and rewrite `zap.lock`.
pub fn relock(root: &Path) -> Result<Lockfile> {
    let manifest = load_manifest(root)?;

    let mut graph = Graph::default();
    let mut roots = Vec::new();
    for (name, dep) in &manifest.dependencies {
        let source = dep.source(name)?;
        let (version, _) = module_version(&source, root)?;
        roots.push(Requirement {
            name: name.clone(),
            version,
        });
        discover(name, &source, root, &mut graph)?;
    }

    let selected = resolver::resolve(&roots, &graph)?;
    let mut modules = Vec::new();
    for (name, version) in resolver::build_list(&selected) {
        let info = graph.sources.get(&name);
        let (source_str, checksum) = match info {
            Some(info) => {
                let checksum = match &info.dir {
                    Some(dir) => Some(
                        checksum_tree(dir).map_err(|e| Error::io(dir.display().to_string(), e))?,
                    ),
                    None => None, // git source: checksum filled on fetch.
                };
                (info.lock_string.clone(), checksum)
            }
            None => (String::new(), None),
        };
        modules.push(LockedModule {
            name,
            version: version.to_string(),
            source: source_str,
            checksum,
        });
    }

    let lock = Lockfile::new(modules);
    write(&lockfile_path(root), &lock.to_toml()?)?;
    Ok(lock)
}

/// Read the project's lockfile, if present.
pub fn load_lock(root: &Path) -> Result<Option<Lockfile>> {
    let path = lockfile_path(root);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(Lockfile::parse(&read(&path)?)?))
}

/// The outcome of `zap zenv create`, for a human-readable summary.
pub struct ZenvReport {
    pub zenv_dir: PathBuf,
    pub venv_created: bool,
    pub venv_skipped_reason: Option<String>,
    pub modules_materialized: usize,
    pub git_modules_skipped: usize,
}

/// `zap zenv create [--no-venv]` — build `zenv/{pkg,venv,activate}`.
pub fn zenv_create(root: &Path, no_venv: bool) -> Result<ZenvReport> {
    // A manifest must exist so resolution/materialization is meaningful.
    load_manifest(root)?;
    // Ensure the lockfile is current before materializing from it.
    relock(root)?;

    let zdir = zenv::zenv_dir(root);
    let pkg = zenv::pkg_dir(root);
    fs::create_dir_all(&pkg).map_err(|e| Error::io(pkg.display().to_string(), e))?;

    let abs = fs::canonicalize(&zdir).unwrap_or_else(|_| zdir.clone());
    let activate = zdir.join("activate");
    write(&activate, &zenv::activate_script(&abs))?;

    let (venv_created, venv_skipped_reason) = if no_venv {
        (false, Some("--no-venv".to_string()))
    } else {
        create_venv(&zenv::venv_dir(root))
    };

    let (modules_materialized, git_modules_skipped) = materialize(root, &pkg)?;

    Ok(ZenvReport {
        zenv_dir: zdir,
        venv_created,
        venv_skipped_reason,
        modules_materialized,
        git_modules_skipped,
    })
}

/// Best-effort `python3 -m venv <dir>`. A missing/old `python3` is not fatal —
/// the Zap side of the environment still works; the venv can be created later.
fn create_venv(venv: &Path) -> (bool, Option<String>) {
    if venv.exists() {
        return (true, None);
    }
    match Command::new("python3")
        .arg("-m")
        .arg("venv")
        .arg(venv)
        .output()
    {
        Ok(out) if out.status.success() => (true, None),
        Ok(out) => (
            false,
            Some(format!(
                "python3 -m venv failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        ),
        Err(e) => (false, Some(format!("python3 not available: {e}"))),
    }
}

/// Copy every `path`-source module from the lockfile into `pkg/<name>`.
/// Returns `(materialized, git_skipped)`.
fn materialize(root: &Path, pkg: &Path) -> Result<(usize, usize)> {
    let Some(lock) = load_lock(root)? else {
        return Ok((0, 0));
    };
    let mut materialized = 0;
    let mut git_skipped = 0;
    for m in &lock.modules {
        if let Some(rel) = m.source.strip_prefix("path+") {
            let src = root.join(rel);
            let dst = pkg.join(&m.name);
            if dst.exists() {
                fs::remove_dir_all(&dst).map_err(|e| Error::io(dst.display().to_string(), e))?;
            }
            copy_tree(&src, &dst)?;
            materialized += 1;
        } else {
            // git+… sources need a network clone; recorded, fetched later.
            git_skipped += 1;
        }
    }
    Ok((materialized, git_skipped))
}

/// Recursively copy `src` into `dst`, skipping VCS/build/env artifacts.
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).map_err(|e| Error::io(dst.display().to_string(), e))?;
    for entry in fs::read_dir(src).map_err(|e| Error::io(src.display().to_string(), e))? {
        let entry = entry.map_err(|e| Error::io(src.display().to_string(), e))?;
        let name = entry.file_name();
        let ty = entry
            .file_type()
            .map_err(|e| Error::io(entry.path().display().to_string(), e))?;
        if ty.is_dir() {
            if matches!(name.to_string_lossy().as_ref(), ".git" | "target" | "zenv") {
                continue;
            }
            copy_tree(&entry.path(), &dst.join(&name))?;
        } else if ty.is_file() {
            let to = dst.join(&name);
            fs::copy(entry.path(), &to).map_err(|e| Error::io(to.display().to_string(), e))?;
        }
    }
    Ok(())
}

/// `zap zenv clean` — remove the `zenv/` directory. A no-op if absent.
pub fn zenv_clean(root: &Path) -> Result<bool> {
    let zdir = zenv::zenv_dir(root);
    if !zdir.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(&zdir).map_err(|e| Error::io(zdir.display().to_string(), e))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmp(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("zapc_pkg-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn project(root: &Path, name: &str, version: &str, body: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(
            manifest_path(root),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\n\n[dependencies]\n{body}"
            ),
        )
        .unwrap();
    }

    #[test]
    fn add_path_dep_locks_with_checksum() {
        let base = tmp("add");
        let app = base.join("app");
        let dep = base.join("mathx");
        project(&dep, "mathx", "0.2.0", "");
        fs::create_dir_all(dep.join("src")).unwrap();
        fs::write(dep.join("src/lib.zap"), "def f(): pass\n").unwrap();
        project(&app, "app", "0.1.0", "");

        add(
            &app,
            "mathx",
            &Source::Path {
                dir: "../mathx".into(),
            },
        )
        .unwrap();

        let manifest = fs::read_to_string(manifest_path(&app)).unwrap();
        assert!(
            manifest.contains("mathx = { path = \"../mathx\" }"),
            "{manifest}"
        );

        let lock = load_lock(&app).unwrap().unwrap();
        assert_eq!(lock.modules.len(), 1);
        assert_eq!(lock.modules[0].name, "mathx");
        assert_eq!(lock.modules[0].version, "0.2.0");
        assert!(lock.modules[0].source.starts_with("path+"));
        assert!(lock.modules[0]
            .checksum
            .as_ref()
            .unwrap()
            .starts_with("zap1:"));
    }

    #[test]
    fn transitive_path_dep_is_locked() {
        let base = tmp("trans");
        let app = base.join("app");
        let mid = base.join("mid");
        let leaf = base.join("leaf");
        project(&leaf, "leaf", "1.0.0", "");
        project(&mid, "mid", "1.0.0", "leaf = { path = \"../leaf\" }\n");
        project(&app, "app", "0.1.0", "");

        add(
            &app,
            "mid",
            &Source::Path {
                dir: "../mid".into(),
            },
        )
        .unwrap();
        let lock = load_lock(&app).unwrap().unwrap();
        let names: Vec<_> = lock.modules.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"mid"), "{names:?}");
        assert!(names.contains(&"leaf"), "{names:?}");
    }

    #[test]
    fn git_dep_locks_without_checksum() {
        let base = tmp("git");
        let app = base.join("app");
        project(&app, "app", "0.1.0", "");
        add(
            &app,
            "jsonlib",
            &Source::Git {
                url: "https://example.com/jsonlib.git".into(),
                version: "1.4.0".into(),
            },
        )
        .unwrap();
        let lock = load_lock(&app).unwrap().unwrap();
        assert_eq!(lock.modules[0].name, "jsonlib");
        assert_eq!(lock.modules[0].version, "1.4.0");
        assert!(lock.modules[0].source.starts_with("git+"));
        assert!(lock.modules[0].checksum.is_none());
    }

    #[test]
    fn remove_drops_and_relocks() {
        let base = tmp("rm");
        let app = base.join("app");
        let dep = base.join("d");
        project(&dep, "d", "1.0.0", "");
        project(&app, "app", "0.1.0", "");
        add(&app, "d", &Source::Path { dir: "../d".into() }).unwrap();
        assert_eq!(load_lock(&app).unwrap().unwrap().modules.len(), 1);
        remove(&app, "d").unwrap();
        assert_eq!(load_lock(&app).unwrap().unwrap().modules.len(), 0);
    }

    #[test]
    fn zenv_create_materializes_path_modules() {
        let base = tmp("zenv");
        let app = base.join("app");
        let dep = base.join("widget");
        project(&dep, "widget", "0.3.0", "");
        fs::write(dep.join("marker.txt"), "hi").unwrap();
        project(&app, "app", "0.1.0", "");
        add(
            &app,
            "widget",
            &Source::Path {
                dir: "../widget".into(),
            },
        )
        .unwrap();

        let report = zenv_create(&app, true).unwrap();
        assert!(!report.venv_created);
        assert_eq!(report.modules_materialized, 1);
        assert!(zenv::pkg_dir(&app)
            .join("widget")
            .join("marker.txt")
            .exists());
        assert!(zenv::zenv_dir(&app).join("activate").exists());
    }

    #[test]
    fn add_without_manifest_errors() {
        let base = tmp("nomani");
        assert!(add(&base, "x", &Source::Path { dir: "y".into() }).is_err());
    }
}
