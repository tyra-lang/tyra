use std::collections::HashMap;
use std::path::PathBuf;

use tower_lsp::lsp_types::{FileDelete, Position, Range, TextEdit, Url, WorkspaceEdit};
use tyra_ast::Item;
use tyra_diagnostics::Span;

use crate::DocState;
use crate::document_link::path_token_span;
use crate::file_rename::module_segments_for;

/// For each open document, produce `TextEdit`s that delete the entire import
/// line for any `import <module>` statement that resolves to a deleted file.
///
/// Module paths are resolved relative to each importing document's own
/// directory. Synthetic auto-imports are excluded via span-text equality check.
pub(crate) fn compute_edits(
    docs: &HashMap<Url, DocState>,
    files: &[FileDelete],
) -> Option<WorkspaceEdit> {
    let deleted: Vec<PathBuf> = files
        .iter()
        .filter_map(|f| Url::parse(&f.uri).ok()?.to_file_path().ok())
        .collect();
    if deleted.is_empty() {
        return None;
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for (uri, state) in docs.iter() {
        let Some(main_dir) = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        else {
            continue;
        };

        for item in &state.ast.items {
            let Item::Import(imp) = item else { continue };
            let matches_deleted = deleted.iter().any(|p| {
                module_segments_for(&main_dir, p)
                    .map(|segs| segs == imp.path)
                    .unwrap_or(false)
            });
            if !matches_deleted {
                continue;
            }
            // Skip synthetic auto-imports: their spans point to non-import tokens.
            let Some(span) = path_token_span(&state.text, imp.span) else {
                continue;
            };
            let actual = state
                .text
                .get(span.start as usize..span.end as usize)
                .unwrap_or("");
            if actual != imp.path.join(".") {
                continue;
            }
            // Delete the entire line (including trailing newline).
            changes.entry(uri.clone()).or_default().push(TextEdit {
                range: line_range_for(&state.text, imp.span),
                new_text: String::new(),
            });
        }
    }
    if changes.is_empty() {
        return None;
    }
    Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

/// Return an LSP `Range` covering the entire line that `span.start` belongs to,
/// including the trailing `\n`.  When there is no trailing newline (EOF),
/// the range ends at EOF.
///
/// We anchor on `span.start` only because the parser may include a trailing
/// newline in `span.end`, which would cause the range to bleed into the next line.
fn line_range_for(text: &str, span: Span) -> Range {
    let bytes = text.as_bytes();
    // Walk back from span.start to find the beginning of this line.
    let mut start = span.start as usize;
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    // Walk forward from line start to find the end of this line, consuming \n.
    let mut end = start;
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    if end < bytes.len() {
        end += 1; // consume \n
    }
    Range {
        start: byte_to_position(text, start),
        end: byte_to_position(text, end),
    }
}

/// Convert a byte offset to an LSP `Position` (line / UTF-16 character).
/// Import lines are ASCII-only in Tyra, so UTF-8 and UTF-16 coincide here.
fn byte_to_position(text: &str, offset: usize) -> Position {
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    for (i, b) in text.bytes().enumerate() {
        if i == offset {
            break;
        }
        if b == b'\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position::new(line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file_delete(path: &str) -> FileDelete {
        FileDelete {
            uri: Url::from_file_path(path).unwrap().to_string(),
        }
    }

    fn make_doc(src: &str) -> (Url, DocState) {
        let result = tyra_driver::check_in_memory("main.tyra".to_string(), src.to_string(), None);
        let uri = Url::from_file_path("/workspace/main.tyra").unwrap();
        let state = DocState {
            text: src.to_string(),
            sources: result.sources,
            type_index: result.type_index,
            def_index: result.def_index,
            symbols: result.symbols,
            source_id: result.source_id,
            ast: result.ast,
            diagnostics: vec![],
            version: 0,
        };
        (uri, state)
    }

    #[test]
    fn file_delete_emits_edit_for_deleted_module() {
        let src = "import math\nfn main() -> Unit\n  let _ = 1\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), state);

        let f = make_file_delete("/workspace/math.tyra");
        let edit = compute_edits(&docs, &[f]).expect("expected WorkspaceEdit");
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).expect("expected edits for main.tyra");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "");
        // Range: line 0, col 0 → line 1, col 0 (the full "import math\n")
        assert_eq!(edits[0].range.start, Position::new(0, 0));
        assert_eq!(edits[0].range.end, Position::new(1, 0));
    }

    #[test]
    fn file_delete_handles_nested_path() {
        let src = "import core.foo\nfn main() -> Unit\n  let _ = 1\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), state);

        let f = make_file_delete("/workspace/core/foo.tyra");
        let edit = compute_edits(&docs, &[f]).expect("expected WorkspaceEdit");
        let edits = &edit.changes.unwrap()[&uri];
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "");
        assert_eq!(edits[0].range.start, Position::new(0, 0));
        assert_eq!(edits[0].range.end, Position::new(1, 0));
    }

    #[test]
    fn file_delete_skips_unrelated_imports() {
        let src = "import other\nfn main() -> Unit\n  let _ = 1\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri, state);

        let f = make_file_delete("/workspace/math.tyra");
        assert!(compute_edits(&docs, &[f]).is_none());
    }

    #[test]
    fn file_delete_skips_synthetic_auto_imports() {
        // string.trim() causes auto_import_stdlib to inject a synthetic `import string`.
        // That synthetic import's span does not point at an actual `import string` token,
        // so the span-text check filters it out — preventing deletion of the `s.trim()` line.
        let src = "fn main() -> Unit\n  let s = \"hello\"\n  let _ = s.trim()\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri, state);

        let f = make_file_delete("/workspace/string.tyra");
        assert!(compute_edits(&docs, &[f]).is_none());
    }

    #[test]
    fn file_delete_handles_last_line_no_trailing_newline() {
        // `import math` is the only content, with no trailing newline.
        let src = "import math";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), state);

        let f = make_file_delete("/workspace/math.tyra");
        let edit = compute_edits(&docs, &[f]).expect("expected WorkspaceEdit");
        let edits = &edit.changes.unwrap()[&uri];
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "");
        // No trailing newline — range ends at EOF (line 0, col 11)
        assert_eq!(edits[0].range.start, Position::new(0, 0));
        assert_eq!(edits[0].range.end, Position::new(0, 11));
    }

    #[test]
    fn file_delete_skips_outside_importer_dir() {
        // Renamed file is outside /workspace, so module_segments_for returns None.
        let src = "import math\nfn main() -> Unit\n  let _ = 1\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri, state);

        let f = make_file_delete("/other/math.tyra");
        assert!(compute_edits(&docs, &[f]).is_none());
    }
}
