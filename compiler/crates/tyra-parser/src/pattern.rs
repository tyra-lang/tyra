// Pattern parsing for match arms.
// spec reference: §10.3 (match), §8.5 (constructor patterns)

use tyra_ast::*;
use tyra_diagnostics::Report;
use tyra_lexer::TokenKind;

use crate::token_stream::TokenStream;

/// Parse a pattern: wildcard, literal, identifier, or constructor.
pub fn parse_pattern(ts: &mut TokenStream, report: &mut Report) -> Pattern {
    let start = ts.peek_span();

    match ts.peek().clone() {
        // Wildcard: _
        TokenKind::Ident(ref s) if s == "_" => {
            ts.advance();
            Pattern {
                kind: PatternKind::Wildcard,
                span: start,
            }
        }

        // Integer literal
        TokenKind::Int(n) => {
            ts.advance();
            Pattern {
                kind: PatternKind::IntLit(n),
                span: start,
            }
        }

        // Negative integer literal: -42
        TokenKind::Minus => {
            ts.advance();
            if let TokenKind::Int(n) = ts.peek().clone() {
                let end = ts.advance().span;
                Pattern {
                    kind: PatternKind::IntLit(-n),
                    span: start.merge(end),
                }
            } else {
                report.add(
                    tyra_diagnostics::Diagnostic::error("expected integer after `-` in pattern")
                        .with_code("E0105")
                        .with_label(tyra_diagnostics::Label::new(start, "expected integer")),
                );
                Pattern {
                    kind: PatternKind::Wildcard,
                    span: start,
                }
            }
        }

        // Float literal
        TokenKind::Float(f) => {
            ts.advance();
            Pattern {
                kind: PatternKind::FloatLit(f),
                span: start,
            }
        }

        // String literal
        TokenKind::String(s) => {
            ts.advance();
            Pattern {
                kind: PatternKind::StringLit(s),
                span: start,
            }
        }

        // Boolean literal
        TokenKind::True => {
            ts.advance();
            Pattern {
                kind: PatternKind::BoolLit(true),
                span: start,
            }
        }
        TokenKind::False => {
            ts.advance();
            Pattern {
                kind: PatternKind::BoolLit(false),
                span: start,
            }
        }

        // Identifier: could be a binding or a constructor
        TokenKind::Ident(name) => {
            ts.advance();

            // Constructor pattern: Name(fields...) or Name
            if ts.check(&TokenKind::LParen) {
                ts.advance(); // consume '('
                let fields = parse_pattern_fields(ts, report);
                let end = ts.peek_span();
                ts.expect(&TokenKind::RParen, report);
                Pattern {
                    kind: PatternKind::Constructor(name, fields),
                    span: start.merge(end),
                }
            } else {
                // Simple identifier or unit constructor (e.g., `None`, `Cash`)
                // Whether it's a binding or constructor is determined later by the type checker.
                // We use Ident for lowercase names and Constructor (with no fields) for PascalCase.
                if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                    Pattern {
                        kind: PatternKind::Constructor(name, vec![]),
                        span: start,
                    }
                } else {
                    Pattern {
                        kind: PatternKind::Ident(name),
                        span: start,
                    }
                }
            }
        }

        // Tuple pattern: (a, b, ...)
        TokenKind::LParen => {
            ts.advance(); // consume '('
            let mut elems = Vec::new();
            while !ts.check(&TokenKind::RParen) && !ts.at_eof() {
                elems.push(parse_pattern(ts, report));
                if !ts.eat(&TokenKind::Comma) {
                    break;
                }
            }
            let end = ts.peek_span();
            ts.expect(&TokenKind::RParen, report);
            if elems.len() < 2 {
                report.add(
                    tyra_diagnostics::Diagnostic::error(
                        "tuple pattern requires 2 or more elements".to_string(),
                    )
                    .with_code("E0316")
                    .with_label(tyra_diagnostics::Label::new(start, "tuple pattern here")),
                );
            }
            Pattern {
                kind: PatternKind::Tuple(elems),
                span: start.merge(end),
            }
        }

        _ => {
            let token = ts.advance();
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "expected pattern, found {}",
                    crate::token_stream::kind_name(&token.kind),
                ))
                .with_code("E0106")
                .with_label(tyra_diagnostics::Label::new(token.span, "expected pattern")),
            );
            Pattern {
                kind: PatternKind::Wildcard,
                span: start,
            }
        }
    }
}

/// Parse constructor pattern fields: `field: pattern, ...` or shorthand `name`.
/// The spec (§8.5) says `when Card(last4)` is shorthand for `when Card(last4: last4)`.
fn parse_pattern_fields(ts: &mut TokenStream, report: &mut Report) -> Vec<PatternField> {
    let mut fields = Vec::new();
    while !ts.check(&TokenKind::RParen) && !ts.at_eof() {
        let start = ts.peek_span();

        // Try to parse as `name: pattern` (accepting keywords as field names)
        let maybe_name = match ts.peek().clone() {
            TokenKind::Ident(name) => Some(name),
            ref kw => crate::token_stream::keyword_as_ident(kw).map(String::from),
        };
        if let Some(name) = maybe_name {
            ts.advance();

            if ts.check(&TokenKind::Colon) {
                // Explicit: `field_name: pattern`
                ts.advance();
                let pattern = parse_pattern(ts, report);
                let span = start.merge(pattern.span);
                fields.push(PatternField {
                    field_name: name,
                    pattern,
                    span,
                });
            } else {
                // Shorthand: `name` → `name: name` (§8.5)
                let pattern = if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                    // If PascalCase, it's a nested constructor pattern
                    if ts.check(&TokenKind::LParen) {
                        ts.advance();
                        let nested_fields = parse_pattern_fields(ts, report);
                        let end = ts.peek_span();
                        ts.expect(&TokenKind::RParen, report);
                        Pattern {
                            kind: PatternKind::Constructor(name.clone(), nested_fields),
                            span: start.merge(end),
                        }
                    } else {
                        Pattern {
                            kind: PatternKind::Constructor(name.clone(), vec![]),
                            span: start,
                        }
                    }
                } else {
                    Pattern {
                        kind: PatternKind::Ident(name.clone()),
                        span: start,
                    }
                };
                fields.push(PatternField {
                    field_name: name,
                    pattern,
                    span: start,
                });
            }
        } else {
            // Non-identifier in field position — parse as pattern
            let pattern = parse_pattern(ts, report);
            let span = pattern.span;
            fields.push(PatternField {
                field_name: String::new(),
                pattern,
                span,
            });
        }

        if !ts.eat(&TokenKind::Comma) {
            break;
        }
    }
    fields
}
