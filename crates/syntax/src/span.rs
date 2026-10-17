//! Source spans — byte offsets into a single source file.

/// A half-open byte range `[start, end)` into the source text.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Span {
        Span { start, end }
    }

    /// A zero-width span at `pos` (useful for synthesized/EOF tokens).
    pub const fn empty_at(pos: u32) -> Span {
        Span {
            start: pos,
            end: pos,
        }
    }

    pub const fn len(self) -> u32 {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// The smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Span;

    #[test]
    fn len_and_empty() {
        assert_eq!(Span::new(2, 5).len(), 3);
        assert!(Span::empty_at(7).is_empty());
        assert!(!Span::new(0, 1).is_empty());
    }

    #[test]
    fn merge() {
        assert_eq!(Span::new(2, 4).to(Span::new(8, 10)), Span::new(2, 10));
    }
}
