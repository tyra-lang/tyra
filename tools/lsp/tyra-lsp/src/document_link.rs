use std::path::Path;

use tower_lsp::lsp_types::{DocumentLink, Url};
use tyra_ast::{Item, SourceFile};
use tyra_diagnostics::{SourceMap, Span};

use crate::span_to_lsp_range;

/// Collect document links for all `import` statements in `ast` whose module
/// files can be resolved relative to `main_dir`.  Built-in modules and imports
/// whose files do not exist on disk are silently skipped.
pub(crate) fn collect(
    ast: &SourceFile,
    text: &str,
    sources: &SourceMap,
    main_dir: Option<&Path>,
) -> Vec<DocumentLink> {
    let Some(dir) = main_dir else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in &ast.items {
        if let Item::Import(imp) = item {
            let Some(file_path) = tyra_driver::resolve_import_file(dir, &imp.path) else {
                continue;
            };
            let Ok(target) = Url::from_file_path(&file_path) else {
                continue;
            };
            // Verify the span actually contains `import <path>` text.
            // auto_import_stdlib injects synthetic ImportDecls that reuse
            // another item's span; those would produce links over unrelated
            // source tokens, so we skip them here.
            let Some(path_span) = path_token_span(text, imp.span) else {
                continue;
            };
            let expected = imp.path.join(".");
            let span_text = text
                .get(path_span.start as usize..path_span.end as usize)
                .unwrap_or("");
            if span_text != expected {
                continue;
            }
            out.push(DocumentLink {
                range: span_to_lsp_range(path_span, sources),
                target: Some(target),
                tooltip: Some(format!("Open module `{}`", imp.path.join("."))),
                data: None,
            });
        }
    }
    out
}

/// Within `[span.start, span.end)` in `text`, locate the dotted module path
/// token that follows the `import` keyword and optional whitespace.
/// Stops at whitespace, `\n`, or the `as` keyword.
pub(crate) fn path_token_span(text: &str, span: Span) -> Option<Span> {
    let start = span.start as usize;
    let end = (span.end as usize).min(text.len());
    let slice = &text[start..end];

    // Skip `import` (7 bytes including the trailing space minimum)
    let after_kw = slice.strip_prefix("import")?;
    // Skip whitespace between `import` and the path
    let trimmed = after_kw.trim_start_matches([' ', '\t']);
    let ws_len = after_kw.len() - trimmed.len();
    let path_start = start + "import".len() + ws_len;

    // Scan forward while character is valid in a dotted module path
    let path_bytes = trimmed.as_bytes();
    let mut len = 0;
    while len < path_bytes.len() {
        let b = path_bytes[len];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' {
            len += 1;
        } else {
            break;
        }
    }
    // Strip trailing dot (shouldn't occur in valid source, but be safe)
    while len > 0 && path_bytes[len - 1] == b'.' {
        len -= 1;
    }
    if len == 0 {
        return None;
    }
    Some(Span::new(
        span.source,
        path_start as u32,
        (path_start + len) as u32,
    ))
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run_collect(src: &str, main_dir: Option<&Path>) -> Vec<DocumentLink> {
        let result = tyra_driver::check_in_memory("test.tyra".to_string(), src.to_string(), None);
        collect(&result.ast, src, &result.sources, main_dir)
    }

    fn make_tmpdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collect_returns_link_for_local_import() {
        let dir = make_tmpdir("tyra_dl_test_local");
        std::fs::write(dir.join("math.tyra"), "").unwrap();

        let src = "import math\n";
        let links = run_collect(src, Some(&dir));
        assert_eq!(links.len(), 1, "expected 1 link, got: {links:?}");
        let target = links[0].target.as_ref().expect("expected target URL");
        assert!(
            target.path().ends_with("math.tyra"),
            "expected math.tyra in target: {target}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_skips_builtin_modules() {
        let dir = make_tmpdir("tyra_dl_test_builtin");
        let src = "import core.sys\n";
        let links = run_collect(src, Some(&dir));
        assert!(
            links.is_empty(),
            "core.sys is builtin, expected no links: {links:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_skips_unresolvable_imports() {
        let dir = make_tmpdir("tyra_dl_test_noresolve");
        let src = "import nonexistent\n";
        let links = run_collect(src, Some(&dir));
        assert!(
            links.is_empty(),
            "expected no links for missing module: {links:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_skips_auto_injected_stdlib_imports() {
        // string.trim() usage triggers auto_import_stdlib to inject
        // `import string` synthetically.  Because there is no `import string`
        // token in the source, no document link should be produced even if
        // a string.tyra file happens to be resolvable.
        let dir = make_tmpdir("tyra_dl_test_autoimport");
        // Create a resolvable string.tyra so the synthetic import would be
        // linkable if we did not filter it.
        std::fs::write(dir.join("string.tyra"), "").unwrap();
        let src = "fn main() -> Unit\n  let s = \"hello\"\n  let _ = s.trim()\nend\n";
        let links = run_collect(src, Some(&dir));
        assert!(
            links.is_empty(),
            "synthetic auto-imports should not produce document links: {links:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
