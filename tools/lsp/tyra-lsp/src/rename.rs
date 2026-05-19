use tyra_diagnostics::{SourceId, Span};

use crate::keywords::TYRA_KEYWORDS;

/// Return true if `name` is a valid Tyra identifier and not a reserved keyword.
///
/// Valid identifiers: `[a-zA-Z_][a-zA-Z0-9_]*`.
pub(crate) fn is_valid_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    !TYRA_KEYWORDS.contains(&name)
}

/// Find the exact span of the binding identifier within a statement-level
/// `def_span`.
///
/// `def_span` covers the entire declaration (e.g. `let x: Int = 1`).
/// We scan left-to-right through the def-span text looking for the first
/// occurrence of `old_name` that is surrounded by non-identifier characters
/// (word boundary).  For all supported symbol kinds the binding name is the
/// first such occurrence:
///   - `let x: T = …`         → `x`
///   - `fn foo(…) -> T … end` → `foo`
///   - `type Foo = …`         → `Foo`
///
/// Returns `None` if `old_name` is not found within the span (should not
/// happen for well-formed, resolved code).
pub(crate) fn find_binding_name_span(text: &str, def_span: Span, old_name: &str) -> Option<Span> {
    let start = def_span.start as usize;
    let end = def_span.end as usize;
    let slice = text.get(start..end)?;

    let name_len = old_name.len();
    let bytes = slice.as_bytes();

    let mut i = 0usize;
    while i + name_len <= bytes.len() {
        if &slice[i..i + name_len] == old_name {
            // Check word boundaries.
            let before_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            let after_ok = i + name_len == bytes.len() || !is_ident_char(bytes[i + name_len]);
            if before_ok && after_ok {
                let abs_start = (start + i) as u32;
                let abs_end = (start + i + name_len) as u32;
                return Some(Span::new(def_span.source, abs_start, abs_end));
            }
        }
        i += 1;
    }
    None
}

/// Scan `text` to find the byte range `[start, end)` of the identifier
/// overlapping `offset`. Returns `None` if `offset` is not on an identifier
/// character.
fn ident_byte_range(text: &str, offset: u32) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let off = offset as usize;
    if off >= bytes.len() || !is_ident_char(bytes[off]) {
        return None;
    }
    let mut start = off;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = off;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }
    Some((start, end))
}

/// Extract the byte span of the identifier overlapping `offset` in `text`.
/// Returns `None` if the byte at `offset` is not an identifier character.
pub(crate) fn extract_identifier_span_at(
    text: &str,
    source: SourceId,
    offset: u32,
) -> Option<Span> {
    let (start, end) = ident_byte_range(text, offset)?;
    Some(Span::new(source, start as u32, end as u32))
}

/// Extract the identifier text that overlaps `offset` in `text`.
///
/// Scans backwards from `offset` to find the start of the identifier, then
/// forwards to find the end. Returns `None` if the byte at `offset` is not
/// an identifier character.
pub(crate) fn extract_identifier_at(text: &str, offset: u32) -> Option<String> {
    let (start, end) = ident_byte_range(text, offset)?;
    Some(text[start..end].to_string())
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_diagnostics::SourceMap;

    #[test]
    fn extract_identifier_span_at_basic() {
        let mut sources = SourceMap::new();
        let src = "let foo = 1\n";
        let id = sources.add("t.tyra".into(), src.into());
        // 'f' is at byte 4
        let span = extract_identifier_span_at(src, id, 4).expect("expected span");
        assert_eq!(span.start, 4);
        assert_eq!(span.end, 7);
        // whitespace returns None
        assert!(extract_identifier_span_at(src, id, 3).is_none());
        // middle of identifier returns full span
        let span2 = extract_identifier_span_at(src, id, 5).expect("mid-ident");
        assert_eq!(span2.start, 4);
        assert_eq!(span2.end, 7);
    }

    #[test]
    fn is_valid_identifier_accepts_normal() {
        assert!(is_valid_identifier("foo"));
        assert!(is_valid_identifier("_x"));
        assert!(is_valid_identifier("x123"));
        assert!(is_valid_identifier("_"));
        assert!(is_valid_identifier("CamelCase"));
    }

    #[test]
    fn is_valid_identifier_rejects_keywords() {
        for kw in TYRA_KEYWORDS {
            assert!(
                !is_valid_identifier(kw),
                "keyword `{kw}` should be rejected"
            );
        }
    }

    #[test]
    fn is_valid_identifier_rejects_invalid() {
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("123x"));
        assert!(!is_valid_identifier("x-y"));
        assert!(!is_valid_identifier("$x"));
        assert!(!is_valid_identifier("x y"));
    }

    #[test]
    fn find_binding_name_span_let() {
        let src = "let x: Int = 1\n";
        let mut sources = SourceMap::new();
        let id = sources.add("t.tyra".into(), src.into());
        // def_span covers the whole statement: bytes 0..15
        let def_span = Span::new(id, 0, 15);
        let span = find_binding_name_span(src, def_span, "x").expect("should find x");
        // "let x" — 'x' is at byte 4
        assert_eq!(span.start, 4);
        assert_eq!(span.end, 5);
        assert_eq!(&src[span.start as usize..span.end as usize], "x");
    }

    #[test]
    fn find_binding_name_span_fn() {
        let src = "fn foo() -> Int\n  0\nend\n";
        let mut sources = SourceMap::new();
        let id = sources.add("t.tyra".into(), src.into());
        let def_span = Span::new(id, 0, src.len() as u32);
        let span = find_binding_name_span(src, def_span, "foo").expect("should find foo");
        // "fn foo" — 'foo' starts at byte 3
        assert_eq!(span.start, 3);
        assert_eq!(span.end, 6);
        assert_eq!(&src[span.start as usize..span.end as usize], "foo");
    }

    #[test]
    fn extract_identifier_at_basic() {
        let text = "let foo: Int = 1\n";
        // offset 4 = 'f' of 'foo'
        assert_eq!(extract_identifier_at(text, 4), Some("foo".into()));
        // offset 5 = 'o' (middle of 'foo')
        assert_eq!(extract_identifier_at(text, 5), Some("foo".into()));
        // offset 3 = ' ' (space)
        assert_eq!(extract_identifier_at(text, 3), None);
        // offset 0 = 'l' of 'let'
        assert_eq!(extract_identifier_at(text, 0), Some("let".into()));
    }

    #[test]
    fn find_binding_name_span_rejects_substring() {
        // "let xy: Int = 1" — looking for "x" should NOT match "xy"
        let src = "let xy: Int = 1\n";
        let mut sources = SourceMap::new();
        let id = sources.add("t.tyra".into(), src.into());
        let def_span = Span::new(id, 0, src.len() as u32);
        assert!(find_binding_name_span(src, def_span, "x").is_none());
    }
}
