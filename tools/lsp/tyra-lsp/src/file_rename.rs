use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::{FileRename, TextEdit, Url, WorkspaceEdit};
use tyra_ast::Item;

use crate::document_link::path_token_span;
use crate::{DocState, span_to_lsp_range};

/// Convert `<base>/a/b/c.ty` to `["a", "b", "c"]`.
/// Returns `None` if the path is not under `base` or the extension is not `tyra`.
pub(crate) fn module_segments_for(base: &Path, file: &Path) -> Option<Vec<String>> {
    let rel = file.strip_prefix(base).ok()?;
    if rel.extension()?.to_str()? != "ty" {
        return None;
    }
    let mut segs: Vec<String> = rel
        .iter()
        .map(|os| os.to_string_lossy().into_owned())
        .collect();
    let last = segs.last_mut()?;
    *last = last.strip_suffix(".ty")?.to_string();
    if segs.iter().any(|s| s.is_empty()) {
        return None;
    }
    Some(segs)
}

/// For each open document, produce `TextEdit`s rewriting any `import <old>`
/// statement to `import <new>` for each entry in `renames`.
///
/// Module paths are resolved relative to each importing document's own directory,
/// matching Tyra's `main_dir`-relative import semantics. Only currently-open
/// documents are updated. Synthetic auto-imports (span text mismatch) are skipped.
pub(crate) fn compute_edits(
    docs: &HashMap<Url, DocState>,
    renames: &[FileRename],
) -> Option<WorkspaceEdit> {
    let rename_paths: Vec<(PathBuf, PathBuf)> = renames
        .iter()
        .filter_map(|r| {
            let old = Url::parse(&r.old_uri).ok()?.to_file_path().ok()?;
            let new = Url::parse(&r.new_uri).ok()?.to_file_path().ok()?;
            Some((old, new))
        })
        .collect();
    if rename_paths.is_empty() {
        return None;
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for (uri, state) in docs.iter() {
        // Resolve imports relative to the importing file's directory.
        let Some(main_dir) = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        else {
            continue;
        };

        for item in &state.ast.items {
            let Item::Import(imp) = item else { continue };
            // Find a rename pair whose old path resolves to this import's module
            // path from the importer's directory.
            let new_dotted = rename_paths.iter().find_map(|(old, new)| {
                let o = module_segments_for(&main_dir, old)?;
                let n = module_segments_for(&main_dir, new)?;
                if o == imp.path && o != n {
                    Some(n.join("."))
                } else {
                    None
                }
            });
            let Some(new_dotted) = new_dotted else {
                continue;
            };
            let Some(span) = path_token_span(&state.text, imp.span) else {
                continue;
            };
            // Skip synthetic auto-imports: their spans point to non-import tokens.
            let actual = state
                .text
                .get(span.start as usize..span.end as usize)
                .unwrap_or("");
            if actual != imp.path.join(".") {
                continue;
            }
            changes.entry(uri.clone()).or_default().push(TextEdit {
                range: span_to_lsp_range(span, &state.sources),
                new_text: new_dotted,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file_rename(old: &str, new: &str) -> FileRename {
        FileRename {
            old_uri: Url::from_file_path(old).unwrap().to_string(),
            new_uri: Url::from_file_path(new).unwrap().to_string(),
        }
    }

    fn make_doc(src: &str) -> (Url, DocState) {
        let result = tyra_driver::check_in_memory("main.ty".to_string(), src.to_string(), None);
        let uri = Url::from_file_path("/workspace/main.ty").unwrap();
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
    fn module_segments_for_basic() {
        let base = PathBuf::from("/workspace");
        assert_eq!(
            module_segments_for(&base, Path::new("/workspace/a/b/c.ty")),
            Some(vec!["a".into(), "b".into(), "c".into()])
        );
        assert_eq!(
            module_segments_for(&base, Path::new("/workspace/math.ty")),
            Some(vec!["math".into()])
        );
        // outside base
        assert!(module_segments_for(&base, Path::new("/other/math.ty")).is_none());
        // wrong extension
        assert!(module_segments_for(&base, Path::new("/workspace/foo.rs")).is_none());
    }

    #[test]
    fn file_rename_emits_edit_for_renamed_module() {
        let src = "import math\nfn main() -> Unit\n  let _ = 1\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), state);

        let r = make_file_rename("/workspace/math.ty", "/workspace/mathx.ty");
        let edit = compute_edits(&docs, &[r]).expect("expected WorkspaceEdit");
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).expect("expected edits for main.ty");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "mathx");
        // Range covers the "math" token: starts at byte 7 (after "import ")
        assert_eq!(edits[0].range.start.character, 7);
    }

    #[test]
    fn file_rename_handles_nested_path() {
        let src = "import core.foo\nfn main() -> Unit\n  let _ = 1\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri.clone(), state);

        let r = make_file_rename("/workspace/core/foo.ty", "/workspace/core/bar.ty");
        let edit = compute_edits(&docs, &[r]).expect("expected WorkspaceEdit");
        let edits = &edit.changes.unwrap()[&uri];
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "core.bar");
    }

    #[test]
    fn file_rename_skips_unrelated_imports() {
        let src = "import other\nfn main() -> Unit\n  let _ = 1\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri, state);

        let r = make_file_rename("/workspace/math.ty", "/workspace/mathx.ty");
        assert!(compute_edits(&docs, &[r]).is_none());
    }

    #[test]
    fn file_rename_skips_outside_importer_dir() {
        // When the renamed file is not resolvable from the importer's directory,
        // no edit is produced. Here the importer is at /workspace/main.ty
        // (main_dir = /workspace) and the renamed file is at /other/math.ty —
        // outside /workspace, so module_segments_for returns None.
        let src = "import math\nfn main() -> Unit\n  let _ = 1\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri, state);

        let r = make_file_rename("/other/math.ty", "/other/mathx.ty");
        assert!(compute_edits(&docs, &[r]).is_none());
    }

    #[test]
    fn file_rename_skips_synthetic_auto_imports() {
        // string.trim() triggers auto_import_stdlib to inject `import string`
        // synthetically.  Because the span does not point to an `import string`
        // token, the span-text check filters it out.
        let src = "fn main() -> Unit\n  let s = \"hello\"\n  let _ = s.trim()\nend\n";
        let (uri, state) = make_doc(src);
        let mut docs = HashMap::new();
        docs.insert(uri, state);

        let r = make_file_rename("/workspace/string.ty", "/workspace/stringx.ty");
        assert!(compute_edits(&docs, &[r]).is_none());
    }
}
