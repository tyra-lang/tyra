// Token types for the Tyra language.
// spec reference: §5 (lexical rules), §7.2 (literals), §7.3 (strings)

use tyra_diagnostics::Span;

/// A segment of a string with interpolation (used in InterpString token).
#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
    /// Literal text segment.
    Lit(String),
    /// Expression text from `#{...}` (to be parsed by the parser).
    Expr(String),
}

/// A token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// All token kinds in the Tyra language.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // -- Literals --
    /// Integer literal: `42`, `0`
    Int(i64),
    /// Float literal: `3.14`, `0.0`
    Float(f64),
    /// String literal (no interpolation): `"hello"`
    /// Contains the parsed string content with escapes resolved.
    String(String),
    /// String with interpolation: `"hello, #{name}!"`
    /// Contains alternating literal and expression-text segments.
    InterpString(Vec<InterpPart>),
    /// Raw string literal: `r"..."`
    /// Contains the raw content verbatim.
    RawString(String),
    /// `true`
    True,
    /// `false`
    False,

    // -- Identifiers --
    /// Any identifier: variable, function, type, module name
    Ident(String),

    // -- Keywords (spec §5.2) --
    Fn,
    Data,
    Value,
    Type,
    Trait,
    Impl,
    Let,
    Mut,
    If,
    Else,
    Match,
    When,
    For,
    In,
    While,
    Return,
    Defer,
    Async,
    Await,
    Spawn,
    Import,
    Export,
    And,
    Or,
    Not,
    End,

    // -- Punctuation / Operators --
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `=`
    Eq,
    /// `==`
    EqEq,
    /// `!=`
    BangEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `===`
    EqEqEq,
    /// `->`
    Arrow,
    /// `.`
    Dot,
    /// `:`
    Colon,
    /// `::`
    ColonColon,
    /// `?`
    Question,
    /// `|`
    Pipe,
    /// `,`
    Comma,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `{`
    LBrace,
    /// `}`
    RBrace,

    // -- Special --
    /// Newline (significant: statement terminator per §5.4)
    Newline,
    /// End of file
    Eof,
    /// Invalid/unrecognized character
    Error,
}

impl TokenKind {
    /// Look up a keyword from an identifier string.
    /// Returns None if the string is not a keyword.
    pub fn keyword(s: &str) -> Option<TokenKind> {
        match s {
            "fn" => Some(TokenKind::Fn),
            "data" => Some(TokenKind::Data),
            "value" => Some(TokenKind::Value),
            "type" => Some(TokenKind::Type),
            "trait" => Some(TokenKind::Trait),
            "impl" => Some(TokenKind::Impl),
            "let" => Some(TokenKind::Let),
            "mut" => Some(TokenKind::Mut),
            "if" => Some(TokenKind::If),
            "else" => Some(TokenKind::Else),
            "match" => Some(TokenKind::Match),
            "when" => Some(TokenKind::When),
            "for" => Some(TokenKind::For),
            "in" => Some(TokenKind::In),
            "while" => Some(TokenKind::While),
            "return" => Some(TokenKind::Return),
            "defer" => Some(TokenKind::Defer),
            "async" => Some(TokenKind::Async),
            "await" => Some(TokenKind::Await),
            "spawn" => Some(TokenKind::Spawn),
            "import" => Some(TokenKind::Import),
            "export" => Some(TokenKind::Export),
            "and" => Some(TokenKind::And),
            "or" => Some(TokenKind::Or),
            "not" => Some(TokenKind::Not),
            "true" => Some(TokenKind::True),
            "false" => Some(TokenKind::False),
            "end" => Some(TokenKind::End),
            _ => None,
        }
    }
}
