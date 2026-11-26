//! Bundled pure-Zap stdlib modules (Option B, a D-0081 extension).
//!
//! A *bundled* stdlib module is ordinary pure-Zap source that ships **embedded in
//! the compiler** and is resolved by `import <name>` as a built-in — one blessed,
//! version-coherent implementation for the whole ecosystem, with no `zap add` and
//! no dependency-graph fan-in. Unlike the Rust **seam** stdlib modules (`math`,
//! `os`, …, [`zapc_sema::is_builtin_module`]), a bundled module is real Zap: the
//! module linker parses its embedded source and merges it into the program like
//! any other module (with a mangle prefix), so it carries interfaces, generics,
//! and `comptime` reflection — exactly what a seam cannot.
//!
//! The module linker ([`crate::modlink`]) resolves a bundled name to a synthetic
//! path label ([`synthetic_path`]); `load_file` recognizes that label and sources
//! the embedded text ([`source_for_label`]) instead of reading the filesystem.
//!
//! The first bundled module is `serde` (epic #1552) — the serialization
//! foundation, which must be a single blessed version because a large fraction of
//! the package ecosystem depends on it.

/// The embedded source of a bundled pure-Zap stdlib module, by its import name,
/// or `None` if `name` is not a bundled module.
pub fn bundled_source(name: &str) -> Option<&'static str> {
    match name {
        "serde" => Some(include_str!("../stdlib_zap/serde.zap")),
        _ => None,
    }
}

/// Whether `name` is a bundled pure-Zap stdlib module.
pub fn is_bundled(name: &str) -> bool {
    bundled_source(name).is_some()
}

/// The synthetic module path a bundled module loads under. Stable and unique, so
/// it serves as both the linker's dedup key and the diagnostic file label. It is
/// never a real filesystem path (it cannot be `canonicalize`d, which is why the
/// linker's `canonical` falls back to the path unchanged).
pub fn synthetic_path(name: &str) -> String {
    format!("<stdlib-zap:{name}>")
}

/// If `label` is a bundled synthetic path (from [`synthetic_path`]), the embedded
/// source it refers to; otherwise `None`.
pub fn source_for_label(label: &str) -> Option<&'static str> {
    let name = label.strip_prefix("<stdlib-zap:")?.strip_suffix('>')?;
    bundled_source(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_is_a_nonempty_bundled_module() {
        assert!(is_bundled("serde"));
        let src = bundled_source("serde").expect("serde source embedded");
        assert!(src.contains("def serialize[T, S: Serializer]"));
        assert!(src.contains("interface Deserializer:"));
    }

    #[test]
    fn unknown_names_are_not_bundled() {
        assert!(!is_bundled("math"));
        assert!(!is_bundled("definitely_not_a_module"));
        assert!(bundled_source("nope").is_none());
    }

    #[test]
    fn synthetic_path_round_trips_to_its_source() {
        let label = synthetic_path("serde");
        assert_eq!(label, "<stdlib-zap:serde>");
        assert!(source_for_label(&label).is_some());
        assert!(source_for_label("/some/real/path.zap").is_none());
        assert!(source_for_label("<stdlib-zap:nope>").is_none());
    }
}
