use tower_lsp::lsp_types::*;
use tyra_ast::{Item, SourceFile, Stmt, TypeDefKind, TypeExprKind};
use tyra_diagnostics::{SourceId, SourceMap, Span}; // SourceId kept for public API

use crate::span_to_lsp_range;

/// Convert the post-resolve AST into a hierarchical `DocumentSymbol` list.
///
/// Only top-level items are walked; locals inside function bodies are omitted.
/// `selectionRange` mirrors `range` (item-level span) until the parser emits
/// per-identifier spans.
pub(crate) fn build_document_symbols(
    _source_id: SourceId,
    ast: &SourceFile,
    sources: &SourceMap,
) -> Vec<DocumentSymbol> {
    let mut out = Vec::new();
    for item in &ast.items {
        if let Some(sym) = item_to_symbol(item, sources) {
            out.push(sym);
        }
    }
    out
}

fn item_to_symbol(item: &Item, sources: &SourceMap) -> Option<DocumentSymbol> {
    match item {
        Item::FnDef(f) => Some(make_symbol(
            f.name.clone(),
            SymbolKind::FUNCTION,
            f.span,
            sources,
            vec![],
        )),
        Item::DataDef(d) => {
            let children: Vec<DocumentSymbol> = d
                .fields
                .iter()
                .map(|field| {
                    make_symbol(
                        field.name.clone(),
                        SymbolKind::FIELD,
                        field.span,
                        sources,
                        vec![],
                    )
                })
                .collect();
            Some(make_symbol(
                d.name.clone(),
                SymbolKind::STRUCT,
                d.span,
                sources,
                children,
            ))
        }
        Item::ValueDef(v) => {
            let children: Vec<DocumentSymbol> = v
                .fields
                .iter()
                .map(|field| {
                    make_symbol(
                        field.name.clone(),
                        SymbolKind::FIELD,
                        field.span,
                        sources,
                        vec![],
                    )
                })
                .collect();
            Some(make_symbol(
                v.name.clone(),
                SymbolKind::CLASS,
                v.span,
                sources,
                children,
            ))
        }
        Item::TypeDef(t) => {
            let (kind, children) = match &t.kind {
                TypeDefKind::Alias(_) => (SymbolKind::CLASS, vec![]),
                TypeDefKind::Adt(variants) => {
                    let children: Vec<DocumentSymbol> = variants
                        .iter()
                        .map(|v| {
                            make_symbol(
                                v.name.clone(),
                                SymbolKind::ENUM_MEMBER,
                                v.span,
                                sources,
                                vec![],
                            )
                        })
                        .collect();
                    (SymbolKind::ENUM, children)
                }
            };
            Some(make_symbol(t.name.clone(), kind, t.span, sources, children))
        }
        Item::TraitDef(tr) => {
            let children: Vec<DocumentSymbol> = tr
                .methods
                .iter()
                .map(|m| make_symbol(m.name.clone(), SymbolKind::METHOD, m.span, sources, vec![]))
                .collect();
            Some(make_symbol(
                tr.name.clone(),
                SymbolKind::INTERFACE,
                tr.span,
                sources,
                children,
            ))
        }
        Item::ImplDef(im) => {
            let name = impl_display_name(im);
            let children: Vec<DocumentSymbol> = im
                .methods
                .iter()
                .map(|m| make_symbol(m.name.clone(), SymbolKind::METHOD, m.span, sources, vec![]))
                .collect();
            Some(make_symbol(
                name,
                SymbolKind::CLASS,
                im.span,
                sources,
                children,
            ))
        }
        Item::Stmt(Stmt::Let(l)) => Some(make_symbol(
            l.name.clone(),
            SymbolKind::VARIABLE,
            l.span,
            sources,
            vec![],
        )),
        Item::Stmt(Stmt::Mut(m)) => Some(make_symbol(
            m.name.clone(),
            SymbolKind::VARIABLE,
            m.span,
            sources,
            vec![],
        )),
        // Import and other stmts (return, defer, break, expr) are not outline-relevant.
        Item::Import(_) | Item::Stmt(_) => None,
        Item::TestDef(td) => Some(make_symbol(
            format!("test {:?}", td.name),
            SymbolKind::FUNCTION,
            td.span,
            sources,
            vec![],
        )),
    }
}

fn impl_display_name(im: &tyra_ast::ImplDef) -> String {
    let target = type_expr_name(&im.target_type.kind);
    if im.trait_name.is_empty() {
        format!("impl {target}")
    } else {
        format!("impl {} for {target}", im.trait_name)
    }
}

fn type_expr_name(kind: &TypeExprKind) -> String {
    match kind {
        TypeExprKind::Named(n) => n.clone(),
        TypeExprKind::Generic(n, args) => {
            if args.is_empty() {
                n.clone()
            } else {
                let arg_names: Vec<String> = args.iter().map(|a| type_expr_name(&a.kind)).collect();
                format!("{}<{}>", n, arg_names.join(", "))
            }
        }
        TypeExprKind::Fn(_, _) => "fn".to_string(),
        TypeExprKind::Tuple(elems) => {
            let names: Vec<String> = elems.iter().map(|e| type_expr_name(&e.kind)).collect();
            format!("({})", names.join(", "))
        }
    }
}

#[allow(deprecated)]
fn make_symbol(
    name: String,
    kind: SymbolKind,
    span: Span,
    sources: &SourceMap,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    let range = span_to_lsp_range(span, sources);
    let children = if children.is_empty() {
        None
    } else {
        Some(children)
    };
    DocumentSymbol {
        name,
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_diagnostics::SourceMap;

    fn run(src: &str) -> Vec<DocumentSymbol> {
        let mut sources = SourceMap::new();
        let mut report = tyra_diagnostics::Report::new();
        let id = sources.add("t.ty".into(), src.into());
        let ast = tyra_parser::parse(id, &sources, &mut report);
        build_document_symbols(id, &ast, &sources)
    }

    #[test]
    fn outline_emits_fn_data_typedef() {
        let src = concat!(
            "fn foo() -> Int\n  0\nend\n",
            "data Pair\n  x: Int\n  y: Int\nend\n",
            "type Color =\n  | Red\n  | Green\n  | Blue\n",
        );
        let syms = run(src);
        assert_eq!(syms.len(), 3, "expected 3 top-level symbols, got: {syms:?}");

        assert_eq!(syms[0].name, "foo");
        assert_eq!(syms[0].kind, SymbolKind::FUNCTION);

        assert_eq!(syms[1].name, "Pair");
        assert_eq!(syms[1].kind, SymbolKind::STRUCT);
        let pair_children = syms[1].children.as_ref().expect("Pair should have fields");
        assert_eq!(pair_children.len(), 2);
        assert_eq!(pair_children[0].name, "x");
        assert_eq!(pair_children[1].name, "y");

        assert_eq!(syms[2].name, "Color");
        assert_eq!(syms[2].kind, SymbolKind::ENUM);
        let color_children = syms[2]
            .children
            .as_ref()
            .expect("Color should have variants");
        assert_eq!(color_children.len(), 3);
        assert_eq!(color_children[0].name, "Red");
        assert_eq!(color_children[1].name, "Green");
        assert_eq!(color_children[2].name, "Blue");
    }

    #[test]
    fn outline_emits_impl_methods() {
        let src = concat!(
            "trait Greet\n  fn greet(self) -> String\nend\n",
            "data Foo\nend\n",
            "impl Greet for Foo\n",
            "  fn greet(self) -> String\n",
            "    \"hi\"\n",
            "  end\n",
            "end\n",
        );
        let syms = run(src);
        let impl_sym = syms
            .iter()
            .find(|s| s.name.contains("Greet") && s.name.contains("Foo"))
            .expect("impl symbol not found");
        assert_eq!(impl_sym.kind, SymbolKind::CLASS);
        let methods = impl_sym
            .children
            .as_ref()
            .expect("impl should have method children");
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].name, "greet");
        assert_eq!(methods[0].kind, SymbolKind::METHOD);
    }

    #[test]
    fn outline_skips_imports_and_locals() {
        // `z` inside fn body should not appear as top-level.
        let src = concat!("fn foo() -> Int\n", "  let z = 1\n", "  z\n", "end\n",);
        let syms = run(src);
        assert_eq!(syms.len(), 1, "only `foo` should appear");
        assert_eq!(syms[0].name, "foo");
    }
}
