//! Registry package-name rules and normalization (design/0027-registry.md §3.2).
//!
//! A registry name is the **global** identifier a package is published under —
//! distinct from the *logical* name an `import` binds (§3.1). Two names that
//! fold to the same normalized form are the **same** package and cannot be
//! separately registered; this is the anti-squatting rule and the uniqueness
//! key the index stores.
//!
//! Normalization is, deliberately, the Cargo/Go idea reduced to its essentials:
//!
//! - **case-insensitive** — `JsonLib` == `jsonlib`;
//! - **`-` and `_` are folded together** and runs collapse — `json-lib`,
//!   `json_lib`, and `json__lib` are all `json_lib`.
//!
//! The canonical form uses `_` as the one separator and lower-case throughout.
//! A manifest may spell a dependency in any equivalent casing/separator; the
//! normalized form is what resolution, the lockfile identity, and the index
//! path (§1.1) all key off.

use crate::error::{Error, ErrorCode, Result};

/// Names that collide with the language, the `std` surface, or interop and are
/// refused at publish (and rejected here when named as a dependency). Kept
/// small and explicit; compared against the normalized bare name.
const RESERVED: &[&str] = &[
    "std", "zap", "zapc", "python", "core", "builtin", "builtins",
];

/// A validated, normalized registry name: an optional single-level `@scope` and
/// a bare name, each already lower-cased with `-`/`_` folded to `_`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegistryName {
    /// The scope without its leading `@` (e.g. `acme`), if the name is scoped.
    pub scope: Option<String>,
    /// The bare package name (e.g. `widgets`).
    pub name: String,
}

impl RegistryName {
    /// Parse and normalize a registry name as written in a manifest.
    ///
    /// Accepts a bare `name` or a scoped `@scope/name`; folds case and `-`/`_`;
    /// and rejects an empty, malformed, or reserved name with a stable
    /// [`ErrorCode::ZAP6104`].
    pub fn parse(raw: &str) -> Result<RegistryName> {
        let (scope, bare) = match raw.strip_prefix('@') {
            Some(rest) => {
                let (scope, name) = rest.split_once('/').ok_or_else(|| {
                    conflict(format!(
                        "scoped registry name `{raw}` must be written `@scope/name`"
                    ))
                })?;
                (Some(normalize_segment(scope, raw, "scope")?), name)
            }
            None => (None, raw),
        };
        let name = normalize_segment(bare, raw, "name")?;
        if scope.is_none() && RESERVED.contains(&name.as_str()) {
            return Err(conflict(format!(
                "registry name `{raw}` is reserved and cannot be used"
            )));
        }
        Ok(RegistryName { scope, name })
    }

    /// The canonical `@scope/name` (or `name`) string — the index uniqueness key.
    pub fn normalized(&self) -> String {
        match &self.scope {
            Some(s) => format!("@{s}/{}", self.name),
            None => self.name.clone(),
        }
    }
}

impl std::fmt::Display for RegistryName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.normalized())
    }
}

/// Validate one segment (scope or bare name) against the §3.2 shape and return
/// its normalized form. Shape (case-insensitive): a leading alphanumeric, then
/// up to 63 of `[a-z0-9_-]`; the folded result uses `_` and collapses runs.
fn normalize_segment(seg: &str, raw: &str, what: &str) -> Result<String> {
    if seg.is_empty() {
        return Err(conflict(format!(
            "registry name `{raw}` has an empty {what}"
        )));
    }
    let bytes = seg.as_bytes();
    if bytes.len() > 64 {
        return Err(conflict(format!(
            "registry name `{raw}` {what} is longer than 64 characters"
        )));
    }
    for (i, &b) in bytes.iter().enumerate() {
        let ok = b.is_ascii_alphanumeric() || (i > 0 && matches!(b, b'_' | b'-'));
        if !ok {
            return Err(conflict(format!(
                "registry name `{raw}` has an invalid {what}: only ASCII letters, \
                 digits, `-` and `_` are allowed (and the first character must be \
                 a letter or digit)"
            )));
        }
    }
    Ok(fold(seg))
}

/// Lower-case the segment and fold `-`/`_` to a single `_`, collapsing runs.
fn fold(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    let mut last_sep = false;
    for c in seg.chars() {
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

fn conflict(message: String) -> Error {
    Error::registry(ErrorCode::ZAP6104, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(s: &str) -> String {
        RegistryName::parse(s).unwrap().normalized()
    }

    #[test]
    fn case_and_separators_fold_to_one_identity() {
        assert_eq!(norm("jsonlib"), "jsonlib");
        assert_eq!(norm("JsonLib"), "jsonlib");
        assert_eq!(norm("json_lib"), "json_lib");
        assert_eq!(norm("json-lib"), "json_lib");
        assert_eq!(norm("JSON--LIB"), "json_lib");
        assert_eq!(norm("json__lib"), "json_lib");
        // All the dashed/underscored spellings are one normalized name.
        assert_eq!(norm("Json-Lib"), norm("json_lib"));
    }

    #[test]
    fn scopes_are_single_level_and_normalized() {
        let n = RegistryName::parse("@Acme/Wid-gets").unwrap();
        assert_eq!(n.scope.as_deref(), Some("acme"));
        assert_eq!(n.name, "wid_gets");
        assert_eq!(n.normalized(), "@acme/wid_gets");
    }

    #[test]
    fn reserved_bare_names_are_refused() {
        for r in ["std", "zap", "python", "Std", "ZAP"] {
            let e = RegistryName::parse(r).unwrap_err();
            assert!(matches!(e, Error::Registry { code, .. } if code == ErrorCode::ZAP6104));
        }
        // A reserved word is fine *inside* a scope's package name.
        assert!(RegistryName::parse("@acme/std").is_ok());
    }

    #[test]
    fn malformed_names_are_rejected() {
        assert!(RegistryName::parse("").is_err());
        assert!(RegistryName::parse("-leading").is_err());
        assert!(RegistryName::parse("_leading").is_err());
        assert!(RegistryName::parse("has space").is_err());
        assert!(RegistryName::parse("has.dot").is_err());
        assert!(RegistryName::parse("@/noscope").is_err());
        assert!(RegistryName::parse("@scope").is_err());
        assert!(RegistryName::parse("@scope/").is_err());
        assert!(RegistryName::parse(&"x".repeat(65)).is_err());
    }
}
