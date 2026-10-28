//! The `zap publish` write path (design/0027-registry.md §5; D-0083, I#237).
//!
//! `zap publish` is the counterpart to the #236 read path: it packages the
//! current project and makes a new `(name, version)` resolvable by publishing
//! an archive plus one appended index line (§1.2) that the read client then
//! verifies against the `zap1:` tree hash (§2). The identity it writes is the
//! *same* content hash a consumer re-checks, so publish and read never disagree.
//!
//! ## The concrete target, and the hosting gap it surfaces
//!
//! Design §5 routes a publish through an **authenticated HTTP `api` endpoint**
//! backed by real hosted infrastructure (write API + token store + archive
//! bucket). That infrastructure is **gated on the org move** and does not exist
//! yet (§5.2/§7). Rather than invent a service, this cut implements the one
//! target that needs no hosting and is itself a legitimate deployment model: a
//! **writable static index on a local path or `file://` URL** (the same static
//! index the read path consumes, and exactly how a private/corporate index or a
//! CI job staging to object storage works, §8.3). Publishing *there* is a pure,
//! deterministic, offline filesystem operation: build the archive, append the
//! index line, write `config.json` on first use.
//!
//! Pointing publish at an `https://` index asks for the hosted write API and
//! returns a clear, coded error ([`ErrorCode::ZAP6108`]) naming the gap. The
//! local credential store ([`login`]) lands so the token mechanics (§5 step 3)
//! exist and are exercised; the token is only *consumed* by that gated remote
//! path, so a local/`file://` index needs none.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::archive::{self, Codec};
use crate::checksum::checksum_tree;
use crate::error::{Error, ErrorCode, Result, Warning};
use crate::index::{self, Bucketing, Config};
use crate::manifest::{Manifest, Source};
use crate::name::RegistryName;
use crate::registry;

/// Where a publish is directed, resolved from an index base URL/path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A writable static index rooted at this local directory (`file://` or a
    /// bare path). Publishing writes the archive and appends the index line.
    LocalIndex(PathBuf),
    /// An `https://`/`http://` index, which needs the hosted, authenticated
    /// write API — not yet deployed (gated on the org move, §5.2/§7).
    HostedApi(String),
}

/// Resolve an index base (URL or path) to a publish [`Target`]. An `http(s)://`
/// base is the hosted-API target; everything else is a local static index.
pub fn resolve_target(base: &str) -> Target {
    if base.starts_with("https://") || base.starts_with("http://") {
        Target::HostedApi(base.to_string())
    } else {
        let path = registry::file_url_to_path(base).unwrap_or_else(|| base.to_string());
        Target::LocalIndex(PathBuf::from(path))
    }
}

/// The result of a `zap publish` (or a `--dry-run` plan of one).
#[derive(Debug, Clone)]
pub struct Published {
    /// The normalized registry name published under.
    pub name: String,
    /// The published version.
    pub version: String,
    /// The `zap1:` tree-hash identity consumers verify against.
    pub cksum: String,
    /// The archive-bytes SHA-256 pre-check digest (hex, no prefix).
    pub acksum: String,
    /// The archive's size in bytes.
    pub archive_len: usize,
    /// The index file the version line was (or would be) appended to, when the
    /// target is a local index.
    pub index_file: Option<PathBuf>,
    /// The archive file written (or that would be written), when local.
    pub archive_file: Option<PathBuf>,
    /// Non-fatal publish diagnostics (e.g. unrepresentable `git`/`path` deps).
    pub warnings: Vec<Warning>,
    /// True if nothing was written (a `--dry-run` plan).
    pub dry_run: bool,
}

/// A validated, packaged project ready to publish: the §5 steps 1–2 output,
/// independent of the target.
struct Packaged {
    name: RegistryName,
    version: String,
    archive: Vec<u8>,
    cksum: String,
    acksum: String,
    /// Registry dependencies, as `{name, req, major}` index-line entries.
    deps: Vec<(String, String, u64)>,
    warnings: Vec<Warning>,
}

/// Validate `[package]` and build the deterministic archive + checksums (§5
/// steps 1–2). Rejects a missing/invalid name or version before any I/O to a
/// target, so a bad manifest fails the same way regardless of where it points.
fn package(root: &Path, manifest: &Manifest) -> Result<Packaged> {
    let raw_name = manifest.package.name.trim();
    if raw_name.is_empty() {
        return Err(Error::registry(
            ErrorCode::ZAP6107,
            "`[package].name` is empty; a publish needs a registry name",
        ));
    }
    // Name rules / reserved words map to ZAP6104 inside `RegistryName::parse`.
    let name = RegistryName::parse(raw_name)?;
    let version = manifest.package.version.trim().to_string();
    Version::parse(&version).map_err(|e| {
        Error::registry(
            ErrorCode::ZAP6107,
            format!("`[package].version` (`{version}`) is not valid SemVer: {e}"),
        )
    })?;

    // Registry deps become index-line deps; a published package's deps must be
    // registry-resolvable (§1.2). `git`/`path` deps cannot be expressed in the
    // index, so warn rather than silently drop them (a leaf package has none).
    let mut deps = Vec::new();
    let mut warnings = Vec::new();
    for (logical, dep) in &manifest.dependencies {
        match dep.source(logical)? {
            Source::Registry {
                name: reg, version, ..
            } => {
                let drn = RegistryName::parse(&reg)?;
                let req = version.unwrap_or_else(|| "0.0.0".to_string());
                let major = Version::parse(&req).map(|v| v.major).unwrap_or(0);
                deps.push((drn.normalized(), req, major));
            }
            Source::Git { .. } | Source::Path { .. } => {
                warnings.push(Warning::new(
                    ErrorCode::ZAP6107,
                    format!(
                        "dependency `{logical}` is a git/path source and cannot be recorded in \
                         the registry index; consumers of the published package will not resolve \
                         it. Publish its dependencies to the registry too, or vendor them."
                    ),
                ));
            }
        }
    }

    let archive = archive::build(root, Codec::Store)?;
    let acksum = archive::digest(&archive);
    let cksum = checksum_tree(root).map_err(|e| Error::io(root.display().to_string(), e))?;

    Ok(Packaged {
        name,
        version,
        archive,
        cksum,
        acksum,
        deps,
        warnings,
    })
}

/// Compose the line-delimited-JSON index line for a packaged version (§1.2),
/// matching the schema the read path parses.
fn index_line(pkg: &Packaged) -> String {
    let deps = pkg
        .deps
        .iter()
        .map(|(name, req, major)| {
            zapc_json::Json::obj(vec![
                ("name", zapc_json::Json::str(name.as_str())),
                ("req", zapc_json::Json::str(req.as_str())),
                ("major", zapc_json::Json::int(*major as i64)),
            ])
        })
        .collect();
    zapc_json::Json::obj(vec![
        ("name", zapc_json::Json::str(pkg.name.normalized())),
        ("vers", zapc_json::Json::str(pkg.version.as_str())),
        ("deps", zapc_json::Json::Arr(deps)),
        ("cksum", zapc_json::Json::str(pkg.cksum.as_str())),
        ("acksum", zapc_json::Json::str(pkg.acksum.as_str())),
        ("yanked", zapc_json::Json::Bool(false)),
    ])
    .to_string()
}

/// `zap publish` — package the project at `root` and publish it to `base`
/// (an index URL/path), or plan the publish when `dry_run` is set.
///
/// With `base = None` and `dry_run`, this validates and packages the project
/// and reports the plan without a target (useful for a pre-flight check). With
/// `base = None` and no dry-run, it is an error: a publish needs a target.
pub fn publish(root: &Path, base: Option<&str>, dry_run: bool) -> Result<Published> {
    let manifest = crate::ops::load_manifest(root)?;
    let pkg = package(root, &manifest)?;

    let Some(base) = base else {
        if dry_run {
            return Ok(Published {
                name: pkg.name.normalized(),
                version: pkg.version,
                cksum: pkg.cksum,
                acksum: pkg.acksum,
                archive_len: pkg.archive.len(),
                index_file: None,
                archive_file: None,
                warnings: pkg.warnings,
                dry_run: true,
            });
        }
        return Err(Error::registry(
            ErrorCode::ZAP6108,
            "no publish target: pass `--index <id>` (a `[registries]` entry), `--registry <url>`, \
             or set ZAP_REGISTRY to a local/`file://` static index",
        ));
    };

    match resolve_target(base) {
        Target::HostedApi(url) => Err(Error::registry(
            ErrorCode::ZAP6108,
            format!(
                "`{url}` is an https index, which needs the hosted authenticated write API — not \
                 yet deployed (gated on the org move, D-0027 §5.2/§7). Publish to a local or \
                 `file://` static index instead."
            ),
        )),
        Target::LocalIndex(dir) => publish_local(&dir, pkg, dry_run),
    }
}

/// Publish a packaged version into a writable static index at `dir`.
fn publish_local(dir: &Path, pkg: Packaged, dry_run: bool) -> Result<Published> {
    // Resolve (and, for a real publish, create) the index directory so its
    // `file://` base and `dl` template are absolute and portable.
    let index_dir = if dir.exists() {
        fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
    } else if dry_run {
        // Nothing is created in a dry-run; resolve against the parent if we can.
        absolutize(dir)
    } else {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir.display().to_string(), e))?;
        fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
    };

    let config_path = index_dir.join("config.json");
    let config = load_or_default_config(&config_path, &index_dir)?;

    // The archive output path, from the `dl` template. It must be local; an
    // https `dl` means the archive store is remote (hosted), which is gated.
    let codec = Codec::from_dl(&config.dl)?;
    let archive_url = config.archive_url(&pkg.name, &pkg.version);
    let archive_path = registry::file_url_to_path(&archive_url).map(PathBuf::from);
    let Some(archive_path) = archive_path else {
        return Err(Error::registry(
            ErrorCode::ZAP6108,
            format!(
                "index `config.json` points archives at `{archive_url}`, a remote store that \
                 needs the hosted write API (gated, D-0027 §5.2/§7); a local publish needs a \
                 `file://` `dl` template"
            ),
        ));
    };
    let _ = codec; // Store today; the seam keeps `.zap.tar.zst` a future drop-in.

    // Immutability (§4): refuse to overwrite an already-published version.
    let index_rel = index::index_path(&pkg.name, config.bucketing);
    let index_file = index_dir.join(&index_rel);
    if let Some(existing) = read_existing(&index_file)? {
        let entries = index::parse_package(&existing)?;
        if entries.iter().any(|e| e.vers == pkg.version) {
            return Err(Error::registry(
                ErrorCode::ZAP6106,
                format!(
                    "`{}` {} is already published to this index; published versions are immutable \
                     (bump the version, or yank the old one)",
                    pkg.name.normalized(),
                    pkg.version
                ),
            ));
        }
    }

    let line = index_line(&pkg);

    if dry_run {
        return Ok(Published {
            name: pkg.name.normalized(),
            version: pkg.version,
            cksum: pkg.cksum,
            acksum: pkg.acksum,
            archive_len: pkg.archive.len(),
            index_file: Some(index_file),
            archive_file: Some(archive_path),
            warnings: pkg.warnings,
            dry_run: true,
        });
    }

    // Write `config.json` (first publish only), the archive, then append the
    // index line last so a package is never visible without its archive.
    if !config_path.exists() {
        write_file(&config_path, &config_json(&config))?;
    }
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent.display().to_string(), e))?;
    }
    fs::write(&archive_path, &pkg.archive)
        .map_err(|e| Error::io(archive_path.display().to_string(), e))?;

    if let Some(parent) = index_file.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent.display().to_string(), e))?;
    }
    let mut contents = read_existing(&index_file)?.unwrap_or_default();
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(&line);
    contents.push('\n');
    write_file(&index_file, &contents)?;

    Ok(Published {
        name: pkg.name.normalized(),
        version: pkg.version,
        cksum: pkg.cksum,
        acksum: pkg.acksum,
        archive_len: pkg.archive.len(),
        index_file: Some(index_file),
        archive_file: Some(archive_path),
        warnings: pkg.warnings,
        dry_run: false,
    })
}

/// Load an index's `config.json`, or synthesize a default for a fresh index
/// (archives under `<index>/archives`, the current `Store` codec).
fn load_or_default_config(config_path: &Path, index_dir: &Path) -> Result<Config> {
    if config_path.exists() {
        let text = fs::read_to_string(config_path)
            .map_err(|e| Error::io(config_path.display().to_string(), e))?;
        return Config::parse(&text);
    }
    let archives = index_dir.join("archives");
    let dl = format!(
        "{}/{{name}}/{{name}}-{{vers}}.{}",
        registry::file_url(&archives),
        Codec::Store.extension()
    );
    Ok(Config {
        index_version: 1,
        dl,
        api: None,
        bucketing: Bucketing::CargoSparse,
    })
}

/// Render a [`Config`] as `config.json` text the read path parses.
fn config_json(config: &Config) -> String {
    let Bucketing::CargoSparse = config.bucketing;
    let mut pairs = vec![
        (
            "index_version",
            zapc_json::Json::int(config.index_version as i64),
        ),
        ("dl", zapc_json::Json::str(config.dl.as_str())),
        ("bucketing", zapc_json::Json::str("cargo-sparse")),
    ];
    if let Some(api) = &config.api {
        pairs.push(("api", zapc_json::Json::str(api.as_str())));
    }
    zapc_json::Json::obj(pairs).to_string()
}

/// Read an index file's text, or `None` if it does not exist yet.
fn read_existing(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|e| Error::io(path.display().to_string(), e))
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).map_err(|e| Error::io(path.display().to_string(), e))
}

/// Make a (possibly relative, possibly nonexistent) path absolute without
/// touching the filesystem, for a dry-run's reported paths.
fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

// --- credential store (`zap login`, §5 step 3) -----------------------------

/// The on-disk token store: index base/host -> bearer token. Kept **outside**
/// any repo (under `$ZAP_CACHE`), so a token never lands in a checkout or lock.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Credentials {
    #[serde(default)]
    tokens: BTreeMap<String, String>,
}

/// The credentials file path: `$ZAP_CACHE/credentials`, else
/// `<home>/.zap/credentials` (never inside the project).
pub fn credentials_path() -> PathBuf {
    let base = std::env::var(crate::zenv::ZAP_CACHE)
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".zap")
        });
    base.join("credentials")
}

fn load_credentials() -> Result<Credentials> {
    let path = credentials_path();
    if !path.exists() {
        return Ok(Credentials::default());
    }
    let text = fs::read_to_string(&path).map_err(|e| Error::io(path.display().to_string(), e))?;
    toml::from_str(&text).map_err(|e| {
        Error::registry(
            ErrorCode::ZAP6109,
            format!("credentials file `{}` is malformed: {e}", path.display()),
        )
    })
}

/// `zap login` — store a bearer `token` for the registry `index_key` (its base
/// URL/host). The token is validated non-empty and written to the out-of-repo
/// store; it is only consumed by the hosted write API. Returns the store path.
pub fn store_token(index_key: &str, token: &str) -> Result<PathBuf> {
    let token = token.trim();
    if token.is_empty() {
        return Err(Error::registry(
            ErrorCode::ZAP6109,
            "an empty token was supplied; `zap login` needs a non-empty token",
        ));
    }
    let mut creds = load_credentials()?;
    creds
        .tokens
        .insert(index_key.to_string(), token.to_string());
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent.display().to_string(), e))?;
    }
    let text = toml::to_string_pretty(&creds).map_err(|e| {
        Error::registry(
            ErrorCode::ZAP6109,
            format!("cannot serialize credentials: {e}"),
        )
    })?;
    write_file(
        &path,
        &format!("# Zap registry credentials — do NOT commit. Managed by `zap login`.\n{text}"),
    )?;
    Ok(path)
}

/// Load the stored token for an index base/host, if any.
pub fn load_token(index_key: &str) -> Result<Option<String>> {
    Ok(load_credentials()?.tokens.get(index_key).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_project(root: &Path, name: &str, version: &str, deps: &str) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("zap.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"{version}\"\n\n[dependencies]\n{deps}"
            ),
        )
        .unwrap();
        fs::write(
            root.join("src/lib.zap"),
            format!("# {name}\ndef f(): pass\n"),
        )
        .unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static C: AtomicU32 = AtomicU32::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("zapc_pub-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn resolve_target_splits_local_from_hosted() {
        assert!(matches!(
            resolve_target("https://index.zaplang.org/"),
            Target::HostedApi(_)
        ));
        assert!(matches!(
            resolve_target("file:///tmp/idx"),
            Target::LocalIndex(_)
        ));
        assert!(matches!(resolve_target("/tmp/idx"), Target::LocalIndex(_)));
    }

    #[test]
    fn publish_writes_archive_config_and_line() {
        let base = tmp("ok");
        let proj = base.join("proj");
        let index = base.join("index");
        sample_project(&proj, "widgets", "1.0.0", "");

        let out = publish(&proj, Some(index.to_str().unwrap()), false).unwrap();
        assert_eq!(out.name, "widgets");
        assert_eq!(out.version, "1.0.0");
        assert!(out.cksum.starts_with("zap1:"));
        assert!(!out.dry_run);
        assert!(index.join("config.json").exists());
        // The index line is parseable and carries our checksum.
        let idx = fs::read_to_string(out.index_file.unwrap()).unwrap();
        let entries = index::parse_package(&idx).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].vers, "1.0.0");
        assert_eq!(entries[0].cksum, out.cksum);
        assert!(out.archive_file.unwrap().exists());
    }

    #[test]
    fn republishing_a_version_is_rejected() {
        let base = tmp("dup");
        let proj = base.join("proj");
        let index = base.join("index");
        sample_project(&proj, "dup", "2.0.0", "");
        publish(&proj, Some(index.to_str().unwrap()), false).unwrap();
        let err = publish(&proj, Some(index.to_str().unwrap()), false).unwrap_err();
        assert!(matches!(err, Error::Registry { code, .. } if code == ErrorCode::ZAP6106));
    }

    #[test]
    fn second_version_appends_without_rewriting() {
        let base = tmp("append");
        let proj = base.join("proj");
        let index = base.join("index");
        sample_project(&proj, "libz", "1.0.0", "");
        publish(&proj, Some(index.to_str().unwrap()), false).unwrap();
        sample_project(&proj, "libz", "1.1.0", "");
        let out = publish(&proj, Some(index.to_str().unwrap()), false).unwrap();
        let entries =
            index::parse_package(&fs::read_to_string(out.index_file.unwrap()).unwrap()).unwrap();
        let vers: Vec<_> = entries.iter().map(|e| e.vers.as_str()).collect();
        assert_eq!(vers, vec!["1.0.0", "1.1.0"]);
    }

    #[test]
    fn hosted_https_target_is_gated() {
        let base = tmp("gated");
        let proj = base.join("proj");
        sample_project(&proj, "p", "1.0.0", "");
        let err = publish(&proj, Some("https://index.zaplang.org/"), false).unwrap_err();
        assert!(matches!(err, Error::Registry { code, .. } if code == ErrorCode::ZAP6108));
    }

    #[test]
    fn invalid_manifest_version_is_rejected() {
        let base = tmp("badver");
        let proj = base.join("proj");
        let index = base.join("index");
        sample_project(&proj, "p", "not-semver", "");
        let err = publish(&proj, Some(index.to_str().unwrap()), true).unwrap_err();
        assert!(matches!(err, Error::Registry { code, .. } if code == ErrorCode::ZAP6107));
    }

    #[test]
    fn reserved_name_is_rejected() {
        let base = tmp("reserved");
        let proj = base.join("proj");
        let index = base.join("index");
        sample_project(&proj, "std", "1.0.0", "");
        let err = publish(&proj, Some(index.to_str().unwrap()), true).unwrap_err();
        assert!(matches!(err, Error::Registry { code, .. } if code == ErrorCode::ZAP6104));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let base = tmp("dry");
        let proj = base.join("proj");
        let index = base.join("index");
        sample_project(&proj, "p", "1.0.0", "");
        let out = publish(&proj, Some(index.to_str().unwrap()), true).unwrap();
        assert!(out.dry_run);
        assert!(
            !index.join("config.json").exists(),
            "dry-run must not write"
        );
    }

    #[test]
    fn registry_dep_recorded_git_dep_warns() {
        let base = tmp("deps");
        let proj = base.join("proj");
        let index = base.join("index");
        sample_project(
            &proj,
            "app",
            "1.0.0",
            "json = { registry = \"jsonlib\", version = \"1.4.0\" }\n\
             g = { git = \"https://x/g.git\", version = \"1.0.0\" }\n",
        );
        let out = publish(&proj, Some(index.to_str().unwrap()), false).unwrap();
        // The git dep produces a warning; the registry dep lands in the line.
        assert_eq!(out.warnings.len(), 1);
        let entries =
            index::parse_package(&fs::read_to_string(out.index_file.unwrap()).unwrap()).unwrap();
        assert_eq!(entries[0].deps.len(), 1);
        assert_eq!(entries[0].deps[0].name, "jsonlib");
        assert_eq!(entries[0].deps[0].req, "1.4.0");
        assert_eq!(entries[0].deps[0].major, 1);
    }

    #[test]
    fn token_store_round_trips_and_rejects_empty() {
        let cache = tmp("creds");
        std::env::set_var(crate::zenv::ZAP_CACHE, &cache);
        let key = "file:///tmp/some-index";
        assert!(load_token(key).unwrap().is_none());
        store_token(key, "  secret-abc  ").unwrap();
        assert_eq!(load_token(key).unwrap().as_deref(), Some("secret-abc"));
        assert!(matches!(
            store_token(key, "   ").unwrap_err(),
            Error::Registry { code, .. } if code == ErrorCode::ZAP6109
        ));
        std::env::remove_var(crate::zenv::ZAP_CACHE);
    }
}
