// SourceMap manages source files loaded into the compiler.
// Each file gets a SourceId for compact Span references.

/// Opaque identifier for a source file.
/// Constructed only via `SourceMap::add`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceId(u32);

impl SourceId {
    /// For testing only. Production code should use `SourceMap::add`.
    #[cfg(test)]
    pub(crate) fn test(id: u32) -> Self {
        Self(id)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// A single loaded source file.
#[derive(Debug)]
struct SourceFile {
    name: String,
    content: String,
    /// Byte offsets of each line start, for line/column lookup.
    line_starts: Vec<u32>,
}

impl SourceFile {
    fn new(name: String, content: String) -> Self {
        let line_starts = std::iter::once(0)
            .chain(
                content
                    .bytes()
                    .enumerate()
                    .filter(|(_, b)| *b == b'\n')
                    .map(|(i, _)| (i + 1) as u32),
            )
            .collect();
        Self {
            name,
            content,
            line_starts,
        }
    }

    /// Convert a byte offset to (line, column), both 1-based.
    fn line_col(&self, offset: u32) -> (u32, u32) {
        let line_idx = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        let line = (line_idx + 1) as u32;
        let col = (offset - self.line_starts[line_idx]) + 1;
        (line, col)
    }
}

/// Registry of all source files.
#[derive(Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Add a source file and return its ID.
    pub fn add(&mut self, name: String, content: String) -> SourceId {
        let id = SourceId(self.files.len() as u32);
        self.files.push(SourceFile::new(name, content));
        id
    }

    pub fn name(&self, id: SourceId) -> &str {
        &self.files[id.index()].name
    }

    pub fn content(&self, id: SourceId) -> &str {
        &self.files[id.index()].content
    }

    /// Get a slice of source text for a span.
    pub fn slice(&self, id: SourceId, start: u32, end: u32) -> &str {
        &self.files[id.index()].content[start as usize..end as usize]
    }

    /// Convert a byte offset to (line, column), both 1-based.
    pub fn line_col(&self, id: SourceId, offset: u32) -> (u32, u32) {
        self.files[id.index()].line_col(offset)
    }

    /// Convert an LSP `Position` (0-based line/col) to a byte offset.
    ///
    /// `col` is treated as a UTF-8 byte column (identical to UTF-16 for
    /// ASCII-only source). Returns `None` when `line` or `col` is out of range.
    ///
    /// # Known limitation
    /// Non-ASCII characters before the cursor position will produce an incorrect
    /// byte offset because LSP uses UTF-16 code units while this function uses
    /// byte indices. Tyra source files are expected to be ASCII for identifiers;
    /// the only non-ASCII content is inside string literals, which are not
    /// hover targets.
    pub fn offset_at(&self, id: SourceId, line: u32, col: u32) -> Option<u32> {
        let file = &self.files[id.index()];
        let line_idx = line as usize;
        let line_start = *file.line_starts.get(line_idx)? as usize;
        let col_bytes = col as usize;
        let offset = line_start + col_bytes;
        if offset <= file.content.len() {
            Some(offset as u32)
        } else {
            None
        }
    }

    /// Get the content of a specific line (1-based), without trailing line endings.
    pub fn line_content(&self, id: SourceId, line: u32) -> &str {
        let file = &self.files[id.index()];
        let line_idx = (line - 1) as usize;
        let start = file.line_starts[line_idx] as usize;
        let end = file
            .line_starts
            .get(line_idx + 1)
            .map(|&s| s as usize)
            .unwrap_or(file.content.len());
        file.content[start..end].trim_end_matches(&['\r', '\n'] as &[char])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_map_basics() {
        let mut map = SourceMap::new();
        let id = map.add("test.tyra".into(), "hello\nworld\n".into());
        assert_eq!(map.name(id), "test.tyra");
        assert_eq!(map.content(id), "hello\nworld\n");
    }

    #[test]
    fn line_col_first_line() {
        let mut map = SourceMap::new();
        let id = map.add("test.tyra".into(), "let x = 10\nlet y = 20\n".into());
        assert_eq!(map.line_col(id, 0), (1, 1));
        assert_eq!(map.line_col(id, 4), (1, 5));
    }

    #[test]
    fn line_col_second_line() {
        let mut map = SourceMap::new();
        let id = map.add("test.tyra".into(), "let x = 10\nlet y = 20\n".into());
        // "let x = 10\n" is 11 bytes, so line 2 starts at offset 11
        assert_eq!(map.line_col(id, 11), (2, 1));
        assert_eq!(map.line_col(id, 15), (2, 5));
    }

    #[test]
    fn line_content_lookup() {
        let mut map = SourceMap::new();
        let id = map.add("test.tyra".into(), "first\nsecond\nthird\n".into());
        assert_eq!(map.line_content(id, 1), "first");
        assert_eq!(map.line_content(id, 2), "second");
        assert_eq!(map.line_content(id, 3), "third");
    }

    #[test]
    fn slice_extraction() {
        let mut map = SourceMap::new();
        let id = map.add("test.tyra".into(), "hello, tyra".into());
        assert_eq!(map.slice(id, 0, 5), "hello");
        assert_eq!(map.slice(id, 7, 11), "tyra");
    }

    #[test]
    fn offset_at_basics() {
        let mut map = SourceMap::new();
        // "let x = 1\nlet y = 2\n" → line_starts = [0, 10, 20], content.len() = 20
        let id = map.add("t.tyra".into(), "let x = 1\nlet y = 2\n".into());
        // line 0 col 0 → byte 0
        assert_eq!(map.offset_at(id, 0, 0), Some(0));
        // line 0 col 4 → byte 4 ("x" is at col 4)
        assert_eq!(map.offset_at(id, 0, 4), Some(4));
        // line 1 col 0 → byte 10 (after the '\n')
        assert_eq!(map.offset_at(id, 1, 0), Some(10));
        // line 2 col 0 → byte 20 (EOF position — valid)
        assert_eq!(map.offset_at(id, 2, 0), Some(20));
        // line 3 is beyond the last line_start → None
        assert_eq!(map.offset_at(id, 3, 0), None);
    }

    #[test]
    fn empty_file() {
        let mut map = SourceMap::new();
        let id = map.add("empty.tyra".into(), "".into());
        assert_eq!(map.line_col(id, 0), (1, 1));
        assert_eq!(map.content(id), "");
    }

    #[test]
    fn no_trailing_newline() {
        let mut map = SourceMap::new();
        let id = map.add("test.tyra".into(), "let x = 10".into());
        assert_eq!(map.line_content(id, 1), "let x = 10");
        assert_eq!(map.line_col(id, 0), (1, 1));
        assert_eq!(map.line_col(id, 10), (1, 11));
    }

    #[test]
    fn windows_line_endings() {
        let mut map = SourceMap::new();
        let id = map.add("win.tyra".into(), "first\r\nsecond\r\n".into());
        assert_eq!(map.line_content(id, 1), "first");
        assert_eq!(map.line_content(id, 2), "second");
    }
}
