//! Language understanding (D-0055 surface 1).
//!
//! Serves the Zap language reference — the spec, grammar, design decisions,
//! topic design docs, and a curated set of canonical examples — so an LLM can
//! produce correct Zap even with little or no Zap in its training corpus. This
//! directly attacks the "new language, no corpus" problem.
//!
//! The reference ships *with the toolchain*: the documents are embedded into
//! the binary with [`include_str!`], so `zap mcp` serves them identically no
//! matter which directory the agent runs it from (the user's own project will
//! not contain `SPEC.md`). They are exposed two ways: as MCP **resources**
//! (`zap://reference/<name>`, `zap://example/<name>`) for resource-aware
//! clients, and through the `zap_reference` **tool** for tool-only clients.

use crate::json::Json;

/// A single reference document.
pub struct Doc {
    /// Short stable identifier, used in the resource URI and the tool's `name`.
    pub name: &'static str,
    /// Human-readable title.
    pub title: &'static str,
    /// `reference` (prose/grammar) or `example` (runnable Zap).
    pub category: &'static str,
    pub mime: &'static str,
    pub body: &'static str,
}

impl Doc {
    /// The `zap://…` resource URI for this document.
    pub fn uri(&self) -> String {
        format!("zap://{}/{}", self.category, self.name)
    }
}

// Paths are relative to this file: crates/driver/src/mcp/ → repo root is four
// levels up.
macro_rules! doc {
    ($name:literal, $title:literal, $cat:literal, $mime:literal, $path:literal) => {
        Doc {
            name: $name,
            title: $title,
            category: $cat,
            mime: $mime,
            body: include_str!(concat!("../../../../", $path)),
        }
    };
}

/// The full reference catalog. Order is the order of [`DOCS`] listing.
pub static DOCS: &[Doc] = &[
    doc!(
        "spec",
        "Zap Language Specification (authoritative)",
        "reference",
        "text/markdown",
        "SPEC.md"
    ),
    doc!(
        "grammar",
        "Zap grammar (EBNF)",
        "reference",
        "text/plain",
        "docs/GRAMMAR.ebnf"
    ),
    doc!(
        "decisions",
        "Locked design decisions (D-0001..)",
        "reference",
        "text/markdown",
        "DECISIONS.md"
    ),
    doc!(
        "design-regex",
        "Design: Perl-style regex (D-0010)",
        "reference",
        "text/markdown",
        "design/0001-perl-regex.md"
    ),
    doc!(
        "design-arrays",
        "Design: numeric arrays & SIMD",
        "reference",
        "text/markdown",
        "design/0002-arrays.md"
    ),
    doc!(
        "design-python-interop",
        "Design: Python interop",
        "reference",
        "text/markdown",
        "design/0003-python-interop.md"
    ),
    doc!(
        "design-decorators",
        "Design: decorators & annotations",
        "reference",
        "text/markdown",
        "design/0004-decorators.md"
    ),
    doc!(
        "design-concurrency",
        "Design: concurrency (coroutines/channels/scope)",
        "reference",
        "text/markdown",
        "design/0005-concurrency.md"
    ),
    doc!(
        "design-types",
        "Design: types & generics",
        "reference",
        "text/markdown",
        "design/0006-types-and-generics.md"
    ),
    doc!(
        "design-strings",
        "Design: strings",
        "reference",
        "text/markdown",
        "design/0007-strings.md"
    ),
    doc!(
        "design-gc",
        "Design: precise GC",
        "reference",
        "text/markdown",
        "design/0008-precise-gc.md"
    ),
    // Canonical examples — small, idiomatic programs that parse and type-check.
    doc!(
        "ex-fib",
        "Example: recursion (fib)",
        "example",
        "text/x-zap",
        "tests/golden/fib.zap"
    ),
    doc!(
        "ex-loop-sum",
        "Example: loops",
        "example",
        "text/x-zap",
        "tests/golden/loop_sum.zap"
    ),
    doc!(
        "ex-point",
        "Example: struct + methods",
        "example",
        "text/x-zap",
        "tests/golden/point.zap"
    ),
    doc!(
        "ex-dict",
        "Example: dict",
        "example",
        "text/x-zap",
        "tests/golden/dict.zap"
    ),
    doc!(
        "ex-list",
        "Example: list",
        "example",
        "text/x-zap",
        "tests/golden/list_int.zap"
    ),
    doc!(
        "ex-set",
        "Example: set",
        "example",
        "text/x-zap",
        "tests/golden/set_int.zap"
    ),
    doc!(
        "ex-heap",
        "Example: heap[T] (D-0057)",
        "example",
        "text/x-zap",
        "tests/golden/heap.zap"
    ),
    doc!(
        "ex-enum",
        "Example: enum / sum type",
        "example",
        "text/x-zap",
        "tests/golden/enum_shape.zap"
    ),
    doc!(
        "ex-tuple",
        "Example: tuples",
        "example",
        "text/x-zap",
        "tests/golden/tuple_basic.zap"
    ),
    doc!(
        "ex-decorators",
        "Example: decorators",
        "example",
        "text/x-zap",
        "tests/golden/decorators.zap"
    ),
    doc!(
        "ex-coroutines",
        "Example: coroutines & channels",
        "example",
        "text/x-zap",
        "tests/golden/coroutines.zap"
    ),
    doc!(
        "ex-scope",
        "Example: structured concurrency (scope)",
        "example",
        "text/x-zap",
        "tests/golden/scope_group.zap"
    ),
    doc!(
        "ex-regex",
        "Example: Perl-style regex",
        "example",
        "text/x-zap",
        "tests/golden/regex.zap"
    ),
    doc!(
        "ex-exceptions",
        "Example: exceptions",
        "example",
        "text/x-zap",
        "tests/golden/exceptions_propagate.zap"
    ),
    doc!(
        "ex-value-struct",
        "Example: value struct (D-0051)",
        "example",
        "text/x-zap",
        "tests/golden/value_struct.zap"
    ),
    doc!(
        "ex-python-interop",
        "Example: Python interop",
        "example",
        "text/x-zap",
        "tests/golden/python_interop.zap"
    ),
];

/// Find a document by name.
pub fn find(name: &str) -> Option<&'static Doc> {
    DOCS.iter().find(|d| d.name == name)
}

/// Find a document by its `zap://…` resource URI.
pub fn find_by_uri(uri: &str) -> Option<&'static Doc> {
    DOCS.iter().find(|d| d.uri() == uri)
}

/// The catalog as a JSON array of `{name, title, category, uri}` (no bodies) —
/// what `zap_reference` returns when called with no `name`.
pub fn catalog_json() -> Json {
    Json::Arr(
        DOCS.iter()
            .map(|d| {
                Json::obj(vec![
                    ("name", Json::str(d.name)),
                    ("title", Json::str(d.title)),
                    ("category", Json::str(d.category)),
                    ("uri", Json::str(d.uri())),
                ])
            })
            .collect(),
    )
}

/// The MCP `resources/list` array of `{uri, name, description, mimeType}`.
pub fn resources_json() -> Json {
    Json::Arr(
        DOCS.iter()
            .map(|d| {
                Json::obj(vec![
                    ("uri", Json::str(d.uri())),
                    ("name", Json::str(d.name)),
                    ("description", Json::str(d.title)),
                    ("mimeType", Json::str(d.mime)),
                ])
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_non_empty_and_has_grammar_and_examples() {
        assert!(DOCS.len() >= 10);
        assert!(find("grammar").is_some());
        assert!(find("spec").is_some());
        assert!(DOCS.iter().any(|d| d.category == "example"));
    }

    #[test]
    fn grammar_body_is_the_ebnf() {
        let g = find("grammar").unwrap();
        assert!(g.body.contains("Zap grammar (EBNF)"));
    }

    #[test]
    fn example_bodies_type_check() {
        // Every embedded canonical example must be valid Zap (parse + sema),
        // or we would be teaching agents broken code.
        for d in DOCS.iter().filter(|d| d.category == "example") {
            let r = super::super::check::check_source(d.body);
            assert!(
                r.ok,
                "canonical example `{}` does not check: {:?}",
                d.name, r.diagnostics
            );
        }
    }

    #[test]
    fn uris_are_unique_and_resolvable() {
        for d in DOCS {
            assert_eq!(find_by_uri(&d.uri()).map(|x| x.name), Some(d.name));
        }
    }
}
