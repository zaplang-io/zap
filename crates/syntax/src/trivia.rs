//! Comment trivia (I#157/A1, #225).
//!
//! The lexer discards comments and blank lines (they produce no tokens), so the
//! AST carries no record of them. A formatter (`zap fmt`, #226) or doc generator
//! round-tripping through the AST would therefore **delete every comment**.
//!
//! This module adds an *additive* side-channel: [`crate::lexer::tokenize_with_trivia`]
//! returns the ordinary token stream **plus** a `Vec<Comment>`. The plain
//! [`crate::lexer::tokenize`] path is byte-identical to before, so nothing
//! downstream (parser, sema, codegen) sees the trivia — the trivia is purely for
//! tooling that wants to re-emit source faithfully.
//!
//! The chosen model is *comment-attachment by span*, not a lossless CST: every
//! AST node already carries a [`Span`], which is enough position information for
//! the formatter to attach each comment to the nearest node and re-emit it.
//! Attachment itself lives in the formatter; this module only captures the raw
//! comments and offers whitespace helpers computed from the source.

use crate::span::Span;

/// Where a comment sits relative to the code on its source line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentPlacement {
    /// The comment is the only thing on its line (ignoring indentation): a
    /// standalone/leading/dangling comment.
    OwnLine,
    /// The comment trails code on the same line (e.g. `x = 1  # note`).
    EndOfLine,
}

/// A single `#` comment captured from the source.
///
/// `text` is the raw lexeme starting at `#`, with trailing whitespace trimmed;
/// `span` covers exactly that trimmed text. Escape/content interpretation is
/// left to the formatter (which normalizes a single leading space, Black-style).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub span: Span,
    pub text: String,
    pub placement: CommentPlacement,
}

/// Count the newlines in `src[from..to]`.
///
/// `from`/`to` are byte offsets; out-of-range or inverted ranges count as zero.
pub fn newlines_between(src: &str, from: usize, to: usize) -> usize {
    if from >= to || to > src.len() {
        return 0;
    }
    src.as_bytes()[from..to]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
}

/// The number of fully blank lines between a span ending at `from` and the next
/// span starting at `to` (byte offsets).
///
/// Two adjacent code lines (one newline between them) have zero blank lines; `n`
/// newlines leave `n - 1` blank lines. Used by the formatter to preserve (and
/// collapse) author blank lines Python-style.
pub fn blank_lines_between(src: &str, from: usize, to: usize) -> usize {
    newlines_between(src, from, to).saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newline_counting() {
        let s = "a\n\n\nb";
        // "a" ends at 1, "b" starts at 4: three newlines, two blank lines.
        assert_eq!(newlines_between(s, 1, 4), 3);
        assert_eq!(blank_lines_between(s, 1, 4), 2);
    }

    #[test]
    fn adjacent_lines_have_no_blank() {
        let s = "a\nb";
        assert_eq!(blank_lines_between(s, 1, 2), 0);
    }

    #[test]
    fn out_of_range_is_zero() {
        let s = "abc";
        assert_eq!(newlines_between(s, 2, 1), 0);
        assert_eq!(newlines_between(s, 0, 99), 0);
    }
}
