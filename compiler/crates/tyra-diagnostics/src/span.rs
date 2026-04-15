// Span represents a range in source code, used for error reporting.
// Every AST node and token carries a Span.

use crate::SourceId;

/// A byte-offset range in a specific source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(source: SourceId, start: u32, end: u32) -> Self {
        debug_assert!(start <= end);
        Self { source, start, end }
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Merge two spans into one covering both. Both must be in the same source.
    pub fn merge(self, other: Span) -> Span {
        debug_assert_eq!(self.source, other.source);
        Span {
            source: self.source,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_merge() {
        let src = SourceId::test(0);
        let a = Span::new(src, 0, 5);
        let b = Span::new(src, 3, 10);
        let merged = a.merge(b);
        assert_eq!(merged.start, 0);
        assert_eq!(merged.end, 10);
    }

    #[test]
    fn span_len() {
        let s = Span::new(SourceId::test(0), 10, 20);
        assert_eq!(s.len(), 10);
        assert!(!s.is_empty());
    }

    #[test]
    fn span_empty() {
        let s = Span::new(SourceId::test(0), 5, 5);
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
    }
}
