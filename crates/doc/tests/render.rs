//! Renderer tests for `zap doc` (I#157/C2, issue #233).
//!
//! The Markdown output is pinned by a committed **golden** (`tests/golden/`),
//! asserting byte-stable rendering of a representative module — functions, a
//! struct with fields + methods, an enum, an interface, a constant, an
//! undocumented item, and a private `_`-item (omitted by default, marked under
//! `--private`). The HTML output is checked by structural invariants: every
//! item has an anchor, signatures and escaped docstrings are present, intra-doc
//! links resolve, and a docstring full of HTML metacharacters cannot inject
//! markup. Both renderers are asserted deterministic (two runs are identical).
//!
//! Regenerate the golden after an intended change with
//! `ZAP_DOC_BLESS=1 cargo test -p zapc_doc --test render`.

use std::path::PathBuf;
use zapc_doc::{DocModel, DocOptions};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Compare `actual` against `tests/golden/<name>`, or rewrite it when
/// `ZAP_DOC_BLESS` is set.
fn golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("ZAP_DOC_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read golden {}: {e} (run with ZAP_DOC_BLESS=1)",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "golden {name} mismatch — rerun with ZAP_DOC_BLESS=1 if intended"
    );
}

fn public_model() -> DocModel {
    DocModel::from_source(
        &fixture("shapes.zap"),
        Some("shapes"),
        &DocOptions::default(),
    )
}

fn private_model() -> DocModel {
    DocModel::from_source(
        &fixture("shapes.zap"),
        Some("shapes"),
        &DocOptions {
            include_private: true,
        },
    )
}

// ----------------------------------------------------------------------------
// Markdown
// ----------------------------------------------------------------------------

#[test]
fn markdown_matches_golden() {
    golden("shapes.md", &public_model().to_markdown());
}

#[test]
fn markdown_is_deterministic() {
    assert_eq!(public_model().to_markdown(), public_model().to_markdown());
}

#[test]
fn markdown_omits_private_by_default() {
    let md = public_model().to_markdown();
    assert!(!md.contains("_scale"), "private function leaked");
    assert!(!md.contains("_tag"), "private field leaked");
    assert!(!md.contains("(private)"), "no private marker should appear");
}

#[test]
fn markdown_marks_private_items_when_included() {
    let md = private_model().to_markdown();
    assert!(md.contains("_scale"), "private function should appear");
    assert!(md.contains("_tag"), "private field should appear");
    assert!(md.contains("*(private)*"), "private marker expected");
}

// ----------------------------------------------------------------------------
// HTML — structural invariants
// ----------------------------------------------------------------------------

#[test]
fn html_is_deterministic() {
    assert_eq!(public_model().to_html(), public_model().to_html());
}

#[test]
fn html_has_item_anchors_and_signatures() {
    let html = public_model().to_html();
    // Anchors for a top-level item and a nested method.
    assert!(html.contains("id=\"shapes-area\""));
    assert!(html.contains("id=\"shapes-point\""));
    assert!(html.contains("id=\"shapes-point-dist\""));
    // Signatures present and HTML-escaped (`->` becomes `-&gt;`).
    assert!(html.contains("def area(float64 r) -&gt; float64"));
    assert!(html.contains("@memoize"));
    // Table of contents links to the anchors.
    assert!(html.contains("<a href=\"#shapes-area\">area</a>"));
}

#[test]
fn html_resolves_intra_doc_links() {
    let html = public_model().to_html();
    // `Point` in `dist`'s signature links to the struct's anchor.
    assert!(html.contains("<a href=\"#shapes-point\">Point</a> other"));
}

#[test]
fn html_renders_docstring_markdown() {
    let html = public_model().to_html();
    // Google-style section labels are emphasised; inline code is rendered.
    assert!(html.contains("<strong>Args:</strong>"));
    assert!(html.contains("<code>x</code>")); // `clamp`'s ``x`` double-backtick
}

#[test]
fn html_escapes_docstring_injection() {
    let src = "\
def danger() -> int:
    \"\"\"Pwn <script>alert('xss')</script> & \\\"quotes\\\".\"\"\"
    return 0
";
    let model = DocModel::from_source(src, Some("m"), &DocOptions::default());
    let html = model.to_html();
    assert!(
        !html.contains("<script>alert"),
        "docstring HTML must not pass through unescaped"
    );
    assert!(
        html.contains("&lt;script&gt;alert"),
        "expected escaped form"
    );
    assert!(html.contains("&amp;"));
}

#[test]
fn html_marks_private_when_included() {
    let html = private_model().to_html();
    assert!(html.contains("id=\"shapes-point-_tag\"") || html.contains("_tag"));
    assert!(html.contains("<span class=\"badge\">private</span>"));
}
