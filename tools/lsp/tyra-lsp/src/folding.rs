use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind};
use tyra_ast::{ElseBranch, Expr, ExprKind, Item, SourceFile, Stmt};
use tyra_diagnostics::{SourceId, SourceMap, Span};

use crate::span_to_lsp_range;

/// Build folding ranges from the AST.
///
/// Emits one range per multi-line block construct: top-level items (fn, data,
/// type, trait, impl, value), their nested methods, and control-flow
/// expressions (if/else-if, while, for, match arms, lambda) found inside
/// function bodies.  Consecutive `import` items are merged into one
/// `FoldingRangeKind::Imports` range.
///
/// The `end_line` of each range is set to one line before the closing token
/// so that the closing keyword (`end`) remains visible when folded.
pub(crate) fn build_ranges(
    ast: &SourceFile,
    sources: &SourceMap,
    source_id: SourceId,
) -> Vec<FoldingRange> {
    let mut out = Vec::new();
    let mut import_run: Option<Span> = None;

    for item in &ast.items {
        match item {
            Item::Import(imp) => {
                import_run = Some(match import_run {
                    None => imp.span,
                    Some(prev) => Span::new(source_id, prev.start, imp.span.end),
                });
            }
            other => {
                // Flush any accumulated import run before the next non-import item.
                if let Some(run) = import_run.take() {
                    push_import_span(run, sources, &mut out);
                }
                visit_item(other, sources, &mut out);
            }
        }
    }
    // Flush trailing import run (file ends with imports).
    if let Some(run) = import_run.take() {
        push_import_span(run, sources, &mut out);
    }

    out
}

fn visit_item(item: &Item, sources: &SourceMap, out: &mut Vec<FoldingRange>) {
    match item {
        Item::FnDef(f) => {
            push_span(f.span, None, sources, out);
            visit_stmts(&f.body, sources, out);
        }
        Item::DataDef(d) => {
            push_span(d.span, None, sources, out);
        }
        Item::ValueDef(v) => {
            push_span(v.span, None, sources, out);
        }
        Item::TypeDef(t) => {
            push_span(t.span, None, sources, out);
        }
        Item::TraitDef(tr) => {
            push_span(tr.span, None, sources, out);
            for method in &tr.methods {
                push_span(method.span, None, sources, out);
                visit_stmts(&method.body, sources, out);
            }
        }
        Item::ImplDef(im) => {
            push_span(im.span, None, sources, out);
            for method in &im.methods {
                push_span(method.span, None, sources, out);
                visit_stmts(&method.body, sources, out);
            }
        }
        Item::Stmt(s) => {
            visit_stmt(s, sources, out);
        }
        Item::Import(_) => {}
        Item::TestDef(td) => {
            push_span(td.span, None, sources, out);
            visit_stmts(&td.body, sources, out);
        }
    }
}

fn visit_stmts(stmts: &[Stmt], sources: &SourceMap, out: &mut Vec<FoldingRange>) {
    for s in stmts {
        visit_stmt(s, sources, out);
    }
}

fn visit_stmt(stmt: &Stmt, sources: &SourceMap, out: &mut Vec<FoldingRange>) {
    match stmt {
        Stmt::Let(l) => visit_expr(&l.value, sources, out),
        Stmt::Mut(m) => visit_expr(&m.value, sources, out),
        Stmt::Return(r) => {
            if let Some(e) = &r.value {
                visit_expr(e, sources, out);
            }
        }
        Stmt::Defer(d) => visit_expr(&d.expr, sources, out),
        Stmt::Expr(e) => visit_expr(&e.expr, sources, out),
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

fn visit_expr(expr: &Expr, sources: &SourceMap, out: &mut Vec<FoldingRange>) {
    match &expr.kind {
        ExprKind::If(if_expr) => {
            push_span(if_expr.span, None, sources, out);
            visit_stmts(&if_expr.then_body, sources, out);
            match &if_expr.else_body {
                Some(ElseBranch::Else(stmts)) => {
                    visit_stmts(stmts, sources, out);
                }
                Some(ElseBranch::ElseIf(nested)) => {
                    // Delegate entirely to visit_expr — it handles push + body + further chains.
                    visit_expr(
                        &Expr {
                            kind: ExprKind::If(nested.clone()),
                            span: nested.span,
                        },
                        sources,
                        out,
                    );
                }
                None => {}
            }
        }
        ExprKind::While(w) => {
            push_span(w.span, None, sources, out);
            visit_stmts(&w.body, sources, out);
        }
        ExprKind::For(f) => {
            push_span(f.span, None, sources, out);
            visit_stmts(&f.body, sources, out);
        }
        ExprKind::Match(m) => {
            push_span(m.span, None, sources, out);
            for arm in &m.arms {
                push_span(arm.span, None, sources, out);
                visit_stmts(&arm.body, sources, out);
            }
        }
        ExprKind::Lambda(l) => {
            push_span(l.span, None, sources, out);
            visit_stmts(&l.body, sources, out);
        }
        _ => {}
    }
}

/// Push a folding range for a block construct that ends with an `end` keyword.
///
/// When the span includes a trailing newline (`range.end.character == 0`),
/// `range.end.line` points one past the `end` line; subtract 2 to land on the
/// last body line and keep `end` visible when folded.
/// When there is no trailing newline (`range.end.character > 0`), the span ends
/// on the `end` line itself; subtract 1 to land on the last body line.
fn push_span(
    span: Span,
    kind: Option<FoldingRangeKind>,
    sources: &SourceMap,
    out: &mut Vec<FoldingRange>,
) {
    let range = span_to_lsp_range(span, sources);
    if range.start.line >= range.end.line {
        return;
    }
    let end_line = if range.end.character == 0 {
        // Span ends after a newline: end.line is one past `end`.
        range.end.line.saturating_sub(2)
    } else {
        // Span ends on the `end` line itself (no trailing newline).
        range.end.line.saturating_sub(1)
    };
    if end_line < range.start.line {
        return;
    }
    out.push(FoldingRange {
        start_line: range.start.line,
        start_character: None,
        end_line,
        end_character: None,
        kind,
        collapsed_text: None,
    });
}

/// Push a folding range for a consecutive run of `import` lines.
///
/// The span ends after the trailing newline of the last import, so
/// `range.end.line` is one past the last import line.  Subtract 1 to fold
/// through (and hide) the last import line.
fn push_import_span(span: Span, sources: &SourceMap, out: &mut Vec<FoldingRange>) {
    let range = span_to_lsp_range(span, sources);
    if range.start.line >= range.end.line {
        return;
    }
    let end_line = range.end.line.saturating_sub(1);
    if end_line < range.start.line {
        return;
    }
    out.push(FoldingRange {
        start_line: range.start.line,
        start_character: None,
        end_line,
        end_character: None,
        kind: Some(FoldingRangeKind::Imports),
        collapsed_text: None,
    });
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_diagnostics::SourceMap;

    fn compile_ranges(src: &str) -> Vec<FoldingRange> {
        let mut sources = SourceMap::new();
        let mut report = tyra_diagnostics::Report::new();
        let id = sources.add("test.tyra".into(), src.into());
        let ast = tyra_parser::parse(id, &sources, &mut report);
        build_ranges(&ast, &sources, id)
    }

    #[test]
    fn fn_block_emits_range() {
        // fn main() — line 0, end — line 2; end_line = 2 - 1 = 1
        let ranges = compile_ranges("fn main()\n  let x = 1\nend\n");
        assert!(!ranges.is_empty(), "expected at least one folding range");
        let fn_range = &ranges[0];
        assert_eq!(fn_range.start_line, 0);
        assert_eq!(fn_range.end_line, 1, "end_line should be line before 'end'");
    }

    #[test]
    fn single_line_construct_skipped() {
        // A single-line source: no folding range.
        let ranges = compile_ranges("let x = 1\n");
        assert!(
            ranges.is_empty(),
            "single-line construct should produce no folding range, got: {ranges:?}"
        );
    }

    #[test]
    fn nested_if_emits_range() {
        let src = "fn main()\n  if true\n    let x = 1\n  end\nend\n";
        let ranges = compile_ranges(src);
        // Should have at least 2: the fn and the if.
        assert!(
            ranges.len() >= 2,
            "expected fn range + if range, got: {ranges:?}"
        );
        let starts: Vec<u32> = ranges.iter().map(|r| r.start_line).collect();
        assert!(starts.contains(&0), "fn range (line 0) missing: {ranges:?}");
        assert!(starts.contains(&1), "if range (line 1) missing: {ranges:?}");
    }

    #[test]
    fn match_arm_ranges() {
        let src = concat!(
            "fn check(n: Int)\n",
            "  match n\n",
            "  when 0\n",
            "    let x = 1\n",
            "  end\n",
            "end\n",
        );
        let ranges = compile_ranges(src);
        // fn + match + at least 1 arm
        assert!(
            ranges.len() >= 2,
            "expected fn + match ranges, got: {ranges:?}"
        );
        let starts: Vec<u32> = ranges.iter().map(|r| r.start_line).collect();
        assert!(starts.contains(&0), "fn range missing: {ranges:?}");
        assert!(starts.contains(&1), "match range missing: {ranges:?}");
    }

    #[test]
    fn consecutive_imports_grouped() {
        let src = concat!(
            "import math\n",
            "import string\n",
            "import io\n",
            "fn main()\n",
            "  let x = 1\n",
            "end\n",
        );
        let ranges = compile_ranges(src);
        let import_ranges: Vec<&FoldingRange> = ranges
            .iter()
            .filter(|r| r.kind == Some(FoldingRangeKind::Imports))
            .collect();
        assert_eq!(
            import_ranges.len(),
            1,
            "expected exactly 1 Imports range, got: {ranges:?}"
        );
        assert_eq!(import_ranges[0].start_line, 0);
        assert_eq!(
            import_ranges[0].end_line, 2,
            "end_line should be last import line (line 2)"
        );
    }

    #[test]
    fn impl_methods_each_emit() {
        let src = concat!(
            "trait Greet\n",
            "  fn greet(self) -> String\n",
            "end\n",
            "data Foo\n",
            "end\n",
            "impl Greet for Foo\n",
            "  fn greet(self) -> String\n",
            "    \"hi\"\n",
            "  end\n",
            "end\n",
        );
        let ranges = compile_ranges(src);
        // trait + trait method + data + impl + impl method
        assert!(
            ranges.len() >= 4,
            "expected multiple ranges for trait/data/impl, got: {ranges:?}"
        );
    }

    #[test]
    fn else_if_chain_recurses() {
        let src = concat!(
            "fn f(n: Int)\n",
            "  if n == 1\n",
            "    let a = 1\n",
            "  else if n == 2\n",
            "    let b = 2\n",
            "  end\n",
            "end\n",
        );
        let ranges = compile_ranges(src);
        // fn + if + else-if
        assert!(
            ranges.len() >= 2,
            "expected fn + at least one if range, got: {ranges:?}"
        );
    }

    #[test]
    fn while_and_for_emit_ranges() {
        let src = concat!(
            "fn f()\n",
            "  while true\n",
            "    let x = 1\n",
            "  end\n",
            "  for i in xs\n",
            "    let y = 2\n",
            "  end\n",
            "end\n",
        );
        let ranges = compile_ranges(src);
        let starts: Vec<u32> = ranges.iter().map(|r| r.start_line).collect();
        assert!(
            starts.contains(&1),
            "while range (line 1) missing: {starts:?}"
        );
        assert!(
            starts.contains(&4),
            "for range (line 4) missing: {starts:?}"
        );
    }

    #[test]
    fn no_trailing_newline_still_emits_range() {
        // Source without a trailing newline; span ends on the `end` line itself.
        let ranges = compile_ranges("fn main()\n  let x = 1\nend");
        assert!(
            !ranges.is_empty(),
            "expected at least one range even without trailing newline"
        );
        assert_eq!(ranges[0].start_line, 0);
        assert_eq!(
            ranges[0].end_line, 1,
            "end_line should be body line when no trailing newline"
        );
    }

    #[test]
    fn else_if_chain_no_duplicates() {
        // Each IfExpr span should appear exactly once.
        let src = concat!(
            "fn f(n: Int)\n",
            "  if n == 1\n",
            "    let a = 1\n",
            "  else if n == 2\n",
            "    let b = 2\n",
            "  end\n",
            "end\n",
        );
        let ranges = compile_ranges(src);
        // Collect (start_line, end_line) pairs and check no duplicates.
        let mut pairs: Vec<(u32, u32)> =
            ranges.iter().map(|r| (r.start_line, r.end_line)).collect();
        let original_len = pairs.len();
        pairs.dedup();
        assert_eq!(
            pairs.len(),
            original_len,
            "duplicate folding ranges detected: {ranges:?}"
        );
    }
}
