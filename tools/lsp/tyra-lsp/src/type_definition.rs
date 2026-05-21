use tyra_ast::{Item, SourceFile};
use tyra_diagnostics::{SourceId, Span};
use tyra_driver::{Ty, TypeIndex};

/// Find the def span of the user-defined type for the expression under cursor.
/// Returns None for primitive types, prelude generics, fn types, and
/// unresolved/error types.
pub(crate) fn find_type_def_span(
    ast: &SourceFile,
    type_index: &TypeIndex,
    source_id: SourceId,
    offset: u32,
) -> Option<Span> {
    let ty = smallest_ty_at(type_index, source_id, offset)?;
    let name = ty_name(ty)?;
    find_def_by_name(ast, &name)
}

fn smallest_ty_at(type_index: &TypeIndex, source_id: SourceId, offset: u32) -> Option<&Ty> {
    type_index
        .iter()
        .filter(|(span, _)| span.source == source_id && span.start <= offset && offset < span.end)
        .min_by_key(|(span, _)| span.end - span.start)
        .map(|(_, ty)| ty)
}

fn ty_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Named(n) | Ty::Generic(n, _) => Some(n.clone()),
        _ => None,
    }
}

fn find_def_by_name(ast: &SourceFile, name: &str) -> Option<Span> {
    for item in &ast.items {
        match item {
            Item::DataDef(d) if d.name == name => return Some(d.span),
            Item::ValueDef(v) if v.name == name => return Some(v.span),
            Item::TypeDef(t) if t.name == name => return Some(t.span),
            _ => {}
        }
    }
    None
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, line: u32, col: u32) -> Option<Span> {
        let result = tyra_driver::check_in_memory("test.tyra".to_string(), src.to_string(), None);
        let offset = result
            .sources
            .offset_at_utf16(result.source_id, line, col)?;
        find_type_def_span(&result.ast, &result.type_index, result.source_id, offset)
    }

    #[test]
    fn find_type_def_span_finds_value_def() {
        // fn f(u: User) — cursor on 'u' (param) → type User → value User span
        let src = concat!(
            "value User\n",    // line 0
            "end\n",           // line 1
            "fn f(u: User)\n", // line 2
            "end\n",           // line 3
        );
        // Cursor on 'u' at line 2, col 5.
        let span = run(src, 2, 5).expect("expected Some span for user-defined type");
        // The returned span should cover 'value User\nend' which starts at offset 0.
        assert_eq!(span.start, 0, "expected span to start at 'value User'");
    }

    #[test]
    fn find_type_def_span_returns_none_for_primitive() {
        // 'n' has type Int → primitive, no def
        let src = "fn f(n: Int)\nend\n";
        let result = run(src, 0, 5);
        assert!(
            result.is_none(),
            "expected None for primitive type, got: {result:?}"
        );
    }

    #[test]
    fn find_type_def_span_returns_none_for_prelude_generic() {
        // 'xs' has type List<Int> → prelude generic, not in AST
        let src = "fn f(xs: List<Int>)\nend\n";
        let result = run(src, 0, 5);
        assert!(
            result.is_none(),
            "expected None for prelude generic, got: {result:?}"
        );
    }
}
