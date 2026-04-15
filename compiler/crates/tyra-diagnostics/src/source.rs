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
