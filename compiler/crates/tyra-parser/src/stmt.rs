// Statement and control flow parsing.
// spec reference: §7.1 (let/mut), §9.5 (return), §10 (if/match/for/while), §12.3 (defer)

use tyra_ast::*;
use tyra_diagnostics::Report;
use tyra_lexer::TokenKind;

use crate::expr::parse_expr;
use crate::token_stream::TokenStream;

/// Parse a statement.
pub fn parse_stmt(ts: &mut TokenStream, report: &mut Report) -> Stmt {
    match ts.peek() {
        TokenKind::Let => parse_let(ts, report),
        TokenKind::Mut => parse_mut(ts, report),
        TokenKind::Return => parse_return(ts, report),
        TokenKind::Defer => parse_defer(ts, report),
        TokenKind::Break => parse_break(ts, report),
        TokenKind::Continue => parse_continue(ts, report),
        // `import`/`export` inside a function body is invalid (§13.2).
        // Emit E0110 with a clear message and skip to end-of-line so we
        // don't cascade into E0101 ("expected newline") on the module path.
        TokenKind::Import | TokenKind::Export => {
            let span = ts.peek_span();
            let kw = match ts.peek() {
                TokenKind::Import => "import",
                _ => "export",
            };
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "`{kw}` statements must appear at the top of the file, \
                     not inside a function body"
                ))
                .with_code("E0110")
                .with_label(tyra_diagnostics::Label::new(
                    span,
                    format!("move this `{kw}` to the top of the file, before any `fn` definitions"),
                )),
            );
            // Skip remaining tokens on this line to suppress cascade errors.
            while !matches!(
                ts.peek(),
                TokenKind::Newline
                    | TokenKind::Eof
                    | TokenKind::End
                    | TokenKind::Else
                    | TokenKind::When
            ) {
                ts.advance();
            }
            Stmt::Expr(ExprStmt {
                expr: Expr {
                    kind: ExprKind::UnitLit,
                    span,
                },
                span,
            })
        }
        _ => {
            let start = ts.peek_span();
            let expr = parse_expr(ts, report);
            let span = start.merge(expr.span);
            ts.expect_newline_or_eof(report);
            Stmt::Expr(ExprStmt { expr, span })
        }
    }
}

/// Parse a block of statements until `end` or another closing keyword.
pub fn parse_body(ts: &mut TokenStream, report: &mut Report) -> Vec<Stmt> {
    let mut stmts = Vec::new();
    ts.skip_newlines();
    while !matches!(
        ts.peek(),
        TokenKind::End | TokenKind::Else | TokenKind::When | TokenKind::Eof
    ) {
        stmts.push(parse_stmt(ts, report));
        ts.skip_newlines();
    }
    stmts
}

// -- Statement parsers --

fn parse_let(ts: &mut TokenStream, report: &mut Report) -> Stmt {
    let start = ts.advance().span; // consume 'let'
    let name = ts.expect_ident_or_field_keyword(report).unwrap_or_default();
    let type_annotation = parse_optional_type_annotation(ts, report);
    ts.expect(&TokenKind::Eq, report);
    let value = parse_expr(ts, report);
    let span = start.merge(value.span);
    ts.expect_newline_or_eof(report);
    Stmt::Let(LetStmt {
        name,
        type_annotation,
        value,
        span,
    })
}

fn parse_mut(ts: &mut TokenStream, report: &mut Report) -> Stmt {
    let start = ts.advance().span; // consume 'mut'
    let name = ts.expect_ident_or_field_keyword(report).unwrap_or_default();
    let type_annotation = parse_optional_type_annotation(ts, report);
    ts.expect(&TokenKind::Eq, report);
    let value = parse_expr(ts, report);
    let span = start.merge(value.span);
    ts.expect_newline_or_eof(report);
    Stmt::Mut(MutStmt {
        name,
        type_annotation,
        value,
        span,
    })
}

fn parse_return(ts: &mut TokenStream, report: &mut Report) -> Stmt {
    let start = ts.advance().span; // consume 'return'
    // return with no value: next token is newline or eof
    let value = if matches!(
        ts.peek(),
        TokenKind::Newline | TokenKind::Eof | TokenKind::End | TokenKind::Else | TokenKind::When
    ) {
        None
    } else {
        Some(parse_expr(ts, report))
    };
    let span = match &value {
        Some(v) => start.merge(v.span),
        None => start,
    };
    ts.expect_newline_or_eof(report);
    Stmt::Return(ReturnStmt { value, span })
}

fn parse_defer(ts: &mut TokenStream, report: &mut Report) -> Stmt {
    let start = ts.advance().span; // consume 'defer'
    let expr = parse_expr(ts, report);
    let span = start.merge(expr.span);
    ts.expect_newline_or_eof(report);
    Stmt::Defer(DeferStmt { expr, span })
}

fn parse_break(ts: &mut TokenStream, report: &mut Report) -> Stmt {
    let span = ts.advance().span; // consume 'break'
    ts.expect_newline_or_eof(report);
    Stmt::Break(BreakStmt { span })
}

fn parse_continue(ts: &mut TokenStream, report: &mut Report) -> Stmt {
    let span = ts.advance().span; // consume 'continue'
    ts.expect_newline_or_eof(report);
    Stmt::Continue(ContinueStmt { span })
}

// -- Control flow --

/// Parse `if cond body [else if ... | else ...] end` (§10.2)
///
/// Supports both block form (newline after condition) and inline form
/// (`if cond expr else expr end`) where the body is a single expression
/// on the same line. The inline form is accepted because `when` arms
/// already allow it and AI-generated code uses both styles.
pub fn parse_if(ts: &mut TokenStream, report: &mut Report) -> IfExpr {
    let start = ts.advance().span; // consume 'if'
    let condition = parse_expr(ts, report);

    // Block form when newline follows; inline form otherwise.
    // Else/End/Eof after the condition means an empty then-body (unusual but valid).
    let then_body = if ts.eat_newline()
        || ts.check(&TokenKind::Eof)
        || ts.check(&TokenKind::End)
        || ts.check(&TokenKind::Else)
    {
        parse_body(ts, report)
    } else {
        // Inline: single expression terminated by else/end (both accepted by
        // expect_newline_or_eof as implicit block terminators).
        let expr_start = ts.peek_span();
        let expr = parse_expr(ts, report);
        let stmt_span = expr_start.merge(expr.span);
        ts.skip_newlines();
        vec![Stmt::Expr(ExprStmt {
            expr,
            span: stmt_span,
        })]
    };

    let else_body = if ts.check(&TokenKind::Else) {
        ts.advance(); // consume 'else'
        if ts.check(&TokenKind::If) {
            // else if chain
            let inner = parse_if(ts, report);
            Some(ElseBranch::ElseIf(Box::new(inner)))
        } else {
            // else block (inline else already works: expect_newline_or_eof
            // accepts End as an implicit terminator so parse_body handles it)
            ts.skip_newlines();
            let body = parse_body(ts, report);
            ts.expect(&TokenKind::End, report);
            Some(ElseBranch::Else(body))
        }
    } else {
        ts.expect(&TokenKind::End, report);
        None
    };

    let end = ts.peek_span();
    IfExpr {
        condition,
        then_body,
        else_body,
        span: start.merge(end),
    }
}

/// Parse `match expr when ... end` (§10.3)
pub fn parse_match(ts: &mut TokenStream, report: &mut Report) -> MatchExpr {
    let start = ts.advance().span; // consume 'match'
    let subject = parse_expr(ts, report);
    ts.expect_newline_or_eof(report);
    ts.skip_newlines();

    let mut arms = Vec::new();
    while ts.check(&TokenKind::When) {
        arms.push(parse_match_arm(ts, report));
        ts.skip_newlines();
    }

    ts.expect(&TokenKind::End, report);
    let end = ts.peek_span();
    MatchExpr {
        subject,
        arms,
        span: start.merge(end),
    }
}

fn parse_match_arm(ts: &mut TokenStream, report: &mut Report) -> MatchArm {
    let start = ts.advance().span; // consume 'when'
    let pattern = crate::pattern::parse_pattern(ts, report);
    // No newline required after the pattern. parse_body skips leading
    // newlines and stops at `when` / `end` / `else` / `eof`, so both
    // multi-line `when PAT NL <stmts>` and single-line `when PAT EXPR`
    // forms work.
    let body = parse_body(ts, report);
    let span = if let Some(last) = body.last() {
        start.merge(stmt_span(last))
    } else {
        start
    };
    MatchArm {
        pattern,
        body,
        span,
    }
}

/// Parse `for binding in iter body end` (§10.5)
pub fn parse_for(ts: &mut TokenStream, report: &mut Report) -> ForExpr {
    let start = ts.advance().span; // consume 'for'
    let binding = ts.expect_ident(report).unwrap_or_default();
    ts.expect(&TokenKind::In, report);
    let iter = parse_expr(ts, report);
    ts.expect_newline_or_eof(report);
    let body = parse_body(ts, report);
    ts.expect(&TokenKind::End, report);
    let end = ts.peek_span();
    ForExpr {
        binding,
        iter,
        body,
        span: start.merge(end),
    }
}

/// Parse `while cond body end` (§10.4)
pub fn parse_while(ts: &mut TokenStream, report: &mut Report) -> WhileExpr {
    let start = ts.advance().span; // consume 'while'
    let condition = parse_expr(ts, report);
    ts.expect_newline_or_eof(report);
    let body = parse_body(ts, report);
    ts.expect(&TokenKind::End, report);
    let end = ts.peek_span();
    WhileExpr {
        condition,
        body,
        span: start.merge(end),
    }
}

// -- Helpers --

/// Parse optional `: Type` annotation.
fn parse_optional_type_annotation(ts: &mut TokenStream, report: &mut Report) -> Option<TypeExpr> {
    if ts.check(&TokenKind::Colon) {
        ts.advance();
        Some(crate::type_expr::parse_type(ts, report))
    } else {
        None
    }
}

fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Let(s) => s.span,
        Stmt::Mut(s) => s.span,
        Stmt::Return(s) => s.span,
        Stmt::Defer(s) => s.span,
        Stmt::Break(s) => s.span,
        Stmt::Continue(s) => s.span,
        Stmt::Expr(s) => s.span,
    }
}
