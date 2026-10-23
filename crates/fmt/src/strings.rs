//! String-literal and comment text normalization (D-0037).
//!
//! The lexer stores a string's inner text (`value`) with escapes verbatim and
//! *without* recording which quote delimited it or whether it was triple-quoted.
//! The formatter therefore reconstructs a canonical spelling that re-parses to
//! the identical `value`:
//!
//! - A value containing a literal newline must be triple-quoted (a single-line
//!   string cannot hold one), so that is the trigger for `"""`.
//! - Otherwise prefer `"`; fall back to `'` when `"` appears unescaped and `'`
//!   does not; if both appear unescaped, keep `"` and backslash-escape the
//!   unescaped `"` (non-raw only — raw strings fall back to triple quotes).
//!
//! Every rule is a fixed point: re-formatting already-canonical output is a
//! no-op (idempotence).

/// Does `ch` occur in `value` not preceded by an escaping backslash? For a raw
/// string every backslash is literal, so any occurrence counts as unescaped.
fn contains_unescaped(value: &str, ch: char, raw: bool) -> bool {
    let mut backslashes = 0usize;
    for c in value.chars() {
        if c == ch && (raw || backslashes % 2 == 0) {
            return true;
        }
        backslashes = if c == '\\' && !raw {
            backslashes + 1
        } else {
            0
        };
    }
    false
}

/// Backslash-escape every unescaped occurrence of `ch` in `value` (non-raw).
fn escape_unescaped(value: &str, ch: char) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    let mut backslashes = 0usize;
    for c in value.chars() {
        if c == ch && backslashes % 2 == 0 {
            out.push('\\');
        }
        out.push(c);
        backslashes = if c == '\\' { backslashes + 1 } else { 0 };
    }
    out
}

/// The canonical prefix for a string literal, in fixed `r`/`b`/`f` order.
fn prefix(raw: bool, bytes: bool, fstring: bool) -> String {
    let mut p = String::new();
    if raw {
        p.push('r');
    }
    if bytes {
        p.push('b');
    }
    if fstring {
        p.push('f');
    }
    p
}

/// Render a string literal from its parsed parts to canonical source (D-0037).
pub fn format_string(value: &str, raw: bool, bytes: bool, fstring: bool) -> String {
    let pre = prefix(raw, bytes, fstring);

    // A literal newline forces triple quoting; a raw string that contains both
    // quote kinds also needs triple quotes (it cannot escape its own delimiter).
    let needs_triple = value.contains('\n')
        || (raw && contains_unescaped(value, '"', true) && contains_unescaped(value, '\'', true));
    if needs_triple {
        let delim = if value.contains("\"\"\"") && !value.contains("'''") {
            "'''"
        } else {
            "\"\"\""
        };
        return format!("{pre}{delim}{value}{delim}");
    }

    if !contains_unescaped(value, '"', raw) {
        format!("{pre}\"{value}\"")
    } else if !contains_unescaped(value, '\'', raw) {
        format!("{pre}'{value}'")
    } else {
        // Both quotes appear unescaped (non-raw): keep `"`, escape the inner `"`.
        format!("{pre}\"{}\"", escape_unescaped(value, '"'))
    }
}

/// Normalize a comment's lexeme (it already starts with `#`, trailing whitespace
/// trimmed): ensure a single space after the leading `#` run unless the body is
/// empty or begins with `!`/`:` (shebang / typed-comment conventions).
pub fn normalize_comment(text: &str) -> String {
    let hashes = text.chars().take_while(|&c| c == '#').count();
    let (hash, rest) = text.split_at(hashes);
    if rest.is_empty() {
        return hash.to_string();
    }
    let first = rest.chars().next().unwrap();
    if first == ' ' || first == '!' || first == ':' {
        text.to_string()
    } else {
        format!("{hash} {rest}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_double_quotes() {
        assert_eq!(format_string("hi", false, false, false), "\"hi\"");
        assert_eq!(format_string("{x}", false, false, true), "f\"{x}\"");
        assert_eq!(format_string("\\d+", true, false, false), "r\"\\d+\"");
    }

    #[test]
    fn falls_back_to_single_quotes() {
        // Contains an unescaped `"`, no `'` → use single quotes (no escaping).
        assert_eq!(
            format_string("say \"hi\"", false, false, false),
            "'say \"hi\"'"
        );
        assert_eq!(format_string("a\"b", false, false, false), "'a\"b'");
    }

    #[test]
    fn both_quotes_escapes_double() {
        // Unescaped `"` and `'` → keep `"`, escape the `"`.
        let out = format_string("a\"b'c", false, false, false);
        assert_eq!(out, "\"a\\\"b'c\"");
        // Idempotent: the escaped form re-formats unchanged.
        assert_eq!(format_string("a\\\"b'c", false, false, false), out);
    }

    #[test]
    fn newline_triggers_triple() {
        assert_eq!(
            format_string("a\nb", false, false, false),
            "\"\"\"a\nb\"\"\""
        );
    }

    #[test]
    fn comment_spacing() {
        assert_eq!(normalize_comment("#foo"), "# foo");
        assert_eq!(normalize_comment("# foo"), "# foo");
        assert_eq!(normalize_comment("#"), "#");
        assert_eq!(normalize_comment("#!shebang"), "#!shebang");
        assert_eq!(normalize_comment("## section"), "## section");
        assert_eq!(normalize_comment("#: typed"), "#: typed");
    }
}
