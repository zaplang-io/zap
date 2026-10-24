//! HTML renderer for `zap doc` (I#157/C2, issue #233).
//!
//! Renders a [`DocModel`] to a single, self-contained `index.html`: plain
//! semantic HTML with a small **inline** stylesheet — no JavaScript, no
//! external assets — matching the toolchain's minimal-deps house style. The
//! page has a header (module name + summary), a `Contents` sidebar listing
//! every item with `#anchor` links, and one `<section>` per item carrying its
//! signature and rendered docstring.
//!
//! **Intra-doc links.** A type reference in a signature or a field/variant type
//! that names a struct/enum/interface declared in this module links to that
//! type's anchor (see [`crate::render::linkify_signature`]).
//!
//! **Escaping.** Every identifier, signature, and docstring is HTML-escaped
//! (and docstring Markdown is rendered through the safe subset in
//! [`crate::render::md_to_html`]), so a docstring containing HTML
//! metacharacters cannot inject markup.
//!
//! Output is deterministic — declaration order is preserved and nothing
//! host- or time-dependent is emitted — so it is safe to commit and diff.

use crate::render::{self, escape_html, linkify_signature, Anchors};
use crate::{DocField, DocItem, DocItemBody, DocModel, DocVariant, Generic, Visibility};

const GROUPS: [&str; 5] = ["Functions", "Types", "Enums", "Interfaces", "Constants"];

/// The inline stylesheet — compact, readable, and light/dark aware.
const STYLE: &str = "\
:root { color-scheme: light dark; --fg: #1a1a1a; --bg: #ffffff; --muted: #666; \
--border: #e2e2e2; --code-bg: #f4f4f6; --link: #0b5fff; --accent: #7a3cff; }\n\
@media (prefers-color-scheme: dark) { :root { --fg: #e6e6e6; --bg: #16181d; \
--muted: #9aa0a6; --border: #2c2f36; --code-bg: #22262e; --link: #6ea8ff; \
--accent: #b794ff; } }\n\
* { box-sizing: border-box; }\n\
body { margin: 0; color: var(--fg); background: var(--bg); \
font: 16px/1.6 -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; \
display: grid; grid-template-columns: minmax(0, 1fr); }\n\
@media (min-width: 900px) { body { grid-template-columns: 16rem minmax(0, 1fr); } \
nav.toc { position: sticky; top: 0; align-self: start; height: 100vh; overflow: auto; } }\n\
header, main { grid-column: 2; padding: 1.5rem 2rem; max-width: 60rem; }\n\
@media (max-width: 899px) { header, main { grid-column: 1; padding: 1rem; } }\n\
header { border-bottom: 1px solid var(--border); }\n\
h1 { margin: 0 0 .25rem; font-size: 1.9rem; }\n\
header .summary { color: var(--muted); margin: 0; }\n\
nav.toc { grid-column: 1; grid-row: 1 / span 2; padding: 1.5rem 1rem; \
border-right: 1px solid var(--border); font-size: .9rem; }\n\
nav.toc h2 { font-size: .8rem; text-transform: uppercase; letter-spacing: .05em; \
color: var(--muted); }\n\
nav.toc ul { list-style: none; padding-left: .75rem; margin: .3rem 0; }\n\
nav.toc > ul { padding-left: 0; }\n\
nav.toc a { color: var(--fg); text-decoration: none; }\n\
nav.toc a:hover { color: var(--link); text-decoration: underline; }\n\
nav.toc .group-name { font-weight: 600; margin-top: .6rem; display: block; }\n\
section.group > h2 { border-bottom: 1px solid var(--border); padding-bottom: .3rem; \
margin-top: 2.2rem; }\n\
section.item { margin: 1.5rem 0; }\n\
section.item h3, section.item h4 { margin: .2rem 0; font-family: ui-monospace, \
SFMono-Regular, Menlo, Consolas, monospace; }\n\
pre.sig { background: var(--code-bg); border: 1px solid var(--border); \
border-radius: 6px; padding: .6rem .8rem; overflow: auto; }\n\
pre.sig, code { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; \
font-size: .9em; }\n\
pre.sig a { color: var(--accent); text-decoration: none; }\n\
pre.sig a:hover { text-decoration: underline; }\n\
.doc :not(pre) > code { background: var(--code-bg); padding: .1em .35em; \
border-radius: 4px; }\n\
.doc pre { background: var(--code-bg); border: 1px solid var(--border); \
border-radius: 6px; padding: .6rem .8rem; overflow: auto; }\n\
.where { color: var(--muted); font-size: .9rem; margin: .3rem 0; }\n\
.badge { font-size: .65rem; text-transform: uppercase; letter-spacing: .04em; \
color: var(--muted); border: 1px solid var(--border); border-radius: 4px; \
padding: .05rem .3rem; vertical-align: middle; }\n\
.members { border-left: 2px solid var(--border); padding-left: 1rem; margin-left: .2rem; }\n\
table.fields { border-collapse: collapse; width: 100%; margin: .5rem 0; }\n\
table.fields th, table.fields td { text-align: left; padding: .35rem .6rem; \
border-bottom: 1px solid var(--border); }\n\
table.fields th { color: var(--muted); font-weight: 600; font-size: .85rem; }\n\
ul.variants { padding-left: 1.2rem; }\n\
footer { grid-column: 2; padding: 1.5rem 2rem; color: var(--muted); font-size: .85rem; \
border-top: 1px solid var(--border); }\n\
@media (max-width: 899px) { footer { grid-column: 1; } }\n";

pub(crate) fn render(model: &DocModel) -> String {
    let anchors = Anchors::build(model);
    let title = model.module.name.as_deref().unwrap_or("Module");
    let mut out = String::new();

    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    out.push_str("<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str(&format!(
        "<title>{} — zap doc</title>\n",
        escape_html(title)
    ));
    out.push_str("<style>\n");
    out.push_str(STYLE);
    out.push_str("</style>\n</head>\n<body>\n");

    let groups = group(model);

    // Sidebar / table of contents.
    out.push_str("<nav class=\"toc\">\n<h2>Contents</h2>\n<ul>\n");
    for (label, items) in &groups {
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "<li><span class=\"group-name\">{label}</span>\n<ul>\n"
        ));
        for it in items {
            out.push_str(&format!(
                "<li><a href=\"#{}\">{}</a></li>\n",
                anchors.of(&it.qualified_name),
                escape_html(&it.name),
            ));
        }
        out.push_str("</ul>\n</li>\n");
    }
    out.push_str("</ul>\n</nav>\n");

    // Header.
    out.push_str("<header>\n");
    out.push_str(&format!("<h1>{}</h1>\n", escape_html(title)));
    if let Some(d) = &model.module.doc {
        if !d.summary.is_empty() {
            out.push_str(&format!(
                "<p class=\"summary\">{}</p>\n",
                escape_html(&d.summary)
            ));
        }
    }
    out.push_str("</header>\n");

    // Body: module description, then one group per kind.
    out.push_str("<main>\n");
    if let Some(desc) = render::description(&model.module.doc) {
        out.push_str("<div class=\"doc\">\n");
        out.push_str(&render::md_to_html(&desc));
        out.push_str("</div>\n");
    }
    for (label, items) in &groups {
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("<section class=\"group\">\n<h2>{label}</h2>\n"));
        for it in items {
            item(&mut out, it, &anchors, 3);
        }
        out.push_str("</section>\n");
    }
    out.push_str("</main>\n");

    out.push_str("<footer>Generated by <code>zap doc</code>.</footer>\n");
    out.push_str("</body>\n</html>\n");
    out
}

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

fn item(out: &mut String, it: &DocItem, anchors: &Anchors, level: usize) {
    let h = format!("h{}", level.min(6));
    out.push_str(&format!(
        "<section class=\"item\" id=\"{}\">\n",
        anchors.of(&it.qualified_name)
    ));
    out.push_str(&format!(
        "<{h}>{}{}</{h}>\n",
        escape_html(&it.name),
        badge(it.visibility),
    ));

    match &it.body {
        DocItemBody::Function(f) | DocItemBody::Method(f) => {
            push_signature(out, &f.decorators, &f.signature, Some(anchors));
            push_bounds(out, &f.generics);
            push_doc(out, it);
        }
        DocItemBody::Struct(s) => {
            // A type header names only the type itself and its generic params,
            // so there is nothing to link — skip linkification to avoid a
            // self-link.
            push_signature(out, &s.decorators, &s.signature, None);
            push_bounds(out, &s.generics);
            push_doc(out, it);
            push_fields(out, &s.fields, anchors);
            push_members(out, &s.methods, anchors, level + 1);
        }
        DocItemBody::Enum(e) => {
            push_signature(out, &e.decorators, &e.signature, None);
            push_bounds(out, &e.generics);
            push_doc(out, it);
            push_variants(out, &e.variants, anchors);
            push_members(out, &e.methods, anchors, level + 1);
        }
        DocItemBody::Interface(i) => {
            push_signature(out, &[], &i.signature, None);
            push_bounds(out, &i.generics);
            push_doc(out, it);
            push_members(out, &i.methods, anchors, level + 1);
        }
        DocItemBody::Const(c) => {
            push_signature(out, &[], &c.signature, Some(anchors));
            push_doc(out, it);
        }
    }
    out.push_str("</section>\n");
}

/// A `<pre>` holding decorator lines (escaped) and the signature. With
/// `anchors`, type references in the signature are linked to their declarations;
/// pass `None` for a type header (which references no other type).
fn push_signature(
    out: &mut String,
    decorators: &[String],
    signature: &str,
    anchors: Option<&Anchors>,
) {
    out.push_str("<pre class=\"sig\"><code>");
    for d in decorators {
        out.push_str(&escape_html(d));
        out.push('\n');
    }
    match anchors {
        Some(a) => out.push_str(&linkify_signature(signature, a)),
        None => out.push_str(&escape_html(signature)),
    }
    out.push_str("</code></pre>\n");
}

fn push_bounds(out: &mut String, generics: &[Generic]) {
    let bounds: Vec<String> = generics
        .iter()
        .filter_map(|g| {
            g.bound
                .as_ref()
                .map(|b| format!("{}: {}", escape_html(&g.name), escape_html(b)))
        })
        .collect();
    if !bounds.is_empty() {
        out.push_str(&format!(
            "<p class=\"where\">where {}</p>\n",
            bounds.join(", ")
        ));
    }
}

fn push_doc(out: &mut String, it: &DocItem) {
    if let Some(desc) = render::description(&it.doc) {
        out.push_str("<div class=\"doc\">\n");
        out.push_str(&render::md_to_html(&desc));
        out.push_str("</div>\n");
    }
}

fn push_fields(out: &mut String, fields: &[DocField], anchors: &Anchors) {
    if fields.is_empty() {
        return;
    }
    let has_bits = fields.iter().any(|f| f.bits.is_some());
    out.push_str(
        "<table class=\"fields\">\n<thead>\n<tr><th>Name</th><th>Type</th><th>Default</th>",
    );
    if has_bits {
        out.push_str("<th>Bits</th>");
    }
    out.push_str("</tr>\n</thead>\n<tbody>\n");
    for f in fields {
        out.push_str("<tr><td>");
        out.push_str(&escape_html(&f.name));
        out.push_str(&badge(f.visibility));
        out.push_str("</td><td><code>");
        out.push_str(&linkify_signature(&f.ty, anchors));
        out.push_str("</code></td><td>");
        if let Some(def) = &f.default {
            out.push_str("<code>");
            out.push_str(&escape_html(def));
            out.push_str("</code>");
        }
        out.push_str("</td>");
        if has_bits {
            out.push_str("<td>");
            if let Some(b) = f.bits {
                out.push_str(&b.to_string());
            }
            out.push_str("</td>");
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n");
}

fn push_variants(out: &mut String, variants: &[DocVariant], anchors: &Anchors) {
    if variants.is_empty() {
        return;
    }
    out.push_str("<ul class=\"variants\">\n");
    for v in variants {
        out.push_str("<li><code>");
        out.push_str(&variant_html(v, anchors));
        out.push_str("</code>");
        out.push_str(&badge(v.visibility));
        out.push_str("</li>\n");
    }
    out.push_str("</ul>\n");
}

/// `Circle(float64 r)` / `GREEN = 10` / `Red`, with payload types linkified.
fn variant_html(v: &DocVariant, anchors: &Anchors) -> String {
    if !v.fields.is_empty() {
        let fs: Vec<String> = v
            .fields
            .iter()
            .map(|f| {
                format!(
                    "{} {}",
                    linkify_signature(&f.ty, anchors),
                    escape_html(&f.name)
                )
            })
            .collect();
        format!("{}({})", escape_html(&v.name), fs.join(", "))
    } else if let Some(val) = &v.value {
        format!("{} = {}", escape_html(&v.name), escape_html(val))
    } else {
        escape_html(&v.name)
    }
}

fn push_members(out: &mut String, methods: &[DocItem], anchors: &Anchors, level: usize) {
    if methods.is_empty() {
        return;
    }
    out.push_str("<div class=\"members\">\n");
    for m in methods {
        item(out, m, anchors, level);
    }
    out.push_str("</div>\n");
}

/// A `<span class="badge">private</span>` for a private declaration (shown only
/// under `--private`), else empty.
fn badge(v: Visibility) -> String {
    match v {
        Visibility::Private => " <span class=\"badge\">private</span>".to_string(),
        Visibility::Public => String::new(),
    }
}
