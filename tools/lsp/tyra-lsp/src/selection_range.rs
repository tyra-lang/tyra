use tower_lsp::lsp_types::SelectionRange;
use tyra_ast::{
    ElseBranch, Expr, ExprKind, IfExpr, Item, LambdaExpr, SourceFile, Stmt, StringPart,
};
use tyra_diagnostics::{SourceId, SourceMap, Span};

use crate::span_to_lsp_range;

/// Build a `SelectionRange` chain (smallest → outermost) for a byte offset.
///
/// Returns `None` when no AST node spans the offset (e.g. trailing whitespace
/// or an empty file).
pub(crate) fn compute(
    ast: &SourceFile,
    sources: &SourceMap,
    source_id: SourceId,
    offset: u32,
) -> Option<SelectionRange> {
    let mut chain: Vec<Span> = Vec::new();

    let item = ast
        .items
        .iter()
        .find(|item| span_contains(item_span(item), source_id, offset))?;

    chain.push(item_span(item));
    descend_item(item, source_id, offset, &mut chain);

    build_chain(&chain, sources)
}

// ── Span helpers ─────────────────────────────────────────────────────────────

fn span_contains(span: Span, source_id: SourceId, offset: u32) -> bool {
    span.source == source_id && span.start <= offset && offset < span.end
}

fn item_span(item: &Item) -> Span {
    match item {
        Item::FnDef(f) => f.span,
        Item::ValueDef(v) => v.span,
        Item::DataDef(d) => d.span,
        Item::TypeDef(t) => t.span,
        Item::TraitDef(tr) => tr.span,
        Item::ImplDef(im) => im.span,
        Item::Import(imp) => imp.span,
        Item::Stmt(s) => stmt_span(s),
        Item::TestDef(td) => td.span,
    }
}

fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Let(l) => l.span,
        Stmt::Mut(m) => m.span,
        Stmt::Return(r) => r.span,
        Stmt::Defer(d) => d.span,
        Stmt::Break(b) => b.span,
        Stmt::Continue(c) => c.span,
        Stmt::TupleLet(tl) => tl.span,
        Stmt::Expr(e) => e.span,
    }
}

// ── Descent (each fn assumes its parent span is already in `chain`) ───────────

fn descend_item(item: &Item, source_id: SourceId, offset: u32, chain: &mut Vec<Span>) {
    match item {
        Item::FnDef(f) => {
            for param in &f.params {
                if span_contains(param.span, source_id, offset) {
                    chain.push(param.span);
                    return;
                }
            }
            descend_stmts(&f.body, source_id, offset, chain);
        }
        Item::TraitDef(tr) => {
            for method in &tr.methods {
                if span_contains(method.span, source_id, offset) {
                    chain.push(method.span);
                    for param in &method.params {
                        if span_contains(param.span, source_id, offset) {
                            chain.push(param.span);
                            return;
                        }
                    }
                    descend_stmts(&method.body, source_id, offset, chain);
                    return;
                }
            }
        }
        Item::ImplDef(im) => {
            for method in &im.methods {
                if span_contains(method.span, source_id, offset) {
                    chain.push(method.span);
                    for param in &method.params {
                        if span_contains(param.span, source_id, offset) {
                            chain.push(param.span);
                            return;
                        }
                    }
                    descend_stmts(&method.body, source_id, offset, chain);
                    return;
                }
            }
        }
        Item::Stmt(s) => descend_stmt(s, source_id, offset, chain),
        // DataDef, ValueDef, TypeDef, Import: no expr-level children.
        _ => {}
    }
}

/// Find the first stmt in `stmts` whose span contains `offset`, push it, and descend.
/// Returns true if a matching stmt was found.
fn descend_stmts(stmts: &[Stmt], source_id: SourceId, offset: u32, chain: &mut Vec<Span>) -> bool {
    for stmt in stmts {
        let sp = stmt_span(stmt);
        if span_contains(sp, source_id, offset) {
            // Avoid duplicating the span that was already pushed by the caller
            // (happens when Item::Stmt's item_span == stmt_span).
            if chain.last().copied() != Some(sp) {
                chain.push(sp);
            }
            descend_stmt(stmt, source_id, offset, chain);
            return true;
        }
    }
    false
}

fn descend_stmt(stmt: &Stmt, source_id: SourceId, offset: u32, chain: &mut Vec<Span>) {
    match stmt {
        Stmt::Let(l) => try_descend_expr(&l.value, source_id, offset, chain),
        Stmt::Mut(m) => try_descend_expr(&m.value, source_id, offset, chain),
        Stmt::Return(r) => {
            if let Some(e) = &r.value {
                try_descend_expr(e, source_id, offset, chain);
            }
        }
        Stmt::Defer(d) => try_descend_expr(&d.expr, source_id, offset, chain),
        Stmt::TupleLet(tl) => try_descend_expr(&tl.value, source_id, offset, chain),
        Stmt::Expr(e) => try_descend_expr(&e.expr, source_id, offset, chain),
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

/// Push `expr.span` if it contains `offset` (and is not already the last entry),
/// then descend into its children.
fn try_descend_expr(expr: &Expr, source_id: SourceId, offset: u32, chain: &mut Vec<Span>) {
    if span_contains(expr.span, source_id, offset) {
        if chain.last().copied() != Some(expr.span) {
            chain.push(expr.span);
        }
        descend_expr(expr, source_id, offset, chain);
    }
}

/// Descend into an expression's children, assuming `expr.span` is already in `chain`.
fn descend_expr(expr: &Expr, source_id: SourceId, offset: u32, chain: &mut Vec<Span>) {
    match &expr.kind {
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit
        | ExprKind::Ident(_) => {}

        ExprKind::StringInterp(parts) => {
            for part in parts {
                if let StringPart::Expr(e) = part
                    && span_contains(e.span, source_id, offset)
                {
                    try_descend_expr(e, source_id, offset, chain);
                    return;
                }
            }
        }

        ExprKind::ListLit(exprs) => {
            for e in exprs {
                if span_contains(e.span, source_id, offset) {
                    try_descend_expr(e, source_id, offset, chain);
                    return;
                }
            }
        }

        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                if span_contains(k.span, source_id, offset) {
                    try_descend_expr(k, source_id, offset, chain);
                    return;
                }
                if span_contains(v.span, source_id, offset) {
                    try_descend_expr(v, source_id, offset, chain);
                    return;
                }
            }
        }

        ExprKind::FieldAccess(base, _) => {
            if span_contains(base.span, source_id, offset) {
                try_descend_expr(base, source_id, offset, chain);
            }
        }

        ExprKind::BinaryOp(lhs, _, rhs) => {
            if span_contains(lhs.span, source_id, offset) {
                try_descend_expr(lhs, source_id, offset, chain);
            } else if span_contains(rhs.span, source_id, offset) {
                try_descend_expr(rhs, source_id, offset, chain);
            }
        }

        ExprKind::UnaryOp(_, e) => {
            if span_contains(e.span, source_id, offset) {
                try_descend_expr(e, source_id, offset, chain);
            }
        }

        ExprKind::Assign(lhs, rhs) => {
            if span_contains(lhs.span, source_id, offset) {
                try_descend_expr(lhs, source_id, offset, chain);
            } else if span_contains(rhs.span, source_id, offset) {
                try_descend_expr(rhs, source_id, offset, chain);
            }
        }

        ExprKind::Call(callee, args) => {
            if span_contains(callee.span, source_id, offset) {
                try_descend_expr(callee, source_id, offset, chain);
                return;
            }
            for arg in args {
                if span_contains(arg.span, source_id, offset) {
                    if chain.last().copied() != Some(arg.span) {
                        chain.push(arg.span);
                    }
                    try_descend_expr(&arg.value, source_id, offset, chain);
                    return;
                }
            }
        }

        ExprKind::TurbofishCall(callee, _, args) => {
            if span_contains(callee.span, source_id, offset) {
                try_descend_expr(callee, source_id, offset, chain);
                return;
            }
            for arg in args {
                if span_contains(arg.span, source_id, offset) {
                    if chain.last().copied() != Some(arg.span) {
                        chain.push(arg.span);
                    }
                    try_descend_expr(&arg.value, source_id, offset, chain);
                    return;
                }
            }
        }

        ExprKind::Index(base, idx) => {
            if span_contains(base.span, source_id, offset) {
                try_descend_expr(base, source_id, offset, chain);
            } else if span_contains(idx.span, source_id, offset) {
                try_descend_expr(idx, source_id, offset, chain);
            }
        }

        ExprKind::Propagate(e) | ExprKind::Await(e) | ExprKind::Spawn(e) => {
            if span_contains(e.span, source_id, offset) {
                try_descend_expr(e, source_id, offset, chain);
            }
        }

        ExprKind::If(if_expr) => {
            descend_if_expr(if_expr, source_id, offset, chain);
        }

        ExprKind::Match(m) => {
            if span_contains(m.subject.span, source_id, offset) {
                try_descend_expr(&m.subject, source_id, offset, chain);
                return;
            }
            for arm in &m.arms {
                if span_contains(arm.span, source_id, offset) {
                    chain.push(arm.span);
                    if span_contains(arm.pattern.span, source_id, offset) {
                        chain.push(arm.pattern.span);
                        return;
                    }
                    descend_stmts(&arm.body, source_id, offset, chain);
                    return;
                }
            }
        }

        ExprKind::While(w) => {
            if span_contains(w.condition.span, source_id, offset) {
                try_descend_expr(&w.condition, source_id, offset, chain);
                return;
            }
            descend_stmts(&w.body, source_id, offset, chain);
        }

        ExprKind::For(f) => {
            if span_contains(f.iter.span, source_id, offset) {
                try_descend_expr(&f.iter, source_id, offset, chain);
                return;
            }
            descend_stmts(&f.body, source_id, offset, chain);
        }

        ExprKind::Lambda(l) => {
            descend_lambda(l, source_id, offset, chain);
        }

        ExprKind::Tuple(elems) => {
            for e in elems {
                if span_contains(e.span, source_id, offset) {
                    try_descend_expr(e, source_id, offset, chain);
                    return;
                }
            }
        }
    }
}

fn descend_if_expr(if_expr: &IfExpr, source_id: SourceId, offset: u32, chain: &mut Vec<Span>) {
    if span_contains(if_expr.condition.span, source_id, offset) {
        try_descend_expr(&if_expr.condition, source_id, offset, chain);
        return;
    }
    if descend_stmts(&if_expr.then_body, source_id, offset, chain) {
        return;
    }
    match &if_expr.else_body {
        Some(ElseBranch::Else(stmts)) => {
            descend_stmts(stmts, source_id, offset, chain);
        }
        Some(ElseBranch::ElseIf(nested)) => {
            if span_contains(nested.span, source_id, offset) {
                chain.push(nested.span);
                descend_if_expr(nested, source_id, offset, chain);
            }
        }
        None => {}
    }
}

fn descend_lambda(lambda: &LambdaExpr, source_id: SourceId, offset: u32, chain: &mut Vec<Span>) {
    for param in &lambda.params {
        if span_contains(param.span, source_id, offset) {
            chain.push(param.span);
            return;
        }
    }
    descend_stmts(&lambda.body, source_id, offset, chain);
}

// ── Build SelectionRange chain ────────────────────────────────────────────────

/// Fold a vec of spans ordered [outermost, ..., innermost] into a nested
/// `SelectionRange` where each `parent` is the enclosing (larger) scope.
fn build_chain(spans: &[Span], sources: &SourceMap) -> Option<SelectionRange> {
    spans.iter().fold(None, |parent, &span| {
        Some(SelectionRange {
            range: span_to_lsp_range(span, sources),
            parent: parent.map(Box::new),
        })
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, line: u32, col: u32) -> Option<SelectionRange> {
        let mut sources = SourceMap::new();
        let mut report = tyra_diagnostics::Report::new();
        let id = sources.add("test.ty".into(), src.into());
        let ast = tyra_parser::parse(id, &sources, &mut report);
        let offset = sources.offset_at_utf16(id, line, col)?;
        compute(&ast, &sources, id, offset)
    }

    fn chain_depth(sr: &SelectionRange) -> usize {
        1 + sr.parent.as_ref().map_or(0, |p| chain_depth(p))
    }

    #[test]
    fn cursor_inside_binary_expr_returns_chain() {
        // fn main()\n  let x = 1 + 2\nend\n
        // Cursor on '2' → line 1, col 13
        let src = "fn main()\n  let x = 1 + 2\nend\n";
        let sr = run(src, 1, 13).expect("expected Some SelectionRange");
        // chain should include at least: fn, let, binary-op (or similar nesting)
        assert!(
            chain_depth(&sr) >= 2,
            "expected chain depth ≥ 2, got {}: {:?}",
            chain_depth(&sr),
            sr
        );
    }

    #[test]
    fn cursor_in_if_branch_returns_chain_containing_if() {
        let src = concat!(
            "fn f(n: Int)\n",  // line 0
            "  if n == 1\n",   // line 1
            "    let x = 1\n", // line 2
            "  end\n",         // line 3
            "end\n",           // line 4
        );
        // Cursor on 'x' → line 2, col 8
        let sr = run(src, 2, 8).expect("expected Some SelectionRange");
        assert!(
            chain_depth(&sr) >= 3,
            "expected fn → if → let chain (depth ≥ 3), got {}: {:?}",
            chain_depth(&sr),
            sr
        );
    }

    #[test]
    fn cursor_outside_ast_returns_none() {
        // A trailing blank line after 'end' has no AST node.
        let src = "fn main()\n  let x = 1\nend\n\n";
        // line 3 (the extra blank line) is outside all spans.
        let result = run(src, 3, 0);
        assert!(
            result.is_none(),
            "expected None for cursor past end, got: {result:?}"
        );
    }

    #[test]
    fn cursor_at_top_level_let_returns_chain() {
        // Top-level let: Item::Stmt(Stmt::Let(...))
        let src = "let x = 42\n";
        // Cursor on '4' → line 0, col 8
        let sr = run(src, 0, 8).expect("expected Some SelectionRange");
        // chain should have at least 1 level (the let stmt)
        assert!(
            chain_depth(&sr) >= 1,
            "expected depth ≥ 1 for top-level let, got {}: {:?}",
            chain_depth(&sr),
            sr
        );
    }

    #[test]
    fn cursor_on_fn_keyword_returns_fn_range() {
        let src = "fn main()\n  let x = 1\nend\n";
        // Cursor at start of file (on 'fn').
        let sr = run(src, 0, 0).expect("expected Some SelectionRange");
        assert!(
            chain_depth(&sr) >= 1,
            "expected at least fn-level range, got {}: {:?}",
            chain_depth(&sr),
            sr
        );
    }
}
