use tower_lsp::lsp_types::{
    InlayHint, InlayHintKind, InlayHintLabel, Position, Range,
};
use tyra_ast::{ElseBranch, Expr, ExprKind, Item, SourceFile, Stmt};
use tyra_diagnostics::{SourceId, SourceMap};
use tyra_driver::{Ty, TypeIndex};

/// Build inlay type hints for `let`/`mut` bindings without an explicit type
/// annotation. Only bindings whose inferred type is fully resolved (not
/// `Ty::Var` or `Ty::Error`) produce a hint.
pub(crate) fn build_hints(
    ast: &SourceFile,
    type_index: &TypeIndex,
    sources: &SourceMap,
    source_id: SourceId,
    viewport: Range,
) -> Vec<InlayHint> {
    let vp_start = sources
        .offset_at_utf16(source_id, viewport.start.line, viewport.start.character)
        .unwrap_or(0) as usize;
    let vp_end = sources
        .offset_at_utf16(source_id, viewport.end.line, viewport.end.character)
        .unwrap_or(u32::MAX) as usize;

    let mut out = Vec::new();

    for item in &ast.items {
        match item {
            Item::FnDef(f) => {
                visit_stmts(&f.body, type_index, sources, source_id, vp_start, vp_end, &mut out);
            }
            Item::ImplDef(b) => {
                for method in &b.methods {
                    visit_stmts(
                        &method.body,
                        type_index,
                        sources,
                        source_id,
                        vp_start,
                        vp_end,
                        &mut out,
                    );
                }
            }
            Item::Stmt(s) => {
                visit_stmt(s, type_index, sources, source_id, vp_start, vp_end, &mut out);
            }
            _ => {}
        }
    }

    out
}

fn visit_stmts(
    stmts: &[Stmt],
    type_index: &TypeIndex,
    sources: &SourceMap,
    source_id: SourceId,
    vp_start: usize,
    vp_end: usize,
    out: &mut Vec<InlayHint>,
) {
    for s in stmts {
        visit_stmt(s, type_index, sources, source_id, vp_start, vp_end, out);
    }
}

fn visit_stmt(
    stmt: &Stmt,
    type_index: &TypeIndex,
    sources: &SourceMap,
    source_id: SourceId,
    vp_start: usize,
    vp_end: usize,
    out: &mut Vec<InlayHint>,
) {
    match stmt {
        Stmt::Let(l) => {
            if l.type_annotation.is_none() {
                maybe_emit(
                    l.span,
                    &l.name,
                    "let ",
                    type_index,
                    sources,
                    source_id,
                    vp_start,
                    vp_end,
                    out,
                );
            }
            visit_expr(&l.value, type_index, sources, source_id, vp_start, vp_end, out);
        }
        Stmt::Mut(m) => {
            if m.type_annotation.is_none() {
                maybe_emit(
                    m.span,
                    &m.name,
                    "mut ",
                    type_index,
                    sources,
                    source_id,
                    vp_start,
                    vp_end,
                    out,
                );
            }
            visit_expr(&m.value, type_index, sources, source_id, vp_start, vp_end, out);
        }
        Stmt::Return(r) => {
            if let Some(e) = &r.value {
                visit_expr(e, type_index, sources, source_id, vp_start, vp_end, out);
            }
        }
        Stmt::Defer(d) => {
            visit_expr(&d.expr, type_index, sources, source_id, vp_start, vp_end, out);
        }
        Stmt::Expr(e) => {
            visit_expr(&e.expr, type_index, sources, source_id, vp_start, vp_end, out);
        }
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

fn visit_expr(
    expr: &Expr,
    type_index: &TypeIndex,
    sources: &SourceMap,
    source_id: SourceId,
    vp_start: usize,
    vp_end: usize,
    out: &mut Vec<InlayHint>,
) {
    match &expr.kind {
        ExprKind::If(if_expr) => {
            visit_expr(
                &if_expr.condition,
                type_index,
                sources,
                source_id,
                vp_start,
                vp_end,
                out,
            );
            visit_stmts(
                &if_expr.then_body,
                type_index,
                sources,
                source_id,
                vp_start,
                vp_end,
                out,
            );
            match &if_expr.else_body {
                Some(ElseBranch::Else(stmts)) => {
                    visit_stmts(stmts, type_index, sources, source_id, vp_start, vp_end, out);
                }
                Some(ElseBranch::ElseIf(nested)) => {
                    visit_expr(
                        &Expr { kind: ExprKind::If(nested.clone()), span: nested.span },
                        type_index,
                        sources,
                        source_id,
                        vp_start,
                        vp_end,
                        out,
                    );
                }
                None => {}
            }
        }
        ExprKind::While(w) => {
            visit_expr(&w.condition, type_index, sources, source_id, vp_start, vp_end, out);
            visit_stmts(&w.body, type_index, sources, source_id, vp_start, vp_end, out);
        }
        ExprKind::For(f) => {
            visit_expr(&f.iter, type_index, sources, source_id, vp_start, vp_end, out);
            visit_stmts(&f.body, type_index, sources, source_id, vp_start, vp_end, out);
        }
        ExprKind::Match(m) => {
            visit_expr(&m.subject, type_index, sources, source_id, vp_start, vp_end, out);
            for arm in &m.arms {
                visit_stmts(&arm.body, type_index, sources, source_id, vp_start, vp_end, out);
            }
        }
        ExprKind::Lambda(l) => {
            visit_stmts(&l.body, type_index, sources, source_id, vp_start, vp_end, out);
        }
        // Leaf / structural expressions with no nested stmts.
        _ => {}
    }
}

/// Emit an InlayHint for a let/mut binding if the type is known and resolved.
///
/// `kw` is the keyword prefix (`"let "` or `"mut "`, always 4 bytes) used to
/// locate the name within the statement span. The name token is ASCII-only so
/// `name.len()` equals the UTF-8 byte length.
#[allow(clippy::too_many_arguments)]
fn maybe_emit(
    stmt_span: tyra_diagnostics::Span,
    name: &str,
    kw: &str,
    type_index: &TypeIndex,
    sources: &SourceMap,
    source_id: SourceId,
    vp_start: usize,
    vp_end: usize,
    out: &mut Vec<InlayHint>,
) {
    let Some(ty) = type_index.get(&stmt_span) else { return };

    // Skip unresolved / error types — they'd produce noise.
    if matches!(ty, Ty::Var(_) | Ty::Error) {
        return;
    }

    // Byte offset of the character right after the identifier.
    // stmt_span.start points at 'l' in "let " or 'm' in "mut ".
    let name_end_byte = stmt_span.start as usize + kw.len() + name.len();

    if name_end_byte < vp_start || name_end_byte > vp_end {
        return;
    }

    let Some((line, character)) = sources.line_col_utf16(source_id, name_end_byte as u32) else {
        return;
    };

    out.push(InlayHint {
        position: Position { line, character },
        label: InlayHintLabel::String(format!(": {ty}")),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(false),
        data: None,
    });
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tyra_diagnostics::{Span, SourceMap};
    use tyra_driver::{Ty, TypeIndex};

    use super::*;

    fn compile_hints(src: &str) -> Vec<InlayHint> {
        let result = tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
        let viewport = Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: u32::MAX, character: 0 },
        };
        build_hints(&result.ast, &result.type_index, &result.sources, result.source_id, viewport)
    }

    #[test]
    fn let_without_annotation_emits_hint() {
        let hints = compile_hints("fn main()\n  let x = 1\nend\n");
        assert!(!hints.is_empty(), "expected at least one hint");
        let labels: Vec<String> = hints
            .iter()
            .filter_map(|h| match &h.label {
                InlayHintLabel::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert!(labels.iter().any(|l| l == ": Int"), "expected `: Int`, got {labels:?}");
    }

    #[test]
    fn let_with_annotation_skipped() {
        let hints = compile_hints("fn main()\n  let x: Int = 1\nend\n");
        assert!(hints.is_empty(), "expected no hints when annotation present, got: {hints:?}");
    }

    #[test]
    fn mut_without_annotation_emits_hint() {
        let hints = compile_hints("fn main()\n  mut y = 3.14\nend\n");
        let labels: Vec<String> = hints
            .iter()
            .filter_map(|h| match &h.label {
                InlayHintLabel::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert!(labels.iter().any(|l| l == ": Float"), "expected `: Float`, got {labels:?}");
    }

    #[test]
    fn var_type_skipped() {
        // Build a TypeIndex with only Var type — should produce no hints.
        let mut sources = SourceMap::new();
        let id = sources.add("t.tyra".into(), "fn f()\n  let x = 1\nend\n".into());
        let stmt_span = Span::new(id, 7, 17);
        let mut type_index: TypeIndex = HashMap::new();
        type_index.insert(stmt_span, Ty::Var(0));
        let result = tyra_driver::check_in_memory("t.tyra".into(), "fn f()\n  let x = 1\nend\n".into(), None);
        let viewport = Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: u32::MAX, character: 0 },
        };
        let hints = build_hints(&result.ast, &type_index, &result.sources, result.source_id, viewport);
        assert!(hints.is_empty(), "Var type should produce no hint");
    }

    #[test]
    fn error_type_skipped() {
        let mut sources = SourceMap::new();
        let id = sources.add("t.tyra".into(), "fn f()\n  let x = 1\nend\n".into());
        let stmt_span = Span::new(id, 7, 17);
        let mut type_index: TypeIndex = HashMap::new();
        type_index.insert(stmt_span, Ty::Error);
        let result = tyra_driver::check_in_memory("t.tyra".into(), "fn f()\n  let x = 1\nend\n".into(), None);
        let viewport = Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: u32::MAX, character: 0 },
        };
        let hints = build_hints(&result.ast, &type_index, &result.sources, result.source_id, viewport);
        assert!(hints.is_empty(), "Error type should produce no hint");
    }

    #[test]
    fn top_level_let_emits_hint() {
        // Top-level executable statements (Item::Stmt) must also produce hints.
        let hints = compile_hints("let x = 42\n");
        let labels: Vec<String> = hints
            .iter()
            .filter_map(|h| match &h.label {
                InlayHintLabel::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert!(
            labels.iter().any(|l| l == ": Int"),
            "expected `: Int` for top-level let, got {labels:?}"
        );
    }

    #[test]
    fn viewport_filters_outside_range() {
        // Two lets on different lines; viewport covers only line 1.
        let src = "fn main()\n  let a = 1\n  let b = 2\nend\n";
        let result = tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
        // viewport: only line 1 (0-indexed)
        let viewport = Range {
            start: Position { line: 1, character: 0 },
            end: Position { line: 2, character: 0 },
        };
        let hints = build_hints(
            &result.ast,
            &result.type_index,
            &result.sources,
            result.source_id,
            viewport,
        );
        assert_eq!(hints.len(), 1, "expected exactly 1 hint in viewport, got: {hints:?}");
    }
}
