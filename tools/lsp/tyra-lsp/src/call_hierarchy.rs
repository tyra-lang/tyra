use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, SymbolKind, Url,
};
use tyra_ast::{ElseBranch, Expr, ExprKind, FnDef, Item, SourceFile, Stmt, StringPart};
use tyra_diagnostics::{SourceId, Span};
use tyra_driver::DefIndex;

use crate::{DocState, span_to_lsp_range};
use crate::{references, rename};

// ── Public entry points ───────────────────────────────────────────────────────

/// Return a `CallHierarchyItem` for the `fn` under the cursor, or `None` if
/// the cursor is not on a function definition or use.
pub(crate) fn prepare(state: &DocState, uri: &Url, offset: u32) -> Option<CallHierarchyItem> {
    // Case 1: cursor is on a use-site identifier that resolves to a fn.
    // Only the ref span (key in def_index) needs to contain the offset.
    if let Some(def_span) = references::find_def_span_at_cursor(state, offset) {
        let in_use_site = state.def_index.iter().any(|(&ref_span, &ds)| {
            ds == def_span
                && ref_span.source == state.source_id
                && ref_span.start <= offset
                && offset < ref_span.end
        });
        if in_use_site && let Some(f) = find_fn_by_span(&state.ast, def_span) {
            return Some(fn_to_item(state, uri, f));
        }
    }
    // Case 2: cursor is on the fn name token at the definition site.
    let f = fn_at_name_span(&state.ast, &state.text, state.source_id, offset)?;
    Some(fn_to_item(state, uri, f))
}

/// Return all callers of the function represented by `item`, with the specific
/// call-site ranges within each caller's body.
pub(crate) fn incoming(
    state: &DocState,
    uri: &Url,
    item: &CallHierarchyItem,
) -> Vec<CallHierarchyIncomingCall> {
    let Some(def_span) = resolve_item_span(state, item) else {
        return vec![];
    };

    // All use-sites that resolve to this fn's def span.
    let use_spans = references::find_uses_for_def(&state.def_index, def_span, state.source_id);

    // Group use-sites by their enclosing FnDef span.
    let mut by_caller: HashMap<Span, Vec<Span>> = HashMap::new();
    for use_span in use_spans {
        if let Some(caller) = enclosing_fn(&state.ast, state.source_id, use_span.start) {
            by_caller.entry(caller.span).or_default().push(use_span);
        }
        // Top-level uses (outside any fn) are silently omitted in v1.
    }

    by_caller
        .into_iter()
        .filter_map(|(caller_span, from_spans)| {
            let caller = find_fn_by_span(&state.ast, caller_span)?;
            let from_ranges = from_spans
                .iter()
                .map(|&s| span_to_lsp_range(s, &state.sources))
                .collect();
            Some(CallHierarchyIncomingCall {
                from: fn_to_item(state, uri, caller),
                from_ranges,
            })
        })
        .collect()
}

/// Return all functions called from within the body of `item`, with the
/// specific call-site ranges within that body.
pub(crate) fn outgoing(
    state: &DocState,
    uri: &Url,
    item: &CallHierarchyItem,
) -> Vec<CallHierarchyOutgoingCall> {
    let Some(def_span) = resolve_item_span(state, item) else {
        return vec![];
    };
    let Some(f) = find_fn_by_span(&state.ast, def_span) else {
        return vec![];
    };

    let calls = collect_outgoing_calls(&f.body, &state.def_index, &state.ast, state.source_id);

    // Group call-site spans by the target fn's def span.
    let mut by_target: HashMap<Span, Vec<Span>> = HashMap::new();
    for (callee_span, target_def_span) in calls {
        by_target
            .entry(target_def_span)
            .or_default()
            .push(callee_span);
    }

    by_target
        .into_iter()
        .filter_map(|(target_def_span, from_spans)| {
            let target = find_fn_by_span(&state.ast, target_def_span)?;
            let from_ranges = from_spans
                .iter()
                .map(|&s| span_to_lsp_range(s, &state.sources))
                .collect();
            Some(CallHierarchyOutgoingCall {
                to: fn_to_item(state, uri, target),
                from_ranges,
            })
        })
        .collect()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Resolve a `CallHierarchyItem` back to the FnDef's span by re-locating the
/// definition from `item.selection_range.start`.
fn resolve_item_span(state: &DocState, item: &CallHierarchyItem) -> Option<Span> {
    let sel = &item.selection_range.start;
    let offset = state
        .sources
        .offset_at_utf16(state.source_id, sel.line, sel.character)?;
    // Try def_index first.
    if let Some(s) = references::find_def_span_at_cursor(state, offset)
        && find_fn_by_span(&state.ast, s).is_some()
    {
        return Some(s);
    }
    // Fallback: selection_range.start is inside the fn's own block.
    let f = enclosing_fn(&state.ast, state.source_id, offset)?;
    Some(f.span)
}

/// Find a top-level `fn` or an impl/trait method by its definition span
/// (`FnDef.span` covers the whole `fn … end` block).
pub(crate) fn find_fn_by_span(ast: &SourceFile, def_span: Span) -> Option<&FnDef> {
    for item in &ast.items {
        match item {
            Item::FnDef(f) if f.span == def_span => return Some(f),
            Item::TraitDef(tr) => {
                for m in &tr.methods {
                    if m.span == def_span {
                        return Some(m);
                    }
                }
            }
            Item::ImplDef(im) => {
                for m in &im.methods {
                    if m.span == def_span {
                        return Some(m);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Find a `FnDef` whose **name token** span contains `offset`.
///
/// This is used to handle definition-site cursor positions (e.g. cursor on
/// "foo" in `fn foo() ... end`) without falling back to `enclosing_fn`, which
/// would accept any position inside the function body.
fn fn_at_name_span<'a>(
    ast: &'a SourceFile,
    text: &str,
    source_id: SourceId,
    offset: u32,
) -> Option<&'a FnDef> {
    let in_name = |f: &FnDef| {
        rename::find_binding_name_span(text, f.span, &f.name)
            .is_some_and(|ns| ns.source == source_id && ns.start <= offset && offset < ns.end)
    };
    for item in &ast.items {
        match item {
            Item::FnDef(f) if in_name(f) => return Some(f),
            Item::TraitDef(tr) => {
                for m in &tr.methods {
                    if in_name(m) {
                        return Some(m);
                    }
                }
            }
            Item::ImplDef(im) => {
                for m in &im.methods {
                    if in_name(m) {
                        return Some(m);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the smallest `FnDef` (top-level or impl/trait method) whose body span
/// contains `offset`.  Returns `None` for top-level executable statements.
pub(crate) fn enclosing_fn<'a>(
    ast: &'a SourceFile,
    source_id: SourceId,
    offset: u32,
) -> Option<&'a FnDef> {
    let in_span =
        |span: Span| span.source == source_id && span.start <= offset && offset < span.end;

    let mut best: Option<&'a FnDef> = None;
    let mut best_size = u32::MAX;

    let mut consider = |f: &'a FnDef| {
        if in_span(f.span) {
            let size = f.span.end - f.span.start;
            if size < best_size {
                best = Some(f);
                best_size = size;
            }
        }
    };

    for item in &ast.items {
        match item {
            Item::FnDef(f) => consider(f),
            Item::TraitDef(tr) => tr.methods.iter().for_each(&mut consider),
            Item::ImplDef(im) => im.methods.iter().for_each(&mut consider),
            _ => {}
        }
    }
    best
}

/// Build a `CallHierarchyItem` for a `FnDef`.
///
/// `range` covers the whole `fn … end` block; `selection_range` is narrowed to
/// the function name token (best-effort, falls back to `range`).
fn fn_to_item(state: &DocState, uri: &Url, f: &FnDef) -> CallHierarchyItem {
    let range = span_to_lsp_range(f.span, &state.sources);
    let selection_range = rename::find_binding_name_span(&state.text, f.span, &f.name)
        .map(|s| span_to_lsp_range(s, &state.sources))
        .unwrap_or(range);
    CallHierarchyItem {
        name: f.name.clone(),
        kind: SymbolKind::FUNCTION,
        tags: None,
        detail: None,
        uri: uri.clone(),
        range,
        selection_range,
        data: None,
    }
}

/// Walk the body of a function and collect `(callee_span, target_fn_def_span)`
/// for every `Ident`-callee `Call` expression whose callee resolves to a
/// top-level fn or impl/trait method.
pub(crate) fn collect_outgoing_calls(
    body: &[Stmt],
    def_index: &DefIndex,
    ast: &SourceFile,
    source_id: SourceId,
) -> Vec<(Span, Span)> {
    let mut out = Vec::new();
    collect_in_stmts(body, def_index, ast, source_id, &mut out);
    out
}

fn collect_in_stmts(
    stmts: &[Stmt],
    def_index: &DefIndex,
    ast: &SourceFile,
    source_id: SourceId,
    out: &mut Vec<(Span, Span)>,
) {
    for stmt in stmts {
        collect_in_stmt(stmt, def_index, ast, source_id, out);
    }
}

fn collect_in_stmt(
    stmt: &Stmt,
    def_index: &DefIndex,
    ast: &SourceFile,
    source_id: SourceId,
    out: &mut Vec<(Span, Span)>,
) {
    match stmt {
        Stmt::Let(l) => collect_in_expr(&l.value, def_index, ast, source_id, out),
        Stmt::Mut(m) => collect_in_expr(&m.value, def_index, ast, source_id, out),
        Stmt::Return(r) => {
            if let Some(e) = &r.value {
                collect_in_expr(e, def_index, ast, source_id, out);
            }
        }
        Stmt::Defer(d) => collect_in_expr(&d.expr, def_index, ast, source_id, out),
        Stmt::Expr(e) => collect_in_expr(&e.expr, def_index, ast, source_id, out),
        Stmt::TupleLet(tl) => collect_in_expr(&tl.value, def_index, ast, source_id, out),
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

fn collect_in_expr(
    expr: &Expr,
    def_index: &DefIndex,
    ast: &SourceFile,
    source_id: SourceId,
    out: &mut Vec<(Span, Span)>,
) {
    match &expr.kind {
        ExprKind::Call(callee, args) => {
            // Record Ident callees that resolve to a known fn.
            if let ExprKind::Ident(_) = &callee.kind
                && let Some(&def_span) = def_index.get(&callee.span)
                && find_fn_by_span(ast, def_span).is_some()
            {
                out.push((callee.span, def_span));
            }
            collect_in_expr(callee, def_index, ast, source_id, out);
            for arg in args {
                collect_in_expr(&arg.value, def_index, ast, source_id, out);
            }
        }
        ExprKind::TurbofishCall(callee, _, args) => {
            if let ExprKind::Ident(_) = &callee.kind
                && let Some(&def_span) = def_index.get(&callee.span)
                && find_fn_by_span(ast, def_span).is_some()
            {
                out.push((callee.span, def_span));
            }
            collect_in_expr(callee, def_index, ast, source_id, out);
            for arg in args {
                collect_in_expr(&arg.value, def_index, ast, source_id, out);
            }
        }
        ExprKind::BinaryOp(lhs, _, rhs) => {
            collect_in_expr(lhs, def_index, ast, source_id, out);
            collect_in_expr(rhs, def_index, ast, source_id, out);
        }
        ExprKind::UnaryOp(_, e)
        | ExprKind::Propagate(e)
        | ExprKind::Await(e)
        | ExprKind::Spawn(e) => {
            collect_in_expr(e, def_index, ast, source_id, out);
        }
        ExprKind::Assign(lhs, rhs) => {
            collect_in_expr(lhs, def_index, ast, source_id, out);
            collect_in_expr(rhs, def_index, ast, source_id, out);
        }
        ExprKind::Index(base, idx) => {
            collect_in_expr(base, def_index, ast, source_id, out);
            collect_in_expr(idx, def_index, ast, source_id, out);
        }
        ExprKind::FieldAccess(base, _) => {
            collect_in_expr(base, def_index, ast, source_id, out);
        }
        ExprKind::ListLit(exprs) => {
            for e in exprs {
                collect_in_expr(e, def_index, ast, source_id, out);
            }
        }
        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                collect_in_expr(k, def_index, ast, source_id, out);
                collect_in_expr(v, def_index, ast, source_id, out);
            }
        }
        ExprKind::StringInterp(parts) => {
            for part in parts {
                if let StringPart::Expr(e) = part {
                    collect_in_expr(e, def_index, ast, source_id, out);
                }
            }
        }
        ExprKind::If(if_expr) => {
            collect_in_expr(&if_expr.condition, def_index, ast, source_id, out);
            collect_in_stmts(&if_expr.then_body, def_index, ast, source_id, out);
            match &if_expr.else_body {
                Some(ElseBranch::Else(stmts)) => {
                    collect_in_stmts(stmts, def_index, ast, source_id, out);
                }
                Some(ElseBranch::ElseIf(nested)) => {
                    let nested_expr = Expr {
                        kind: ExprKind::If(nested.clone()),
                        span: nested.span,
                    };
                    collect_in_expr(&nested_expr, def_index, ast, source_id, out);
                }
                None => {}
            }
        }
        ExprKind::Match(m) => {
            collect_in_expr(&m.subject, def_index, ast, source_id, out);
            for arm in &m.arms {
                collect_in_stmts(&arm.body, def_index, ast, source_id, out);
            }
        }
        ExprKind::While(w) => {
            collect_in_expr(&w.condition, def_index, ast, source_id, out);
            collect_in_stmts(&w.body, def_index, ast, source_id, out);
        }
        ExprKind::For(f) => {
            collect_in_expr(&f.iter, def_index, ast, source_id, out);
            collect_in_stmts(&f.body, def_index, ast, source_id, out);
        }
        ExprKind::Lambda(l) => {
            collect_in_stmts(&l.body, def_index, ast, source_id, out);
        }
        ExprKind::Tuple(elems) => {
            for e in elems {
                collect_in_expr(e, def_index, ast, source_id, out);
            }
        }
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit
        | ExprKind::Ident(_) => {}
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Url;

    const URI: &str = "file:///tmp/test.tyra";

    fn make_state(src: &str) -> DocState {
        let result = tyra_driver::check_in_memory("test.tyra".to_string(), src.to_string(), None);
        DocState {
            text: src.to_string(),
            sources: result.sources,
            type_index: result.type_index,
            def_index: result.def_index,
            symbols: result.symbols,
            source_id: result.source_id,
            ast: result.ast,
            diagnostics: vec![],
            version: 0,
        }
    }

    fn uri() -> Url {
        Url::parse(URI).unwrap()
    }

    #[test]
    fn prepare_returns_none_for_top_level_non_fn() {
        // Top-level import — not a fn name or fn use-site.
        let src = "import math\n";
        let state = make_state(src);
        let offset = state
            .sources
            .offset_at_utf16(state.source_id, 0, 0)
            .unwrap();
        let result = prepare(&state, &uri(), offset);
        assert!(
            result.is_none(),
            "expected None for import statement, got: {result:?}"
        );
    }

    #[test]
    fn prepare_returns_none_for_cursor_inside_fn_body() {
        // Cursor on '1' inside fn body — not a fn name or use-site.
        let src = "fn foo()\n  1\nend\n";
        let state = make_state(src);
        // '1' is at line 1, col 2.
        let offset = state
            .sources
            .offset_at_utf16(state.source_id, 1, 2)
            .unwrap();
        let result = prepare(&state, &uri(), offset);
        assert!(
            result.is_none(),
            "expected None for literal inside fn body, got: {result:?}"
        );
    }

    #[test]
    fn prepare_returns_item_for_fn_def_position() {
        let src = "fn foo()\n  1\nend\n";
        let state = make_state(src);
        // 'f' in 'fn foo()' is at line 0, col 0. 'foo' starts at col 3.
        let offset = state
            .sources
            .offset_at_utf16(state.source_id, 0, 3)
            .unwrap();
        let result = prepare(&state, &uri(), offset);
        assert!(
            result.is_some(),
            "expected Some for cursor on fn name 'foo'"
        );
        assert_eq!(result.unwrap().name, "foo");
    }

    #[test]
    fn incoming_finds_caller_fn() {
        let src = concat!(
            "fn callee()\n",
            "  1\n",
            "end\n",
            "fn caller()\n",
            "  callee()\n",
            "end\n",
        );
        let state = make_state(src);
        let uri = uri();

        // Build a CallHierarchyItem for 'callee' (fn name at line 0, col 3).
        let offset = state
            .sources
            .offset_at_utf16(state.source_id, 0, 3)
            .unwrap();
        let item = prepare(&state, &uri, offset).expect("expected item for 'callee'");

        let calls = incoming(&state, &uri, &item);
        assert_eq!(
            calls.len(),
            1,
            "expected exactly 1 incoming caller, got: {calls:?}"
        );
        assert_eq!(
            calls[0].from.name, "caller",
            "caller should be named 'caller'"
        );
        assert!(
            !calls[0].from_ranges.is_empty(),
            "from_ranges should not be empty"
        );
    }

    #[test]
    fn outgoing_finds_called_fn() {
        let src = concat!(
            "fn callee()\n",
            "  1\n",
            "end\n",
            "fn caller()\n",
            "  callee()\n",
            "end\n",
        );
        let state = make_state(src);
        let uri = uri();

        // Build a CallHierarchyItem for 'caller' (fn name at line 3, col 3).
        let offset = state
            .sources
            .offset_at_utf16(state.source_id, 3, 3)
            .unwrap();
        let item = prepare(&state, &uri, offset).expect("expected item for 'caller'");

        let calls = outgoing(&state, &uri, &item);
        assert_eq!(
            calls.len(),
            1,
            "expected exactly 1 outgoing call, got: {calls:?}"
        );
        assert_eq!(
            calls[0].to.name, "callee",
            "callee should be named 'callee'"
        );
        assert!(
            !calls[0].from_ranges.is_empty(),
            "from_ranges should not be empty"
        );
    }
}
