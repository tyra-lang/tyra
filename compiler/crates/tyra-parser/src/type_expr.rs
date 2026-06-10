// Type expression parsing.
// spec reference: §8 (type system), §8.4 (generics), §9.4 (function types)

use tyra_ast::*;
use tyra_diagnostics::Report;
use tyra_lexer::TokenKind;

use crate::token_stream::TokenStream;

/// Parse a type expression: `Int`, `List<T>`, `fn(Int) -> Bool`, `(A, B)`.
pub fn parse_type(ts: &mut TokenStream, report: &mut Report) -> TypeExpr {
    let start = ts.peek_span();

    match ts.peek().clone() {
        // Tuple type: (A, B, ...)
        TokenKind::LParen => {
            ts.advance(); // consume '('
            let mut elems = Vec::new();
            while !ts.check(&TokenKind::RParen) && !ts.at_eof() {
                elems.push(parse_type(ts, report));
                if !ts.eat(&TokenKind::Comma) {
                    break;
                }
            }
            let end = ts.peek_span();
            ts.expect(&TokenKind::RParen, report);
            if elems.len() < 2 {
                report.add(
                    tyra_diagnostics::Diagnostic::error(
                        "tuple type requires 2 or more elements".to_string(),
                    )
                    .with_code("E0316")
                    .with_label(tyra_diagnostics::Label::new(start, "tuple type here")),
                );
            }
            TypeExpr {
                kind: TypeExprKind::Tuple(elems),
                span: start.merge(end),
            }
        }

        // Function type: fn(A, B) -> C
        TokenKind::Fn => {
            ts.advance();
            ts.expect(&TokenKind::LParen, report);
            let mut param_types = Vec::new();
            while !ts.check(&TokenKind::RParen) && !ts.at_eof() {
                param_types.push(parse_type(ts, report));
                if !ts.eat(&TokenKind::Comma) {
                    break;
                }
            }
            ts.expect(&TokenKind::RParen, report);
            ts.expect(&TokenKind::Arrow, report);
            let return_type = parse_type(ts, report);
            let span = start.merge(return_type.span);
            TypeExpr {
                kind: TypeExprKind::Fn(param_types, Box::new(return_type)),
                span,
            }
        }

        // Named type, possibly generic: `Int`, `List<T>`, `Result<T, E>`
        // Also handles module-qualified types: `server.Request` → `Named("Request")`
        TokenKind::Ident(name) => {
            ts.advance();

            // Qualified type name: module.TypeName → drop module prefix (§13, v0.1)
            let name = if ts.check(&TokenKind::Dot) {
                // peek ahead: if next after '.' is an Ident, it's a qualified type
                ts.advance(); // consume '.'
                ts.expect_ident(report).unwrap_or(name)
            } else {
                name
            };

            if ts.check(&TokenKind::Lt) {
                ts.advance(); // consume '<'
                let mut args = vec![parse_type(ts, report)];
                while ts.eat(&TokenKind::Comma) {
                    args.push(parse_type(ts, report));
                }
                let end = ts.peek_span();
                ts.expect(&TokenKind::Gt, report);
                TypeExpr {
                    kind: TypeExprKind::Generic(name, args),
                    span: start.merge(end),
                }
            } else {
                TypeExpr {
                    kind: TypeExprKind::Named(name),
                    span: start,
                }
            }
        }

        _ => {
            let token = ts.advance();
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "expected type, found {}",
                    crate::token_stream::kind_name(&token.kind),
                ))
                .with_code("E0107")
                .with_label(tyra_diagnostics::Label::new(token.span, "expected type")),
            );
            TypeExpr {
                kind: TypeExprKind::Named("_error".into()),
                span: start,
            }
        }
    }
}

/// Parse optional type parameters: `<T>`, `<T: Eq>`, `<T: Eq + Hash>`.
pub fn parse_type_params(ts: &mut TokenStream, report: &mut Report) -> Vec<TypeParam> {
    if !ts.check(&TokenKind::Lt) {
        return vec![];
    }
    ts.advance(); // consume '<'

    let mut params = Vec::new();
    loop {
        let start = ts.peek_span();
        let name = ts.expect_ident(report).unwrap_or_default();

        let constraints = if ts.check(&TokenKind::Colon) {
            ts.advance();
            let mut cs = vec![parse_type(ts, report)];
            if ts.check(&TokenKind::Plus) {
                ts.advance();
                cs.push(parse_type(ts, report));
            }
            cs
        } else {
            vec![]
        };

        let end = ts.peek_span();
        params.push(TypeParam {
            name,
            constraints,
            span: start.merge(end),
        });

        if !ts.eat(&TokenKind::Comma) {
            break;
        }
    }

    ts.expect(&TokenKind::Gt, report);
    params
}
