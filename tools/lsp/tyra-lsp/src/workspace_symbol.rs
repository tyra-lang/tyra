use std::collections::HashMap;

use tower_lsp::lsp_types::{DocumentSymbol, Location, SymbolInformation, Url};

use crate::DocState;

/// Collect all symbols from open documents that match `query`
/// (case-insensitive substring; empty query returns all symbols).
pub(crate) fn collect(query: &str, docs: &HashMap<Url, DocState>) -> Vec<SymbolInformation> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    for (uri, state) in docs {
        let symbols = crate::outline::build_document_symbols(
            state.source_id,
            &state.ast,
            &state.sources,
        );
        flatten(&symbols, uri, None, &q, &mut out);
    }
    out
}

fn flatten(
    symbols: &[DocumentSymbol],
    uri: &Url,
    container: Option<&str>,
    q: &str,
    out: &mut Vec<SymbolInformation>,
) {
    for s in symbols {
        if q.is_empty() || s.name.to_lowercase().contains(q) {
            #[allow(deprecated)]
            out.push(SymbolInformation {
                name: s.name.clone(),
                kind: s.kind,
                tags: s.tags.clone(),
                deprecated: None,
                location: Location { uri: uri.clone(), range: s.selection_range },
                container_name: container.map(|c| c.to_string()),
            });
        }
        if let Some(children) = &s.children {
            flatten(children, uri, Some(&s.name), q, out);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_docs(uri: &str, src: &str) -> HashMap<Url, DocState> {
        let result =
            tyra_driver::check_in_memory("test.tyra".to_string(), src.to_string(), None);
        let state = crate::DocState {
            text: src.to_string(),
            sources: result.sources,
            type_index: result.type_index,
            def_index: result.def_index,
            symbols: result.symbols,
            source_id: result.source_id,
            ast: result.ast,
            diagnostics: vec![],
        };
        let url = Url::parse(uri).unwrap();
        let mut map = HashMap::new();
        map.insert(url, state);
        map
    }

    #[test]
    fn empty_query_returns_all_symbols() {
        let src = "fn foo()\nend\nfn bar()\nend\n";
        let docs = make_docs("file:///tmp/a.tyra", src);
        let syms = collect("", &docs);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"foo"), "expected 'foo' in: {names:?}");
        assert!(names.contains(&"bar"), "expected 'bar' in: {names:?}");
    }

    #[test]
    fn query_filters_case_insensitive_substring() {
        let src = "fn greet()\nend\nfn foo()\nend\n";
        let docs = make_docs("file:///tmp/b.tyra", src);
        let syms = collect("GREE", &docs);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"greet"), "expected 'greet' in: {names:?}");
        assert!(!names.contains(&"foo"), "expected 'foo' excluded, got: {names:?}");
    }
}
