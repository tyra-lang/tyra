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
    ///
    /// Clamps at the last token (Eof in well-formed streams) so a parser
    /// that keeps advancing past end never panics. It will keep seeing
    /// Eof and should terminate normally.
    pub fn advance(&mut self) -> Token {
        self.skip_non_significant_newlines();
        let idx = self.pos.min(self.tokens.len().saturating_sub(1));
        let token = self.tokens[idx].clone();
        self.track_brackets(&token.kind);
        self.pos = idx + 1;
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
    ///
    /// Also accepts `end`, `else`, and `when` as implicit terminators.
    /// This lets one-line forms like `match e when Some(x) x when None 0 end`
    /// parse: each arm body is a single expression with no trailing newline,
    /// followed directly by the next `when` or by `end`.
    pub fn expect_newline_or_eof(&mut self, report: &mut Report) {
        if self.bracket_depth > 0 {
            return; // inside brackets, newlines are not required
        }
        match self.raw_peek() {
            TokenKind::Newline => {
                self.pos += 1;
            }
            TokenKind::Eof | TokenKind::End | TokenKind::Else | TokenKind::When => {} // OK — implicit block terminator
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

    /// Expect an identifier or a contextual keyword usable as a field name.
    /// Keywords like `value`, `data`, `type` are valid as field names in
    /// value/data type definitions and ADT variant fields.
    pub fn expect_ident_or_field_keyword(&mut self, report: &mut Report) -> Option<String> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Some(name)
            }
            ref kw => {
                if let Some(name) = keyword_as_ident(kw) {
                    let name = name.to_string();
                    self.advance();
                    return Some(name);
                }
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

    /// Check if the token after the current logical token is a colon.
    /// Accounts for newline-skipping via peek_skip_newlines.
    pub fn peek_ahead_is_colon(&self) -> bool {
        let (_, current_idx) = self.peek_skip_newlines();
        let mut i = current_idx + 1;
        while i < self.tokens.len() && self.tokens[i].kind == TokenKind::Newline {
            i += 1;
        }
        i < self.tokens.len() && self.tokens[i].kind == TokenKind::Colon
    }

    // -- Internal helpers --

    /// Peek at the raw current token without skipping newlines.
    /// Clamps at end-of-stream to avoid panic on overrun.
    fn raw_peek(&self) -> &TokenKind {
        let idx = self.pos.min(self.tokens.len().saturating_sub(1));
        &self.tokens[idx].kind
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
    ///
    /// If the cursor runs off the end (e.g., caller advanced past Eof on
    /// a malformed program, or tokens vector is pathological), clamp to
    /// the last token so we never panic with index-out-of-bounds. The
    /// lexer always appends an Eof terminator, so the clamp lands on it
    /// in well-formed streams.
    fn peek_skip_newlines(&self) -> (&TokenKind, usize) {
        let mut i = self.pos;
        while self.bracket_depth > 0 && i < self.tokens.len() {
            if self.tokens[i].kind == TokenKind::Newline {
                i += 1;
            } else {
                break;
            }
        }
        let idx = i.min(self.tokens.len().saturating_sub(1));
        (&self.tokens[idx].kind, idx)
    }

    /// Peek at the token AFTER the current logical token, skipping newlines.
    /// Used for 2-token lookahead (e.g., contextual keyword detection).
    pub fn peek_second(&self) -> &TokenKind {
        let (_, first_idx) = self.peek_skip_newlines();
        let mut i = first_idx + 1;
        while i < self.tokens.len() && self.tokens[i].kind == TokenKind::Newline {
            i += 1;
        }
        let idx = i.min(self.tokens.len().saturating_sub(1));
        &self.tokens[idx].kind
    }

    /// Peek past any newline tokens to see what follows.
    /// Used for lookahead without consuming tokens (e.g., ADT detection).
    pub fn peek_past_newlines(&self) -> &TokenKind {
        let mut i = self.pos;
        while i < self.tokens.len() && self.tokens[i].kind == TokenKind::Newline {
            i += 1;
        }
        let idx = i.min(self.tokens.len().saturating_sub(1));
        &self.tokens[idx].kind
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
        TokenKind::InterpString(_) => "interpolated string",
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
        TokenKind::Break => "`break`",
        TokenKind::Continue => "`continue`",
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
        TokenKind::Percent => "`%`",
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

/// Convert a keyword token to its string form if it can be used as a field name.
/// Most keywords are valid as field names in contextual positions.
pub fn keyword_as_ident(kind: &TokenKind) -> Option<&'static str> {
    match kind {
        TokenKind::Value => Some("value"),
        TokenKind::Data => Some("data"),
        TokenKind::Type => Some("type"),
        TokenKind::Trait => Some("trait"),
        TokenKind::Impl => Some("impl"),
        TokenKind::Mut => Some("mut"),
        TokenKind::Async => Some("async"),
        TokenKind::Await => Some("await"),
        TokenKind::Spawn => Some("spawn"),
        TokenKind::Import => Some("import"),
        TokenKind::Export => Some("export"),
        TokenKind::Defer => Some("defer"),
        // Control flow keywords (fn, let, if, else, match, when, for, in,
        // while, return, end, and, or, not) are NOT valid as field names.
        _ => None,
    }
}
