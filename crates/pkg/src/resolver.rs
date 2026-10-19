//! Minimal Version Selection (MVS), Go-style (D-0052).
//!
//! MVS selects, for each module, the **maximum of the minimum versions**
//! required anywhere in the dependency graph. It is deterministic and
//! reproducible by construction — no SAT solver, no "latest that fits" drift.
//!
//! A **major version is the compatibility boundary**: the resolver's module
//! identity is `(name, major)`, so `foo` v1 and `foo` v2 are distinct modules
//! that may coexist (Go's "different major = different module path"). Within one
//! `(name, major)` the greatest required minimum wins.
//!
//! The algorithm is pure and generic over a [`Registry`] (which answers "what
//! does module@version require?"), so it is unit-tested with an in-memory map
//! and reused by the on-disk project resolver in [`crate::ops`].

use std::collections::BTreeMap;

use semver::Version;

use crate::error::{Error, Result};

/// A module's resolver identity: its name and major version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId {
    pub name: String,
    pub major: u64,
}

impl ModuleId {
    pub fn of(name: &str, version: &Version) -> ModuleId {
        ModuleId {
            name: name.to_string(),
            major: version.major,
        }
    }
}

/// A single `(module, minimum version)` requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub name: String,
    pub version: Version,
}

impl Requirement {
    /// Parse `name` + a semver string, mapping a bad version to a clear error.
    pub fn parse(name: &str, version: &str) -> Result<Requirement> {
        Ok(Requirement {
            name: name.to_string(),
            version: parse_version(name, version)?,
        })
    }
}

/// Parse a semver string, attributing a parse error to `name`.
pub fn parse_version(name: &str, version: &str) -> Result<Version> {
    Version::parse(version).map_err(|e| {
        Error::Resolve(format!(
            "module `{name}` has invalid version `{version}`: {e}"
        ))
    })
}

/// Answers "what direct requirements does `name`@`version` declare?".
pub trait Registry {
    fn requirements(&self, name: &str, version: &Version) -> Result<Vec<Requirement>>;
}

/// Run MVS from the root's direct requirements, returning the selected version
/// of every module in the build list.
///
/// Termination: each module identity's selected version only ever increases,
/// and there are finitely many versions, so the worklist drains.
pub fn resolve(
    roots: &[Requirement],
    registry: &dyn Registry,
) -> Result<BTreeMap<ModuleId, Version>> {
    let mut selected: BTreeMap<ModuleId, Version> = BTreeMap::new();
    let mut work: Vec<Requirement> = roots.to_vec();

    while let Some(req) = work.pop() {
        let id = ModuleId::of(&req.name, &req.version);
        // If an equal-or-greater version of this (name, major) is already
        // selected, its requirements are (or will be) covered.
        if let Some(cur) = selected.get(&id) {
            if *cur >= req.version {
                continue;
            }
        }
        selected.insert(id, req.version.clone());
        for dep in registry.requirements(&req.name, &req.version)? {
            work.push(dep);
        }
    }

    Ok(selected)
}

/// The build list as `(name, version)` pairs, sorted by name then version.
pub fn build_list(selected: &BTreeMap<ModuleId, Version>) -> Vec<(String, Version)> {
    let mut list: Vec<(String, Version)> = selected
        .iter()
        .map(|(id, v)| (id.name.clone(), v.clone()))
        .collect();
    list.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An in-memory registry: `(name, version)` -> its requirements.
    #[derive(Default)]
    struct MapRegistry {
        reqs: HashMap<(String, Version), Vec<Requirement>>,
    }

    impl MapRegistry {
        fn add(&mut self, name: &str, version: &str, reqs: &[(&str, &str)]) {
            let v = Version::parse(version).unwrap();
            let rs = reqs
                .iter()
                .map(|(n, vs)| Requirement::parse(n, vs).unwrap())
                .collect();
            self.reqs.insert((name.to_string(), v), rs);
        }
    }

    impl Registry for MapRegistry {
        fn requirements(&self, name: &str, version: &Version) -> Result<Vec<Requirement>> {
            Ok(self
                .reqs
                .get(&(name.to_string(), version.clone()))
                .cloned()
                .unwrap_or_default())
        }
    }

    fn req(name: &str, version: &str) -> Requirement {
        Requirement::parse(name, version).unwrap()
    }

    fn selected_version<'a>(
        sel: &'a BTreeMap<ModuleId, Version>,
        name: &str,
        major: u64,
    ) -> Option<&'a Version> {
        sel.get(&ModuleId {
            name: name.to_string(),
            major,
        })
    }

    #[test]
    fn takes_the_max_of_minimums() {
        // root needs a@1.1 and b; b needs a@1.3. MVS picks a@1.3.
        let mut reg = MapRegistry::default();
        reg.add("a", "1.1.0", &[]);
        reg.add("a", "1.3.0", &[]);
        reg.add("b", "1.0.0", &[("a", "1.3.0")]);

        let sel = resolve(&[req("a", "1.1.0"), req("b", "1.0.0")], &reg).unwrap();
        assert_eq!(
            selected_version(&sel, "a", 1).unwrap(),
            &Version::parse("1.3.0").unwrap()
        );
    }

    #[test]
    fn transitive_requirements_are_included() {
        let mut reg = MapRegistry::default();
        reg.add("app_dep", "1.0.0", &[("deep", "2.5.0")]);
        reg.add("deep", "2.5.0", &[]);

        let sel = resolve(&[req("app_dep", "1.0.0")], &reg).unwrap();
        assert_eq!(build_list(&sel).len(), 2);
        assert_eq!(
            selected_version(&sel, "deep", 2).unwrap(),
            &Version::parse("2.5.0").unwrap()
        );
    }

    #[test]
    fn different_majors_coexist() {
        // foo v1 and foo v2 are distinct modules; both are selected.
        let mut reg = MapRegistry::default();
        reg.add("x", "1.0.0", &[("foo", "1.2.0")]);
        reg.add("y", "1.0.0", &[("foo", "2.1.0")]);
        reg.add("foo", "1.2.0", &[]);
        reg.add("foo", "2.1.0", &[]);

        let sel = resolve(&[req("x", "1.0.0"), req("y", "1.0.0")], &reg).unwrap();
        assert_eq!(
            selected_version(&sel, "foo", 1).unwrap(),
            &Version::parse("1.2.0").unwrap()
        );
        assert_eq!(
            selected_version(&sel, "foo", 2).unwrap(),
            &Version::parse("2.1.0").unwrap()
        );
    }

    #[test]
    fn higher_version_requirements_are_processed() {
        // Raising a's selected version must pull in the requirements of the
        // *higher* version, not just the one first seen.
        let mut reg = MapRegistry::default();
        reg.add("a", "1.0.0", &[]);
        reg.add("a", "1.5.0", &[("extra", "1.0.0")]);
        reg.add("b", "1.0.0", &[("a", "1.5.0")]);
        reg.add("extra", "1.0.0", &[]);

        let sel = resolve(&[req("a", "1.0.0"), req("b", "1.0.0")], &reg).unwrap();
        assert_eq!(
            selected_version(&sel, "a", 1).unwrap(),
            &Version::parse("1.5.0").unwrap()
        );
        assert!(
            selected_version(&sel, "extra", 1).is_some(),
            "requirement of the raised version was dropped"
        );
    }

    #[test]
    fn empty_graph_resolves_to_nothing() {
        let reg = MapRegistry::default();
        let sel = resolve(&[], &reg).unwrap();
        assert!(sel.is_empty());
    }

    #[test]
    fn diamond_converges() {
        // root -> l, r; l -> base@1.1; r -> base@1.4. base@1.4 wins once.
        let mut reg = MapRegistry::default();
        reg.add("l", "1.0.0", &[("base", "1.1.0")]);
        reg.add("r", "1.0.0", &[("base", "1.4.0")]);
        reg.add("base", "1.1.0", &[]);
        reg.add("base", "1.4.0", &[]);

        let sel = resolve(&[req("l", "1.0.0"), req("r", "1.0.0")], &reg).unwrap();
        let list = build_list(&sel);
        assert_eq!(list.iter().filter(|(n, _)| n == "base").count(), 1);
        assert_eq!(
            selected_version(&sel, "base", 1).unwrap(),
            &Version::parse("1.4.0").unwrap()
        );
    }
}
