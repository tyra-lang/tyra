use serde_json::json;
use tower_lsp::lsp_types::{SymbolKind, TypeHierarchyItem, Url};
use tyra_ast::{Item, SourceFile, TypeExprKind};
use tyra_diagnostics::{SourceMap, Span};

use crate::{rename, span_to_lsp_range};

pub(crate) fn prepare(
    ast: &SourceFile,
    text: &str,
    sources: &SourceMap,
    uri: &Url,
    offset: u32,
) -> Vec<TypeHierarchyItem> {
    for item in &ast.items {
        match item {
            Item::TraitDef(tr) => {
                if name_token_at(text, tr.span, &tr.name, offset) {
                    let name_span =
                        rename::find_binding_name_span(text, tr.span, &tr.name).unwrap_or(tr.span);
                    return vec![make_item(
                        uri,
                        tr.name.clone(),
                        SymbolKind::INTERFACE,
                        tr.span,
                        name_span,
                        sources,
                        "trait",
                    )];
                }
            }
            Item::ValueDef(v) => {
                if name_token_at(text, v.span, &v.name, offset) {
                    let name_span =
                        rename::find_binding_name_span(text, v.span, &v.name).unwrap_or(v.span);
                    return vec![make_item(
                        uri,
                        v.name.clone(),
                        SymbolKind::CLASS,
                        v.span,
                        name_span,
                        sources,
                        "concrete",
                    )];
                }
            }
            Item::DataDef(d) => {
                if name_token_at(text, d.span, &d.name, offset) {
                    let name_span =
                        rename::find_binding_name_span(text, d.span, &d.name).unwrap_or(d.span);
                    return vec![make_item(
                        uri,
                        d.name.clone(),
                        SymbolKind::STRUCT,
                        d.span,
                        name_span,
                        sources,
                        "concrete",
                    )];
                }
            }
            Item::TypeDef(t) => {
                if name_token_at(text, t.span, &t.name, offset) {
                    let name_span =
                        rename::find_binding_name_span(text, t.span, &t.name).unwrap_or(t.span);
                    return vec![make_item(
                        uri,
                        t.name.clone(),
                        SymbolKind::TYPE_PARAMETER,
                        t.span,
                        name_span,
                        sources,
                        "concrete",
                    )];
                }
            }
            Item::ImplDef(im) => {
                // Cursor on the trait name token of `impl Foo for X`
                if name_token_at(text, im.span, &im.trait_name, offset) {
                    if let Some(tr) = find_trait_def(ast, &im.trait_name) {
                        let name_span = rename::find_binding_name_span(text, tr.span, &tr.name)
                            .unwrap_or(tr.span);
                        return vec![make_item(
                            uri,
                            tr.name.clone(),
                            SymbolKind::INTERFACE,
                            tr.span,
                            name_span,
                            sources,
                            "trait",
                        )];
                    }
                }
                // Cursor on the target type token of `impl Foo for X`.
                // Use im.target_type.span (not im.span) so that a type name
                // that appears in trait type-args (e.g. `impl Into<X> for X`)
                // does not shadow the actual target position.
                if let TypeExprKind::Named(ref target) = im.target_type.kind {
                    if name_token_at(text, im.target_type.span, target, offset) {
                        // Find the original def for the target type
                        if let Some(result) = find_concrete_def(ast, text, sources, uri, target) {
                            return vec![result];
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Vec::new()
}

/// Supertypes: traits implemented by a concrete type. Always empty for traits.
pub(crate) fn supertypes(
    ast: &SourceFile,
    text: &str,
    sources: &SourceMap,
    uri: &Url,
    item: &TypeHierarchyItem,
) -> Vec<TypeHierarchyItem> {
    if !is_concrete(item) {
        return Vec::new();
    }
    let target = &item.name;
    let mut out = Vec::new();
    for it in &ast.items {
        if let Item::ImplDef(im) = it {
            if let TypeExprKind::Named(ref t) = im.target_type.kind {
                if t == target {
                    if let Some(tr) = find_trait_def(ast, &im.trait_name) {
                        let name_span = rename::find_binding_name_span(text, tr.span, &tr.name)
                            .unwrap_or(tr.span);
                        out.push(make_item(
                            uri,
                            tr.name.clone(),
                            SymbolKind::INTERFACE,
                            tr.span,
                            name_span,
                            sources,
                            "trait",
                        ));
                    }
                }
            }
        }
    }
    out
}

/// Subtypes: concrete types that implement a trait. Always empty for concretes.
pub(crate) fn subtypes(
    ast: &SourceFile,
    text: &str,
    sources: &SourceMap,
    uri: &Url,
    item: &TypeHierarchyItem,
) -> Vec<TypeHierarchyItem> {
    if is_concrete(item) {
        return Vec::new();
    }
    let trait_name = &item.name;
    let mut out = Vec::new();
    for it in &ast.items {
        if let Item::ImplDef(im) = it {
            if im.trait_name == *trait_name {
                if let TypeExprKind::Named(ref target) = im.target_type.kind {
                    if let Some(result) = find_concrete_def(ast, text, sources, uri, target) {
                        out.push(result);
                    }
                }
            }
        }
    }
    out
}

fn is_concrete(item: &TypeHierarchyItem) -> bool {
    item.data
        .as_ref()
        .and_then(|d| d.get("kind"))
        .and_then(|v| v.as_str())
        == Some("concrete")
}

fn name_token_at(text: &str, def_span: Span, name: &str, offset: u32) -> bool {
    rename::find_binding_name_span(text, def_span, name)
        .map(|s| s.start <= offset && offset < s.end)
        .unwrap_or(false)
}

fn find_trait_def<'a>(ast: &'a SourceFile, name: &str) -> Option<&'a tyra_ast::TraitDef> {
    ast.items.iter().find_map(|it| match it {
        Item::TraitDef(t) if t.name == name => Some(t),
        _ => None,
    })
}

/// Find the TypeHierarchyItem for a named concrete type (value/data/type) by name.
fn find_concrete_def(
    ast: &SourceFile,
    text: &str,
    sources: &SourceMap,
    uri: &Url,
    name: &str,
) -> Option<TypeHierarchyItem> {
    for it in &ast.items {
        match it {
            Item::ValueDef(v) if v.name == name => {
                let ns = rename::find_binding_name_span(text, v.span, &v.name).unwrap_or(v.span);
                return Some(make_item(
                    uri,
                    v.name.clone(),
                    SymbolKind::CLASS,
                    v.span,
                    ns,
                    sources,
                    "concrete",
                ));
            }
            Item::DataDef(d) if d.name == name => {
                let ns = rename::find_binding_name_span(text, d.span, &d.name).unwrap_or(d.span);
                return Some(make_item(
                    uri,
                    d.name.clone(),
                    SymbolKind::STRUCT,
                    d.span,
                    ns,
                    sources,
                    "concrete",
                ));
            }
            Item::TypeDef(t) if t.name == name => {
                let ns = rename::find_binding_name_span(text, t.span, &t.name).unwrap_or(t.span);
                return Some(make_item(
                    uri,
                    t.name.clone(),
                    SymbolKind::TYPE_PARAMETER,
                    t.span,
                    ns,
                    sources,
                    "concrete",
                ));
            }
            _ => {}
        }
    }
    None
}

fn make_item(
    uri: &Url,
    name: String,
    kind: SymbolKind,
    def_span: Span,
    name_span: Span,
    sources: &SourceMap,
    tag: &str,
) -> TypeHierarchyItem {
    TypeHierarchyItem {
        name: name.clone(),
        kind,
        tags: None,
        detail: None,
        uri: uri.clone(),
        range: span_to_lsp_range(def_span, sources),
        selection_range: span_to_lsp_range(name_span, sources),
        data: Some(json!({"kind": tag, "name": name})),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Url;

    const URI: &str = "file:///tmp/test.tyra";

    fn run_prepare(src: &str, line: u32, col: u32) -> Vec<TypeHierarchyItem> {
        let result = tyra_driver::check_in_memory("test.tyra".to_string(), src.to_string(), None);
        let offset = result
            .sources
            .offset_at_utf16(result.source_id, line, col)
            .unwrap_or(0);
        let uri = Url::parse(URI).unwrap();
        prepare(&result.ast, src, &result.sources, &uri, offset)
    }

    fn run_supertypes(src: &str, item: TypeHierarchyItem) -> Vec<TypeHierarchyItem> {
        let result = tyra_driver::check_in_memory("test.tyra".to_string(), src.to_string(), None);
        let uri = Url::parse(URI).unwrap();
        supertypes(&result.ast, src, &result.sources, &uri, &item)
    }

    fn run_subtypes(src: &str, item: TypeHierarchyItem) -> Vec<TypeHierarchyItem> {
        let result = tyra_driver::check_in_memory("test.tyra".to_string(), src.to_string(), None);
        let uri = Url::parse(URI).unwrap();
        subtypes(&result.ast, src, &result.sources, &uri, &item)
    }

    #[test]
    fn prepare_returns_trait_item_at_trait_def() {
        // Cursor on 'Foo' in 'trait Foo'
        let src = "trait Foo\nend\n";
        let items = run_prepare(src, 0, 6);
        assert_eq!(items.len(), 1, "expected 1 item, got: {items:?}");
        assert_eq!(items[0].kind, SymbolKind::INTERFACE);
        assert_eq!(
            items[0]
                .data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(|v| v.as_str()),
            Some("trait")
        );
    }

    #[test]
    fn prepare_returns_concrete_item_at_value_def() {
        // Cursor on 'User' in 'value User'
        let src = "value User\nend\n";
        let items = run_prepare(src, 0, 6);
        assert_eq!(items.len(), 1, "expected 1 item, got: {items:?}");
        assert_eq!(items[0].kind, SymbolKind::CLASS);
        assert_eq!(
            items[0]
                .data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(|v| v.as_str()),
            Some("concrete")
        );
    }

    #[test]
    fn prepare_at_impl_target_returns_concrete() {
        // Cursor on 'User' in 'impl Foo for User'
        let src = "trait Foo\nend\nvalue User\nend\nimpl Foo for User\nend\n";
        // 'impl Foo for User' is on line 4, 'User' starts at col 13
        let items = run_prepare(src, 4, 13);
        assert_eq!(
            items.len(),
            1,
            "expected 1 item for impl target, got: {items:?}"
        );
        assert_eq!(items[0].name, "User");
        assert_eq!(
            items[0]
                .data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(|v| v.as_str()),
            Some("concrete")
        );
    }

    #[test]
    fn subtypes_lists_impl_targets_for_trait() {
        let src = concat!(
            "trait Foo\nend\n",
            "value A\nend\n",
            "value B\nend\n",
            "impl Foo for A\nend\n",
            "impl Foo for B\nend\n",
        );
        let items = run_prepare(src, 0, 6); // prepare on 'Foo'
        assert_eq!(items.len(), 1);
        let sub = run_subtypes(src, items.into_iter().next().unwrap());
        assert_eq!(sub.len(), 2, "expected 2 subtypes (A, B), got: {sub:?}");
        let names: Vec<&str> = sub.iter().map(|i| i.name.as_str()).collect();
        assert!(
            names.contains(&"A") && names.contains(&"B"),
            "got: {names:?}"
        );
    }

    #[test]
    fn supertypes_lists_traits_implemented_by_concrete() {
        let src = concat!(
            "trait Foo\nend\n",
            "trait Bar\nend\n",
            "value X\nend\n",
            "impl Foo for X\nend\n",
            "impl Bar for X\nend\n",
        );
        let items = run_prepare(src, 4, 6); // prepare on 'X' in 'value X'
        assert_eq!(items.len(), 1);
        let sup = run_supertypes(src, items.into_iter().next().unwrap());
        assert_eq!(
            sup.len(),
            2,
            "expected 2 supertypes (Foo, Bar), got: {sup:?}"
        );
        let names: Vec<&str> = sup.iter().map(|i| i.name.as_str()).collect();
        assert!(
            names.contains(&"Foo") && names.contains(&"Bar"),
            "got: {names:?}"
        );
    }
}
