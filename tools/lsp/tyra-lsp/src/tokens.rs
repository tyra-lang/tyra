use std::collections::HashMap;

use tower_lsp::lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensLegend,
};
use tyra_ast::{
    ElseBranch, Expr, ExprKind, Item, SourceFile, Stmt, TypeDefKind, TypeExprKind,
};
use tyra_diagnostics::{SourceId, SourceMap, Span};
use tyra_driver::{DefIndex, PRELUDE_CONSTRUCTORS, PRELUDE_FUNCTIONS, PRELUDE_TYPES};
use tyra_lexer::TokenKind;

// ── Legend ────────────────────────────────────────────────────────────────────

const TT_KEYWORD: u32 = 0;
const TT_FUNCTION: u32 = 1;
const TT_TYPE: u32 = 2;
const TT_ENUM_MEMBER: u32 = 3;
const TT_PARAMETER: u32 = 4;
const TT_VARIABLE: u32 = 5;
const TT_STRING: u32 = 6;
const TT_NUMBER: u32 = 7;
const TT_COMMENT: u32 = 8;

const MOD_DECLARATION: u32 = 1 << 0;
const MOD_READONLY: u32 = 1 << 1;
const MOD_DEFAULT_LIBRARY: u32 = 1 << 2;
const MOD_ASYNC: u32 = 1 << 3;

pub(crate) fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::KEYWORD,     // 0
            SemanticTokenType::FUNCTION,    // 1
            SemanticTokenType::TYPE,        // 2
            SemanticTokenType::ENUM_MEMBER, // 3
            SemanticTokenType::PARAMETER,   // 4
            SemanticTokenType::VARIABLE,    // 5
            SemanticTokenType::STRING,      // 6
            SemanticTokenType::NUMBER,      // 7
            SemanticTokenType::COMMENT,     // 8
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,     // bit 0
            SemanticTokenModifier::READONLY,        // bit 1
            SemanticTokenModifier::DEFAULT_LIBRARY, // bit 2
            SemanticTokenModifier::ASYNC,           // bit 3
        ],
    }
}

// ── Raw token (absolute UTF-16 position) ─────────────────────────────────────

#[derive(Debug, Clone)]
struct RawToken {
    line: u32,
    col: u32,
    length: u32,
    ty: u32,
    modifiers: u32,
}

/// Convert a `Span` to a single-line `RawToken`. Returns `None` for
/// multi-line spans (forbidden by LSP spec) or spans outside the source.
fn span_to_raw(
    span: Span,
    ty: u32,
    modifiers: u32,
    sources: &SourceMap,
) -> Option<RawToken> {
    let (sl, sc) = sources.line_col_utf16(span.source, span.start)?;
    let (el, ec) = sources.line_col_utf16(span.source, span.end)?;
    if sl != el || ec < sc {
        return None; // multi-line or degenerate
    }
    Some(RawToken { line: sl, col: sc, length: ec - sc, ty, modifiers })
}

// ── DefKind map ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum DefKind {
    Function,
    Type,
    EnumMember,
    Variable,
    Parameter,
}

fn build_def_kind_map(ast: &SourceFile) -> HashMap<Span, DefKind> {
    let mut map = HashMap::new();
    for item in &ast.items {
        visit_item_defs(item, &mut map);
    }
    map
}

fn visit_item_defs(item: &Item, map: &mut HashMap<Span, DefKind>) {
    match item {
        Item::FnDef(f) => {
            map.insert(f.span, DefKind::Function);
            for p in &f.params {
                map.insert(p.span, DefKind::Parameter);
            }
            visit_stmts_defs(&f.body, map);
        }
        Item::DataDef(d) => {
            map.insert(d.span, DefKind::Type);
        }
        Item::ValueDef(v) => {
            map.insert(v.span, DefKind::Type);
        }
        Item::TypeDef(t) => {
            map.insert(t.span, DefKind::Type);
            if let TypeDefKind::Adt(variants) = &t.kind {
                for v in variants {
                    map.insert(v.span, DefKind::EnumMember);
                }
            }
        }
        Item::TraitDef(tr) => {
            map.insert(tr.span, DefKind::Type);
            for m in &tr.methods {
                map.insert(m.span, DefKind::Function);
                for p in &m.params {
                    map.insert(p.span, DefKind::Parameter);
                }
            }
        }
        Item::ImplDef(im) => {
            for m in &im.methods {
                map.insert(m.span, DefKind::Function);
                for p in &m.params {
                    map.insert(p.span, DefKind::Parameter);
                }
                visit_stmts_defs(&m.body, map);
            }
        }
        Item::Import(_) => {}
        Item::Stmt(s) => visit_stmt_defs(s, map),
    }
}

fn visit_stmts_defs(stmts: &[Stmt], map: &mut HashMap<Span, DefKind>) {
    for s in stmts {
        visit_stmt_defs(s, map);
    }
}

fn visit_stmt_defs(stmt: &Stmt, map: &mut HashMap<Span, DefKind>) {
    match stmt {
        Stmt::Let(l) => {
            map.insert(l.span, DefKind::Variable);
            visit_expr_defs(&l.value, map);
        }
        Stmt::Mut(m) => {
            map.insert(m.span, DefKind::Variable);
            visit_expr_defs(&m.value, map);
        }
        Stmt::Return(r) => {
            if let Some(e) = &r.value {
                visit_expr_defs(e, map);
            }
        }
        Stmt::Defer(d) => visit_expr_defs(&d.expr, map),
        Stmt::Break(_) => {}
        Stmt::Expr(e) => visit_expr_defs(&e.expr, map),
    }
}

fn visit_expr_defs(expr: &Expr, map: &mut HashMap<Span, DefKind>) {
    match &expr.kind {
        ExprKind::Lambda(l) => {
            for p in &l.params {
                map.insert(p.span, DefKind::Parameter);
            }
            visit_stmts_defs(&l.body, map);
        }
        ExprKind::If(if_expr) => {
            visit_expr_defs(&if_expr.condition, map);
            visit_stmts_defs(&if_expr.then_body, map);
            match &if_expr.else_body {
                Some(ElseBranch::Else(stmts)) => visit_stmts_defs(stmts, map),
                Some(ElseBranch::ElseIf(nested)) => {
                    visit_expr_defs(
                        &Expr { kind: ExprKind::If(nested.clone()), span: nested.span },
                        map,
                    );
                }
                None => {}
            }
        }
        ExprKind::While(w) => {
            visit_expr_defs(&w.condition, map);
            visit_stmts_defs(&w.body, map);
        }
        ExprKind::For(f) => {
            visit_expr_defs(&f.iter, map);
            visit_stmts_defs(&f.body, map);
        }
        ExprKind::Match(m) => {
            visit_expr_defs(&m.subject, map);
            for arm in &m.arms {
                visit_stmts_defs(&arm.body, map);
            }
        }
        ExprKind::BinaryOp(a, _, b) | ExprKind::Assign(a, b) | ExprKind::Index(a, b) => {
            visit_expr_defs(a, map);
            visit_expr_defs(b, map);
        }
        ExprKind::UnaryOp(_, e)
        | ExprKind::Propagate(e)
        | ExprKind::Await(e)
        | ExprKind::Spawn(e) => visit_expr_defs(e, map),
        ExprKind::Call(callee, args) => {
            visit_expr_defs(callee, map);
            for a in args {
                visit_expr_defs(&a.value, map);
            }
        }
        ExprKind::TurbofishCall(callee, _, args) => {
            visit_expr_defs(callee, map);
            for a in args {
                visit_expr_defs(&a.value, map);
            }
        }
        ExprKind::FieldAccess(e, _) => visit_expr_defs(e, map),
        ExprKind::ListLit(items) => {
            for e in items {
                visit_expr_defs(e, map);
            }
        }
        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                visit_expr_defs(k, map);
                visit_expr_defs(v, map);
            }
        }
        ExprKind::StringInterp(_) => {
            // Subexpression spans live in a synthetic <interp> source — skip.
        }
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit
        | ExprKind::Ident(_) => {}
    }
}

// ── Lexer pass ────────────────────────────────────────────────────────────────

/// Emit tokens from the lexer token stream.
/// Returns (raw tokens, string byte ranges) — the string ranges are used later
/// to exclude `#` characters inside strings from the comment pass.
fn lexer_pass(
    lex_tokens: &[tyra_lexer::Token],
    sources: &SourceMap,
    out: &mut Vec<RawToken>,
) -> Vec<(u32, u32)> {
    let mut string_ranges: Vec<(u32, u32)> = Vec::new();
    let mut prev_kind: Option<&TokenKind> = None;
    // Track most recent `async` keyword for `fn` decoration
    let mut saw_async = false;

    for tok in lex_tokens {
        match &tok.kind {
            // Keywords → KEYWORD
            TokenKind::Fn
            | TokenKind::Data
            | TokenKind::Value
            | TokenKind::Type
            | TokenKind::Trait
            | TokenKind::Impl
            | TokenKind::Let
            | TokenKind::Mut
            | TokenKind::If
            | TokenKind::Else
            | TokenKind::Match
            | TokenKind::When
            | TokenKind::For
            | TokenKind::In
            | TokenKind::While
            | TokenKind::Break
            | TokenKind::Return
            | TokenKind::Defer
            | TokenKind::Async
            | TokenKind::Await
            | TokenKind::Spawn
            | TokenKind::Import
            | TokenKind::Export
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::Not
            | TokenKind::End
            | TokenKind::True
            | TokenKind::False => {
                if matches!(tok.kind, TokenKind::Async) {
                    saw_async = true;
                } else if !matches!(tok.kind, TokenKind::Fn) {
                    // Preserve saw_async through `fn` so `async fn foo` applies
                    // MOD_ASYNC to the function-name identifier that follows.
                    saw_async = false;
                }
                if let Some(raw) = span_to_raw(tok.span, TT_KEYWORD, 0, sources) {
                    out.push(raw);
                }
            }

            // Number literals
            TokenKind::Int(_) | TokenKind::Float(_) => {
                saw_async = false;
                if let Some(raw) = span_to_raw(tok.span, TT_NUMBER, 0, sources) {
                    out.push(raw);
                }
            }

            // String literals
            TokenKind::String(_) | TokenKind::RawString(_) | TokenKind::InterpString(_) => {
                saw_async = false;
                string_ranges.push((tok.span.start, tok.span.end));
                if let Some(raw) = span_to_raw(tok.span, TT_STRING, 0, sources) {
                    out.push(raw);
                }
            }

            // Identifier — classify by preceding keyword
            TokenKind::Ident(_) => {
                match prev_kind {
                    Some(TokenKind::Fn) => {
                        let async_mod = if saw_async { MOD_ASYNC } else { 0 };
                        if let Some(raw) =
                            span_to_raw(tok.span, TT_FUNCTION, MOD_DECLARATION | async_mod, sources)
                        {
                            out.push(raw);
                        }
                    }
                    Some(
                        TokenKind::Data
                        | TokenKind::Value
                        | TokenKind::Type
                        | TokenKind::Trait
                        | TokenKind::Impl,
                    ) => {
                        if let Some(raw) =
                            span_to_raw(tok.span, TT_TYPE, MOD_DECLARATION, sources)
                        {
                            out.push(raw);
                        }
                    }
                    Some(TokenKind::Let) => {
                        if let Some(raw) = span_to_raw(
                            tok.span,
                            TT_VARIABLE,
                            MOD_DECLARATION | MOD_READONLY,
                            sources,
                        ) {
                            out.push(raw);
                        }
                    }
                    Some(TokenKind::Mut) => {
                        if let Some(raw) =
                            span_to_raw(tok.span, TT_VARIABLE, MOD_DECLARATION, sources)
                        {
                            out.push(raw);
                        }
                    }
                    _ => {
                        // Plain ident — emitted by AST use-site pass; skip here.
                    }
                }
                saw_async = false;
                prev_kind = Some(&tok.kind);
                continue;
            }

            // Newline, Eof, Error, punctuation — ignore
            _ => {
                saw_async = false;
            }
        }
        prev_kind = Some(&tok.kind);
    }

    string_ranges
}

// ── AST use-site pass ─────────────────────────────────────────────────────────

fn classify_ident(
    name: &str,
    span: Span,
    def_index: &DefIndex,
    def_kind_map: &HashMap<Span, DefKind>,
) -> (u32, u32) {
    if !name.starts_with("__") {
        if PRELUDE_FUNCTIONS.contains(&name) {
            return (TT_FUNCTION, MOD_DEFAULT_LIBRARY);
        }
    }
    if PRELUDE_TYPES.contains(&name) {
        return (TT_TYPE, MOD_DEFAULT_LIBRARY);
    }
    if PRELUDE_CONSTRUCTORS.contains(&name) {
        return (TT_ENUM_MEMBER, MOD_DEFAULT_LIBRARY);
    }

    if let Some(def_span) = def_index.get(&span) {
        match def_kind_map.get(def_span) {
            Some(DefKind::Function) => return (TT_FUNCTION, 0),
            Some(DefKind::Type) => return (TT_TYPE, 0),
            Some(DefKind::EnumMember) => return (TT_ENUM_MEMBER, 0),
            Some(DefKind::Variable) => return (TT_VARIABLE, 0),
            Some(DefKind::Parameter) => return (TT_PARAMETER, 0),
            None => {}
        }
    }

    (TT_VARIABLE, 0)
}

fn ast_use_site_pass(
    ast: &SourceFile,
    def_index: &DefIndex,
    def_kind_map: &HashMap<Span, DefKind>,
    source_id: SourceId,
    sources: &SourceMap,
    out: &mut Vec<RawToken>,
) {
    for item in &ast.items {
        visit_item_tokens(item, def_index, def_kind_map, source_id, sources, out);
    }
}

fn visit_item_tokens(
    item: &Item,
    def_index: &DefIndex,
    def_kind_map: &HashMap<Span, DefKind>,
    source_id: SourceId,
    sources: &SourceMap,
    out: &mut Vec<RawToken>,
) {
    match item {
        Item::FnDef(f) => {
            for p in &f.params {
                visit_type_expr_tokens(&p.type_annotation, source_id, sources, out);
            }
            if let Some(rt) = &f.return_type {
                visit_type_expr_tokens(rt, source_id, sources, out);
            }
            visit_stmts_tokens(&f.body, def_index, def_kind_map, source_id, sources, out);
        }
        Item::DataDef(d) => {
            for field in &d.fields {
                visit_type_expr_tokens(&field.type_annotation, source_id, sources, out);
            }
        }
        Item::ValueDef(v) => {
            for field in &v.fields {
                visit_type_expr_tokens(&field.type_annotation, source_id, sources, out);
            }
        }
        Item::TypeDef(t) => match &t.kind {
            TypeDefKind::Alias(te) => visit_type_expr_tokens(te, source_id, sources, out),
            TypeDefKind::Adt(variants) => {
                for v in variants {
                    for field in &v.fields {
                        visit_type_expr_tokens(&field.type_annotation, source_id, sources, out);
                    }
                }
            }
        },
        Item::TraitDef(tr) => {
            for m in &tr.methods {
                for p in &m.params {
                    visit_type_expr_tokens(&p.type_annotation, source_id, sources, out);
                }
                if let Some(rt) = &m.return_type {
                    visit_type_expr_tokens(rt, source_id, sources, out);
                }
            }
        }
        Item::ImplDef(im) => {
            visit_type_expr_tokens(&im.target_type, source_id, sources, out);
            for m in &im.methods {
                for p in &m.params {
                    visit_type_expr_tokens(&p.type_annotation, source_id, sources, out);
                }
                if let Some(rt) = &m.return_type {
                    visit_type_expr_tokens(rt, source_id, sources, out);
                }
                visit_stmts_tokens(&m.body, def_index, def_kind_map, source_id, sources, out);
            }
        }
        Item::Import(_) => {}
        Item::Stmt(s) => {
            visit_stmt_tokens(s, def_index, def_kind_map, source_id, sources, out);
        }
    }
}

fn visit_stmts_tokens(
    stmts: &[Stmt],
    def_index: &DefIndex,
    def_kind_map: &HashMap<Span, DefKind>,
    source_id: SourceId,
    sources: &SourceMap,
    out: &mut Vec<RawToken>,
) {
    for s in stmts {
        visit_stmt_tokens(s, def_index, def_kind_map, source_id, sources, out);
    }
}

fn visit_stmt_tokens(
    stmt: &Stmt,
    def_index: &DefIndex,
    def_kind_map: &HashMap<Span, DefKind>,
    source_id: SourceId,
    sources: &SourceMap,
    out: &mut Vec<RawToken>,
) {
    match stmt {
        Stmt::Let(l) => {
            if let Some(ta) = &l.type_annotation {
                visit_type_expr_tokens(ta, source_id, sources, out);
            }
            visit_expr_tokens(&l.value, def_index, def_kind_map, source_id, sources, out);
        }
        Stmt::Mut(m) => {
            if let Some(ta) = &m.type_annotation {
                visit_type_expr_tokens(ta, source_id, sources, out);
            }
            visit_expr_tokens(&m.value, def_index, def_kind_map, source_id, sources, out);
        }
        Stmt::Return(r) => {
            if let Some(e) = &r.value {
                visit_expr_tokens(e, def_index, def_kind_map, source_id, sources, out);
            }
        }
        Stmt::Defer(d) => {
            visit_expr_tokens(&d.expr, def_index, def_kind_map, source_id, sources, out);
        }
        Stmt::Break(_) => {}
        Stmt::Expr(e) => {
            visit_expr_tokens(&e.expr, def_index, def_kind_map, source_id, sources, out);
        }
    }
}

fn visit_expr_tokens(
    expr: &Expr,
    def_index: &DefIndex,
    def_kind_map: &HashMap<Span, DefKind>,
    source_id: SourceId,
    sources: &SourceMap,
    out: &mut Vec<RawToken>,
) {
    match &expr.kind {
        ExprKind::Ident(name) => {
            let (ty, mods) = classify_ident(name, expr.span, def_index, def_kind_map);
            if let Some(raw) = span_to_raw(expr.span, ty, mods, sources) {
                out.push(raw);
            }
        }
        ExprKind::Call(callee, args) => {
            visit_expr_tokens(callee, def_index, def_kind_map, source_id, sources, out);
            for a in args {
                visit_expr_tokens(&a.value, def_index, def_kind_map, source_id, sources, out);
            }
        }
        ExprKind::TurbofishCall(callee, type_args, args) => {
            visit_expr_tokens(callee, def_index, def_kind_map, source_id, sources, out);
            for ta in type_args {
                visit_type_expr_tokens(ta, source_id, sources, out);
            }
            for a in args {
                visit_expr_tokens(&a.value, def_index, def_kind_map, source_id, sources, out);
            }
        }
        ExprKind::FieldAccess(e, _) => {
            visit_expr_tokens(e, def_index, def_kind_map, source_id, sources, out);
            // field name has no dedicated tight span in AST — skip
        }
        ExprKind::BinaryOp(a, _, b) | ExprKind::Assign(a, b) | ExprKind::Index(a, b) => {
            visit_expr_tokens(a, def_index, def_kind_map, source_id, sources, out);
            visit_expr_tokens(b, def_index, def_kind_map, source_id, sources, out);
        }
        ExprKind::UnaryOp(_, e)
        | ExprKind::Propagate(e)
        | ExprKind::Await(e)
        | ExprKind::Spawn(e) => {
            visit_expr_tokens(e, def_index, def_kind_map, source_id, sources, out);
        }
        ExprKind::If(if_expr) => {
            visit_expr_tokens(
                &if_expr.condition,
                def_index,
                def_kind_map,
                source_id,
                sources,
                out,
            );
            visit_stmts_tokens(
                &if_expr.then_body,
                def_index,
                def_kind_map,
                source_id,
                sources,
                out,
            );
            match &if_expr.else_body {
                Some(ElseBranch::Else(stmts)) => {
                    visit_stmts_tokens(stmts, def_index, def_kind_map, source_id, sources, out);
                }
                Some(ElseBranch::ElseIf(nested)) => {
                    visit_expr_tokens(
                        &Expr { kind: ExprKind::If(nested.clone()), span: nested.span },
                        def_index,
                        def_kind_map,
                        source_id,
                        sources,
                        out,
                    );
                }
                None => {}
            }
        }
        ExprKind::While(w) => {
            visit_expr_tokens(
                &w.condition,
                def_index,
                def_kind_map,
                source_id,
                sources,
                out,
            );
            visit_stmts_tokens(&w.body, def_index, def_kind_map, source_id, sources, out);
        }
        ExprKind::For(f) => {
            visit_expr_tokens(&f.iter, def_index, def_kind_map, source_id, sources, out);
            visit_stmts_tokens(&f.body, def_index, def_kind_map, source_id, sources, out);
        }
        ExprKind::Match(m) => {
            visit_expr_tokens(
                &m.subject,
                def_index,
                def_kind_map,
                source_id,
                sources,
                out,
            );
            for arm in &m.arms {
                visit_stmts_tokens(
                    &arm.body,
                    def_index,
                    def_kind_map,
                    source_id,
                    sources,
                    out,
                );
            }
        }
        ExprKind::Lambda(l) => {
            for p in &l.params {
                visit_type_expr_tokens(&p.type_annotation, source_id, sources, out);
            }
            if let Some(rt) = &l.return_type {
                visit_type_expr_tokens(rt, source_id, sources, out);
            }
            visit_stmts_tokens(&l.body, def_index, def_kind_map, source_id, sources, out);
        }
        ExprKind::ListLit(items) => {
            for e in items {
                visit_expr_tokens(e, def_index, def_kind_map, source_id, sources, out);
            }
        }
        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                visit_expr_tokens(k, def_index, def_kind_map, source_id, sources, out);
                visit_expr_tokens(v, def_index, def_kind_map, source_id, sources, out);
            }
        }
        ExprKind::StringInterp(_) => {
            // Interpolated subexpressions are re-parsed into a synthetic <interp>
            // source whose spans are not offsets in the main document.  Walking
            // them would emit tokens at wrong positions and violate the LSP
            // non-overlap / ordering requirement.  The enclosing string span is
            // already emitted as STRING by the lexer pass.
        }
        // Leaf: no sub-expressions to walk
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit => {}
    }
}

fn visit_type_expr_tokens(
    te: &tyra_ast::TypeExpr,
    source_id: SourceId,
    sources: &SourceMap,
    out: &mut Vec<RawToken>,
) {
    match &te.kind {
        TypeExprKind::Named(n) => {
            let mods = if PRELUDE_TYPES.contains(&n.as_str()) { MOD_DEFAULT_LIBRARY } else { 0 };
            if let Some(raw) = span_to_raw(te.span, TT_TYPE, mods, sources) {
                out.push(raw);
            }
        }
        TypeExprKind::Generic(n, args) => {
            // Emit the head name only (not the full `Name<Args>` span).
            let head = Span {
                source: source_id,
                start: te.span.start,
                end: te.span.start + n.len() as u32,
            };
            let mods = if PRELUDE_TYPES.contains(&n.as_str()) { MOD_DEFAULT_LIBRARY } else { 0 };
            if let Some(raw) = span_to_raw(head, TT_TYPE, mods, sources) {
                out.push(raw);
            }
            for a in args {
                visit_type_expr_tokens(a, source_id, sources, out);
            }
        }
        TypeExprKind::Fn(params, ret) => {
            for p in params {
                visit_type_expr_tokens(p, source_id, sources, out);
            }
            visit_type_expr_tokens(ret, source_id, sources, out);
        }
    }
}

// ── Comment pass ──────────────────────────────────────────────────────────────

fn comment_pass(
    text: &str,
    string_ranges: &[(u32, u32)],
    source_id: SourceId,
    sources: &SourceMap,
    out: &mut Vec<RawToken>,
) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i] == b'#' {
            let pos = i as u32;
            if !in_string_range(pos, string_ranges) {
                // Comment from here to end of line
                let start = pos;
                let mut j = i + 1;
                while j < len && bytes[j] != b'\n' {
                    j += 1;
                }
                let end = j as u32;
                let span = Span { source: source_id, start, end };
                if let Some(raw) = span_to_raw(span, TT_COMMENT, 0, sources) {
                    out.push(raw);
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
}

fn in_string_range(pos: u32, ranges: &[(u32, u32)]) -> bool {
    ranges.iter().any(|(s, e)| *s <= pos && pos < *e)
}

// ── Encode & combine ──────────────────────────────────────────────────────────

/// Build the full semantic token list for one document.
pub(crate) fn build_full(
    text: &str,
    ast: &SourceFile,
    def_index: &DefIndex,
    source_id: SourceId,
    sources: &SourceMap,
) -> SemanticTokens {
    let mut report = tyra_diagnostics::Report::new();
    let lex_tokens = tyra_lexer::tokenize(source_id, sources, &mut report);

    let mut raw: Vec<RawToken> = Vec::new();

    // 1. Lexer pass (keywords, literals, def-site idents)
    let string_ranges = lexer_pass(&lex_tokens, sources, &mut raw);

    // 2. AST use-site pass (Ident exprs + TypeExpr names)
    let def_kind_map = build_def_kind_map(ast);
    ast_use_site_pass(ast, def_index, &def_kind_map, source_id, sources, &mut raw);

    // 3. Comment pass
    comment_pass(text, &string_ranges, source_id, sources, &mut raw);

    // 4. Sort by (line, col) then dedup overlaps
    raw.sort_by(|a, b| a.line.cmp(&b.line).then(a.col.cmp(&b.col)));
    raw.dedup_by(|a, b| a.line == b.line && a.col == b.col);

    // 5. Relative encoding
    let mut data: Vec<SemanticToken> = Vec::with_capacity(raw.len());
    let mut prev_line = 0u32;
    let mut prev_col = 0u32;
    for tok in &raw {
        if tok.length == 0 {
            continue;
        }
        let delta_line = tok.line - prev_line;
        let delta_start = if delta_line == 0 {
            tok.col - prev_col
        } else {
            tok.col
        };
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length: tok.length,
            token_type: tok.ty,
            token_modifiers_bitset: tok.modifiers,
        });
        prev_line = tok.line;
        prev_col = tok.col;
    }

    SemanticTokens { result_id: None, data }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_diagnostics::SourceMap;

    fn run(src: &str) -> SemanticTokens {
        let mut sources = SourceMap::new();
        let id = sources.add("t.tyra".into(), src.into());
        let mut report = tyra_diagnostics::Report::new();
        let ast = tyra_parser::parse(id, &sources, &mut report);
        let tyra_driver::CheckResult { def_index, .. } =
            tyra_driver::check_in_memory("t.tyra".into(), src.into(), None);
        build_full(src, &ast, &def_index, id, &sources)
    }

    fn find_token_type<'a>(
        tokens: &'a [SemanticToken],
        ty: u32,
    ) -> Option<&'a SemanticToken> {
        tokens.iter().find(|t| t.token_type == ty)
    }

    #[test]
    fn legend_includes_required_types() {
        let l = legend();
        let type_names: Vec<_> = l.token_types.iter().map(|t| t.as_str()).collect();
        assert!(type_names.contains(&"keyword"), "missing keyword: {type_names:?}");
        assert!(type_names.contains(&"function"), "missing function: {type_names:?}");
        assert!(type_names.contains(&"type"), "missing type: {type_names:?}");
        assert!(type_names.contains(&"variable"), "missing variable: {type_names:?}");
        assert!(type_names.contains(&"string"), "missing string: {type_names:?}");
        assert!(type_names.contains(&"number"), "missing number: {type_names:?}");
        assert!(type_names.contains(&"comment"), "missing comment: {type_names:?}");
    }

    #[test]
    fn keyword_token_emitted() {
        let tokens = run("fn foo() -> Int\n  0\nend\n");
        let kw = find_token_type(&tokens.data, TT_KEYWORD);
        assert!(kw.is_some(), "expected at least one KEYWORD token");
        // First keyword is `fn` on line 0, col 0
        let first_kw = &tokens.data[0];
        assert_eq!(first_kw.token_type, TT_KEYWORD, "first token should be KEYWORD (`fn`)");
        assert_eq!(first_kw.delta_line, 0);
        assert_eq!(first_kw.delta_start, 0);
        assert_eq!(first_kw.length, 2); // "fn"
    }

    #[test]
    fn def_site_fn_name_classified_as_function() {
        let tokens = run("fn foo() -> Int\n  0\nend\n");
        // Find a FUNCTION token
        let func = find_token_type(&tokens.data, TT_FUNCTION);
        assert!(func.is_some(), "expected FUNCTION token for `foo`");
        let func = func.unwrap();
        // It should have the DECLARATION modifier
        assert_ne!(
            func.token_modifiers_bitset & MOD_DECLARATION,
            0,
            "expected DECLARATION modifier on `foo`"
        );
    }

    #[test]
    fn let_binding_classified_as_variable_readonly() {
        let tokens = run("let x: Int = 1\n");
        let var = find_token_type(&tokens.data, TT_VARIABLE);
        assert!(var.is_some(), "expected VARIABLE token for `x`");
        let var = var.unwrap();
        assert_ne!(
            var.token_modifiers_bitset & MOD_READONLY,
            0,
            "expected READONLY modifier on let binding"
        );
        assert_ne!(
            var.token_modifiers_bitset & MOD_DECLARATION,
            0,
            "expected DECLARATION modifier on let binding"
        );
    }

    #[test]
    fn comment_classified() {
        let tokens = run("# hello\nlet x = 1\n");
        let comment = find_token_type(&tokens.data, TT_COMMENT);
        assert!(comment.is_some(), "expected COMMENT token");
        let c = comment.unwrap();
        assert_eq!(c.delta_line, 0, "comment should be on line 0");
        assert_eq!(c.delta_start, 0, "comment should start at col 0");
        assert_eq!(c.length, 7, "comment length should be 7 (`# hello`)");
    }

    #[test]
    fn relative_encoding_correct() {
        // `fn` on line 0 col 0 (len=2), `foo` on line 0 col 3 (len=3)
        let tokens = run("fn foo() -> Int\n  0\nend\n");
        // First token: delta_line=0, delta_start=0 (fn)
        assert_eq!(tokens.data[0].delta_line, 0);
        assert_eq!(tokens.data[0].delta_start, 0);
        // Second token: same line, delta_start >= 2 (space after fn)
        if tokens.data.len() > 1 {
            assert_eq!(tokens.data[1].delta_line, 0, "second token should be on same line");
        }
    }
}
