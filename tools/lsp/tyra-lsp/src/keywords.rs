/// Tyra language keywords — single source of truth within tyra-lsp.
///
/// Keep in sync with `tyra-lexer`'s `TokenKind::keyword()` match arm.
/// Used for completion filtering and rename validation.
pub(crate) static TYRA_KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "if", "else", "end", "when", "match", "for", "in", "while", "break",
    "return", "import", "export", "value", "data", "type", "trait", "impl", "true", "false", "and",
    "or", "not", "defer", "spawn", "await", "async",
];
