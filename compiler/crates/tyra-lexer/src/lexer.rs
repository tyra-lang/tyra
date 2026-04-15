// The main lexer: converts source text into a sequence of tokens.
// spec reference: §5 (lexical rules), §7.2 (literals), §7.3 (strings)

use tyra_diagnostics::{Diagnostic, Label, Report, SourceId, SourceMap, Span};

use crate::cursor::Cursor;
use crate::token::{Token, TokenKind};

/// Tokenize a source file, returning all tokens and any diagnostics.
pub fn tokenize(source_id: SourceId, sources: &SourceMap, report: &mut Report) -> Vec<Token> {
    let source = sources.content(source_id);
    let mut cursor = Cursor::new(source);
    let mut tokens = Vec::new();

    while !cursor.is_eof() {
        let start = cursor.pos();
        let kind = scan_token(&mut cursor, source_id, report);

        // Skip whitespace (space, tab) — they produce no token.
        if kind.is_none() {
            continue;
        }
        let kind = kind.unwrap();

        let end = cursor.pos();
        tokens.push(Token::new(kind, Span::new(source_id, start, end)));
    }

    // Always end with Eof
    let eof_pos = cursor.pos();
    tokens.push(Token::new(
        TokenKind::Eof,
        Span::new(source_id, eof_pos, eof_pos),
    ));

    tokens
}

/// Scan a single token. Returns None for whitespace (skip).
fn scan_token(cursor: &mut Cursor, source_id: SourceId, report: &mut Report) -> Option<TokenKind> {
    let ch = cursor.peek()?;

    match ch {
        // Whitespace (not newline) — skip
        ' ' | '\t' | '\r' => {
            cursor.advance();
            None
        }

        // Newline — significant (§5.4)
        '\n' => {
            cursor.advance();
            Some(TokenKind::Newline)
        }

        // Comment (§5.3): # to end of line
        '#' => {
            cursor.advance();
            cursor.eat_while(|c| c != '\n');
            None // comments are discarded
        }

        // String literals (§7.3)
        '"' => Some(scan_string(cursor, source_id, report)),

        // Raw string: r"..."
        'r' if cursor.peek_next() == Some('"') => {
            cursor.advance(); // consume 'r'
            Some(scan_raw_string(cursor, source_id, report))
        }

        // Numbers
        '0'..='9' => Some(scan_number(cursor, source_id, report)),

        // Identifiers and keywords
        'a'..='z' | 'A'..='Z' | '_' => Some(scan_ident(cursor)),

        // Two-character operators
        '=' => {
            cursor.advance();
            if cursor.eat('=') {
                if cursor.eat('=') {
                    Some(TokenKind::EqEqEq)
                } else {
                    Some(TokenKind::EqEq)
                }
            } else {
                Some(TokenKind::Eq)
            }
        }
        '!' => {
            cursor.advance();
            if cursor.eat('=') {
                Some(TokenKind::BangEq)
            } else {
                let start = cursor.pos() - 1;
                report.add(
                    Diagnostic::error("unexpected character `!`")
                        .with_code("E0002")
                        .with_label(Label::new(
                            Span::new(source_id, start, cursor.pos()),
                            "use `not` for logical negation",
                        )),
                );
                Some(TokenKind::Error)
            }
        }
        '<' => {
            cursor.advance();
            if cursor.eat('=') {
                Some(TokenKind::LtEq)
            } else {
                Some(TokenKind::Lt)
            }
        }
        '>' => {
            cursor.advance();
            if cursor.eat('=') {
                Some(TokenKind::GtEq)
            } else {
                Some(TokenKind::Gt)
            }
        }
        '-' => {
            cursor.advance();
            if cursor.eat('>') {
                Some(TokenKind::Arrow)
            } else {
                Some(TokenKind::Minus)
            }
        }
        ':' => {
            cursor.advance();
            if cursor.eat(':') {
                Some(TokenKind::ColonColon)
            } else {
                Some(TokenKind::Colon)
            }
        }

        // Single-character tokens
        '+' => {
            cursor.advance();
            Some(TokenKind::Plus)
        }
        '*' => {
            cursor.advance();
            Some(TokenKind::Star)
        }
        '/' => {
            cursor.advance();
            Some(TokenKind::Slash)
        }
        '.' => {
            cursor.advance();
            Some(TokenKind::Dot)
        }
        '?' => {
            cursor.advance();
            Some(TokenKind::Question)
        }
        '|' => {
            cursor.advance();
            Some(TokenKind::Pipe)
        }
        ',' => {
            cursor.advance();
            Some(TokenKind::Comma)
        }
        '(' => {
            cursor.advance();
            Some(TokenKind::LParen)
        }
        ')' => {
            cursor.advance();
            Some(TokenKind::RParen)
        }
        '[' => {
            cursor.advance();
            Some(TokenKind::LBracket)
        }
        ']' => {
            cursor.advance();
            Some(TokenKind::RBracket)
        }
        '{' => {
            cursor.advance();
            Some(TokenKind::LBrace)
        }
        '}' => {
            cursor.advance();
            Some(TokenKind::RBrace)
        }

        // Unrecognized
        _ => {
            let start = cursor.pos();
            cursor.advance();
            report.add(
                Diagnostic::error(format!("unexpected character `{ch}`"))
                    .with_code("E0002")
                    .with_label(Label::new(
                        Span::new(source_id, start, cursor.pos()),
                        "not valid in Tyra source",
                    )),
            );
            Some(TokenKind::Error)
        }
    }
}

/// Scan an identifier or keyword.
fn scan_ident(cursor: &mut Cursor) -> TokenKind {
    let start = cursor.pos();
    cursor.eat_while(|c| c.is_ascii_alphanumeric() || c == '_');
    let text = cursor.slice_from(start);
    TokenKind::keyword(text).unwrap_or_else(|| TokenKind::Ident(text.to_string()))
}

/// Scan a number literal (Int or Float).
fn scan_number(cursor: &mut Cursor, source_id: SourceId, report: &mut Report) -> TokenKind {
    let start = cursor.pos();
    cursor.eat_while(|c| c.is_ascii_digit());

    // Check for decimal point (Float)
    if cursor.peek() == Some('.') && cursor.peek_next().is_some_and(|c| c.is_ascii_digit()) {
        cursor.advance(); // consume '.'
        cursor.eat_while(|c| c.is_ascii_digit());
        let text = cursor.slice_from(start);
        match text.parse::<f64>() {
            Ok(v) => TokenKind::Float(v),
            Err(_) => {
                report.add(
                    Diagnostic::error(format!("float literal `{text}` is out of range"))
                        .with_code("E0005")
                        .with_label(Label::new(
                            Span::new(source_id, start, cursor.pos()),
                            "value out of range for Float",
                        )),
                );
                TokenKind::Error
            }
        }
    } else {
        let text = cursor.slice_from(start);
        match text.parse::<i64>() {
            Ok(n) => TokenKind::Int(n),
            Err(_) => {
                report.add(
                    Diagnostic::error(format!("integer literal `{text}` overflows Int (i64)"))
                        .with_code("E0005")
                        .with_label(Label::new(
                            Span::new(source_id, start, cursor.pos()),
                            "value too large for Int",
                        )),
                );
                TokenKind::Error
            }
        }
    }
}

/// Scan a regular string literal with escape sequences.
///
/// TODO: String interpolation `#{...}` (spec §7.3) is currently lexed as plain
/// string content. When the parser needs interpolation support, this should be
/// refactored to emit segmented tokens (StringStart/StringPart/InterpolationStart/
/// InterpolationEnd/StringEnd) so the parser doesn't need to re-lex string content.
/// See: https://github.com/tyra-lang/tyra/issues/TBD
fn scan_string(cursor: &mut Cursor, source_id: SourceId, report: &mut Report) -> TokenKind {
    cursor.advance(); // consume opening '"'
    let mut value = String::new();

    loop {
        match cursor.peek() {
            None | Some('\n') => {
                let pos = cursor.pos();
                report.add(
                    Diagnostic::error("unterminated string literal")
                        .with_code("E0003")
                        .with_label(Label::new(
                            Span::new(source_id, pos, pos),
                            "string not closed before end of line",
                        )),
                );
                return TokenKind::Error;
            }
            Some('"') => {
                cursor.advance(); // consume closing '"'
                return TokenKind::String(value);
            }
            Some('\\') => {
                cursor.advance(); // consume '\'
                match cursor.peek() {
                    Some('n') => {
                        cursor.advance();
                        value.push('\n');
                    }
                    Some('t') => {
                        cursor.advance();
                        value.push('\t');
                    }
                    Some('r') => {
                        cursor.advance();
                        value.push('\r');
                    }
                    Some('\\') => {
                        cursor.advance();
                        value.push('\\');
                    }
                    Some('"') => {
                        cursor.advance();
                        value.push('"');
                    }
                    Some('0') => {
                        cursor.advance();
                        value.push('\0');
                    }
                    Some('u') => {
                        cursor.advance(); // consume 'u'
                        scan_unicode_escape(cursor, source_id, report, &mut value);
                    }
                    Some(c) => {
                        let pos = cursor.pos() - 1;
                        cursor.advance();
                        report.add(
                            Diagnostic::error(format!("unknown escape sequence `\\{c}`"))
                                .with_code("E0004")
                                .with_label(Label::new(
                                    Span::new(source_id, pos, cursor.pos()),
                                    "invalid escape",
                                )),
                        );
                        value.push(c);
                    }
                    None => {
                        // Backslash at EOF — will be caught by the unterminated check
                    }
                }
            }
            Some(c) => {
                cursor.advance();
                value.push(c);
            }
        }
    }
}

/// Scan `\u{XXXX}` Unicode escape (1-6 hex digits).
fn scan_unicode_escape(
    cursor: &mut Cursor,
    source_id: SourceId,
    report: &mut Report,
    value: &mut String,
) {
    if !cursor.eat('{') {
        let pos = cursor.pos();
        report.add(
            Diagnostic::error("expected `{` after `\\u`")
                .with_code("E0004")
                .with_label(Label::new(
                    Span::new(source_id, pos.saturating_sub(2), pos),
                    "Unicode escape requires `\\u{XXXX}`",
                )),
        );
        return;
    }

    let start = cursor.pos();
    let hex = cursor.eat_while(|c| c.is_ascii_hexdigit());
    let hex_len = hex.len();

    if hex_len == 0 || hex_len > 6 {
        report.add(
            Diagnostic::error("Unicode escape must have 1-6 hex digits")
                .with_code("E0004")
                .with_label(Label::new(
                    Span::new(source_id, start, cursor.pos()),
                    "expected 1-6 hex digits",
                )),
        );
        cursor.eat('}');
        return;
    }

    if !cursor.eat('}') {
        let pos = cursor.pos();
        report.add(
            Diagnostic::error("expected `}` to close Unicode escape")
                .with_code("E0004")
                .with_label(Label::new(Span::new(source_id, pos, pos), "missing `}`")),
        );
        return;
    }

    let code_point = u32::from_str_radix(hex, 16).unwrap_or(0);
    match char::from_u32(code_point) {
        Some(ch) => value.push(ch),
        None => {
            report.add(
                Diagnostic::error(format!("invalid Unicode code point: U+{code_point:04X}"))
                    .with_code("E0004")
                    .with_label(Label::new(
                        Span::new(source_id, start, cursor.pos()),
                        "not a valid Unicode scalar value",
                    )),
            );
        }
    }
}

/// Scan a raw string literal: r"..."
fn scan_raw_string(cursor: &mut Cursor, source_id: SourceId, report: &mut Report) -> TokenKind {
    cursor.advance(); // consume opening '"' (the 'r' was already consumed)
    let start = cursor.pos();

    loop {
        match cursor.peek() {
            None | Some('\n') => {
                let pos = cursor.pos();
                report.add(
                    Diagnostic::error("unterminated raw string literal")
                        .with_code("E0003")
                        .with_label(Label::new(
                            Span::new(source_id, pos, pos),
                            "raw string not closed before end of line",
                        )),
                );
                return TokenKind::Error;
            }
            Some('"') => {
                let content = cursor.slice_from(start).to_string();
                cursor.advance(); // consume closing '"'
                return TokenKind::RawString(content);
            }
            Some(_) => {
                cursor.advance();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(source: &str) -> (Vec<Token>, Report) {
        let mut sources = SourceMap::new();
        let id = sources.add("test.tyra".into(), source.into());
        let mut report = Report::new();
        let tokens = tokenize(id, &sources, &mut report);
        (tokens, report)
    }

    fn kinds(source: &str) -> Vec<TokenKind> {
        let (tokens, _) = lex(source);
        tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn empty_source() {
        assert_eq!(kinds(""), vec![TokenKind::Eof]);
    }

    #[test]
    fn whitespace_only() {
        assert_eq!(kinds("   \t  "), vec![TokenKind::Eof]);
    }

    #[test]
    fn simple_keywords() {
        assert_eq!(
            kinds("fn let mut end"),
            vec![
                TokenKind::Fn,
                TokenKind::Let,
                TokenKind::Mut,
                TokenKind::End,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn identifiers() {
        assert_eq!(
            kinds("foo Bar _baz x1"),
            vec![
                TokenKind::Ident("foo".into()),
                TokenKind::Ident("Bar".into()),
                TokenKind::Ident("_baz".into()),
                TokenKind::Ident("x1".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn integer_literal() {
        assert_eq!(
            kinds("42 0 123"),
            vec![
                TokenKind::Int(42),
                TokenKind::Int(0),
                TokenKind::Int(123),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn float_literal() {
        assert_eq!(
            kinds("3.14 0.0"),
            vec![
                TokenKind::Float(3.14),
                TokenKind::Float(0.0),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn dot_not_float() {
        // `x.y` should be ident, dot, ident — not a float
        assert_eq!(
            kinds("x.y"),
            vec![
                TokenKind::Ident("x".into()),
                TokenKind::Dot,
                TokenKind::Ident("y".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn string_literal() {
        assert_eq!(
            kinds(r#""hello""#),
            vec![TokenKind::String("hello".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn string_escapes() {
        assert_eq!(
            kinds(r#""a\nb\t""#),
            vec![TokenKind::String("a\nb\t".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn string_unicode_escape() {
        // \u{1F600} = 😀
        assert_eq!(
            kinds(r#""\u{1F600}""#),
            vec![TokenKind::String("😀".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn raw_string() {
        assert_eq!(
            kinds(r#"r"\d{3}-\d{4}""#),
            vec![TokenKind::RawString(r"\d{3}-\d{4}".into()), TokenKind::Eof,]
        );
    }

    #[test]
    fn raw_string_no_interpolation() {
        assert_eq!(
            kinds(r##"r"#{not_interpolated}""##),
            vec![
                TokenKind::RawString("#{not_interpolated}".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn operators() {
        assert_eq!(
            kinds("+ - * / = == != < <= > >= -> . : :: ? |"),
            vec![
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Eq,
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::Lt,
                TokenKind::LtEq,
                TokenKind::Gt,
                TokenKind::GtEq,
                TokenKind::Arrow,
                TokenKind::Dot,
                TokenKind::Colon,
                TokenKind::ColonColon,
                TokenKind::Question,
                TokenKind::Pipe,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn brackets() {
        assert_eq!(
            kinds("( ) [ ] { }"),
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn newlines_are_tokens() {
        assert_eq!(
            kinds("a\nb"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::Newline,
                TokenKind::Ident("b".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(
            kinds("a # comment\nb"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::Newline,
                TokenKind::Ident("b".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn boolean_literals() {
        assert_eq!(
            kinds("true false"),
            vec![TokenKind::True, TokenKind::False, TokenKind::Eof]
        );
    }

    #[test]
    fn logical_keywords() {
        assert_eq!(
            kinds("and or not"),
            vec![
                TokenKind::And,
                TokenKind::Or,
                TokenKind::Not,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn triple_equals() {
        assert_eq!(kinds("==="), vec![TokenKind::EqEqEq, TokenKind::Eof]);
    }

    #[test]
    fn unterminated_string_error() {
        let (_, report) = lex("\"hello");
        assert!(report.has_errors());
        assert_eq!(report.error_count(), 1);
    }

    #[test]
    fn unknown_escape_error() {
        let (tokens, report) = lex(r#""\q""#);
        assert!(report.has_errors());
        // Still produces a string token (with the raw char)
        assert!(
            tokens
                .iter()
                .any(|t| matches!(t.kind, TokenKind::String(_)))
        );
    }

    #[test]
    fn bang_alone_is_error() {
        let (_, report) = lex("!");
        assert!(report.has_errors());
    }

    #[test]
    fn integer_overflow_is_error() {
        let (_, report) = lex("99999999999999999999");
        assert!(report.has_errors());
        assert_eq!(report.error_count(), 1);
    }

    #[test]
    fn empty_string() {
        assert_eq!(
            kinds(r#""""#),
            vec![TokenKind::String("".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn not_eq_adjacent() {
        // a!=b should be Ident, BangEq, Ident
        assert_eq!(
            kinds("a!=b"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::BangEq,
                TokenKind::Ident("b".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn four_equals() {
        // ==== should be EqEqEq, Eq
        assert_eq!(
            kinds("===="),
            vec![TokenKind::EqEqEq, TokenKind::Eq, TokenKind::Eof]
        );
    }

    #[test]
    fn leading_zeros() {
        assert_eq!(kinds("007"), vec![TokenKind::Int(7), TokenKind::Eof]);
    }

    #[test]
    fn float_method_call() {
        // 0.0.method() should not misparse
        assert_eq!(
            kinds("0.0.method()"),
            vec![
                TokenKind::Float(0.0),
                TokenKind::Dot,
                TokenKind::Ident("method".into()),
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn comment_at_eof_no_newline() {
        assert_eq!(
            kinds("a # comment"),
            vec![TokenKind::Ident("a".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn real_tyra_program() {
        let source = r#"fn main() -> Unit
  print("hello, tyra")
end"#;
        let (tokens, report) = lex(source);
        assert!(!report.has_errors());
        let k: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(k[0], &TokenKind::Fn);
        assert_eq!(k[1], &TokenKind::Ident("main".into()));
        assert_eq!(k[2], &TokenKind::LParen);
        assert_eq!(k[3], &TokenKind::RParen);
        assert_eq!(k[4], &TokenKind::Arrow);
        assert_eq!(k[5], &TokenKind::Ident("Unit".into()));
        assert_eq!(k[6], &TokenKind::Newline);
    }

    #[test]
    fn comma_separated() {
        assert_eq!(
            kinds("a, b, c"),
            vec![
                TokenKind::Ident("a".into()),
                TokenKind::Comma,
                TokenKind::Ident("b".into()),
                TokenKind::Comma,
                TokenKind::Ident("c".into()),
                TokenKind::Eof,
            ]
        );
    }
}
