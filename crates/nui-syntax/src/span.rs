//! Source spans: byte-offset ranges into the source text.

/// Half-open byte-offset range `[start, end)` into the source text.
///
/// Offsets always sit on UTF-8 character boundaries because tokens are
/// produced by the lexer character by character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// First byte offset.
    pub start: u32,
    /// One past the last byte offset.
    pub end: u32,
}

impl Span {
    /// Constructs a span from its offsets.
    pub const fn new(start: u32, end: u32) -> Span {
        return Span { start, end };
    }

    /// Returns the smallest span covering both `self` and `other`.
    pub fn merge(self, other: Span) -> Span {
        return Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_covers_both_spans() {
        let first = Span::new(4, 8);
        let second = Span::new(2, 6);
        assert_eq!(first.merge(second), Span::new(2, 8));
    }

    #[test]
    fn merge_with_disjoint_span() {
        let first = Span::new(0, 3);
        let second = Span::new(10, 12);
        assert_eq!(first.merge(second), Span::new(0, 12));
    }

    #[test]
    fn merge_is_commutative() {
        let first = Span::new(1, 5);
        let second = Span::new(4, 9);
        assert_eq!(first.merge(second), second.merge(first));
    }
}
