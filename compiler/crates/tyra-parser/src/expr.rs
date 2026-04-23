// Expression parser using Pratt parsing for operator precedence.
// spec reference: §7 (literals), §9 (calls), §10 (control flow), §11 (collections),
//                 §12.2 (?), §14.3 (.await), §14.4 (spawn)

use tyra_ast::*;
use tyra_diagnostics::Report;
use tyra_lexer::TokenKind;

use crate::token_stream::TokenStream;

/// Operator precedence levels (higher = tighter binding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    None = 0,
    Assignment = 1, // =
    Or = 2,         // or
    And = 3,        // and
    Equality = 4,   // == != ===
    Comparison = 5, // < <= > >=
    Term = 6,       // + -
    Factor = 7,     // * /
    Unary = 8,      // - not
    Postfix = 9,    // ? .await .field call[] ()
}

fn infix_precedence(kind: &TokenKind) -> Option<Prec> {
    match kind {
        // Note: TokenKind::Eq (assignment) is handled as a special case in
        // parse_expr_prec, not via the infix operator path.
        TokenKind::Or => Some(Prec::Or),
        TokenKind::And => Some(Prec::And),
        TokenKind::EqEq | TokenKind::BangEq | TokenKind::EqEqEq => Some(Prec::Equality),
        TokenKind::Lt | TokenKind::LtEq | TokenKind::Gt | TokenKind::GtEq => Some(Prec::Comparison),
        TokenKind::Plus | TokenKind::Minus => Some(Prec::Term),
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => Some(Prec::Factor),
        // Postfix operators are handled separately in parse_postfix
        _ => None,
    }
}

fn token_to_binop(kind: &TokenKind) -> Option<BinOp> {
    match kind {
        TokenKind::Plus => Some(BinOp::Add),
        TokenKind::Minus => Some(BinOp::Sub),
        TokenKind::Star => Some(BinOp::Mul),
        TokenKind::Slash => Some(BinOp::Div),
        TokenKind::Percent => Some(BinOp::Rem),
        TokenKind::EqEq => Some(BinOp::Eq),
        TokenKind::BangEq => Some(BinOp::NotEq),
        TokenKind::Lt => Some(BinOp::Lt),
        TokenKind::LtEq => Some(BinOp::LtEq),
        TokenKind::Gt => Some(BinOp::Gt),
        TokenKind::GtEq => Some(BinOp::GtEq),
        TokenKind::EqEqEq => Some(BinOp::RefEq),
        TokenKind::And => Some(BinOp::And),
        TokenKind::Or => Some(BinOp::Or),
        _ => None,
    }
}

/// Parse an expression at the given minimum precedence.
pub fn parse_expr(ts: &mut TokenStream, report: &mut Report) -> Expr {
    parse_expr_prec(ts, report, Prec::None)
}

fn parse_expr_prec(ts: &mut TokenStream, report: &mut Report, min_prec: Prec) -> Expr {
    let mut left = parse_prefix(ts, report);

    loop {
        // Postfix operators: ?, .await, .field, (), []
        left = parse_postfix(ts, report, left);

        // Assignment: lhs = rhs (right-associative, lowest precedence)
        if matches!(ts.peek(), TokenKind::Eq) && min_prec <= Prec::Assignment {
            ts.advance();
            let right = parse_expr_prec(ts, report, Prec::Assignment);
            let span = left.span.merge(right.span);
            left = Expr {
                kind: ExprKind::Assign(Box::new(left), Box::new(right)),
                span,
            };
            continue;
        }

        // Infix binary operators
        let Some(prec) = infix_precedence(ts.peek()) else {
            break;
        };
        if prec <= min_prec || prec == Prec::Assignment {
            break;
        }

        let op_token = ts.advance();
        let op = token_to_binop(&op_token.kind).unwrap();
        let right = parse_expr_prec(ts, report, prec);
        let span = left.span.merge(right.span);
        left = Expr {
            kind: ExprKind::BinaryOp(Box::new(left), op, Box::new(right)),
            span,
        };
    }

    left
}

/// Parse postfix operators: .field, .await, ?, (), [], turbofish
fn parse_postfix(ts: &mut TokenStream, report: &mut Report, mut expr: Expr) -> Expr {
    loop {
        match ts.peek() {
            // .field or .await
            TokenKind::Dot => {
                ts.advance();
                match ts.peek().clone() {
                    TokenKind::Await => {
                        let end = ts.advance().span;
                        let span = expr.span.merge(end);
                        expr = Expr {
                            kind: ExprKind::Await(Box::new(expr)),
                            span,
                        };
                    }
                    TokenKind::Ident(name) => {
                        let end = ts.advance().span;
                        let span = expr.span.merge(end);
                        expr = Expr {
                            kind: ExprKind::FieldAccess(Box::new(expr), name),
                            span,
                        };
                    }
                    _ => {
                        let span = ts.peek_span();
                        report.add(
                            tyra_diagnostics::Diagnostic::error(
                                "expected field name or `await` after `.`",
                            )
                            .with_code("E0103")
                            .with_label(tyra_diagnostics::Label::new(span, "expected identifier")),
                        );
                        break;
                    }
                }
            }
            // ? propagation (§12.2)
            TokenKind::Question => {
                let end = ts.advance().span;
                let span = expr.span.merge(end);
                expr = Expr {
                    kind: ExprKind::Propagate(Box::new(expr)),
                    span,
                };
            }
            // Function call: expr(args)
            TokenKind::LParen => {
                let args = parse_call_args(ts, report);
                let end = ts.peek_span();
                let span = expr.span.merge(end);
                expr = Expr {
                    kind: ExprKind::Call(Box::new(expr), args),
                    span,
                };
            }
            // Turbofish: expr::<Type>(args) (§8.4)
            TokenKind::ColonColon => {
                ts.advance(); // consume '::'
                ts.expect(&TokenKind::Lt, report);
                let mut type_args = vec![crate::type_expr::parse_type(ts, report)];
                while ts.eat(&TokenKind::Comma) {
                    type_args.push(crate::type_expr::parse_type(ts, report));
                }
                ts.expect(&TokenKind::Gt, report);
                let args = parse_call_args(ts, report);
                let end = ts.peek_span();
                let span = expr.span.merge(end);
                expr = Expr {
                    kind: ExprKind::TurbofishCall(Box::new(expr), type_args, args),
                    span,
                };
            }
            // Index: expr[index]
            TokenKind::LBracket => {
                ts.advance();
                let index = parse_expr(ts, report);
                let end_span = ts.peek_span();
                ts.expect(&TokenKind::RBracket, report);
                let span = expr.span.merge(end_span);
                expr = Expr {
                    kind: ExprKind::Index(Box::new(expr), Box::new(index)),
                    span,
                };
            }
            _ => break,
        }
    }
    expr
}

/// Parse prefix expressions: literals, identifiers, unary ops, control flow, spawn.
fn parse_prefix(ts: &mut TokenStream, report: &mut Report) -> Expr {
    let token = ts.peek().clone();
    let start = ts.peek_span();

    match token {
        // Literals
        TokenKind::Int(n) => {
            ts.advance();
            Expr {
                kind: ExprKind::IntLit(n),
                span: start,
            }
        }
        TokenKind::Float(f) => {
            ts.advance();
            Expr {
                kind: ExprKind::FloatLit(f),
                span: start,
            }
        }
        TokenKind::String(s) => {
            ts.advance();
            Expr {
                kind: ExprKind::StringLit(s),
                span: start,
            }
        }
        TokenKind::InterpString(parts) => {
            ts.advance();
            let ast_parts: Vec<tyra_ast::StringPart> = parts
                .into_iter()
                .map(|part| match part {
                    tyra_lexer::InterpPart::Lit(s) => tyra_ast::StringPart::Lit(s),
                    tyra_lexer::InterpPart::Expr(expr_text) => {
                        // Re-lex and parse the expression text
                        let mut sources = tyra_diagnostics::SourceMap::new();
                        let id = sources.add("<interp>".into(), expr_text);
                        let mut inner_report = tyra_diagnostics::Report::new();
                        let tokens = tyra_lexer::tokenize(id, &sources, &mut inner_report);
                        let mut inner_ts = crate::token_stream::TokenStream::new(tokens);
                        let expr = parse_expr(&mut inner_ts, &mut inner_report);
                        // TODO: propagate inner_report errors to outer report
                        tyra_ast::StringPart::Expr(expr)
                    }
                })
                .collect();
            Expr {
                kind: ExprKind::StringInterp(ast_parts),
                span: start,
            }
        }
        TokenKind::RawString(s) => {
            ts.advance();
            Expr {
                kind: ExprKind::StringLit(s),
                span: start,
            }
        }
        TokenKind::True => {
            ts.advance();
            Expr {
                kind: ExprKind::BoolLit(true),
                span: start,
            }
        }
        TokenKind::False => {
            ts.advance();
            Expr {
                kind: ExprKind::BoolLit(false),
                span: start,
            }
        }

        // Identifier
        TokenKind::Ident(name) => {
            ts.advance();
            Expr {
                kind: ExprKind::Ident(name),
                span: start,
            }
        }

        // Contextual keywords usable as identifiers in expression
        // position. `value` / `data` / `type` / `trait` / `impl` are
        // reserved at the top level for declarations (§5.2 / §8.5) but
        // have no expression-level role, so accepting them as Ident here
        // lets AI-generated code like `let value = ...; println("#{value}")`
        // parse. `async` / `await` / `spawn` / `mut` / `import` / `export`
        // / `defer` are NOT included — they have statement/expression
        // syntactic roles (e.g. `t.await`, `spawn f()`).
        TokenKind::Value | TokenKind::Data | TokenKind::Type
        | TokenKind::Trait | TokenKind::Impl => {
            let name = crate::token_stream::keyword_as_ident(ts.peek())
                .map(str::to_string)
                .unwrap_or_default();
            ts.advance();
            Expr {
                kind: ExprKind::Ident(name),
                span: start,
            }
        }

        // Unary minus: -expr
        TokenKind::Minus => {
            ts.advance();
            let operand = parse_expr_prec(ts, report, Prec::Unary);
            let span = start.merge(operand.span);
            Expr {
                kind: ExprKind::UnaryOp(UnaryOp::Neg, Box::new(operand)),
                span,
            }
        }

        // Unary not: not expr (§10.1)
        TokenKind::Not => {
            ts.advance();
            let operand = parse_expr_prec(ts, report, Prec::Unary);
            let span = start.merge(operand.span);
            Expr {
                kind: ExprKind::UnaryOp(UnaryOp::Not, Box::new(operand)),
                span,
            }
        }

        // Parenthesized expression or Unit literal ()
        TokenKind::LParen => {
            ts.advance();
            if ts.check(&TokenKind::RParen) {
                let end = ts.advance().span; // consume ')' and capture its span
                Expr {
                    kind: ExprKind::UnitLit,
                    span: start.merge(end),
                }
            } else {
                let inner = parse_expr(ts, report);
                ts.expect(&TokenKind::RParen, report);
                inner
            }
        }

        // List literal: [a, b, c]
        TokenKind::LBracket => {
            ts.advance();
            let items = parse_comma_separated(ts, report, &TokenKind::RBracket, parse_expr);
            let end = ts.peek_span();
            ts.expect(&TokenKind::RBracket, report);
            Expr {
                kind: ExprKind::ListLit(items),
                span: start.merge(end),
            }
        }

        // Map literal: {k: v, ...}
        TokenKind::LBrace => {
            ts.advance();
            let entries = parse_comma_separated(ts, report, &TokenKind::RBrace, |ts, r| {
                let key = parse_expr(ts, r);
                ts.expect(&TokenKind::Colon, r);
                let value = parse_expr(ts, r);
                (key, value)
            });
            let end = ts.peek_span();
            ts.expect(&TokenKind::RBrace, report);
            Expr {
                kind: ExprKind::MapLit(entries),
                span: start.merge(end),
            }
        }

        // if expression (§10.2)
        TokenKind::If => {
            let if_expr = crate::stmt::parse_if(ts, report);
            Expr {
                span: if_expr.span,
                kind: ExprKind::If(Box::new(if_expr)),
            }
        }

        // match expression (§10.3)
        TokenKind::Match => {
            let match_expr = crate::stmt::parse_match(ts, report);
            Expr {
                span: match_expr.span,
                kind: ExprKind::Match(Box::new(match_expr)),
            }
        }

        // for loop (§10.5)
        TokenKind::For => {
            let for_expr = crate::stmt::parse_for(ts, report);
            Expr {
                span: for_expr.span,
                kind: ExprKind::For(Box::new(for_expr)),
            }
        }

        // while loop (§10.4)
        TokenKind::While => {
            let while_expr = crate::stmt::parse_while(ts, report);
            Expr {
                span: while_expr.span,
                kind: ExprKind::While(Box::new(while_expr)),
            }
        }

        // spawn f(args) (§14.4)
        TokenKind::Spawn => {
            ts.advance();
            let call = parse_expr_prec(ts, report, Prec::Postfix);
            let span = start.merge(call.span);
            Expr {
                kind: ExprKind::Spawn(Box::new(call)),
                span,
            }
        }

        // Anonymous function: fn(...) -> T ... end (§9.4)
        TokenKind::Fn => {
            let lambda = crate::decl::parse_lambda(ts, report);
            Expr {
                span: lambda.span,
                kind: ExprKind::Lambda(Box::new(lambda)),
            }
        }

        // Error recovery
        _ => {
            let t = ts.advance();
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "expected expression, found {}",
                    crate::token_stream::kind_name(&t.kind)
                ))
                .with_code("E0104")
                .with_label(tyra_diagnostics::Label::new(t.span, "expected expression")),
            );
            Expr {
                kind: ExprKind::IntLit(0),
                span: start,
            }
        }
    }
}

/// Parse call arguments: `(label: expr, expr, ...)`
fn parse_call_args(ts: &mut TokenStream, report: &mut Report) -> Vec<Arg> {
    ts.expect(&TokenKind::LParen, report);
    let args = parse_comma_separated(ts, report, &TokenKind::RParen, |ts, r| {
        let start = ts.peek_span();

        // Check for keyword-as-label: `value: expr`, `type: expr`, etc.
        if let Some(kw_name) = crate::token_stream::keyword_as_ident(ts.peek()) {
            // Peek ahead: is the next-next token a colon?
            if ts.peek_ahead_is_colon() {
                let label = kw_name.to_string();
                ts.advance(); // consume keyword
                ts.advance(); // consume ':'
                let value = parse_expr(ts, r);
                let span = start.merge(value.span);
                return Arg {
                    label: Some(label),
                    value,
                    span,
                };
            }
        }

        let first = parse_expr(ts, r);

        // Check if this is a labeled argument: `name: value`
        if ts.check(&TokenKind::Colon)
            && let ExprKind::Ident(label) = first.kind
        {
            ts.advance(); // consume ':'
            let value = parse_expr(ts, r);
            let span = start.merge(value.span);
            return Arg {
                label: Some(label),
                value,
                span,
            };
        }

        Arg {
            label: None,
            span: first.span,
            value: first,
        }
    });
    ts.expect(&TokenKind::RParen, report);
    args
}

/// Parse a comma-separated list until the closing token.
/// Handles trailing commas.
fn parse_comma_separated<T>(
    ts: &mut TokenStream,
    report: &mut Report,
    closing: &TokenKind,
    mut parse_item: impl FnMut(&mut TokenStream, &mut Report) -> T,
) -> Vec<T> {
    let mut items = Vec::new();
    while !ts.check(closing) && !ts.at_eof() {
        items.push(parse_item(ts, report));
        if !ts.eat(&TokenKind::Comma) {
            break;
        }
    }
    items
}
