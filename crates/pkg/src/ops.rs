//! High-level, filesystem-touching operations behind `zap add`/`remove`/`zenv`.
//!
//! The CLI arms in `crates/driver` stay thin: they parse flags and call these.
//! Pure logic (parsing, MVS, checksums, script generation) lives in the sibling
//! modules; this layer is the glue that reads/writes `zap.toml`, `zap.lock`, and
//! the `zenv/` tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;

use crate::archive;
use crate::checksum::checksum_tree;
use crate::error::{Error, ErrorCode, Result, Warning};
use crate::index::IndexEntry;
use crate::lockfile::{LockedModule, Lockfile};
use crate::manifest::{Manifest, Source};
use crate::name::RegistryName;
use crate::registry::{self, DefaultFetcher, Registry};
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

/// `zap add <name> <source>` — insert/update the dependency and re-lock,
/// returning any non-fatal registry diagnostics.
pub fn add(root: &Path, name: &str, source: &Source) -> Result<Vec<Warning>> {
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
    let (_, warnings) = relock_reporting(root)?;
    Ok(warnings)
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

/// How a discovered module is sourced, with the data needed to lock, fetch, and
/// checksum it.
enum SourceKind {
    /// A local directory (its absolute path), checksummed in place.
    Path { dir: PathBuf },
    /// A git source; its checksum is filled on fetch (not here).
    Git,
    /// A registry source; its name/index are carried in the lock string, and
    /// the per-version index data lives in [`Graph::reg_entries`].
    Registry,
}

struct SourceInfo {
    lock_string: String,
    kind: SourceKind,
}

/// The discovered dependency graph: the MVS registry plus per-name source info
/// and, for registry modules, the index entry of every published version.
#[derive(Default)]
struct Graph {
    /// Keyed by the resolver module name: the logical name for `git`/`path`,
    /// the normalized registry name for `registry` (§3.1).
    sources: BTreeMap<String, SourceInfo>,
    reqs: BTreeMap<(String, Version), Vec<Requirement>>,
    /// Registry index entries, keyed by `(module name, version)`, so the
    /// lock/verify step has each selected version's `cksum`/`yanked`/`min_zap`.
    reg_entries: BTreeMap<(String, Version), IndexEntry>,
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

/// Register a module's source, rejecting a name that two incompatible sources
/// claim (e.g. a registry name colliding with a `path`/`git` logical name).
fn insert_source(graph: &mut Graph, name: &str, info: SourceInfo) -> Result<()> {
    if let Some(existing) = graph.sources.get(name) {
        if existing.lock_string != info.lock_string {
            return Err(Error::registry(
                ErrorCode::ZAP6104,
                format!(
                    "module `{name}` is claimed by two different sources \
                     (`{}` and `{}`)",
                    existing.lock_string, info.lock_string
                ),
            ));
        }
        return Ok(());
    }
    graph.sources.insert(name.to_string(), info);
    Ok(())
}

/// The version (and, for `path`, absolute directory) a non-registry source
/// resolves to.
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
        Source::Registry { .. } => unreachable!("registry sources resolve via the index"),
    }
}

/// Discover a `git`/`path` module and (for `path`) its transitive deps.
/// Registry children (which an index package never has) do not occur here.
fn discover_local(name: &str, source: &Source, base: &Path, graph: &mut Graph) -> Result<()> {
    let (version, dir) = module_version(source, base)?;
    let kind = match &dir {
        Some(d) => SourceKind::Path { dir: d.clone() },
        None => SourceKind::Git,
    };
    insert_source(
        graph,
        name,
        SourceInfo {
            lock_string: source.lock_string(),
            kind,
        },
    )?;
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
            if matches!(csource, Source::Registry { .. }) {
                // A registry dependency declared *inside* a local/git module's
                // own manifest is not pulled into the graph in this cut (I#236);
                // root-level registry deps and root-level path/git deps mix
                // freely (§6). Transitive registry-through-local is a follow-up:
                // it needs the index client threaded into local discovery.
                continue;
            }
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
        discover_local(&cname, &csource, &cbase, graph)?;
    }
    Ok(())
}

/// Breadth-first discovery of a registry package and its transitive registry
/// deps, recording every published version's requirements and index entry.
/// Returns the normalized module name of the requested package.
fn discover_registry(
    rn: &RegistryName,
    index: Option<&str>,
    registry: &Registry,
    graph: &mut Graph,
    visited: &mut BTreeSet<String>,
) -> Result<String> {
    let modname = rn.normalized();
    let mut queue: Vec<(RegistryName, Option<String>)> =
        vec![(rn.clone(), index.map(str::to_string))];
    while let Some((name, idx)) = queue.pop() {
        let key = name.normalized();
        if !visited.insert(key.clone()) {
            continue;
        }
        insert_source(
            graph,
            &key,
            SourceInfo {
                lock_string: lock_string_for(&name, idx.as_deref()),
                kind: SourceKind::Registry,
            },
        )?;
        let entries = registry.package(idx.as_deref(), &name)?;
        for entry in entries {
            let version = resolver::parse_version(&key, &entry.vers)?;
            let mut reqs = Vec::new();
            for dep in &entry.deps {
                let drn = RegistryName::parse(&dep.name)?;
                reqs.push(Requirement {
                    name: drn.normalized(),
                    version: resolver::parse_version(&drn.normalized(), &dep.req)?,
                });
                // A dep may resolve through a different index (§8.3); default is
                // the same index as its parent.
                let didx = dep.registry.clone().or_else(|| idx.clone());
                queue.push((drn, didx));
            }
            graph.reqs.insert((key.clone(), version.clone()), reqs);
            graph.reg_entries.insert((key.clone(), version), entry);
        }
    }
    Ok(modname)
}

/// The lock `source` string for a registry `(name, index)` pair.
fn lock_string_for(name: &RegistryName, index: Option<&str>) -> String {
    match index {
        Some(id) => format!("registry+{id}#{}", name.normalized()),
        None => format!("registry+{}", name.normalized()),
    }
}

/// Pick the floor version for a root registry dependency: the explicit minimum
/// if given (which must be published), else the lowest **non-yanked** published
/// version (or the lowest, if all are yanked).
fn registry_floor(rn: &RegistryName, explicit: Option<&str>, graph: &Graph) -> Result<Version> {
    let modname = rn.normalized();
    let mut versions: Vec<(Version, bool)> = graph
        .reg_entries
        .iter()
        .filter(|((n, _), _)| *n == modname)
        .map(|((_, v), e)| (v.clone(), e.yanked))
        .collect();
    versions.sort_by(|a, b| a.0.cmp(&b.0));
    if versions.is_empty() {
        return Err(Error::registry(
            ErrorCode::ZAP6105,
            format!("registry package `{modname}` has no published versions"),
        ));
    }
    if let Some(v) = explicit {
        let want = resolver::parse_version(&modname, v)?;
        if !versions.iter().any(|(ver, _)| *ver == want) {
            return Err(Error::registry(
                ErrorCode::ZAP6105,
                format!("registry package `{modname}` has no published version {v}"),
            ));
        }
        return Ok(want);
    }
    // Lowest non-yanked, else lowest.
    let floor = versions
        .iter()
        .find(|(_, yanked)| !yanked)
        .or_else(|| versions.first())
        .map(|(v, _)| v.clone())
        .unwrap();
    Ok(floor)
}

/// Re-resolve the manifest with MVS and rewrite `zap.lock`, returning any
/// non-fatal registry diagnostics (e.g. yanked-version selection).
///
/// `registry` is the index client; for a pure `git`/`path` project it is never
/// consulted, so resolution stays network-free.
fn relock_with(root: &Path, registry: &Registry) -> Result<(Lockfile, Vec<Warning>)> {
    let manifest = load_manifest(root)?;
    let old = load_lock(root)?;
    let old_cksums = old_checksums(&old);

    let mut graph = Graph::default();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut roots = Vec::new();
    for (name, dep) in &manifest.dependencies {
        let source = dep.source(name)?;
        match &source {
            Source::Registry {
                name: reg_name,
                version,
                index,
            } => {
                let rn = RegistryName::parse(reg_name)?;
                let modname =
                    discover_registry(&rn, index.as_deref(), registry, &mut graph, &mut visited)?;
                let floor = registry_floor(&rn, version.as_deref(), &graph)?;
                roots.push(Requirement {
                    name: modname,
                    version: floor,
                });
            }
            _ => {
                let (version, _) = module_version(&source, root)?;
                roots.push(Requirement {
                    name: name.clone(),
                    version,
                });
                discover_local(name, &source, root, &mut graph)?;
            }
        }
    }

    let selected = resolver::resolve(&roots, &graph)?;
    let mut warnings = Vec::new();
    let mut modules = Vec::new();
    for (name, version) in resolver::build_list(&selected) {
        let info = graph.sources.get(&name);
        let (source_str, checksum) = match info {
            Some(SourceInfo {
                lock_string,
                kind: SourceKind::Path { dir },
            }) => {
                let c = checksum_tree(dir).map_err(|e| Error::io(dir.display().to_string(), e))?;
                (lock_string.clone(), Some(c))
            }
            Some(SourceInfo {
                lock_string,
                kind: SourceKind::Git,
            }) => (lock_string.clone(), None),
            Some(SourceInfo {
                lock_string,
                kind: SourceKind::Registry,
            }) => {
                let entry = graph
                    .reg_entries
                    .get(&(name.clone(), version.clone()))
                    .expect("selected registry version has an index entry");
                registry.check_min_zap(entry)?;
                // Immutability / tamper: the index must not disagree with an
                // existing pin (TOFU; the lockfile is authoritative, §2.2).
                if let Some(old) = old_cksums.get(&(name.clone(), version.to_string())) {
                    if old != &entry.cksum {
                        return Err(Error::registry(
                            ErrorCode::ZAP6100,
                            format!(
                                "registry package `{name}` {version} checksum changed since \
                                 it was locked (locked {old}, index {}); a published version's \
                                 content is immutable",
                                entry.cksum
                            ),
                        ));
                    }
                }
                if entry.yanked {
                    warnings.push(Warning::new(
                        ErrorCode::ZAP6102,
                        format!(
                            "selected `{name}` {version}, which is yanked; raise its minimum to \
                             an unyanked version"
                        ),
                    ));
                }
                (lock_string.clone(), Some(entry.cksum.clone()))
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
    Ok((lock, warnings))
}

/// The `(name, version) -> checksum` map of an existing lock (if any).
fn old_checksums(old: &Option<Lockfile>) -> BTreeMap<(String, String), String> {
    let mut map = BTreeMap::new();
    if let Some(lock) = old {
        for m in &lock.modules {
            if let Some(c) = &m.checksum {
                map.insert((m.name.clone(), m.version.clone()), c.clone());
            }
        }
    }
    map
}

/// Re-resolve the manifest with MVS and rewrite `zap.lock` (the default client).
pub fn relock(root: &Path) -> Result<Lockfile> {
    relock_reporting(root).map(|(lock, _)| lock)
}

/// Like [`relock`], returning registry warnings for the CLI to print.
pub fn relock_reporting(root: &Path) -> Result<(Lockfile, Vec<Warning>)> {
    let manifest = load_manifest(root)?;
    let named = registry_bases(&manifest);
    let fetcher = DefaultFetcher;
    let registry = registry::default_registry(&fetcher, named);
    relock_with(root, &registry)
}

/// The `[registries]` id -> base-URL map from a manifest.
fn registry_bases(manifest: &Manifest) -> BTreeMap<String, String> {
    manifest
        .registries
        .iter()
        .map(|(id, def)| (id.clone(), def.index.clone()))
        .collect()
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
#[derive(Debug)]
pub struct ZenvReport {
    pub zenv_dir: PathBuf,
    pub venv_created: bool,
    pub venv_skipped_reason: Option<String>,
    pub modules_materialized: usize,
    pub git_modules_skipped: usize,
    /// Registry modules downloaded/verified and materialized.
    pub registry_modules_materialized: usize,
    /// Non-fatal registry diagnostics collected during re-lock.
    pub warnings: Vec<Warning>,
}

/// `zap zenv create [--no-venv]` — build `zenv/{pkg,venv,activate}`.
pub fn zenv_create(root: &Path, no_venv: bool) -> Result<ZenvReport> {
    // A manifest must exist so resolution/materialization is meaningful.
    let manifest = load_manifest(root)?;
    let named = registry_bases(&manifest);
    let fetcher = DefaultFetcher;
    let registry = registry::default_registry(&fetcher, named);

    // Ensure the lockfile is current before materializing from it.
    let (_, warnings) = relock_with(root, &registry)?;

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

    let (modules_materialized, git_modules_skipped, registry_modules_materialized) =
        materialize(root, &pkg, &registry)?;

    Ok(ZenvReport {
        zenv_dir: zdir,
        venv_created,
        venv_skipped_reason,
        modules_materialized,
        git_modules_skipped,
        registry_modules_materialized,
        warnings,
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

/// Materialize the lockfile's modules into `pkg/<name>`: `path` sources are
/// copied, `registry` sources are fetched/verified/cached then copied, and
/// `git` sources are left for a later network clone. Returns
/// `(path_materialized, git_skipped, registry_materialized)`.
fn materialize(root: &Path, pkg: &Path, registry: &Registry) -> Result<(usize, usize, usize)> {
    let Some(lock) = load_lock(root)? else {
        return Ok((0, 0, 0));
    };
    let mut path_done = 0;
    let mut git_skipped = 0;
    let mut reg_done = 0;
    for m in &lock.modules {
        if let Some(rel) = m.source.strip_prefix("path+") {
            let src = root.join(rel);
            let dst = pkg.join(&m.name);
            if dst.exists() {
                fs::remove_dir_all(&dst).map_err(|e| Error::io(dst.display().to_string(), e))?;
            }
            copy_tree(&src, &dst)?;
            path_done += 1;
        } else if m.source.starts_with("registry+") {
            materialize_registry(m, pkg, registry)?;
            reg_done += 1;
        } else {
            // git+… sources need a network clone; recorded, fetched later.
            git_skipped += 1;
        }
    }
    Ok((path_done, git_skipped, reg_done))
}

/// Fetch (if not cached), verify, and materialize one registry module into
/// `pkg/<name>`, running the full §2.2 verification.
fn materialize_registry(m: &LockedModule, pkg: &Path, registry: &Registry) -> Result<()> {
    let (index, name) = parse_registry_source(&m.source)?;
    let rn = RegistryName::parse(&name)?;
    let want = m.checksum.as_deref().ok_or_else(|| {
        Error::registry(
            ErrorCode::ZAP6100,
            format!("registry module `{}` has no checksum in zap.lock", m.name),
        )
    })?;
    let cached = ensure_cached(registry, index.as_deref(), &rn, &m.version, want)?;

    let dst = pkg.join(&m.name);
    if dst.exists() {
        fs::remove_dir_all(&dst).map_err(|e| Error::io(dst.display().to_string(), e))?;
    }
    copy_tree(&cached, &dst)?;
    // Re-verify the materialized tree against the locked checksum (catches a
    // corrupted cache or copy).
    let got = checksum_tree(&dst).map_err(|e| Error::io(dst.display().to_string(), e))?;
    if got != want {
        return Err(Error::registry(
            ErrorCode::ZAP6100,
            format!(
                "registry module `{}` {} failed checksum verification (expected {want}, got {got})",
                m.name, m.version
            ),
        ));
    }
    Ok(())
}

/// Ensure the content-addressed cache holds the verified tree for a registry
/// `(name, version, cksum)`, downloading and verifying if absent. Returns the
/// cache directory. An already-populated cache + lock builds offline (§6.1).
fn ensure_cached(
    registry: &Registry,
    index: Option<&str>,
    rn: &RegistryName,
    version: &str,
    want: &str,
) -> Result<PathBuf> {
    let dir = registry_cache_dir(rn, version, want);
    if dir.exists() {
        // Trust the content-addressed path, but cheaply confirm it still hashes
        // to the pinned value (guards a corrupted cache).
        if let Ok(got) = checksum_tree(&dir) {
            if got == want {
                return Ok(dir);
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    // Download needs the index (archive URL + archive-bytes pre-check).
    let entries = registry.package(index, rn)?;
    let entry = entries.iter().find(|e| e.vers == version).ok_or_else(|| {
        Error::registry(
            ErrorCode::ZAP6105,
            format!("registry package `{rn}` has no published version {version}"),
        )
    })?;
    if entry.cksum != want {
        return Err(Error::registry(
            ErrorCode::ZAP6100,
            format!(
                "registry package `{rn}` {version} checksum in the index ({}) does not match the \
                 locked checksum ({want})",
                entry.cksum
            ),
        ));
    }

    // The archive format (and extension) is driven purely by the `dl` template
    // in `config.json`, so `.zap.tar` today and `.zap.tar.zst` later both work
    // through the codec seam with no client change.
    let (bytes, codec) = registry.archive(index, rn, version)?;
    // Cheap pre-unpack transfer check (§2.2 step 3), when recorded.
    if let Some(acksum) = &entry.acksum {
        let got = archive::digest(&bytes);
        if &got != acksum {
            return Err(Error::registry(
                ErrorCode::ZAP6101,
                format!(
                    "registry archive for `{rn}` {version} has digest {got}, expected {acksum}"
                ),
            ));
        }
    }

    // Unpack to a temp dir, verify the tree hash, then atomically publish.
    let tmp = dir.with_file_name(format!(
        ".{}-{}.tmp",
        dir.file_name().and_then(|s| s.to_str()).unwrap_or("pkg"),
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    archive::unpack(&bytes, &tmp, codec)?;
    let got = checksum_tree(&tmp).map_err(|e| Error::io(tmp.display().to_string(), e))?;
    if got != want {
        let _ = fs::remove_dir_all(&tmp);
        return Err(Error::registry(
            ErrorCode::ZAP6100,
            format!(
                "registry package `{rn}` {version} content hash {got} does not match the pinned \
                 checksum {want}; the archive was tampered with or corrupted"
            ),
        ));
    }
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent.display().to_string(), e))?;
    }
    let _ = fs::remove_dir_all(&dir);
    fs::rename(&tmp, &dir).map_err(|e| Error::io(dir.display().to_string(), e))?;
    Ok(dir)
}

/// The content-addressed cache directory for a registry `(name, version,
/// cksum)` under `$ZAP_CACHE/.registry` (or `<home>/.zap/.registry`).
fn registry_cache_dir(rn: &RegistryName, version: &str, cksum: &str) -> PathBuf {
    let base = std::env::var(zenv::ZAP_CACHE)
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".zap")
        });
    // `cksum` is `zap1:<hex>`; keep only the hex for a filesystem-safe segment.
    let hex = cksum.strip_prefix("zap1:").unwrap_or(cksum);
    let name = rn.normalized().replace('/', "_").replace('@', "");
    base.join(".registry").join(name).join(version).join(hex)
}

/// Split a `registry+<id>#<name>` / `registry+<name>` lock source into its
/// optional index id and the (normalized) package name.
fn parse_registry_source(source: &str) -> Result<(Option<String>, String)> {
    let rest = source.strip_prefix("registry+").ok_or_else(|| {
        Error::registry(
            ErrorCode::ZAP6105,
            format!("not a registry source: `{source}`"),
        )
    })?;
    match rest.split_once('#') {
        Some((id, name)) => Ok((Some(id.to_string()), name.to_string())),
        None => Ok((None, rest.to_string())),
    }
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

// --- registry write path & discovery (I#237, D-0083) -----------------------

/// Resolve the index base URL/path for a publish or discovery operation, in
/// priority order: an explicit `--registry <url>`, a `--index <id>` naming a
/// `[registries]` entry (requires a manifest under `root`), `$ZAP_REGISTRY`,
/// else the built-in default ([`registry::DEFAULT_INDEX`], `pkg.zaplang.org`).
/// Always resolves to a base — there is a baked-in default.
fn resolve_index_base(
    root: Option<&Path>,
    index_id: Option<&str>,
    registry_url: Option<&str>,
) -> Result<String> {
    if let Some(url) = registry_url {
        return Ok(url.to_string());
    }
    if let Some(id) = index_id {
        let root = root.ok_or_else(|| {
            Error::registry(
                ErrorCode::ZAP6105,
                format!("`--index {id}` needs a zap.toml with a `[registries.{id}]` entry"),
            )
        })?;
        let manifest = load_manifest(root)?;
        return manifest
            .registries
            .get(id)
            .map(|d| d.index.clone())
            .ok_or_else(|| {
                Error::registry(
                    ErrorCode::ZAP6105,
                    format!("no `[registries.{id}]` entry in zap.toml"),
                )
            });
    }
    Ok(registry::default_index_base())
}

/// `zap publish [--index <id> | --registry <url>] [--dry-run]` — package the
/// project at `root` and publish it to the resolved index (I#237).
pub fn publish(
    root: &Path,
    index_id: Option<&str>,
    registry_url: Option<&str>,
    dry_run: bool,
) -> Result<crate::publish::Published> {
    let base = resolve_index_base(Some(root), index_id, registry_url)?;
    crate::publish::publish(root, Some(&base), dry_run)
}

/// `zap login [--index <id> | --registry <url>]` — store a bearer token for the
/// resolved index in the out-of-repo credential store (I#237, §5 step 3).
pub fn login(
    root: Option<&Path>,
    index_id: Option<&str>,
    registry_url: Option<&str>,
    token: &str,
) -> Result<PathBuf> {
    let base = resolve_index_base(root, index_id, registry_url)?;
    crate::publish::store_token(&base, token)
}

/// `zap search <query> [--index <id> | --registry <url>]` — list packages in a
/// local/`file://` index whose name matches `query` (I#237).
pub fn search(
    root: Option<&Path>,
    index_id: Option<&str>,
    registry_url: Option<&str>,
    query: &str,
) -> Result<Vec<crate::discovery::PackageSummary>> {
    let base = resolve_index_base(root, index_id, registry_url)?;
    crate::discovery::search(&base, query)
}

/// `zap info <name> [--index <id> | --registry <url>]` — fetch a package's
/// published versions from any static index (local or remote) (I#237).
pub fn info(
    root: Option<&Path>,
    index_id: Option<&str>,
    registry_url: Option<&str>,
    name: &str,
) -> Result<Vec<IndexEntry>> {
    let base = resolve_index_base(root, index_id, registry_url)?;
    let rn = RegistryName::parse(name)?;
    let fetcher = DefaultFetcher;
    let reg = Registry::new(
        &fetcher,
        Some(base),
        BTreeMap::new(),
        registry::toolchain_version(),
    );
    reg.package(None, &rn)
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
