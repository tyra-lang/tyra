use tyra_ast::{Item, SourceFile};
use tyra_diagnostics::Span;

use crate::rename;

/// Find implementation spans for the symbol under the cursor.
/// - Cursor on a trait name in a `trait Foo` def → all `impl Foo for ...` spans.
/// - Cursor on a method name in a trait body → matching method spans in all impls.
/// - Cursor on the trait name in an `impl Foo for X` block → all impls of Foo.
/// - Otherwise → empty Vec.
pub(crate) fn find_implementations(ast: &SourceFile, text: &str, offset: u32) -> Vec<Span> {
    for item in &ast.items {
        if let Item::TraitDef(tr) = item {
            if name_token_at(text, tr.span, &tr.name, offset) {
                return collect_impls_of(ast, &tr.name);
            }
            for method in &tr.methods {
                if name_token_at(text, method.span, &method.name, offset) {
                    return collect_method_impls(ast, &tr.name, &method.name);
                }
            }
        }
    }
    for item in &ast.items {
        if let Item::ImplDef(im) = item
            && name_token_at(text, im.span, &im.trait_name, offset)
        {
            return collect_impls_of(ast, &im.trait_name);
        }
    }
    Vec::new()
}

fn name_token_at(text: &str, def_span: Span, name: &str, offset: u32) -> bool {
    rename::find_binding_name_span(text, def_span, name)
        .map(|s| s.start <= offset && offset < s.end)
        .unwrap_or(false)
}

fn collect_impls_of(ast: &SourceFile, trait_name: &str) -> Vec<Span> {
    ast.items
        .iter()
        .filter_map(|it| match it {
            Item::ImplDef(im) if im.trait_name == trait_name => Some(im.span),
            _ => None,
        })
        .collect()
}

fn collect_method_impls(ast: &SourceFile, trait_name: &str, method_name: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for item in &ast.items {
        if let Item::ImplDef(im) = item
            && im.trait_name == trait_name
            && let Some(m) = im.methods.iter().find(|m| m.name == method_name)
        {
            out.push(m.span);
        }
    }
    out
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, line: u32, col: u32) -> Vec<Span> {
        let result = tyra_driver::check_in_memory("test.tyra".to_string(), src.to_string(), None);
        let offset = result
            .sources
            .offset_at_utf16(result.source_id, line, col)
            .unwrap_or(0);
        find_implementations(&result.ast, src, offset)
    }

    #[test]
    fn finds_impls_for_trait_name() {
        // Cursor on 'Foo' at line 0, col 6 (in 'trait Foo')
        let src = concat!(
            "trait Foo\n",         // line 0
            "end\n",               // line 1
            "value User\n",        // line 2
            "end\n",               // line 3
            "impl Foo for User\n", // line 4
            "end\n",               // line 5
        );
        let spans = run(src, 0, 6);
        assert_eq!(spans.len(), 1, "expected 1 impl span, got: {spans:?}");
        // impl block starts at line 4
        let impl_offset = src.find("impl Foo for User").unwrap() as u32;
        assert_eq!(
            spans[0].start, impl_offset,
            "impl span should start at 'impl Foo for User'"
        );
    }

    #[test]
    fn finds_method_impls_from_trait() {
        // Cursor on 'greet' at line 1, col 5 (in 'fn greet()' inside trait)
        let src = concat!(
            "trait Foo\n",        // line 0
            "  fn greet()\n",     // line 1
            "  end\n",            // line 2
            "end\n",              // line 3
            "value Bar\n",        // line 4
            "end\n",              // line 5
            "impl Foo for Bar\n", // line 6
            "  fn greet()\n",     // line 7
            "    1\n",            // line 8
            "  end\n",            // line 9
            "end\n",              // line 10
        );
        let spans = run(src, 1, 5);
        assert_eq!(
            spans.len(),
            1,
            "expected 1 method impl span, got: {spans:?}"
        );
    }

    #[test]
    fn returns_empty_for_let_binding() {
        let src = "fn main()\n  let x = 1\nend\n";
        let spans = run(src, 1, 6);
        assert!(
            spans.is_empty(),
            "expected empty for let binding, got: {spans:?}"
        );
    }
}
