//! Shared rendering helpers for the `zap doc` Markdown and HTML renderers
//! (I#157/C2, issue #233).
//!
//! Both renderers consume the same [`DocModel`](crate::DocModel) and need the
//! same three things: a **stable anchor** for every declaration (so a table of
//! contents and intra-doc links resolve), **HTML escaping**, and a small,
//! **in-house safe subset of Markdown** for rendering docstring prose into HTML.
//!
//! The toolchain ships no third-party runtime dependencies — `zap lsp` took no
//! transport dep, `zap fmt` is hand-rolled, and the #232 doc model refused
//! `serde`. Rather than make `pulldown-cmark` the first such dep for the few
//! Markdown features a docstring uses, [`md_to_html`] renders a documented
//! subset itself, always **after** escaping, so no raw HTML from a docstring can
//! ever reach the output.

use crate::{DocItem, DocItemBody, DocModel};
use std::collections::{HashMap, HashSet};

// ----------------------------------------------------------------------------
// Anchors
// ----------------------------------------------------------------------------

/// A stable, unique id for every declaration in a module, plus a lookup from a
/// type's simple name to its anchor (for intra-doc links).
///
/// Anchors are assigned in declaration order, so the output is deterministic.
/// The same slug is used for the Markdown `<a id>` tag and the HTML `id=`
/// attribute, so links resolve identically regardless of the Markdown viewer's
/// own heading-slug algorithm.
pub(crate) struct Anchors {
    /// Fully-qualified name → unique slug.
    by_qname: HashMap<String, String>,
    /// A type's simple name → its slug (structs, enums, interfaces only).
    types: HashMap<String, String>,
}

impl Anchors {
    pub(crate) fn build(model: &DocModel) -> Anchors {
        let mut a = Anchors {
            by_qname: HashMap::new(),
            types: HashMap::new(),
        };
        let mut used: HashSet<String> = HashSet::new();
        for item in &model.items {
            a.assign(item, &mut used);
        }
        a
    }

    fn assign(&mut self, item: &DocItem, used: &mut HashSet<String>) {
        let slug = unique_slug(&slugify(&item.qualified_name), used);
        if is_type(&item.body) {
            self.types.insert(item.name.clone(), slug.clone());
        }
        self.by_qname.insert(item.qualified_name.clone(), slug);
        // Nested methods get their own anchors too.
        for m in members(&item.body) {
            self.assign(m, used);
        }
    }

    /// The anchor slug for a declaration's fully-qualified name.
    pub(crate) fn of(&self, qualified_name: &str) -> &str {
        self.by_qname
            .get(qualified_name)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// The anchor slug of a type with this simple name, if the module declares
    /// one — the referent for an intra-doc link.
    pub(crate) fn type_slug(&self, name: &str) -> Option<&str> {
        self.types.get(name).map(String::as_str)
    }
}

/// The nested method items of a type body (empty for non-types).
pub(crate) fn members(body: &DocItemBody) -> &[DocItem] {
    match body {
        DocItemBody::Struct(s) => &s.methods,
        DocItemBody::Enum(e) => &e.methods,
        DocItemBody::Interface(i) => &i.methods,
        _ => &[],
    }
}

fn is_type(body: &DocItemBody) -> bool {
    matches!(
        body,
        DocItemBody::Struct(_) | DocItemBody::Enum(_) | DocItemBody::Interface(_)
    )
}

/// Lowercase, keep `[a-z0-9_]`, and collapse every other run into a single `-`.
/// Deterministic and viewer-independent (we also emit the matching explicit
/// anchor).
pub(crate) fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "item".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Make `base` unique within `used` by appending `-2`, `-3`, … on collision.
fn unique_slug(base: &str, used: &mut HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

// ----------------------------------------------------------------------------
// HTML escaping
// ----------------------------------------------------------------------------

/// Escape the five characters that are unsafe in HTML text or attributes. Every
/// identifier, signature, and docstring is passed through this before it reaches
/// the HTML output, so a docstring that contains `<script>` cannot inject
/// markup.
pub(crate) fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Intra-doc links in a rendered (HTML) signature
// ----------------------------------------------------------------------------

/// HTML-escape `sig` and wrap any identifier that names a type declared in this
/// module in a link to that type's anchor. Non-identifier runs and unknown
/// identifiers are escaped verbatim.
pub(crate) fn linkify_signature(sig: &str, anchors: &Anchors) -> String {
    let mut out = String::with_capacity(sig.len());
    let chars: Vec<char> = sig.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_ident_start(c) {
            let start = i;
            while i < chars.len() && is_ident_part(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            match anchors.type_slug(&word) {
                Some(slug) => {
                    out.push_str(&format!(
                        "<a href=\"#{}\">{}</a>",
                        escape_html(slug),
                        escape_html(&word)
                    ));
                }
                None => out.push_str(&escape_html(&word)),
            }
        } else {
            out.push_str(&escape_html(&c.to_string()));
            i += 1;
        }
    }
    out
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

// ----------------------------------------------------------------------------
// Docstring prose
// ----------------------------------------------------------------------------

/// The dedented docstring prose for a declaration, with Google-style section
/// headers (a lone `Args:`/`Returns:`/… line) emphasised as `**Args:**`.
///
/// This is the renderer's faithful, pass-through view of a docstring: the whole
/// cleaned text (not just the summary + parsed sections), so a multi-paragraph
/// description is never lost. Markdown emits it verbatim; HTML runs it through
/// [`md_to_html`]. Returns `None` for an absent or empty docstring.
pub(crate) fn description(doc: &Option<crate::DocString>) -> Option<String> {
    let d = doc.as_ref()?;
    let cleaned = crate::cleandoc(&d.raw);
    if cleaned.trim().is_empty() {
        return None;
    }
    let styled: Vec<String> = cleaned
        .lines()
        .map(|line| match section_label(line) {
            Some(label) => format!("**{label}:**"),
            None => line.to_string(),
        })
        .collect();
    Some(styled.join("\n"))
}

/// If `line` is a lone Google-style section header (`Label:` with nothing after
/// the colon), its label — mirrors the extractor's own rule.
fn section_label(line: &str) -> Option<String> {
    let t = line.trim();
    let head = t.strip_suffix(':')?;
    if head.is_empty()
        || !head.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
        || head.contains("  ")
    {
        return None;
    }
    Some(head.to_string())
}

// ----------------------------------------------------------------------------
// Markdown → HTML: a documented safe subset
// ----------------------------------------------------------------------------

/// Render a docstring's prose from the supported Markdown subset to HTML.
///
/// The subset is deliberately small — exactly what a docstring uses:
///
/// - **Paragraphs** separated by a blank line (`<p>`).
/// - **Fenced code blocks** delimited by ```` ``` ```` (`<pre><code>`), content
///   verbatim.
/// - **Bullet lists** whose items start with `- ` or `* ` (`<ul><li>`).
/// - **Inline**: `` `code` `` (`<code>`), `**bold**` (`<strong>`), `*italic*`
///   (`<em>`).
///
/// Everything is HTML-escaped first, so the subset only ever *adds* known-safe
/// tags — a docstring cannot smuggle raw HTML through.
pub(crate) fn md_to_html(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("```") {
            // Fenced code block: everything up to the closing fence, verbatim.
            i += 1;
            let mut code = String::new();
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code.push_str(lines[i]);
                code.push('\n');
                i += 1;
            }
            if i < lines.len() {
                i += 1; // consume the closing fence
            }
            out.push_str("<pre><code>");
            out.push_str(&escape_html(&code));
            out.push_str("</code></pre>\n");
        } else if is_bullet(line) {
            // A run of bullet-list items.
            out.push_str("<ul>\n");
            while i < lines.len() && is_bullet(lines[i]) {
                let body = &lines[i].trim_start()[2..];
                out.push_str("<li>");
                out.push_str(&inline(body));
                out.push_str("</li>\n");
                i += 1;
            }
            out.push_str("</ul>\n");
        } else if line.trim().is_empty() {
            i += 1;
        } else {
            // A paragraph: consecutive non-blank, non-list, non-fence lines.
            let mut para: Vec<&str> = Vec::new();
            while i < lines.len()
                && !lines[i].trim().is_empty()
                && !is_bullet(lines[i])
                && !lines[i].trim_start().starts_with("```")
            {
                para.push(lines[i].trim());
                i += 1;
            }
            out.push_str("<p>");
            out.push_str(&inline(&para.join(" ")));
            out.push_str("</p>\n");
        }
    }
    out
}

fn is_bullet(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("- ") || t.starts_with("* ")
}

/// Apply inline formatting to one run of text. Escapes first, then handles code
/// spans (so markers inside them are inert), then bold, then italic.
fn inline(text: &str) -> String {
    let escaped = escape_html(text);
    let with_code = apply_code_spans(&escaped);
    apply_emphasis(&with_code)
}

/// Replace backtick code spans with `<code>…</code>`, CommonMark-style: a span
/// opens with a run of N backticks and closes with the next run of exactly N
/// (so both `` `x` `` and the RST-style ``` ``x`` ``` work). An unmatched run is
/// left literal. Operates on already-escaped text; the backtick survives
/// escaping.
fn apply_code_spans(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`' {
            let open = run_len(&chars, i);
            if let Some(end) = closing_run(&chars, i + open, open) {
                let inner: String = chars[i + open..end].iter().collect();
                out.push_str("<code>");
                out.push_str(inner.trim());
                out.push_str("</code>");
                i = end + open;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// The length of the run of `` ` `` starting at `i`.
fn run_len(chars: &[char], i: usize) -> usize {
    let mut n = 0;
    while i + n < chars.len() && chars[i + n] == '`' {
        n += 1;
    }
    n
}

/// The start index of the next backtick run of exactly `want` length at or
/// after `from`.
fn closing_run(chars: &[char], from: usize, want: usize) -> Option<usize> {
    let mut j = from;
    while j < chars.len() {
        if chars[j] == '`' {
            let n = run_len(chars, j);
            if n == want {
                return Some(j);
            }
            j += n;
        } else {
            j += 1;
        }
    }
    None
}

/// Replace `**bold**` with `<strong>` and `*italic*` with `<em>`. Runs after
/// code spans, so markers inside `<code>` are not present here as plain text.
fn apply_emphasis(s: &str) -> String {
    let bold = replace_delim(s, "**", "<strong>", "</strong>");
    replace_delim(&bold, "*", "<em>", "</em>")
}

/// Replace matched pairs of `delim` with `open`/`close`. Only non-overlapping
/// pairs on the same logical run are wrapped; an unmatched trailing delimiter is
/// left as-is.
fn replace_delim(s: &str, delim: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        // Skip over any <code>…</code> region untouched so a `*` inside inline
        // code is not treated as emphasis.
        if let Some(code_at) = rest.find("<code>") {
            if delim == "*" || delim == "**" {
                if let Some(code_end) = rest[code_at..].find("</code>") {
                    let abs_end = code_at + code_end + "</code>".len();
                    // Emphasis in the text before the code span.
                    out.push_str(&wrap_pairs(&rest[..code_at], delim, open, close));
                    out.push_str(&rest[code_at..abs_end]);
                    rest = &rest[abs_end..];
                    continue;
                }
            }
        }
        out.push_str(&wrap_pairs(rest, delim, open, close));
        break;
    }
    out
}

/// Wrap matched `delim` pairs in `open`/`close` within a code-free run.
fn wrap_pairs(s: &str, delim: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let Some(start) = rest.find(delim) else {
            out.push_str(rest);
            break;
        };
        let after = &rest[start + delim.len()..];
        let Some(rel_end) = after.find(delim) else {
            // No closing delimiter: emit the rest verbatim.
            out.push_str(rest);
            break;
        };
        if rel_end == 0 {
            // Empty `**`/`*` pair — emit literally and move on.
            out.push_str(&rest[..start + delim.len()]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str(open);
        out.push_str(&after[..rel_end]);
        out.push_str(close);
        rest = &after[rel_end + delim.len()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_is_lowercase_dashed() {
        assert_eq!(slugify("shapes.Point.dist"), "shapes-point-dist");
        assert_eq!(slugify("m.to_json"), "m-to_json");
    }

    #[test]
    fn escape_covers_metacharacters() {
        assert_eq!(
            escape_html("<a href=\"x\">&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn md_escapes_before_formatting() {
        let html = md_to_html("a <script> tag and **bold**");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn md_code_span_is_inert() {
        let html = md_to_html("use `a*b*c` literally");
        assert!(html.contains("<code>a*b*c</code>"));
        assert!(!html.contains("<em>"));
    }

    #[test]
    fn md_bullets_and_paragraphs() {
        let html = md_to_html("Intro.\n\n- one\n- two");
        assert!(html.contains("<p>Intro.</p>"));
        assert!(html.contains("<ul>\n<li>one</li>\n<li>two</li>\n</ul>"));
    }
}
