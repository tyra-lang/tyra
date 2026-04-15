// Token stream cursor for the parser.
// Handles peeking, advancing, and newline significance (§5.4).

use tyra_diagnostics::{Diagnostic, Label, Report, Span};
use tyra_lexer::{Token, TokenKind};

/// Cursor over a token stream with newline handling per §5.4.
pub struct TokenStream {
    tokens: Vec<Token>,
    pos: usize,
    /// Bracket nesting depth: newlines inside () [] {} are ignored.
    bracket_depth: u32,
}

impl TokenStream {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            bracket_depth: 0,
        }
    }

    /// Peek at the current token kind, skipping non-significant newlines.
    pub fn peek(&self) -> &TokenKind {
        self.peek_skip_newlines().0
    }

    /// Peek at the current token (with span), skipping non-significant newlines.
    pub fn peek_token(&self) -> &Token {
        let (_, idx) = self.peek_skip_newlines();
        &self.tokens[idx]
    }

    /// Peek at the current token's span.
    pub fn peek_span(&self) -> Span {
        self.peek_token().span
    }

    /// Check if the current token matches a kind.
    pub fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(kind)
    }

    /// Check if we're at the end of file.
    pub fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    /// Advance past the current token and return it.
    pub fn advance(&mut self) -> Token {
        self.skip_non_significant_newlines();
        let token = self.tokens[self.pos].clone();
        self.track_brackets(&token.kind);
        self.pos += 1;
        token
    }

    /// Consume a token of the expected kind, or report an error.
    pub fn expect(&mut self, expected: &TokenKind, report: &mut Report) -> Option<Token> {
        if self.check(expected) {
            Some(self.advance())
        } else {
            let token = self.peek_token().clone();
            report.add(
                Diagnostic::error(format!(
                    "expected {}, found {}",
                    kind_name(expected),
                    kind_name(&token.kind),
                ))
                .with_code("E0100")
                .with_label(Label::new(
                    token.span,
                    format!("expected {}", kind_name(expected)),
                )),
            );
            None
        }
    }

    /// Consume a token if it matches the expected kind. Returns true if consumed.
    pub fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume a newline token if present. Returns true if consumed.
    /// Only meaningful outside brackets (inside brackets, newlines are skipped).
    pub fn eat_newline(&mut self) -> bool {
        if self.bracket_depth == 0 && self.raw_peek() == &TokenKind::Newline {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Skip all consecutive newlines (at top level, outside brackets).
    pub fn skip_newlines(&mut self) {
        while self.eat_newline() {}
    }

    /// Expect a newline or EOF as statement terminator.
    pub fn expect_newline_or_eof(&mut self, report: &mut Report) {
        if self.bracket_depth > 0 {
            return; // inside brackets, newlines are not required
        }
        match self.raw_peek() {
            TokenKind::Newline => {
                self.pos += 1;
            }
            TokenKind::Eof => {} // OK
            _ => {
                let span = self.tokens[self.pos].span;
                report.add(
                    Diagnostic::error("expected newline or end of file")
                        .with_code("E0101")
                        .with_label(Label::new(span, "expected newline here")),
                );
            }
        }
    }

    /// Expect an identifier token, returning its name.
    pub fn expect_ident(&mut self, report: &mut Report) -> Option<String> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Some(name)
            }
            _ => {
                let token = self.peek_token().clone();
                report.add(
                    Diagnostic::error(format!(
                        "expected identifier, found {}",
                        kind_name(&token.kind)
                    ))
                    .with_code("E0102")
                    .with_label(Label::new(token.span, "expected identifier")),
                );
                None
            }
        }
    }

    // -- Internal helpers --

    /// Peek at the raw current token without skipping newlines.
    fn raw_peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    /// Skip newlines that are non-significant (inside brackets).
    fn skip_non_significant_newlines(&mut self) {
        while self.bracket_depth > 0 && self.pos < self.tokens.len() {
            if self.tokens[self.pos].kind == TokenKind::Newline {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Peek, skipping non-significant newlines. Returns (kind, index).
    fn peek_skip_newlines(&self) -> (&TokenKind, usize) {
        let mut i = self.pos;
        while self.bracket_depth > 0 && i < self.tokens.len() {
            if self.tokens[i].kind == TokenKind::Newline {
                i += 1;
            } else {
                break;
            }
        }
        (&self.tokens[i].kind, i)
    }

    /// Peek past any newline tokens to see what follows.
    /// Used for lookahead without consuming tokens (e.g., ADT detection).
    pub fn peek_past_newlines(&self) -> &TokenKind {
        let mut i = self.pos;
        while i < self.tokens.len() && self.tokens[i].kind == TokenKind::Newline {
            i += 1;
        }
        &self.tokens[i].kind
    }

    /// Current raw position in the token stream (for progress tracking).
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Track bracket depth for newline significance (§5.4).
    fn track_brackets(&mut self, kind: &TokenKind) {
        match kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                self.bracket_depth += 1;
            }
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                self.bracket_depth = self.bracket_depth.saturating_sub(1);
            }
            _ => {}
        }
    }
}

/// Human-readable name for a token kind (for error messages).
pub fn kind_name(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::Int(_) => "integer literal",
        TokenKind::Float(_) => "float literal",
        TokenKind::String(_) => "string literal",
        TokenKind::RawString(_) => "raw string literal",
        TokenKind::True => "`true`",
        TokenKind::False => "`false`",
        TokenKind::Ident(_) => "identifier",
        TokenKind::Fn => "`fn`",
        TokenKind::Data => "`data`",
        TokenKind::Value => "`value`",
        TokenKind::Type => "`type`",
        TokenKind::Trait => "`trait`",
        TokenKind::Impl => "`impl`",
        TokenKind::Let => "`let`",
        TokenKind::Mut => "`mut`",
        TokenKind::If => "`if`",
        TokenKind::Else => "`else`",
        TokenKind::Match => "`match`",
        TokenKind::When => "`when`",
        TokenKind::For => "`for`",
        TokenKind::In => "`in`",
        TokenKind::While => "`while`",
        TokenKind::Return => "`return`",
        TokenKind::Defer => "`defer`",
        TokenKind::Async => "`async`",
        TokenKind::Await => "`await`",
        TokenKind::Spawn => "`spawn`",
        TokenKind::Import => "`import`",
        TokenKind::Export => "`export`",
        TokenKind::And => "`and`",
        TokenKind::Or => "`or`",
        TokenKind::Not => "`not`",
        TokenKind::End => "`end`",
        TokenKind::Plus => "`+`",
        TokenKind::Minus => "`-`",
        TokenKind::Star => "`*`",
        TokenKind::Slash => "`/`",
        TokenKind::Eq => "`=`",
        TokenKind::EqEq => "`==`",
        TokenKind::BangEq => "`!=`",
        TokenKind::Lt => "`<`",
        TokenKind::LtEq => "`<=`",
        TokenKind::Gt => "`>`",
        TokenKind::GtEq => "`>=`",
        TokenKind::EqEqEq => "`===`",
        TokenKind::Arrow => "`->`",
        TokenKind::Dot => "`.`",
        TokenKind::Colon => "`:`",
        TokenKind::ColonColon => "`::`",
        TokenKind::Question => "`?`",
        TokenKind::Pipe => "`|`",
        TokenKind::Comma => "`,`",
        TokenKind::LParen => "`(`",
        TokenKind::RParen => "`)`",
        TokenKind::LBracket => "`[`",
        TokenKind::RBracket => "`]`",
        TokenKind::LBrace => "`{`",
        TokenKind::RBrace => "`}`",
        TokenKind::Newline => "newline",
        TokenKind::Eof => "end of file",
        TokenKind::Error => "error token",
    }
}
