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

    /// Convert an LSP `Position` (0-based line, UTF-16 `character`) to a byte offset.
    ///
    /// Per LSP 3.17 §3.15, `Position.character` counts UTF-16 code units.
    /// BMP characters (U+0000–U+FFFF) count as 1; supplementary characters
    /// (e.g. emoji) encoded as surrogate pairs count as 2.
    ///
    /// Returns `None` when `line` is out of range.  If `character` falls in
    /// the middle of a surrogate pair the function snaps to the start of that
    /// character (matches common LSP server practice).
    pub fn offset_at_utf16(&self, id: SourceId, line: u32, character: u32) -> Option<u32> {
        let file = &self.files[id.index()];
        let line_start = *file.line_starts.get(line as usize)? as usize;
        let line_end = file
            .line_starts
            .get(line as usize + 1)
            .map(|&n| n as usize)
            .unwrap_or(file.content.len());
        // Strip trailing line endings so that past-EOL positions clamp to the
        // last visible character, not into '\n' / '\r\n' or the next line start.
        let line_text = &file.content[line_start..line_end];
        let line_visible = line_text.trim_end_matches(['\r', '\n']);
        let visible_end = line_start + line_visible.len();

        let mut utf16_seen: u32 = 0;
        for (byte_off, ch) in line_visible.char_indices() {
            if utf16_seen == character {
                return Some((line_start + byte_off) as u32);
            }
            let units = ch.len_utf16() as u32;
            if utf16_seen + units > character {
                // character lands in the middle of a surrogate pair; snap to start.
                return Some((line_start + byte_off) as u32);
            }
            utf16_seen += units;
        }
        // character is at or past end of visible text; clamp to visible_end.
        Some(visible_end as u32)
    }

    /// Convert a byte offset to `(line, utf16_character)`, both 0-based, for LSP.
    ///
    /// `utf16_character` counts UTF-16 code units from the start of the line,
    /// matching the value expected in `Position.character`.
    ///
    /// Returns `None` when `offset` is beyond the file length.
    pub fn line_col_utf16(&self, id: SourceId, offset: u32) -> Option<(u32, u32)> {
        let file = &self.files[id.index()];
        let off = offset as usize;
        if off > file.content.len() {
            return None;
        }
        let line_idx = file
            .line_starts
            .partition_point(|&start| start as usize <= off)
            .saturating_sub(1);
        let line_start = file.line_starts[line_idx] as usize;
        let utf16_col: u32 = file.content[line_start..off]
            .chars()
            .map(|c| c.len_utf16() as u32)
            .sum();
        Some((line_idx as u32, utf16_col))
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
        let id = map.add("test.ty".into(), "hello\nworld\n".into());
        assert_eq!(map.name(id), "test.ty");
        assert_eq!(map.content(id), "hello\nworld\n");
    }

    #[test]
    fn line_col_first_line() {
        let mut map = SourceMap::new();
        let id = map.add("test.ty".into(), "let x = 10\nlet y = 20\n".into());
        assert_eq!(map.line_col(id, 0), (1, 1));
        assert_eq!(map.line_col(id, 4), (1, 5));
    }

    #[test]
    fn line_col_second_line() {
        let mut map = SourceMap::new();
        let id = map.add("test.ty".into(), "let x = 10\nlet y = 20\n".into());
        // "let x = 10\n" is 11 bytes, so line 2 starts at offset 11
        assert_eq!(map.line_col(id, 11), (2, 1));
        assert_eq!(map.line_col(id, 15), (2, 5));
    }

    #[test]
    fn line_content_lookup() {
        let mut map = SourceMap::new();
        let id = map.add("test.ty".into(), "first\nsecond\nthird\n".into());
        assert_eq!(map.line_content(id, 1), "first");
        assert_eq!(map.line_content(id, 2), "second");
        assert_eq!(map.line_content(id, 3), "third");
    }

    #[test]
    fn slice_extraction() {
        let mut map = SourceMap::new();
        let id = map.add("test.ty".into(), "hello, tyra".into());
        assert_eq!(map.slice(id, 0, 5), "hello");
        assert_eq!(map.slice(id, 7, 11), "tyra");
    }

    #[test]
    fn offset_at_basics() {
        let mut map = SourceMap::new();
        // "let x = 1\nlet y = 2\n" → line_starts = [0, 10, 20], content.len() = 20
        let id = map.add("t.ty".into(), "let x = 1\nlet y = 2\n".into());
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
        let id = map.add("empty.ty".into(), "".into());
        assert_eq!(map.line_col(id, 0), (1, 1));
        assert_eq!(map.content(id), "");
    }

    #[test]
    fn no_trailing_newline() {
        let mut map = SourceMap::new();
        let id = map.add("test.ty".into(), "let x = 10".into());
        assert_eq!(map.line_content(id, 1), "let x = 10");
        assert_eq!(map.line_col(id, 0), (1, 1));
        assert_eq!(map.line_col(id, 10), (1, 11));
    }

    #[test]
    fn windows_line_endings() {
        let mut map = SourceMap::new();
        let id = map.add("win.ty".into(), "first\r\nsecond\r\n".into());
        assert_eq!(map.line_content(id, 1), "first");
        assert_eq!(map.line_content(id, 2), "second");
    }

    // ── UTF-16 position tests ──────────────────────────────────────────────

    #[test]
    fn utf16_ascii_roundtrip() {
        // ASCII content: UTF-16 code units == UTF-8 bytes.
        let mut map = SourceMap::new();
        let id = map.add("t.ty".into(), "let x = 1\nlet y = 2\n".into());
        assert_eq!(map.offset_at_utf16(id, 0, 0), Some(0));
        assert_eq!(map.offset_at_utf16(id, 0, 4), Some(4));
        assert_eq!(map.offset_at_utf16(id, 1, 0), Some(10));
        assert_eq!(map.line_col_utf16(id, 0), Some((0, 0)));
        assert_eq!(map.line_col_utf16(id, 4), Some((0, 4)));
        assert_eq!(map.line_col_utf16(id, 10), Some((1, 0)));
    }

    #[test]
    fn utf16_japanese_identifiers() {
        // 名 = U+540D: 3 UTF-8 bytes, 1 UTF-16 code unit (BMP)
        // "ab名cd\n" → byte offsets: a=0 b=1 名=2,3,4 c=5 d=6 \n=7
        let mut map = SourceMap::new();
        let id = map.add("t.ty".into(), "ab名cd\n".into());

        // UTF-16 col 2 → byte offset 2 (start of '名')
        assert_eq!(map.offset_at_utf16(id, 0, 2), Some(2));
        // UTF-16 col 3 → byte offset 5 ('c', one UTF-16 unit past '名')
        assert_eq!(map.offset_at_utf16(id, 0, 3), Some(5));
        // UTF-16 col 4 → byte offset 6 ('d')
        assert_eq!(map.offset_at_utf16(id, 0, 4), Some(6));

        // Reverse: byte 2 → (line 0, utf16_col 2)
        assert_eq!(map.line_col_utf16(id, 2), Some((0, 2)));
        // byte 5 → (line 0, utf16_col 3)
        assert_eq!(map.line_col_utf16(id, 5), Some((0, 3)));
    }

    #[test]
    fn utf16_emoji_surrogate_pair() {
        // 🚀 = U+1F680: 4 UTF-8 bytes, 2 UTF-16 code units (surrogate pair)
        // "a🚀b\n" → bytes: a=0, 🚀=1,2,3,4, b=5, \n=6
        let mut map = SourceMap::new();
        let id = map.add("t.ty".into(), "a🚀b\n".into());

        // UTF-16 col 0 → byte 0 ('a')
        assert_eq!(map.offset_at_utf16(id, 0, 0), Some(0));
        // UTF-16 col 1 → byte 1 (start of '🚀')
        assert_eq!(map.offset_at_utf16(id, 0, 1), Some(1));
        // UTF-16 col 2 → byte 1 (mid-surrogate: snaps to start of '🚀')
        assert_eq!(map.offset_at_utf16(id, 0, 2), Some(1));
        // UTF-16 col 3 → byte 5 ('b')
        assert_eq!(map.offset_at_utf16(id, 0, 3), Some(5));

        // Reverse: byte 1 → (line 0, utf16_col 1)
        assert_eq!(map.line_col_utf16(id, 1), Some((0, 1)));
        // byte 5 → (line 0, utf16_col 3)  (🚀 contributes 2 units)
        assert_eq!(map.line_col_utf16(id, 5), Some((0, 3)));
    }

    #[test]
    fn utf16_overlong_character_clamps_to_visible_end() {
        // An LSP client may send a character value past the line length after
        // a stale edit.  The result should clamp to the end of the visible
        // text (before '\n' / '\r\n'), never into the line ending or the next
        // line.
        let mut map = SourceMap::new();
        let id = map.add("t.ty".into(), "abc\ndef\n".into());
        // Line 0: "abc\n" — visible = "abc" (3 UTF-16 units, bytes 0-2).
        // character=10 → clamp to byte 3 (one past 'c', before '\n').
        assert_eq!(map.offset_at_utf16(id, 0, 10), Some(3));
        // Line 1: "def\n" — visible = "def" (bytes 4-6), visible_end = 7.
        assert_eq!(map.offset_at_utf16(id, 1, 10), Some(7));

        // CRLF line endings: '\r\n' must both be excluded.
        let id2 = map.add("win.ty".into(), "hi\r\nbye\r\n".into());
        // Line 0: "hi\r\n" — visible = "hi" (bytes 0-1), visible_end = 2.
        assert_eq!(map.offset_at_utf16(id2, 0, 99), Some(2));
    }

    #[test]
    fn utf16_multiline_mixed() {
        // line 0: "hello\n"  (6 bytes)
        // line 1: "名前\n"   (7 bytes: 名=3, 前=3, \n=1)
        // line 2: "ok\n"     (3 bytes)
        let src = "hello\n名前\nok\n";
        let mut map = SourceMap::new();
        let id = map.add("t.ty".into(), src.into());

        // line 1, UTF-16 col 0 → byte 6 (start of '名')
        assert_eq!(map.offset_at_utf16(id, 1, 0), Some(6));
        // line 1, UTF-16 col 1 → byte 9 (start of '前')
        assert_eq!(map.offset_at_utf16(id, 1, 1), Some(9));
        // line 2, UTF-16 col 1 → byte 13 + 1 = 14 ('k')
        assert_eq!(map.offset_at_utf16(id, 2, 1), Some(14));

        // Reverse: byte 9 → (line 1, utf16_col 1)
        assert_eq!(map.line_col_utf16(id, 9), Some((1, 1)));
        // byte 14 → (line 2, utf16_col 1)
        assert_eq!(map.line_col_utf16(id, 14), Some((2, 1)));
    }
}
