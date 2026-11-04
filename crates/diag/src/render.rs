//! Human-facing diagnostic rendering (issue #571).
//!
//! The front-end reports a source location as a byte `Span` (`start`/`end`
//! offsets into the file). A byte offset is meaningless to a reader — you
//! cannot find `[56..61]` in your editor — so this module turns a span into a
//! `file:line:column` location and prints the offending source line with a
//! caret pointing at the span, in the familiar `rustc`/clang style:
//!
//! ```text
//! main.zap:3:5: error[ZAP4025]: dict.get() takes exactly one argument (the key), got 2
//!     d.get(k, 0)
//!     ^~~~~~~~~~~
//! ```
//!
//! # Column unit
//!
//! A **column counts Unicode scalar values (`char`s) from the start of the
//! line, 1-based** — the convention `rustc` and most editors use, so a column
//! number matches what a reader counts by eye. (A tab therefore counts as one
//! column.) Lines are likewise 1-based. Using chars rather than bytes keeps the
//! column correct for source containing multi-byte UTF-8.
//!
//! The helpers here take the span as plain `u32` offsets rather than a `Span`
//! so this (leaf) crate stays free of a dependency on `zapc_syntax`.

use std::collections::HashSet;
use std::hash::Hash;

use crate::ErrorCode;

/// Maps byte offsets into a source string to 1-based line/column positions.
///
/// Built once over a file's bytes; `line_col` and the line-text lookups then
/// answer in `O(log lines)`. The index records only line boundaries, so the
/// same `src` it was built from must be passed back in to read column/text.
pub struct LineIndex {
    /// Byte offset of the first character of each line; `line_starts[0] == 0`
    /// and the vector is strictly increasing.
    line_starts: Vec<u32>,
    /// The source length in bytes (offsets past this are clamped to it).
    len: u32,
}

impl LineIndex {
    /// Build the line index for `src`.
    pub fn new(src: &str) -> LineIndex {
        let mut line_starts = vec![0u32];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        LineIndex {
            line_starts,
            len: src.len() as u32,
        }
    }

    /// The 1-based `(line, column)` of `offset` within `src` — the same text
    /// the index was built from. An `offset` past the end of the source is
    /// clamped to the end; one that falls inside a multi-byte character is
    /// rounded down to that character's start.
    pub fn line_col(&self, src: &str, offset: u32) -> (u32, u32) {
        let offset = clamp_to_boundary(src, offset.min(self.len));
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            // `line_starts[0] == 0 <= offset`, so `i >= 1` here.
            Err(i) => i - 1,
        };
        let line_start = self.line_starts[line] as usize;
        let col = src[line_start..offset as usize].chars().count() as u32 + 1;
        (line as u32 + 1, col)
    }

    /// The byte range `[start, end)` of the 1-based `line` *including* its
    /// trailing newline, or `None` if the line is out of range.
    fn line_bytes(&self, line: u32) -> Option<(usize, usize)> {
        let idx = line.checked_sub(1)? as usize;
        let start = *self.line_starts.get(idx)? as usize;
        let end = self
            .line_starts
            .get(idx + 1)
            .map(|&s| s as usize)
            .unwrap_or(self.len as usize);
        Some((start, end))
    }
}

/// Round `offset` down to the nearest UTF-8 character boundary of `src`.
fn clamp_to_boundary(src: &str, mut offset: u32) -> u32 {
    while offset > 0 && !src.is_char_boundary(offset as usize) {
        offset -= 1;
    }
    offset
}

/// Render one diagnostic as the multi-line string the driver prints to stderr
/// (no trailing newline).
///
/// With both a source text and a span, the result is the three-line
/// `file:line:column: error[CODE]: message` / source-line / caret form. Without
/// a span (e.g. a lowering error that carries none) or without the source text
/// (the file could not be re-read), it degrades to the single header line
/// `file: error[CODE]: message`, which still names the code and message.
pub fn render_diagnostic(
    src: Option<&str>,
    file: &str,
    code: ErrorCode,
    message: &str,
    span: Option<(u32, u32)>,
) -> String {
    render_diagnostic_sev(Severity::Error, src, file, code, message, span)
}

/// Diagnostic severity for the rendered header label (`error[...]` vs
/// `warning[...]`). Severity is orthogonal to the code (a given `ZAPnnnn` may in
/// principle be reported at either), so it is carried alongside the code rather
/// than baked into it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// Like [`render_diagnostic`] but with an explicit [`Severity`], so a non-fatal
/// warning renders as `warning[CODE]` in the same three-line form (I#472).
pub fn render_diagnostic_sev(
    severity: Severity,
    src: Option<&str>,
    file: &str,
    code: ErrorCode,
    message: &str,
    span: Option<(u32, u32)>,
) -> String {
    let sev = severity.label();
    let (src, (start, end)) = match (src, span) {
        (Some(src), Some(span)) => (src, span),
        _ => return format!("{file}: {sev}[{code}]: {message}"),
    };

    let index = LineIndex::new(src);
    let start = clamp_to_boundary(src, start.min(index.len));
    let end = clamp_to_boundary(src, end.min(index.len)).max(start);
    let (line, col) = index.line_col(src, start);

    let mut out = format!("{file}:{line}:{col}: {sev}[{code}]: {message}");

    // Underneath the header, echo the offending line and underline the span,
    // clipped to that line (a span that runs onto later lines underlines only
    // the first one).
    if let Some((line_start, line_end)) = index.line_bytes(line) {
        let line_text = src[line_start..line_end].trim_end_matches(['\n', '\r']);
        let line_text_end = line_start + line_text.len();
        let span_end = (end as usize).min(line_text_end);
        let underline = src[start as usize..span_end].chars().count().max(1);

        out.push('\n');
        out.push_str(line_text);
        out.push('\n');
        for _ in 1..col {
            out.push(' ');
        }
        out.push('^');
        for _ in 1..underline {
            out.push('~');
        }
    }
    out
}

/// Return `items` with later duplicates removed, preserving first-occurrence
/// order. Two items are duplicates when `key` maps them to equal values.
///
/// Used to collapse a diagnostic that is reported more than once for a single
/// cause (#571) before rendering, keyed on `(file, code, span, message)`.
pub fn dedup_preserving<T, K, F>(items: Vec<T>, key: F) -> Vec<T>
where
    F: Fn(&T) -> K,
    K: Eq + Hash,
{
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|it| seen.insert(key(it)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: ErrorCode = ErrorCode::ZAP1006; // "unexpected character"

    #[test]
    fn line_col_is_one_based() {
        let src = "ab\ncd\n";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_col(src, 0), (1, 1)); // 'a'
        assert_eq!(idx.line_col(src, 1), (1, 2)); // 'b'
        assert_eq!(idx.line_col(src, 3), (2, 1)); // 'c'
        assert_eq!(idx.line_col(src, 4), (2, 2)); // 'd'
    }

    #[test]
    fn column_counts_chars_not_bytes() {
        // Line 2 is "cαd": 'α' is two UTF-8 bytes, so 'd' is byte 6 but column 3.
        let src = "ab\ncαd\n";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_col(src, 6), (2, 3));
    }

    #[test]
    fn offset_inside_multibyte_char_rounds_down() {
        let src = "cαd"; // 'α' occupies bytes 1..3
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_col(src, 2), (1, 2)); // mid-'α' -> start of 'α'
    }

    #[test]
    fn offset_past_end_is_clamped() {
        let src = "ab\n";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_col(src, 999), idx.line_col(src, src.len() as u32));
    }

    #[test]
    fn renders_file_line_col_with_caret() {
        let src = "ab\ncαd\n";
        // Underline just 'd' (byte 6..7) -> column 3, single caret.
        let got = render_diagnostic(Some(src), "f.zap", CODE, "oops", Some((6, 7)));
        assert_eq!(got, "f.zap:2:3: error[ZAP1006]: oops\ncαd\n  ^");
    }

    #[test]
    fn warning_severity_labels_the_header() {
        // A non-fatal diagnostic renders `warning[CODE]` in the same form (I#472).
        let src = "ab\ncαd\n";
        let got = render_diagnostic_sev(
            Severity::Warning,
            Some(src),
            "f.zap",
            CODE,
            "oops",
            Some((6, 7)),
        );
        assert_eq!(got, "f.zap:2:3: warning[ZAP1006]: oops\ncαd\n  ^");
        // And degrades to the header-only form without a span.
        let no_span =
            render_diagnostic_sev(Severity::Warning, Some(src), "f.zap", CODE, "oops", None);
        assert_eq!(no_span, "f.zap: warning[ZAP1006]: oops");
    }

    #[test]
    fn caret_underlines_the_whole_span_in_chars() {
        let src = "ab\ncαd\n";
        // Underline "cα" (byte 3..6) -> two chars, so '^~'.
        let got = render_diagnostic(Some(src), "f.zap", CODE, "oops", Some((3, 6)));
        assert_eq!(got, "f.zap:2:1: error[ZAP1006]: oops\ncαd\n^~");
    }

    #[test]
    fn empty_span_still_gets_one_caret() {
        let src = "xy\n";
        let got = render_diagnostic(Some(src), "f.zap", CODE, "oops", Some((1, 1)));
        assert_eq!(got, "f.zap:1:2: error[ZAP1006]: oops\nxy\n ^");
    }

    #[test]
    fn span_clipped_to_the_line() {
        // A span that runs to the next line underlines only this line's tail.
        let src = "abc\ndef\n";
        let got = render_diagnostic(Some(src), "f.zap", CODE, "oops", Some((2, 6)));
        // Line 1 is "abc"; from 'c' (col 3) only one char remains on the line.
        assert_eq!(got, "f.zap:1:3: error[ZAP1006]: oops\nabc\n  ^");
    }

    #[test]
    fn degrades_without_span_or_source() {
        let no_span = render_diagnostic(Some("ab\n"), "f.zap", CODE, "oops", None);
        assert_eq!(no_span, "f.zap: error[ZAP1006]: oops");
        let no_src = render_diagnostic(None, "f.zap", CODE, "oops", Some((0, 1)));
        assert_eq!(no_src, "f.zap: error[ZAP1006]: oops");
    }

    #[test]
    fn dedup_is_order_preserving() {
        let items = vec![("a", 1), ("a", 1), ("b", 2), ("a", 1), ("c", 3)];
        let out = dedup_preserving(items, |&x| x);
        assert_eq!(out, vec![("a", 1), ("b", 2), ("c", 3)]);
    }
}
