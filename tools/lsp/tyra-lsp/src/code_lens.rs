use tower_lsp::lsp_types::{CodeLens, Command};
use tyra_ast::Item;

use crate::{references, rename, span_to_lsp_range, DocState};

/// Build code lenses for top-level `fn` definitions.
/// Each lens shows the reference count for the function's name.
pub(crate) fn build_code_lenses(state: &DocState) -> Vec<CodeLens> {
    let mut out = Vec::new();
    for item in &state.ast.items {
        let Item::FnDef(f) = item else { continue };
        let Some(name_span) = rename::find_binding_name_span(&state.text, f.span, &f.name)
        else {
            continue
        };
        // def_index values store the whole def block span, not the name token span.
        let uses =
            references::find_uses_for_def(&state.def_index, f.span, state.source_id);
        let title = if uses.len() == 1 {
            "1 reference".to_string()
        } else {
            format!("{} references", uses.len())
        };
        out.push(CodeLens {
            range: span_to_lsp_range(name_span, &state.sources),
            command: Some(Command {
                title,
                // Non-empty no-op: clients silently ignore unknown commands
                // rather than surfacing "unknown command" errors on click.
                command: "tyra.noop".to_string(),
                arguments: None,
            }),
            data: None,
        });
    }
    out
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> Vec<CodeLens> {
        let result = tyra_driver::check_in_memory("test.tyra".to_string(), src.to_string(), None);
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
        build_code_lenses(&state)
    }

    #[test]
    fn emits_lens_per_top_level_fn() {
        let src = "fn foo()\nend\nfn bar()\nend\n";
        let lenses = run(src);
        assert_eq!(lenses.len(), 2, "expected 2 lenses, got: {lenses:?}");
        let titles: Vec<_> = lenses
            .iter()
            .filter_map(|l| l.command.as_ref().map(|c| c.title.as_str()))
            .collect();
        assert!(
            titles.iter().all(|t| t.contains("0 references")),
            "expected all 0 references, got: {titles:?}"
        );
    }

    #[test]
    fn lens_shows_correct_reference_count() {
        // foo is called twice from bar; bar is never called.
        let src = "fn foo()\n  1\nend\nfn bar()\n  foo()\n  foo()\nend\n";
        let lenses = run(src);
        assert_eq!(lenses.len(), 2, "expected 2 lenses, got: {lenses:?}");
        let foo_lens = lenses
            .iter()
            .find(|l| l.command.as_ref().map(|c| c.title.as_str()) == Some("2 references"))
            .expect("expected a lens with '2 references' for foo");
        assert_eq!(
            foo_lens.command.as_ref().unwrap().title,
            "2 references",
            "foo should have 2 references"
        );
        let bar_lens = lenses
            .iter()
            .find(|l| l.command.as_ref().map(|c| c.title.as_str()) == Some("0 references"));
        assert!(bar_lens.is_some(), "expected a lens with '0 references' for bar");
    }

    #[test]
    fn singular_reference_label() {
        // foo is called exactly once.
        let src = "fn foo()\n  1\nend\nfn bar()\n  foo()\nend\n";
        let lenses = run(src);
        let titles: Vec<_> = lenses
            .iter()
            .filter_map(|l| l.command.as_ref().map(|c| c.title.as_str()))
            .collect();
        assert!(
            titles.contains(&"1 reference"),
            "expected singular '1 reference', got: {titles:?}"
        );
    }
}
