// Low-level character-by-character cursor over source text.
// Does not allocate; works on a borrowed &str.

/// Cursor for scanning source text byte by byte.
pub struct Cursor<'a> {
    source: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { source, pos: 0 }
    }

    /// Current byte offset in the source.
    pub fn pos(&self) -> u32 {
        debug_assert!(self.pos <= u32::MAX as usize, "source file too large");
        self.pos as u32
    }

    /// Peek at the current character without advancing.
    pub fn peek(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    /// Peek at the character after the current one.
    pub fn peek_next(&self) -> Option<char> {
        let mut chars = self.source[self.pos..].chars();
        chars.next();
        chars.next()
    }

    /// Advance by one character and return it.
    pub fn advance(&mut self) -> Option<char> {
        let ch = self.source[self.pos..].chars().next()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    /// Advance if the current character matches the predicate.
    pub fn eat(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Advance while the predicate holds, returning the consumed slice.
    pub fn eat_while(&mut self, predicate: impl Fn(char) -> bool) -> &'a str {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if predicate(ch) {
                self.advance();
            } else {
                break;
            }
        }
        &self.source[start..self.pos]
    }

    /// Check if we've reached the end.
    pub fn is_eof(&self) -> bool {
        self.pos >= self.source.len()
    }

    /// Get a slice of the source from start to current position.
    pub fn slice_from(&self, start: u32) -> &'a str {
        &self.source[start as usize..self.pos]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_basics() {
        let mut c = Cursor::new("abc");
        assert_eq!(c.peek(), Some('a'));
        assert_eq!(c.advance(), Some('a'));
        assert_eq!(c.peek(), Some('b'));
        assert_eq!(c.pos(), 1);
        assert!(!c.is_eof());
    }

    #[test]
    fn cursor_eat() {
        let mut c = Cursor::new("==");
        assert!(c.eat('='));
        assert!(c.eat('='));
        assert!(!c.eat('='));
        assert!(c.is_eof());
    }

    #[test]
    fn cursor_eat_while() {
        let mut c = Cursor::new("12345abc");
        let digits = c.eat_while(|ch| ch.is_ascii_digit());
        assert_eq!(digits, "12345");
        assert_eq!(c.peek(), Some('a'));
    }

    #[test]
    fn cursor_peek_next() {
        let c = Cursor::new("ab");
        assert_eq!(c.peek(), Some('a'));
        assert_eq!(c.peek_next(), Some('b'));
    }

    #[test]
    fn cursor_utf8() {
        let mut c = Cursor::new("aéb");
        assert_eq!(c.advance(), Some('a'));
        assert_eq!(c.advance(), Some('é'));
        assert_eq!(c.pos(), 3); // 'é' is 2 bytes
        assert_eq!(c.advance(), Some('b'));
    }
}
