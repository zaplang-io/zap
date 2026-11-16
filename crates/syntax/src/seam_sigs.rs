//! Seam calling-convention signature registry (#998, D-0204).
//!
//! Built-in (seam) module functions are hand-written check/lower pairs in
//! `zapc_sema::modules` and `zapc_ir::lower::modules` that read their arguments
//! **positionally**. Without a declared parameter list there is nowhere to bind
//! a keyword, no default to fill by name, and no name to validate — so a
//! misspelled keyword on a stdlib call was silently accepted and keyword
//! reordering bound by position (the #998 footgun).
//!
//! This module is the **single source of truth** for the declared parameter
//! names of seam methods, consumed by both facets:
//!
//!   * `zapc_sema::modules::bind_seam_args` — binds keywords by name, fills
//!     omitted trailing optionals, rejects an unknown keyword (`ZAP4023`, the
//!     same a user function emits), and hands the per-method checker the
//!     arguments normalized to declared positional order; and
//!   * `zapc_ir::lower::modules::reorder_seam_args` — the pure lowering twin
//!     that reorders the same call so the emitted runtime call gets the
//!     arguments in the right slots.
//!
//! It is kept in its **own file**, separate from the per-module check/lower
//! code and from [`crate::seam`] (which handles module-qualified generic
//! *construction*, D-0202): the per-module `.rs` files stay pure type/arity
//! logic, and this table is a flat, reviewable list of signatures. Adding
//! keyword/default support to a seam method is a single row here — the central
//! dispatch does the binding, so the per-method `check`/`lower` bodies are
//! untouched (they keep doing arity + type checks on the normalized args).
//!
//! **Convention for the rows.** Parameters are listed in declared positional
//! order; any optional parameters must come **last** (the binder rejects an
//! omitted parameter that is followed by a supplied one, so only trailing
//! parameters may be dropped — which matches every current seam method, whose
//! optionals are the trailing `level`/`headers`). A method with an *interior*
//! optional would need a richer per-parameter model; none exists today. A
//! method with **no** row keeps the legacy positional-only behavior (names
//! ignored), so migration is incremental and the eventual global default of
//! *"an un-listed method rejects keywords"* can be turned on once every
//! keyword-taking method has a row.

/// `(module, method, declared positional parameter names)`. The module name is
/// the seam registry name as dispatched (`"zlib"`, `"gzip"`, `"http"`, …), i.e.
/// the same `module` string the sema/lowering dispatch receives.
pub const SEAM_METHOD_SIGS: &[(&str, &str, &[&str])] = &[
    // ---- zlib / gzip (ticket #106): compression with an optional level ----
    // Shared check/lower path, so both modules carry the same signatures.
    ("zlib", "compress", &["data", "level"]),
    ("zlib", "decompress", &["data"]),
    ("zlib", "crc32", &["data"]),
    ("zlib", "adler32", &["data"]),
    ("gzip", "compress", &["data", "level"]),
    ("gzip", "decompress", &["data"]),
    // ---- http (ticket #104): client verbs take an optional trailing headers ----
    // dict[str,str]; accessors are fixed-arity. The optional `headers` is the
    // last parameter (the former "n or n+1 args" overload).
    ("http", "get", &["url", "headers"]),
    ("http", "post", &["url", "body", "headers"]),
    ("http", "request", &["method", "url", "body", "headers"]),
    ("http", "status", &["resp"]),
    ("http", "body", &["resp"]),
    ("http", "header", &["resp", "name"]),
    ("http", "url", &["host", "port", "path"]),
    ("http", "read_request", &["conn"]),
    ("http", "method", &["req"]),
    ("http", "path", &["req"]),
    ("http", "req_header", &["req", "name"]),
    ("http", "req_headers", &["req"]),
    ("http", "req_body", &["req"]),
    ("http", "req_version", &["req"]),
    ("http", "respond", &["conn", "status", "body", "headers"]),
];

/// The declared positional parameter names of a seam method, or `None` when the
/// method has no registered signature (then the caller keeps the legacy
/// positional-only behavior). The `(module, method)` key is the seam registry
/// name the dispatch uses.
pub fn seam_method_params(module: &str, method: &str) -> Option<&'static [&'static str]> {
    SEAM_METHOD_SIGS
        .iter()
        .find(|(m, me, _)| *m == module && *me == method)
        .map(|(_, _, params)| *params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn lookup_hits_and_misses() {
        assert_eq!(
            seam_method_params("zlib", "compress"),
            Some(&["data", "level"][..])
        );
        assert_eq!(
            seam_method_params("http", "get"),
            Some(&["url", "headers"][..])
        );
        assert_eq!(seam_method_params("zlib", "no_such"), None);
        assert_eq!(seam_method_params("no_such", "compress"), None);
    }

    /// A `(module, method)` row is unique (a duplicate would make `find` ignore
    /// the later one silently).
    #[test]
    fn rows_are_unique() {
        let mut seen = BTreeSet::new();
        for (m, me, _) in SEAM_METHOD_SIGS {
            assert!(
                seen.insert((*m, *me)),
                "duplicate seam signature row: {m}.{me}"
            );
        }
    }

    /// Parameter names within one method are distinct (a repeat would make a
    /// keyword bind to the wrong slot / never match).
    #[test]
    fn params_distinct_within_a_method() {
        for (m, me, params) in SEAM_METHOD_SIGS {
            let mut seen = BTreeSet::new();
            for p in *params {
                assert!(seen.insert(*p), "duplicate parameter '{p}' in {m}.{me}");
            }
        }
    }
}
