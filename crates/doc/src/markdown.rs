//! Markdown renderer for `zap doc` (I#157/C2, issue #233).
//!
//! Renders a [`DocModel`] to a single, byte-stable Markdown page — one module
//! per file, matching the extractor's one-module-per-invocation model. The
//! layout is:
//!
//! 1. `# <module>` and the module docstring prose.
//! 2. A **Contents** index, items grouped by kind, each linking to its anchor.
//! 3. One `##` section per kind (Functions, Types, Enums, Interfaces,
//!    Constants). A type shows its fields (a table) and its methods nested
//!    beneath it; an enum shows its variants.
//!
//! Each item carries an explicit `<a id="…">` anchor (so intra-page links
//! resolve the same in every viewer), its signature in a fenced ` ```zap `
//! block, and its docstring rendered as prose. Output is deterministic:
//! declaration order is preserved and nothing host- or time-dependent is
//! emitted, so the result is golden-testable and safe to commit to a repo.

use crate::render::{self, Anchors};
use crate::{DocField, DocItem, DocItemBody, DocModel, DocVariant, Generic, Visibility};

/// The five item groups, in the fixed order they appear in the page.
const GROUPS: [&str; 5] = ["Functions", "Types", "Enums", "Interfaces", "Constants"];

pub(crate) fn render(model: &DocModel) -> String {
    let anchors = Anchors::build(model);
    let mut out = String::new();

    let title = model.module.name.as_deref().unwrap_or("Module");
    out.push_str(&format!("# {title}\n\n"));
    if let Some(desc) = render::description(&model.module.doc) {
        out.push_str(&desc);
        out.push_str("\n\n");
    }

    let groups = group(model);

    out.push_str("## Contents\n\n");
    for (label, items) in &groups {
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("- **{label}**\n"));
        for it in items {
            out.push_str(&format!(
                "  - [{}](#{}){}\n",
                it.name,
                anchors.of(&it.qualified_name),
                private_tag(it.visibility),
            ));
        }
    }
    out.push('\n');

    for (label, items) in &groups {
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("## {label}\n\n"));
        for it in items {
            item(&mut out, it, &anchors, 3);
        }
    }

    normalize(out)
}

/// Classify the top-level items into the five ordered groups.
fn group(model: &DocModel) -> Vec<(&'static str, Vec<&DocItem>)> {
    let mut groups: Vec<(&'static str, Vec<&DocItem>)> =
        GROUPS.iter().map(|g| (*g, Vec::new())).collect();
    for it in &model.items {
        let idx = match it.body {
            DocItemBody::Function(_) | DocItemBody::Method(_) => 0,
            DocItemBody::Struct(_) => 1,
            DocItemBody::Enum(_) => 2,
            DocItemBody::Interface(_) => 3,
            DocItemBody::Const(_) => 4,
        };
        groups[idx].1.push(it);
    }
    groups
}

/// Render one declaration at the given heading level, recursing into a type's
/// methods one level deeper.
fn item(out: &mut String, it: &DocItem, anchors: &Anchors, level: usize) {
    out.push_str(&format!(
        "<a id=\"{}\"></a>\n\n",
        anchors.of(&it.qualified_name)
    ));
    out.push_str(&format!(
        "{} {}{}\n\n",
        "#".repeat(level),
        it.name,
        private_tag(it.visibility),
    ));

    match &it.body {
        DocItemBody::Function(f) | DocItemBody::Method(f) => {
            out.push_str(&signature_block(&f.decorators, &f.signature));
            push_bounds(out, &f.generics);
            push_description(out, it);
        }
        DocItemBody::Struct(s) => {
            out.push_str(&signature_block(&s.decorators, &s.signature));
            push_bounds(out, &s.generics);
            push_description(out, it);
            push_fields(out, &s.fields);
            push_methods(out, &s.methods, anchors, level + 1);
        }
        DocItemBody::Enum(e) => {
            out.push_str(&signature_block(&e.decorators, &e.signature));
            push_bounds(out, &e.generics);
            push_description(out, it);
            push_variants(out, &e.variants);
            push_methods(out, &e.methods, anchors, level + 1);
        }
        DocItemBody::Interface(i) => {
            out.push_str(&signature_block(&[], &i.signature));
            push_bounds(out, &i.generics);
            push_description(out, it);
            push_methods(out, &i.methods, anchors, level + 1);
        }
        DocItemBody::Const(c) => {
            out.push_str(&signature_block(&[], &c.signature));
            push_description(out, it);
        }
    }
}

/// A fenced ` ```zap ` block holding any decorator lines and the signature.
fn signature_block(decorators: &[String], signature: &str) -> String {
    let mut s = String::from("```zap\n");
    for d in decorators {
        s.push_str(d);
        s.push('\n');
    }
    s.push_str(signature);
    s.push_str("\n```\n\n");
    s
}

/// A `*where T: Bound, …*` line for the bounded generics, if any.
fn push_bounds(out: &mut String, generics: &[Generic]) {
    let bounds: Vec<String> = generics
        .iter()
        .filter_map(|g| g.bound.as_ref().map(|b| format!("{}: {b}", g.name)))
        .collect();
    if !bounds.is_empty() {
        out.push_str(&format!("*where {}*\n\n", bounds.join(", ")));
    }
}

fn push_description(out: &mut String, it: &DocItem) {
    if let Some(desc) = render::description(&it.doc) {
        out.push_str(&desc);
        out.push_str("\n\n");
    }
}

fn push_fields(out: &mut String, fields: &[DocField]) {
    if fields.is_empty() {
        return;
    }
    let has_bits = fields.iter().any(|f| f.bits.is_some());
    out.push_str("**Fields**\n\n");
    if has_bits {
        out.push_str("| Name | Type | Default | Bits |\n");
        out.push_str("|------|------|---------|------|\n");
    } else {
        out.push_str("| Name | Type | Default |\n");
        out.push_str("|------|------|---------|\n");
    }
    for f in fields {
        let name = format!("{}{}", f.name, private_tag(f.visibility));
        let default = f.default.as_deref().unwrap_or("");
        let default = if default.is_empty() {
            String::new()
        } else {
            format!("`{}`", cell(default))
        };
        if has_bits {
            let bits = f.bits.map(|b| b.to_string()).unwrap_or_default();
            out.push_str(&format!(
                "| {} | `{}` | {} | {} |\n",
                name,
                cell(&f.ty),
                default,
                bits
            ));
        } else {
            out.push_str(&format!("| {} | `{}` | {} |\n", name, cell(&f.ty), default));
        }
    }
    out.push('\n');
}

fn push_variants(out: &mut String, variants: &[DocVariant]) {
    if variants.is_empty() {
        return;
    }
    out.push_str("**Variants**\n\n");
    for v in variants {
        out.push_str(&format!(
            "- `{}`{}\n",
            variant_signature(v),
            private_tag(v.visibility)
        ));
    }
    out.push('\n');
}

/// `Circle(float64 r)`, `GREEN = 10`, or a bare `Red`.
fn variant_signature(v: &DocVariant) -> String {
    if !v.fields.is_empty() {
        let fs: Vec<String> = v
            .fields
            .iter()
            .map(|f| format!("{} {}", f.ty, f.name))
            .collect();
        format!("{}({})", v.name, fs.join(", "))
    } else if let Some(val) = &v.value {
        format!("{} = {}", v.name, val)
    } else {
        v.name.clone()
    }
}

fn push_methods(out: &mut String, methods: &[DocItem], anchors: &Anchors, level: usize) {
    if methods.is_empty() {
        return;
    }
    out.push_str("**Methods**\n\n");
    for m in methods {
        item(out, m, anchors, level);
    }
}

/// ` *(private)*` for a private declaration, else empty — the D-0028 marker
/// shown only when `--private` has pulled private items into the model.
fn private_tag(v: Visibility) -> &'static str {
    match v {
        Visibility::Private => " *(private)*",
        Visibility::Public => "",
    }
}

/// Escape the characters that would break a Markdown table cell.
fn cell(s: &str) -> String {
    s.replace('\\', "\\\\").replace('|', "\\|")
}

/// Collapse any run of three or more newlines to a blank-line separator and
/// guarantee exactly one trailing newline, so spacing is deterministic.
fn normalize(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newlines = 0;
    for c in s.chars() {
        if c == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push('\n');
            }
        } else {
            newlines = 0;
            out.push(c);
        }
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}
