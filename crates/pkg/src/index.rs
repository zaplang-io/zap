//! The static-file index schema and its parser (design/0027-registry.md §1).
//!
//! The index is a tree of plain files over HTTPS/`file://` (no server). Each
//! package has one **line-delimited-JSON** file — one object per published
//! version, in publish order — at a sparse fan-out path derived from its
//! normalized name; a single `config.json` at the index root carries the
//! metadata (archive URL template, fan-out scheme). This module is the pure
//! parse + path-layout layer; fetching lives in [`crate::registry`].
//!
//! Parsing uses the in-house JSON codec ([`zapc_json`]) — no `serde_json`.
//! **Unknown fields are ignored** so the schema grows at MINOR (D-0070 §1.4).

use zapc_json::Json;

use crate::error::{Error, ErrorCode, Result};
use crate::name::RegistryName;

/// The fan-out scheme a `config.json` names. Read from the index, never
/// hard-coded, so the layout can evolve (§1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucketing {
    /// Cargo's sparse layout: 1/2/3-char names bucket by length (and, at 3, by
    /// first char), and 4+-char names bucket by `first-two/next-two`.
    CargoSparse,
}

impl Bucketing {
    fn parse(s: &str) -> Result<Bucketing> {
        match s {
            "cargo-sparse" => Ok(Bucketing::CargoSparse),
            other => Err(malformed(format!("unknown index bucketing `{other}`"))),
        }
    }
}

/// The index root metadata (`config.json`, §1.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The index format version (additive growth within a major, §1.4).
    pub index_version: u32,
    /// The archive URL template, with `{name}` and `{vers}` placeholders.
    pub dl: String,
    /// The publish-only write endpoint (#237); unused on the read path.
    pub api: Option<String>,
    /// The fan-out scheme for per-package file paths.
    pub bucketing: Bucketing,
}

impl Config {
    /// Parse a `config.json` document. Unknown fields are ignored.
    pub fn parse(text: &str) -> Result<Config> {
        let json = zapc_json::parse(text).map_err(|e| malformed(format!("config.json: {e}")))?;
        let index_version = json
            .get("index_version")
            .and_then(Json::as_i64)
            .ok_or_else(|| malformed("config.json: missing integer `index_version`"))?;
        let dl = json
            .get("dl")
            .and_then(Json::as_str)
            .ok_or_else(|| malformed("config.json: missing string `dl` template"))?
            .to_string();
        let bucketing = match json.get("bucketing").and_then(Json::as_str) {
            Some(s) => Bucketing::parse(s)?,
            None => Bucketing::CargoSparse,
        };
        let api = json
            .get("api")
            .and_then(Json::as_str)
            .map(|s| s.to_string());
        Ok(Config {
            index_version: index_version.max(0) as u32,
            dl,
            api,
            bucketing,
        })
    }

    /// Expand the `dl` template for a `(name, vers)`. `{name}` and `{vers}` are
    /// substituted; the normalized name is used so a pin is spelling-stable.
    pub fn archive_url(&self, name: &RegistryName, vers: &str) -> String {
        self.dl
            .replace("{name}", &name.normalized())
            .replace("{vers}", vers)
    }
}

/// One dependency of a published version, as recorded in an index line (§1.2):
/// `{name, req, major}`, optionally with `registry` for a cross-index dep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDep {
    pub name: String,
    /// The minimum (MVS) semver requirement, as written.
    pub req: String,
    /// The `(name, major)` identity the client need not re-parse.
    pub major: u64,
    /// A different index this dep is resolved through, if any (§8.3).
    pub registry: Option<String>,
}

/// One published version's index line (§1.2). Unknown fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub name: String,
    pub vers: String,
    pub deps: Vec<IndexDep>,
    /// The `zap1:` tree-hash checksum — the same value the lockfile pins (§2).
    pub cksum: String,
    /// The archive-bytes SHA-256 (hex, no prefix), when the publisher recorded
    /// it: the cheap pre-unpack transfer check of §2.1/§2.2. Optional/additive
    /// — §1.2's base schema omits it, so an older index simply has no pre-check
    /// and the authoritative `zap1:` tree hash still verifies the content.
    pub acksum: Option<String>,
    pub yanked: bool,
    /// The minimum toolchain version that can build this package, if declared.
    pub min_zap: Option<String>,
    /// The language edition tag, if declared (reserved, §1.2).
    pub edition: Option<String>,
}

impl IndexEntry {
    fn from_json(json: &Json) -> Result<IndexEntry> {
        let name = str_field(json, "name")?;
        let vers = str_field(json, "vers")?;
        let cksum = str_field(json, "cksum")?;
        let acksum = json
            .get("acksum")
            .and_then(Json::as_str)
            .map(str::to_string);
        let yanked = matches!(json.get("yanked"), Some(Json::Bool(true)));
        let min_zap = json
            .get("min_zap")
            .and_then(Json::as_str)
            .map(str::to_string);
        let edition = json
            .get("edition")
            .and_then(Json::as_str)
            .map(str::to_string);
        let mut deps = Vec::new();
        if let Some(arr) = json.get("deps").and_then(Json::as_array) {
            for d in arr {
                deps.push(IndexDep {
                    name: str_field(d, "name")?,
                    req: str_field(d, "req")?,
                    major: d.get("major").and_then(Json::as_i64).unwrap_or(0).max(0) as u64,
                    registry: d.get("registry").and_then(Json::as_str).map(str::to_string),
                });
            }
        }
        Ok(IndexEntry {
            name,
            vers,
            deps,
            cksum,
            acksum,
            yanked,
            min_zap,
            edition,
        })
    }
}

/// Parse a package's line-delimited-JSON index file into its version entries,
/// in file (publish) order. Blank lines are skipped; a malformed line is an
/// error (the index is served, not hand-edited).
pub fn parse_package(text: &str) -> Result<Vec<IndexEntry>> {
    let mut entries = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let json = zapc_json::parse(line)
            .map_err(|e| malformed(format!("index line {}: {e}", lineno + 1)))?;
        entries.push(IndexEntry::from_json(&json)?);
    }
    Ok(entries)
}

/// The relative path of a package's index file under the index root, for the
/// given fan-out scheme (§1.1). A scope becomes a leading `@scope/` directory.
pub fn index_path(name: &RegistryName, bucketing: Bucketing) -> String {
    let Bucketing::CargoSparse = bucketing;
    let bare = &name.name;
    let fan = cargo_sparse_fanout(bare);
    match &name.scope {
        Some(scope) => format!("@{scope}/{fan}/{bare}.json"),
        None => format!("{fan}/{bare}.json"),
    }
}

/// Cargo's sparse fan-out directory prefix for a bare (already-normalized)
/// name: `1`/`2` for 1–2 chars, `3/<c0>` for 3, `<c0c1>/<c2c3>` for 4+.
fn cargo_sparse_fanout(bare: &str) -> String {
    let chars: Vec<char> = bare.chars().collect();
    match chars.len() {
        0 => "_".to_string(), // unreachable: validated non-empty upstream.
        1 => "1".to_string(),
        2 => "2".to_string(),
        3 => format!("3/{}", chars[0]),
        _ => format!("{}{}/{}{}", chars[0], chars[1], chars[2], chars[3]),
    }
}

fn str_field(json: &Json, key: &str) -> Result<String> {
    json.get(key)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| malformed(format!("index entry: missing string `{key}`")))
}

fn malformed(message: impl Into<String>) -> Error {
    Error::registry(ErrorCode::ZAP6105, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rn(s: &str) -> RegistryName {
        RegistryName::parse(s).unwrap()
    }

    #[test]
    fn config_parses_and_ignores_unknown_fields() {
        let c = Config::parse(
            r#"{"index_version":1,"dl":"https://a/{name}/{name}-{vers}.zap.tar","api":"https://b","bucketing":"cargo-sparse","future_field":42}"#,
        )
        .unwrap();
        assert_eq!(c.index_version, 1);
        assert_eq!(c.bucketing, Bucketing::CargoSparse);
        assert_eq!(c.api.as_deref(), Some("https://b"));
        assert_eq!(
            c.archive_url(&rn("jsonlib"), "1.4.0"),
            "https://a/jsonlib/jsonlib-1.4.0.zap.tar"
        );
    }

    #[test]
    fn config_rejects_unknown_bucketing() {
        assert!(Config::parse(r#"{"index_version":1,"dl":"x","bucketing":"weird"}"#).is_err());
    }

    #[test]
    fn package_lines_parse_in_order_with_blank_lines_skipped() {
        let text = concat!(
            "{\"name\":\"jsonlib\",\"vers\":\"1.4.0\",\"deps\":[{\"name\":\"bytesx\",\"req\":\"0.3.0\",\"major\":0}],\"cksum\":\"zap1:aa\",\"yanked\":false,\"min_zap\":\"1.2.0\"}\n",
            "\n",
            "{\"name\":\"jsonlib\",\"vers\":\"1.5.0\",\"deps\":[],\"cksum\":\"zap1:bb\",\"yanked\":true,\"extra\":\"ignored\"}\n",
        );
        let e = parse_package(text).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].vers, "1.4.0");
        assert_eq!(e[0].deps[0].name, "bytesx");
        assert_eq!(e[0].deps[0].req, "0.3.0");
        assert_eq!(e[0].min_zap.as_deref(), Some("1.2.0"));
        assert!(!e[0].yanked);
        assert_eq!(e[1].vers, "1.5.0");
        assert!(e[1].yanked);
        assert!(e[1].deps.is_empty());
    }

    #[test]
    fn fan_out_matches_cargo_sparse() {
        assert_eq!(index_path(&rn("a"), Bucketing::CargoSparse), "1/a.json");
        assert_eq!(index_path(&rn("ab"), Bucketing::CargoSparse), "2/ab.json");
        assert_eq!(
            index_path(&rn("abc"), Bucketing::CargoSparse),
            "3/a/abc.json"
        );
        assert_eq!(
            index_path(&rn("cargo"), Bucketing::CargoSparse),
            "ca/rg/cargo.json"
        );
        assert_eq!(
            index_path(&rn("@acme/widgets"), Bucketing::CargoSparse),
            "@acme/wi/dg/widgets.json"
        );
    }

    #[test]
    fn malformed_line_is_an_error_with_code() {
        let e = parse_package("not json\n").unwrap_err();
        assert!(matches!(e, Error::Registry { code, .. } if code == ErrorCode::ZAP6105));
    }
}
