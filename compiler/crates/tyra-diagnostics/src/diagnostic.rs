// Diagnostic represents a single compiler message (error, warning, note).
// Format: error[E0042]: message at file:line:col

use crate::Span;

/// Severity level of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Note,
}

impl Level {
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Note => "note",
        }
    }
}

/// A labeled span pointing to a region of source code.
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

impl Label {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

/// A single diagnostic message.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: Level,
    pub code: Option<String>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            level: Level::Warning,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn note(message: impl Into<String>) -> Self {
        Self {
            level: Level::Note,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceId;

    #[test]
    fn build_error_diagnostic() {
        let diag = Diagnostic::error("unexpected token")
            .with_code("E0001")
            .with_label(Label::new(
                Span::new(SourceId::test(0), 10, 15),
                "expected `end`",
            ))
            .with_note("blocks must be closed with `end`");

        assert_eq!(diag.level, Level::Error);
        assert_eq!(diag.code.as_deref(), Some("E0001"));
        assert_eq!(diag.message, "unexpected token");
        assert_eq!(diag.labels.len(), 1);
        assert_eq!(diag.notes.len(), 1);
    }

    #[test]
    fn level_as_str() {
        assert_eq!(Level::Error.as_str(), "error");
        assert_eq!(Level::Warning.as_str(), "warning");
        assert_eq!(Level::Note.as_str(), "note");
    }
}
