//! The remote registry client — the read path (design/0027-registry.md §6).
//!
//! This is the first *remote* implementation of the "given a name+version,
//! what does it require" idea the [`crate::resolver::Registry`] trait abstracts.
//! It fetches a static index over `file://` (or `https://` via `curl`), parses
//! it ([`crate::index`]), and hands versions and their requirements to the
//! unchanged MVS resolver through [`crate::ops`]. It also downloads and
//! verifies archives (§2.2) — the verification against the `zap1:` tree hash is
//! the integrity guarantee; the index/transport is untrusted.
//!
//! Only `file://` is implemented natively (every #236 test resolves against a
//! local static-fixture tree, §7). `https://` shells out to `curl` when present
//! — the house style took no transport dependency for the LSP/MCP/playground
//! servers, and `std::net` has no TLS. No HTTP or TLS crate is added here.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use semver::Version;

use crate::error::{Error, ErrorCode, Result};
use crate::index::{self, Config, IndexEntry};
use crate::name::RegistryName;

/// The environment variable overriding the default index base URL (§8).
pub const ZAP_REGISTRY: &str = "ZAP_REGISTRY";

/// The built-in default index base: the official Zap registry under the
/// `zaplang.org` host (design/0027-registry.md §8). `$ZAP_REGISTRY` overrides
/// it, a `[registries]` entry names an alternate, and a `registry` dep's
/// `index = "<id>"` selects one — so re-pointing after the org move is a config
/// change, not a code change. Provisional until the registry service is live
/// (I#303/#304); a consumer with no registry dependencies never contacts it.
pub const DEFAULT_INDEX: &str = "https://pkg.zaplang.org/";

/// Fetches the bytes at an index URL. Abstracted so the transport (`file://`,
/// `curl`) is swappable and so tests can resolve against a fixture tree.
pub trait Fetcher {
    /// Fetch the bytes at `url`. A missing resource is an `Err` (ZAP6105).
    fn fetch(&self, url: &str) -> Result<Vec<u8>>;
}

/// The default fetcher: native `file://` reads, and `https://`/`http://` via a
/// `curl` subprocess.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultFetcher;

impl Fetcher for DefaultFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        if let Some(path) = file_url_to_path(url) {
            return std::fs::read(&path)
                .map_err(|e| unreachable_index(format!("cannot read `{url}`: {e}")));
        }
        if url.starts_with("https://") || url.starts_with("http://") {
            return curl_fetch(url);
        }
        // A bare path (no scheme) is treated as a local file for convenience.
        std::fs::read(url).map_err(|e| unreachable_index(format!("cannot read `{url}`: {e}")))
    }
}

/// Fetch over HTTPS by shelling out to `curl` (conservative interim, §6; no TLS
/// crate). `-sSfL`: silent, show errors, fail on 4xx/5xx, follow redirects.
fn curl_fetch(url: &str) -> Result<Vec<u8>> {
    let out = Command::new("curl")
        .args(["-sSfL", "--", url])
        .output()
        .map_err(|e| {
            unreachable_index(format!(
                "https index fetch needs `curl`, which is unavailable ({e}); \
                 use a `file://` index or install curl"
            ))
        })?;
    if !out.status.success() {
        return Err(unreachable_index(format!(
            "curl failed to fetch `{url}`: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Convert a `file://` URL to a filesystem path, or `None` if not a file URL.
/// Public so the publish write path ([`crate::publish`]) can map a static
/// index's `file://` base and `dl` template to directories it writes into.
pub fn file_url_to_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    // `file:///abs` -> `/abs`; `file://host/abs` is uncommon — take the path.
    let path = match rest.strip_prefix('/') {
        // `file:///x` leaves `/x` after the first slash is consumed below.
        Some(_) => rest,
        None => rest,
    };
    // Strip a leading `localhost` authority if present.
    let path = path.strip_prefix("localhost").unwrap_or(path);
    Some(path.to_string())
}

/// The remote registry client: index base URLs plus a fetcher and the running
/// toolchain version (for the `min_zap` gate).
pub struct Registry<'f> {
    fetcher: &'f dyn Fetcher,
    /// The default (unnamed) index base URL, if configured.
    default_base: Option<String>,
    /// Named alternate indexes: id -> base URL (from `[registries]`, §8.3).
    named: BTreeMap<String, String>,
    /// The toolchain version a package's `min_zap` is checked against.
    toolchain: Version,
}

impl<'f> Registry<'f> {
    /// Build a client with an explicit default base and named indexes.
    pub fn new(
        fetcher: &'f dyn Fetcher,
        default_base: Option<String>,
        named: BTreeMap<String, String>,
        toolchain: Version,
    ) -> Registry<'f> {
        Registry {
            fetcher,
            default_base,
            named,
            toolchain,
        }
    }

    /// The running toolchain version.
    pub fn toolchain(&self) -> &Version {
        &self.toolchain
    }

    /// Resolve the index base URL for an optional index id. `None` means the
    /// default index.
    fn base(&self, index: Option<&str>) -> Result<&str> {
        match index {
            Some(id) => self.named.get(id).map(String::as_str).ok_or_else(|| {
                unreachable_index(format!(
                    "dependency names index `{id}`, which has no `[registries.{id}]` entry"
                ))
            }),
            None => self.default_base.as_deref().ok_or_else(|| {
                unreachable_index(
                    "no default registry index is configured (set ZAP_REGISTRY or a \
                     `[registries]` entry, or use `index = \"...\"`)",
                )
            }),
        }
    }

    /// Fetch and parse an index's `config.json`.
    pub fn config(&self, index: Option<&str>) -> Result<Config> {
        let url = join_url(self.base(index)?, "config.json");
        let bytes = self.fetcher.fetch(&url)?;
        let text = String::from_utf8_lossy(&bytes);
        Config::parse(&text)
    }

    /// Fetch and parse a package's version entries from its index file.
    pub fn package(&self, index: Option<&str>, name: &RegistryName) -> Result<Vec<IndexEntry>> {
        let config = self.config(index)?;
        let rel = index::index_path(name, config.bucketing);
        let url = join_url(self.base(index)?, &rel);
        let bytes = self
            .fetcher
            .fetch(&url)
            .map_err(|_| unreachable_index(format!("package `{name}` is not in the index")))?;
        let text = String::from_utf8_lossy(&bytes);
        index::parse_package(&text)
    }

    /// Download a version's archive from the `dl` template, returning the bytes
    /// and the codec the template's extension implies (so the archive format is
    /// a server-side decision carried entirely by `config.json`).
    pub fn archive(
        &self,
        index: Option<&str>,
        name: &RegistryName,
        vers: &str,
    ) -> Result<(Vec<u8>, crate::archive::Codec)> {
        let config = self.config(index)?;
        let codec = crate::archive::Codec::from_dl(&config.dl)?;
        let url = config.archive_url(name, vers);
        let bytes = self.fetcher.fetch(&url)?;
        Ok((bytes, codec))
    }

    /// Error (ZAP6103) if `entry`'s `min_zap` exceeds the running toolchain.
    pub fn check_min_zap(&self, entry: &IndexEntry) -> Result<()> {
        if let Some(min) = &entry.min_zap {
            let need = Version::parse(min).map_err(|e| {
                unreachable_index(format!(
                    "package `{}` {} has an invalid min_zap `{min}`: {e}",
                    entry.name, entry.vers
                ))
            })?;
            if need > self.toolchain {
                return Err(Error::registry(
                    ErrorCode::ZAP6103,
                    format!(
                        "package `{}` {} requires Zap >= {min}, but this toolchain is {}",
                        entry.name, entry.vers, self.toolchain
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// The default toolchain version (this crate's, i.e. the workspace version).
pub fn toolchain_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0))
}

/// Join an index base with a relative path, inserting exactly one `/`.
fn join_url(base: &str, rel: &str) -> String {
    let base = base.trim_end_matches('/');
    let rel = rel.trim_start_matches('/');
    format!("{base}/{rel}")
}

/// The default index base: `$ZAP_REGISTRY` when set and non-empty, else the
/// built-in [`DEFAULT_INDEX`] (`pkg.zaplang.org`). Env always wins.
pub fn default_index_base() -> String {
    std::env::var(ZAP_REGISTRY)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_INDEX.to_string())
}

/// Build the default client wiring from the process environment and a manifest's
/// `[registries]` table. `default_base` is `$ZAP_REGISTRY` if set, else the
/// built-in [`DEFAULT_INDEX`] (§8). A pure `git`/`path` project never consults
/// it, so resolution stays network-free.
pub fn default_registry<'f>(
    fetcher: &'f dyn Fetcher,
    named: BTreeMap<String, String>,
) -> Registry<'f> {
    Registry::new(
        fetcher,
        Some(default_index_base()),
        named,
        toolchain_version(),
    )
}

/// A `file://` URL for a local path (used to point the client at a fixture
/// tree). Absolute paths only.
pub fn file_url(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = p.strip_prefix('/') {
        format!("file:///{stripped}")
    } else {
        format!("file:///{p}")
    }
}

fn unreachable_index(message: impl Into<String>) -> Error {
    Error::registry(ErrorCode::ZAP6105, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_url_round_trips_to_path() {
        let p = Path::new("/tmp/fixture/index");
        let url = file_url(p);
        assert_eq!(url, "file:///tmp/fixture/index");
        assert_eq!(
            file_url_to_path(&url).as_deref(),
            Some("/tmp/fixture/index")
        );
    }

    #[test]
    fn join_url_single_slash() {
        assert_eq!(join_url("file:///a/", "b/c.json"), "file:///a/b/c.json");
        assert_eq!(join_url("https://h/i", "/x"), "https://h/i/x");
    }

    #[test]
    fn default_index_base_prefers_env_then_builtin() {
        // Serialized within this test; ZAP_REGISTRY is read only here among the
        // registry unit tests, and path/git resolution never consults the base.
        std::env::remove_var(ZAP_REGISTRY);
        assert_eq!(default_index_base(), DEFAULT_INDEX);
        assert_eq!(DEFAULT_INDEX, "https://pkg.zaplang.org/");
        std::env::set_var(ZAP_REGISTRY, "file:///tmp/my-index");
        assert_eq!(default_index_base(), "file:///tmp/my-index");
        // An empty override falls back to the built-in default.
        std::env::set_var(ZAP_REGISTRY, "");
        assert_eq!(default_index_base(), DEFAULT_INDEX);
        std::env::remove_var(ZAP_REGISTRY);
    }

    #[test]
    fn missing_default_index_is_a_clear_error() {
        let f = DefaultFetcher;
        let reg = Registry::new(&f, None, BTreeMap::new(), Version::new(1, 0, 0));
        let e = reg.config(None).unwrap_err();
        assert!(matches!(e, Error::Registry { code, .. } if code == ErrorCode::ZAP6105));
    }

    #[test]
    fn min_zap_gate() {
        let f = DefaultFetcher;
        let reg = Registry::new(&f, None, BTreeMap::new(), Version::new(1, 3, 0));
        let mut entry = IndexEntry {
            name: "p".into(),
            vers: "1.0.0".into(),
            deps: vec![],
            cksum: "zap1:00".into(),
            acksum: None,
            yanked: false,
            min_zap: Some("1.2.0".into()),
            edition: None,
        };
        assert!(reg.check_min_zap(&entry).is_ok());
        entry.min_zap = Some("2.0.0".into());
        let e = reg.check_min_zap(&entry).unwrap_err();
        assert!(matches!(e, Error::Registry { code, .. } if code == ErrorCode::ZAP6103));
    }
}
