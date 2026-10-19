//! The author-edited project manifest, `zap.toml` (D-0052).
//!
//! The manifest records package metadata and a map of **logical name ->
//! dependency source + version**. Reading is via typed serde structs;
//! *mutation* (`zap add`/`zap remove`) goes through [`edit`], which uses
//! `toml_edit` so an author's comments and formatting survive.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A parsed `zap.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
}

/// The `[package]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
}

/// A `[dependencies]` value: exactly one source key plus an optional version.
///
/// `jsonlib = { git = "…", version = "1.4.0" }` or `mathx = { path = "../mathx" }`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Dependency {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version: Option<String>,
}

/// A validated dependency source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A VCS source: a git URL plus a required minimum semver.
    Git { url: String, version: String },
    /// A local directory containing a `zap.toml`.
    Path { dir: String },
}

impl Dependency {
    /// Validate the table into a [`Source`], or explain why it is malformed.
    pub fn source(&self, name: &str) -> Result<Source> {
        match (&self.git, &self.path) {
            (Some(_), Some(_)) => Err(Error::Manifest(format!(
                "dependency `{name}` has both `git` and `path`; pick one"
            ))),
            (Some(url), None) => {
                let version = self.version.clone().ok_or_else(|| {
                    Error::Manifest(format!(
                        "git dependency `{name}` needs a `version` (its minimum semver)"
                    ))
                })?;
                Ok(Source::Git {
                    url: url.clone(),
                    version,
                })
            }
            (None, Some(dir)) => Ok(Source::Path { dir: dir.clone() }),
            (None, None) => Err(Error::Manifest(format!(
                "dependency `{name}` has no source; give `git`+`version` or `path`"
            ))),
        }
    }
}

impl Source {
    /// Build the matching [`Dependency`] table value.
    pub fn to_dependency(&self) -> Dependency {
        match self {
            Source::Git { url, version } => Dependency {
                git: Some(url.clone()),
                path: None,
                version: Some(version.clone()),
            },
            Source::Path { dir } => Dependency {
                git: None,
                path: Some(dir.clone()),
                version: None,
            },
        }
    }

    /// The `source = "…"` string recorded in `zap.lock`.
    pub fn lock_string(&self) -> String {
        match self {
            Source::Git { url, .. } => format!("git+{url}"),
            Source::Path { dir } => format!("path+{dir}"),
        }
    }
}

impl Manifest {
    /// Parse manifest text.
    pub fn parse(text: &str) -> Result<Manifest> {
        toml::from_str(text).map_err(|e| Error::Manifest(e.to_string()))
    }

    /// Serialize to canonical manifest text (used by scaffolding, not by edit).
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| Error::Manifest(e.to_string()))
    }
}

/// Comment-preserving edits to a `zap.toml`'s `[dependencies]` table.
pub mod edit {
    use super::*;
    use toml_edit::{DocumentMut, Item, Table, Value};

    fn parse_doc(text: &str) -> Result<DocumentMut> {
        text.parse::<DocumentMut>()
            .map_err(|e| Error::Manifest(e.to_string()))
    }

    /// Insert or replace `[dependencies].<name>` with `source`, returning the
    /// new manifest text. Formatting and comments elsewhere are preserved.
    pub fn add_dependency(text: &str, name: &str, source: &Source) -> Result<String> {
        let mut doc = parse_doc(text)?;
        if doc.get("dependencies").is_none() {
            doc["dependencies"] = Item::Table(Table::new());
        }
        let deps = doc["dependencies"].as_table_mut().ok_or_else(|| {
            Error::Manifest("`[dependencies]` is not a table in zap.toml".to_string())
        })?;

        let mut it = toml_edit::InlineTable::new();
        match source {
            Source::Git { url, version } => {
                it.insert("git", Value::from(url.as_str()));
                it.insert("version", Value::from(version.as_str()));
            }
            Source::Path { dir } => {
                it.insert("path", Value::from(dir.as_str()));
            }
        }
        deps.insert(name, Item::Value(Value::InlineTable(it)));
        Ok(doc.to_string())
    }

    /// Remove `[dependencies].<name>`. Errors if it is absent.
    pub fn remove_dependency(text: &str, name: &str) -> Result<String> {
        let mut doc = parse_doc(text)?;
        let removed = doc
            .get_mut("dependencies")
            .and_then(Item::as_table_mut)
            .map(|deps| deps.remove(name).is_some())
            .unwrap_or(false);
        if !removed {
            return Err(Error::Manifest(format!(
                "no dependency named `{name}` in zap.toml"
            )));
        }
        Ok(doc.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[package]
name = \"demo\"
version = \"0.1.0\"

[dependencies]
jsonlib = { git = \"https://example.com/jsonlib.git\", version = \"1.4.0\" }
mathx = { path = \"../mathx\" }
";

    #[test]
    fn round_trips_sources() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.package.name, "demo");
        assert_eq!(m.dependencies.len(), 2);
        assert_eq!(
            m.dependencies["jsonlib"].source("jsonlib").unwrap(),
            Source::Git {
                url: "https://example.com/jsonlib.git".into(),
                version: "1.4.0".into()
            }
        );
        assert_eq!(
            m.dependencies["mathx"].source("mathx").unwrap(),
            Source::Path {
                dir: "../mathx".into()
            }
        );
    }

    #[test]
    fn git_without_version_is_rejected() {
        let d = Dependency {
            git: Some("u".into()),
            path: None,
            version: None,
        };
        assert!(d.source("x").is_err());
    }

    #[test]
    fn both_sources_rejected() {
        let d = Dependency {
            git: Some("u".into()),
            path: Some("p".into()),
            version: Some("1.0.0".into()),
        };
        assert!(d.source("x").is_err());
    }

    #[test]
    fn edit_preserves_comments_and_adds_inline() {
        let text = "# my project\n[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
        let out = edit::add_dependency(
            text,
            "foo",
            &Source::Git {
                url: "https://x/foo.git".into(),
                version: "2.0.0".into(),
            },
        )
        .unwrap();
        assert!(out.contains("# my project"), "comment lost:\n{out}");
        assert!(out.contains("foo = {"), "not inline:\n{out}");
        assert!(out.contains("version = \"2.0.0\""), "{out}");
        // Re-parsing yields the new dependency.
        let m = Manifest::parse(&out).unwrap();
        assert!(m.dependencies.contains_key("foo"));
    }

    #[test]
    fn edit_remove_then_missing() {
        let out = edit::remove_dependency(SAMPLE, "mathx").unwrap();
        let m = Manifest::parse(&out).unwrap();
        assert!(!m.dependencies.contains_key("mathx"));
        assert!(m.dependencies.contains_key("jsonlib"));
        assert!(edit::remove_dependency(&out, "nope").is_err());
    }
}
