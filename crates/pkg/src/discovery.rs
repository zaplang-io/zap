//! Package discovery — `zap search` and `zap info` (design/0027-registry.md
//! §1; D-0083, I#237).
//!
//! Two shapes of discovery, matching what a **static** index can actually
//! answer:
//!
//! - **`zap info <name>`** is a direct name → one-file lookup. It works against
//!   *any* static index — local `file://` or a remote CDN — because the read
//!   client already fetches exactly the one package file (§6 step 1). It is the
//!   fully-supported remote discovery path.
//! - **`zap search <query>`** needs to *enumerate* packages, which a sparse
//!   static index served over HTTPS cannot do (no directory listing; the design
//!   deliberately rejected a dynamic search service, §1.4). So search walks a
//!   **local/`file://`** index tree. Over an `https://` index it returns a
//!   clear error pointing at `zap info` — honest about the static-index limit
//!   rather than pretending a search endpoint exists.
//!
//! A hosted search endpoint (served from `config.json`'s `api`, or a generated
//! catalog file) is a follow-up gated on the same hosting decision as publish
//! (§5.2/§7).

use std::fs;
use std::path::{Path, PathBuf};

use semver::Version;

use crate::error::{Error, ErrorCode, Result};
use crate::index::{self, IndexEntry};
use crate::registry;

/// A one-line discovery summary of a package in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSummary {
    /// The normalized registry name.
    pub name: String,
    /// The highest published version (yanked ones included as a fallback).
    pub latest: String,
    /// How many versions are published.
    pub versions: usize,
    /// True if every published version is yanked.
    pub all_yanked: bool,
}

/// The highest version among `entries`, preferring a non-yanked one; `None`
/// only for an empty set. Returns `(version_string, all_yanked)`.
pub fn latest(entries: &[IndexEntry]) -> Option<(String, bool)> {
    if entries.is_empty() {
        return None;
    }
    let mut parsed: Vec<(Version, &IndexEntry)> = entries
        .iter()
        .filter_map(|e| Version::parse(&e.vers).ok().map(|v| (v, e)))
        .collect();
    if parsed.is_empty() {
        // No parseable semver; fall back to the last published line.
        let all_yanked = entries.iter().all(|e| e.yanked);
        return Some((entries[entries.len() - 1].vers.clone(), all_yanked));
    }
    parsed.sort_by(|a, b| a.0.cmp(&b.0));
    let all_yanked = parsed.iter().all(|(_, e)| e.yanked);
    // Highest non-yanked, else the highest overall.
    let pick = parsed
        .iter()
        .rev()
        .find(|(_, e)| !e.yanked)
        .or_else(|| parsed.last())
        .unwrap();
    Some((pick.1.vers.clone(), all_yanked))
}

/// `zap search <query>` over a local/`file://` static index at `base`.
///
/// Enumerates every package file under the index tree, parses it, and returns a
/// summary for each whose normalized name contains `query` (case-insensitive,
/// with the same `-`/`_` folding registry names use). An empty query lists all.
/// Results are sorted by name.
pub fn search(base: &str, query: &str) -> Result<Vec<PackageSummary>> {
    let dir = local_index_dir(base)?;
    let needle = fold_query(query);

    let mut out = Vec::new();
    let mut files = Vec::new();
    collect_index_files(&dir, &dir, &mut files);
    for file in files {
        let text =
            fs::read_to_string(&file).map_err(|e| Error::io(file.display().to_string(), e))?;
        let entries = index::parse_package(&text)?;
        let Some((latest, all_yanked)) = latest(&entries) else {
            continue;
        };
        // All lines in a package file share one name; take it from the first.
        let name = entries[0].name.clone();
        if needle.is_empty() || fold_query(&name).contains(&needle) {
            out.push(PackageSummary {
                name,
                latest,
                versions: entries.len(),
                all_yanked,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Resolve an index base to a local directory for enumeration, or explain why
/// enumeration is not possible (a remote static index has no listing).
fn local_index_dir(base: &str) -> Result<PathBuf> {
    if base.starts_with("https://") || base.starts_with("http://") {
        return Err(Error::registry(
            ErrorCode::ZAP6105,
            format!(
                "`{base}` is a remote static index, which cannot be enumerated for search (no \
                 directory listing; a hosted search endpoint is a follow-up, D-0027 §1.4/§7). \
                 Use `zap info <name>` for a direct lookup, or search a local/`file://` index."
            ),
        ));
    }
    let path = registry::file_url_to_path(base).unwrap_or_else(|| base.to_string());
    let dir = PathBuf::from(path);
    if !dir.exists() {
        return Err(Error::registry(
            ErrorCode::ZAP6105,
            format!("index `{}` does not exist", dir.display()),
        ));
    }
    Ok(dir)
}

/// Recursively collect package index files (every `*.json` except the root
/// `config.json`) under `dir`.
fn collect_index_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_index_files(root, &path, out);
        } else if path.extension().map(|e| e == "json").unwrap_or(false) {
            // Skip the index-root `config.json` metadata file.
            let is_root_config = path
                .file_name()
                .map(|n| n == "config.json")
                .unwrap_or(false)
                && path.parent() == Some(root);
            if !is_root_config {
                out.push(path);
            }
        }
    }
}

/// Fold a query/name the way registry names compare: lower-case, `-`/`_` to `_`.
fn fold_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_sep = false;
    for c in s.chars() {
        if c == '-' || c == '_' {
            if !last_sep {
                out.push('_');
                last_sep = true;
            }
        } else {
            out.push(c.to_ascii_lowercase());
            last_sep = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::RegistryName;

    fn entry(name: &str, vers: &str, yanked: bool) -> IndexEntry {
        IndexEntry {
            name: name.to_string(),
            vers: vers.to_string(),
            deps: vec![],
            cksum: "zap1:00".to_string(),
            acksum: None,
            yanked,
            min_zap: None,
            edition: None,
        }
    }

    #[test]
    fn latest_prefers_non_yanked_highest() {
        let e = vec![
            entry("p", "1.0.0", false),
            entry("p", "1.2.0", true),
            entry("p", "1.1.0", false),
        ];
        assert_eq!(latest(&e), Some(("1.1.0".to_string(), false)));
    }

    #[test]
    fn latest_falls_back_to_highest_when_all_yanked() {
        let e = vec![entry("p", "1.0.0", true), entry("p", "1.2.0", true)];
        assert_eq!(latest(&e), Some(("1.2.0".to_string(), true)));
    }

    fn write_index(root: &Path, name: &str, lines: &[&str]) {
        let rn = RegistryName::parse(name).unwrap();
        let rel = index::index_path(&rn, index::Bucketing::CargoSparse);
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
    }

    #[test]
    fn search_enumerates_and_filters_local_index() {
        let dir = std::env::temp_dir().join(format!("zapc_disc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.json"),
            "{\"index_version\":1,\"dl\":\"x\"}",
        )
        .unwrap();
        write_index(
            &dir,
            "jsonlib",
            &[r#"{"name":"jsonlib","vers":"1.4.0","deps":[],"cksum":"zap1:aa","yanked":false}"#],
        );
        write_index(
            &dir,
            "bytesx",
            &[r#"{"name":"bytesx","vers":"0.3.0","deps":[],"cksum":"zap1:bb","yanked":false}"#],
        );

        let all = search(dir.to_str().unwrap(), "").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "bytesx"); // sorted
        assert_eq!(all[1].name, "jsonlib");

        let hit = search(dir.to_str().unwrap(), "JSON").unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].name, "jsonlib");
        assert_eq!(hit[0].latest, "1.4.0");
    }

    #[test]
    fn remote_search_is_a_clear_error() {
        let err = search("https://index.example.org/", "x").unwrap_err();
        assert!(matches!(err, Error::Registry { code, .. } if code == ErrorCode::ZAP6105));
    }
}
