//! Doc-model extraction tests (I#232). Each case extracts from a representative
//! `.zap` source and asserts the resulting model: names, qualified paths,
//! signatures (hover-consistent), docstrings (summary + Google-style sections),
//! nesting (methods/fields/variants under their owner), and visibility (D-0028).
//! Sources are inline strings written in canonical style.

use zapc_doc::{DocItemBody, DocModel, DocOptions, Visibility};

/// Extract the public model for `src` under module name `m`.
fn public(src: &str, m: &str) -> DocModel {
    DocModel::from_source(src, Some(m), &DocOptions::default())
}

/// The top-level item named `name`, or panic.
fn item<'a>(model: &'a DocModel, name: &str) -> &'a zapc_doc::DocItem {
    model
        .items
        .iter()
        .find(|i| i.name == name)
        .unwrap_or_else(|| panic!("no top-level item `{name}`"))
}

const SHAPES: &str = r#""""Shape utilities.

A tiny module used to exercise zap doc.
"""

const float64 PI = 3.14159


def area(float64 r) -> float64:
    """Area of a circle.

    Args:
        r: the radius

    Returns:
        the area
    """
    return PI * r * r


def _scale(float64 r, float64 k) -> float64:
    return r * k


struct Point:
    float64 x
    float64 _secret

    def dist(Point other) -> float64:
        """Euclidean distance to another point."""
        auto dx = self.x - other.x
        return dx


enum Shape:
    Circle(float64 r)
    Square(float64 side)


interface Drawable:
    def draw(self) -> str


def max_of[T: Ordered](T a, T b) -> T:
    """Return the larger of two values."""
    return a if a > b else b
"#;

#[test]
fn module_docstring_summary_and_name() {
    let m = public(SHAPES, "shapes");
    assert_eq!(m.module.name.as_deref(), Some("shapes"));
    let doc = m.module.doc.as_ref().expect("module docstring");
    assert_eq!(doc.summary, "Shape utilities.");
    assert!(doc.raw.starts_with("Shape utilities."));
}

#[test]
fn items_are_in_declaration_order() {
    let m = public(SHAPES, "shapes");
    let names: Vec<&str> = m.items.iter().map(|i| i.name.as_str()).collect();
    // `_scale` is private and omitted; the docstring statement is not an item.
    assert_eq!(
        names,
        vec!["PI", "area", "Point", "Shape", "Drawable", "max_of"]
    );
}

#[test]
fn const_has_type_and_signature() {
    let m = public(SHAPES, "shapes");
    let pi = item(&m, "PI");
    assert_eq!(pi.qualified_name, "shapes.PI");
    assert_eq!(pi.visibility, Visibility::Public);
    let DocItemBody::Const(c) = &pi.body else {
        panic!("const");
    };
    assert_eq!(c.signature, "const float64 PI");
    assert_eq!(c.ty.as_deref(), Some("float64"));
}

#[test]
fn function_signature_matches_hover_and_splits_docstring() {
    let m = public(SHAPES, "shapes");
    let area = item(&m, "area");
    let DocItemBody::Function(f) = &area.body else {
        panic!("function");
    };
    assert_eq!(f.signature, "def area(float64 r) -> float64");
    assert_eq!(f.params.len(), 1);
    assert_eq!(f.params[0].name, "r");
    assert_eq!(f.params[0].ty.as_deref(), Some("float64"));
    assert_eq!(f.ret.as_deref(), Some("float64"));

    let doc = area.doc.as_ref().expect("docstring");
    assert_eq!(doc.summary, "Area of a circle.");
    let labels: Vec<&str> = doc.sections.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, vec!["Args", "Returns"]);
    assert_eq!(doc.sections[0].body, "r: the radius");
    assert_eq!(doc.sections[1].body, "the area");
}

#[test]
fn undocumented_item_has_no_docstring() {
    let m = public(SHAPES, "shapes");
    // `max_of` is documented; `Point` (a type) has no docstring slot today.
    assert!(item(&m, "Point").doc.is_none());
    assert!(item(&m, "max_of").doc.is_some());
}

#[test]
fn generics_carry_bounds_structurally() {
    let m = public(SHAPES, "shapes");
    let DocItemBody::Function(f) = &item(&m, "max_of").body else {
        panic!("function");
    };
    // Signature omits the bound (hover-consistent)...
    assert_eq!(f.signature, "def max_of[T](T a, T b) -> T");
    // ...but the structured generic carries it for #233.
    assert_eq!(f.generics.len(), 1);
    assert_eq!(f.generics[0].name, "T");
    assert_eq!(f.generics[0].bound.as_deref(), Some("Ordered"));
}

#[test]
fn struct_nests_fields_and_methods_with_visibility() {
    let m = public(SHAPES, "shapes");
    let point = item(&m, "Point");
    let DocItemBody::Struct(s) = &point.body else {
        panic!("struct");
    };
    assert_eq!(s.signature, "struct Point");
    assert!(!s.is_value);
    // `_secret` is private and filtered from the public model.
    let field_names: Vec<&str> = s.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(field_names, vec!["x"]);
    assert_eq!(s.fields[0].ty, "float64");

    assert_eq!(s.methods.len(), 1);
    let dist = &s.methods[0];
    assert_eq!(dist.qualified_name, "shapes.Point.dist");
    assert!(matches!(dist.body, DocItemBody::Method(_)));
    let DocItemBody::Method(df) = &dist.body else {
        panic!("method");
    };
    assert_eq!(df.signature, "def dist(Point other) -> float64");
    assert_eq!(
        dist.doc.as_ref().map(|d| d.summary.as_str()),
        Some("Euclidean distance to another point.")
    );
}

#[test]
fn enum_variants_and_payloads() {
    let m = public(SHAPES, "shapes");
    let DocItemBody::Enum(e) = &item(&m, "Shape").body else {
        panic!("enum");
    };
    assert_eq!(e.signature, "enum Shape");
    let names: Vec<&str> = e.variants.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, vec!["Circle", "Square"]);
    assert_eq!(e.variants[0].fields.len(), 1);
    assert_eq!(e.variants[0].fields[0].name, "r");
    assert_eq!(e.variants[0].fields[0].ty, "float64");
}

#[test]
fn interface_methods_have_signatures() {
    let m = public(SHAPES, "shapes");
    let DocItemBody::Interface(i) = &item(&m, "Drawable").body else {
        panic!("interface");
    };
    assert_eq!(i.signature, "interface Drawable");
    assert_eq!(i.methods.len(), 1);
    let DocItemBody::Method(df) = &i.methods[0].body else {
        panic!("method sig");
    };
    assert_eq!(df.signature, "def draw(self) -> str");
    assert_eq!(i.methods[0].qualified_name, "shapes.Drawable.draw");
}

#[test]
fn private_items_included_only_with_the_flag() {
    // Default: private names are absent at every level.
    let pub_model = public(SHAPES, "shapes");
    assert!(pub_model.items.iter().all(|i| i.name != "_scale"));

    // With the flag: `_scale` and the private field reappear, tagged private.
    let all = DocModel::from_source(
        SHAPES,
        Some("shapes"),
        &DocOptions {
            include_private: true,
        },
    );
    let scale = item(&all, "_scale");
    assert_eq!(scale.visibility, Visibility::Private);
    let DocItemBody::Struct(s) = &item(&all, "Point").body else {
        panic!("struct");
    };
    let secret = s
        .fields
        .iter()
        .find(|f| f.name == "_secret")
        .expect("private field present with flag");
    assert_eq!(secret.visibility, Visibility::Private);
}

#[test]
fn value_struct_and_bitfields() {
    let src = "value struct Flags:\n    int64 kind : 3\n    bool on\n";
    let m = public(src, "f");
    let DocItemBody::Struct(s) = &item(&m, "Flags").body else {
        panic!("struct");
    };
    assert!(s.is_value);
    assert_eq!(s.signature, "value struct Flags");
    assert_eq!(s.fields[0].bits, Some(3));
    assert_eq!(s.fields[1].bits, None);
}

#[test]
fn decorators_are_captured() {
    let src = "@memoize\ndef fib(int64 n) -> int64:\n    return n\n";
    let m = public(src, "d");
    let DocItemBody::Function(f) = &item(&m, "fib").body else {
        panic!("function");
    };
    assert_eq!(f.decorators, vec!["@memoize".to_string()]);
}

#[test]
fn json_is_stable_and_well_formed() {
    let m = public(SHAPES, "shapes");
    let json = m.to_json_string();
    // Deterministic, pretty-printed, insertion-ordered.
    assert!(json.starts_with("{\n  \"module\": {"));
    assert!(json.contains("\"qualified_name\": \"shapes.Point.dist\""));
    assert!(json.contains("\"visibility\": \"public\""));
    assert!(json.contains("\"kind\": \"interface\""));
    // Private names never leak into the default model's JSON.
    assert!(!json.contains("_scale"));
    assert!(!json.contains("_secret"));
    // Idempotent.
    assert_eq!(json, public(SHAPES, "shapes").to_json_string());
}

#[test]
fn anonymous_module_qualifies_without_a_prefix() {
    let m = DocModel::from_source(
        "def f() -> int64:\n    return 1\n",
        None,
        &DocOptions::default(),
    );
    assert_eq!(m.items[0].qualified_name, "f");
    assert!(m.module.name.is_none());
}

#[test]
fn extraction_is_robust_to_type_errors() {
    // A body that fails to type-check must not stop doc extraction (the outline
    // path is a pure AST walk, like document_symbols).
    let src =
        "def broken(int64 x) -> int64:\n    \"\"\"Still documented.\"\"\"\n    return nope()\n";
    let m = public(src, "e");
    let f = item(&m, "broken");
    assert_eq!(
        f.doc.as_ref().map(|d| d.summary.as_str()),
        Some("Still documented.")
    );
    let DocItemBody::Function(df) = &f.body else {
        panic!("function");
    };
    assert_eq!(df.signature, "def broken(int64 x) -> int64");
}
