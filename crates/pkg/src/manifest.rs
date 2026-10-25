//! The author-edited project manifest, `zap.toml` (D-0052).
//!
//! The manifest records package metadata and a map of **logical name ->
//! dependency source + version**. Reading is via typed serde structs;
//! *mutation* (`zap add`/`zap remove`) goes through [`edit`], which uses
//! `toml_edit` so an author's comments and formatting survive.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::name::RegistryName;

/// A parsed `zap.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
    /// Named alternate indexes: an index id -> its static-index base URL
    /// (design/0027-registry.md §8.3). The default index is unnamed; a
    /// `registry` dependency may name one of these with `index = "<id>"`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub registries: BTreeMap<String, RegistryDef>,
}

/// The `[package]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
}

/// A `[registries.<id>]` entry: where an alternate static index lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryDef {
    /// The index base URL (an `https://` or `file://` root holding
    /// `config.json` and the per-package files, §1.1).
    pub index: String,
}

/// A `[dependencies]` value: exactly one source key plus an optional version.
///
/// `jsonlib = { git = "…", version = "1.4.0" }`, `mathx = { path = "../mathx" }`,
/// or `json = { registry = "jsonlib", version = "1.4.0" }` (§3.1).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Dependency {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path: Option<String>,
    /// The registry (global) name a `registry` dependency resolves — distinct
    /// from the table key, which is the logical name `import` binds (§3.1).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub registry: Option<String>,
    /// For a `registry` dependency, the id of a `[registries]` entry to resolve
    /// it through; absent means the default index.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub index: Option<String>,
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
    /// A registry source: a global package name resolved through a static
    /// index, with an optional minimum semver and an optional alternate index
    /// id (design/0027-registry.md §3.1). `version` is a **minimum**, exactly
    /// like `git`'s, so MVS is unchanged.
    Registry {
        name: String,
        version: Option<String>,
        index: Option<String>,
    },
}

impl Dependency {
    /// Validate the table into a [`Source`], or explain why it is malformed.
    pub fn source(&self, name: &str) -> Result<Source> {
        // Exactly one of `git` / `path` / `registry` must be present.
        let keys = [
            self.git.is_some(),
            self.path.is_some(),
            self.registry.is_some(),
        ];
        match keys.iter().filter(|p| **p).count() {
            0 => {
                return Err(Error::Manifest(format!(
                    "dependency `{name}` has no source; give `git`+`version`, `path`, \
                     or `registry`"
                )))
            }
            1 => {}
            _ => {
                return Err(Error::Manifest(format!(
                    "dependency `{name}` has more than one source key (`git`/`path`/\
                     `registry`); pick one"
                )))
            }
        }

        if let Some(url) = &self.git {
            let version = self.version.clone().ok_or_else(|| {
                Error::Manifest(format!(
                    "git dependency `{name}` needs a `version` (its minimum semver)"
                ))
            })?;
            return Ok(Source::Git {
                url: url.clone(),
                version,
            });
        }
        if let Some(dir) = &self.path {
            return Ok(Source::Path { dir: dir.clone() });
        }
        // Registry: `version` is optional (a minimum); `index` is optional.
        let reg = self.registry.clone().unwrap();
        Ok(Source::Registry {
            name: reg,
            version: self.version.clone(),
            index: self.index.clone(),
        })
    }
}

impl Source {
    /// Build the matching [`Dependency`] table value.
    pub fn to_dependency(&self) -> Dependency {
        match self {
            Source::Git { url, version } => Dependency {
                git: Some(url.clone()),
                version: Some(version.clone()),
                ..Dependency::default()
            },
            Source::Path { dir } => Dependency {
                path: Some(dir.clone()),
                ..Dependency::default()
            },
            Source::Registry {
                name,
                version,
                index,
            } => Dependency {
                registry: Some(name.clone()),
                index: index.clone(),
                version: version.clone(),
                ..Dependency::default()
            },
        }
    }

    /// The `source = "…"` string recorded in `zap.lock`.
    ///
    /// A registry source is `registry+<index-id>#<name>`, with the default
    /// index id elided to `registry+<name>` (§3.1); `<name>` is the normalized
    /// registry name so the pin is spelling-independent.
    pub fn lock_string(&self) -> String {
        match self {
            Source::Git { url, .. } => format!("git+{url}"),
            Source::Path { dir } => format!("path+{dir}"),
            Source::Registry { name, index, .. } => {
                let norm = RegistryName::parse(name)
                    .map(|n| n.normalized())
                    .unwrap_or_else(|_| name.clone());
                match index {
                    Some(id) => format!("registry+{id}#{norm}"),
                    None => format!("registry+{norm}"),
                }
            }
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
            Source::Registry {
                name: reg,
                version,
                index,
            } => {
                it.insert("registry", Value::from(reg.as_str()));
                if let Some(id) = index {
                    it.insert("index", Value::from(id.as_str()));
                }
                if let Some(v) = version {
                    it.insert("version", Value::from(v.as_str()));
                }
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
            ..Dependency::default()
        };
        assert!(d.source("x").is_err());
    }

    #[test]
    fn both_sources_rejected() {
        let d = Dependency {
            git: Some("u".into()),
            path: Some("p".into()),
            version: Some("1.0.0".into()),
            ..Dependency::default()
        };
        assert!(d.source("x").is_err());
        // git + registry is also rejected.
        let d = Dependency {
            git: Some("u".into()),
            registry: Some("r".into()),
            version: Some("1.0.0".into()),
            ..Dependency::default()
        };
        assert!(d.source("x").is_err());
    }

    #[test]
    fn registry_source_round_trips() {
        let text = "\
[package]
name = \"demo\"
version = \"0.1.0\"

[registries]
corp = { index = \"https://corp.example/index/\" }

[dependencies]
json = { registry = \"jsonlib\", version = \"1.4.0\" }
widget = { registry = \"@acme/widgets\", version = \"0.3.0\" }
intl = { registry = \"icu\", version = \"2.0.0\", index = \"corp\" }
bare = { registry = \"tiny\" }
";
        let m = Manifest::parse(text).unwrap();
        assert_eq!(m.registries["corp"].index, "https://corp.example/index/");
        assert_eq!(
            m.dependencies["json"].source("json").unwrap(),
            Source::Registry {
                name: "jsonlib".into(),
                version: Some("1.4.0".into()),
                index: None,
            }
        );
        // The lock string uses the normalized name; default index id elided.
        assert_eq!(
            m.dependencies["json"].source("json").unwrap().lock_string(),
            "registry+jsonlib"
        );
        assert_eq!(
            m.dependencies["intl"].source("intl").unwrap().lock_string(),
            "registry+corp#icu"
        );
        assert_eq!(
            m.dependencies["widget"]
                .source("widget")
                .unwrap()
                .lock_string(),
            "registry+@acme/widgets"
        );
        // A registry dep needs no version (the minimum is optional).
        assert_eq!(
            m.dependencies["bare"].source("bare").unwrap(),
            Source::Registry {
                name: "tiny".into(),
                version: None,
                index: None,
            }
        );
    }

    #[test]
    fn edit_adds_registry_inline() {
        let text = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
        let out = edit::add_dependency(
            text,
            "json",
            &Source::Registry {
                name: "jsonlib".into(),
                version: Some("1.4.0".into()),
                index: Some("corp".into()),
            },
        )
        .unwrap();
        assert!(out.contains("registry = \"jsonlib\""), "{out}");
        assert!(out.contains("index = \"corp\""), "{out}");
        assert!(out.contains("version = \"1.4.0\""), "{out}");
        let m = Manifest::parse(&out).unwrap();
        assert!(m.dependencies.contains_key("json"));
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
