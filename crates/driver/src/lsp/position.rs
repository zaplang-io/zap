//! The LSP position layer: byte offset → `(line, character)` in UTF-16 code
//! units (I#229).
//!
//! Zap spans are byte offsets into the source (`zapc_syntax::span::Span`), and
//! `zapc_analysis::LineIndex` maps a byte offset to a 1-based line and a
//! **byte** column. LSP wants something different: **0-based** lines and
//! **0-based UTF-16 code-unit** characters (the default `positionEncoding`,
//! which this server advertises). The analysis design note (`0022`) explicitly
//! leaves this UTF-16 layer to #229, sitting on top of the byte index without
//! changing it.
//!
//! This mapper is built per document from the full source the server holds. For
//! ASCII it is a plain byte count (one byte = one UTF-16 unit); the general path
//! accumulates `char::len_utf16` across the line prefix, so astral-plane
//! characters correctly count as two units.

use zapc_syntax::span::Span;

/// A JSON-RPC payload builder lives in [`crate::json`]; here we only produce the
/// numeric `(line, character)` pairs and leave the JSON shaping to the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LspPos {
    /// 0-based line.
    pub line: u32,
    /// 0-based character, in UTF-16 code units.
    pub character: u32,
}

/// Maps byte offsets in one source document to LSP `(line, character)`.
pub struct PositionMapper<'a> {
    src: &'a str,
    /// Byte offset of the start of each line (`line_starts[0] == 0`).
    line_starts: Vec<usize>,
}

impl<'a> PositionMapper<'a> {
    pub fn new(src: &'a str) -> PositionMapper<'a> {
        let mut line_starts = vec![0usize];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        PositionMapper { src, line_starts }
    }

    /// The LSP position of a byte offset, clamped to the end of the source. An
    /// offset that lands inside a multi-byte code point is snapped back to the
    /// code point's start (span endpoints are at boundaries in practice; this is
    /// just a safety net against a panic).
    pub fn position(&self, offset: u32) -> LspPos {
        let offset = (offset as usize).min(self.src.len());
        // The line is the last line-start <= offset.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = self.line_starts[line];
        let mut end = offset;
        while end > line_start && !self.src.is_char_boundary(end) {
            end -= 1;
        }
        let character: u32 = self.src[line_start..end]
            .chars()
            .map(|c| c.len_utf16() as u32)
            .sum();
        LspPos {
            line: line as u32,
            character,
        }
    }

    /// The LSP range `[start, end)` of a span. A zero-width span maps to a
    /// zero-width range at its position.
    pub fn range(&self, span: Span) -> (LspPos, LspPos) {
        (self.position(span.start), self.position(span.end))
    }

    /// The byte offset of an LSP `(line, character)` position — the inverse of
    /// [`position`](Self::position), used to turn a client request's cursor into
    /// an analysis byte offset (#230). `character` counts UTF-16 code units. A
    /// `character` past the line's end clamps to the end of the line's content
    /// (just before the newline); a `line` past the end returns `None`.
    pub fn offset(&self, line: u32, character: u32) -> Option<u32> {
        let line_start = *self.line_starts.get(line as usize)?;
        let line_end = self
            .line_starts
            .get(line as usize + 1)
            .map(|&s| s - 1) // drop the '\n' that starts the next line
            .unwrap_or(self.src.len());
        let mut utf16 = 0u32;
        let mut off = line_start;
        for c in self.src[line_start..line_end].chars() {
            if utf16 >= character {
                break;
            }
            utf16 += c.len_utf16() as u32;
            off += c.len_utf8();
        }
        Some(off as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_byte_count() {
        let src = "let x = 1\nlet y = 2\n";
        let m = PositionMapper::new(src);
        // offset 0 -> (0, 0)
        assert_eq!(
            m.position(0),
            LspPos {
                line: 0,
                character: 0
            }
        );
        // 'x' is at byte 4 on line 0
        assert_eq!(
            m.position(4),
            LspPos {
                line: 0,
                character: 4
            }
        );
        // start of line 1 (byte 10, just after the first '\n')
        assert_eq!(
            m.position(10),
            LspPos {
                line: 1,
                character: 0
            }
        );
        // 'y' is at byte 14 -> line 1, char 4
        assert_eq!(
            m.position(14),
            LspPos {
                line: 1,
                character: 4
            }
        );
    }

    #[test]
    fn utf16_counts_code_units_not_bytes() {
        // "π" is 2 bytes in UTF-8 but 1 UTF-16 unit; "😀" is 4 bytes, 2 units.
        let src = "π = 😀 + x";
        let m = PositionMapper::new(src);
        // byte layout: π(0..2) ' '(2) '='(3) ' '(4) 😀(5..9) ' '(9) '+'(10) ' '(11) x(12)
        // '=' at byte 3 -> char 2 (after π[1 unit] + space[1 unit])
        assert_eq!(
            m.position(3),
            LspPos {
                line: 0,
                character: 2
            }
        );
        // '+' at byte 10 -> π,sp,=,sp = 4 units, 😀 = 2 units, sp = 1 -> 7
        assert_eq!(
            m.position(10),
            LspPos {
                line: 0,
                character: 7
            }
        );
        // 'x' at byte 12 -> 7 (+) + 1 (+) + 1 (sp) = 9
        assert_eq!(
            m.position(12),
            LspPos {
                line: 0,
                character: 9
            }
        );
    }

    #[test]
    fn offset_past_end_clamps() {
        let src = "ab";
        let m = PositionMapper::new(src);
        assert_eq!(
            m.position(999),
            LspPos {
                line: 0,
                character: 2
            }
        );
    }

    #[test]
    fn offset_is_inverse_of_position() {
        let src = "let x = 1\nlet y = 2\n";
        let m = PositionMapper::new(src);
        for off in 0..=src.len() as u32 {
            let p = m.position(off);
            // Round-trips for offsets that are the start of a character.
            if src.is_char_boundary(off as usize) {
                assert_eq!(m.offset(p.line, p.character), Some(off), "offset {off}");
            }
        }
    }

    #[test]
    fn offset_utf16_and_clamping() {
        // "π😀x": π(0..2,1u) 😀(2..6,2u) x(6..7,1u)
        let src = "π😀x\nq";
        let m = PositionMapper::new(src);
        assert_eq!(m.offset(0, 0), Some(0)); // π
        assert_eq!(m.offset(0, 1), Some(2)); // 😀
        assert_eq!(m.offset(0, 3), Some(6)); // x (after π=1 + 😀=2 units)
        assert_eq!(m.offset(0, 4), Some(7)); // end of line content
                                             // A character past the line end clamps to the newline, not line 1.
        assert_eq!(m.offset(0, 99), Some(7));
        // Line 1 starts after the '\n' at byte 8.
        assert_eq!(m.offset(1, 0), Some(8));
        // A line past the end is None.
        assert_eq!(m.offset(5, 0), None);
    }

    #[test]
    fn range_of_span() {
        let src = "foo bar\n";
        let m = PositionMapper::new(src);
        let (s, e) = m.range(Span::new(4, 7));
        assert_eq!(
            s,
            LspPos {
                line: 0,
                character: 4
            }
        );
        assert_eq!(
            e,
            LspPos {
                line: 0,
                character: 7
            }
        );
    }
}
