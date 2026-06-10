use tyra_ast::{Item, SourceFile, TraitDef};
use tyra_diagnostics::Span;

use crate::rename;

/// Find a declaration span for the symbol under the cursor inside an impl block.
/// - Cursor on the trait_name token of `impl Foo for X` → span of `trait Foo`.
/// - Cursor on a method name token inside `impl Foo for X` → span of the
///   matching method in the corresponding `trait Foo` body.
/// - Otherwise → None.
pub(crate) fn find_declaration(ast: &SourceFile, text: &str, offset: u32) -> Option<Span> {
    for item in &ast.items {
        let Item::ImplDef(im) = item else { continue };
        if name_token_at(text, im.span, &im.trait_name, offset) {
            return find_trait_def(ast, &im.trait_name).map(|t| t.span);
        }
        for method in &im.methods {
            if name_token_at(text, method.span, &method.name, offset) {
                let tr = find_trait_def(ast, &im.trait_name)?;
                let m = tr.methods.iter().find(|m| m.name == method.name)?;
                return Some(m.span);
            }
        }
    }
    None
}

fn name_token_at(text: &str, def_span: Span, name: &str, offset: u32) -> bool {
    rename::find_binding_name_span(text, def_span, name)
        .map(|s| s.start <= offset && offset < s.end)
        .unwrap_or(false)
}

fn find_trait_def<'a>(ast: &'a SourceFile, name: &str) -> Option<&'a TraitDef> {
    ast.items.iter().find_map(|it| match it {
        Item::TraitDef(t) if t.name == name => Some(t),
        _ => None,
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, line: u32, col: u32) -> Option<Span> {
        let result = tyra_driver::check_in_memory("test.ty".to_string(), src.to_string(), None);
        let offset = result
            .sources
            .offset_at_utf16(result.source_id, line, col)
            .unwrap_or(0);
        find_declaration(&result.ast, src, offset)
    }

    #[test]
    fn finds_trait_def_from_impl_trait_name() {
        // Cursor on 'Foo' in 'impl Foo for X' → trait Foo span
        let src = concat!(
            "trait Foo\n",      // line 0
            "end\n",            // line 1
            "value X\n",        // line 2
            "end\n",            // line 3
            "impl Foo for X\n", // line 4
            "end\n",            // line 5
        );
        let span = run(src, 4, 5).expect("expected Some span for impl trait name");
        let trait_offset = src.find("trait Foo").unwrap() as u32;
        assert_eq!(span.start, trait_offset, "span should start at 'trait Foo'");
    }

    #[test]
    fn finds_trait_method_from_impl_method() {
        // Cursor on 'greet' inside impl block → trait method span
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
        let span = run(src, 7, 5).expect("expected Some span for impl method");
        // Method span starts at 'fn', not at leading whitespace.
        let trait_method_offset = src.find("fn greet()").unwrap() as u32;
        assert_eq!(
            span.start, trait_method_offset,
            "span should start at trait method 'fn greet'"
        );
    }

    #[test]
    fn returns_none_for_let_binding() {
        let src = "fn main()\n  let x = 1\nend\n";
        let result = run(src, 1, 6);
        assert!(
            result.is_none(),
            "expected None for let binding, got: {result:?}"
        );
    }
}
