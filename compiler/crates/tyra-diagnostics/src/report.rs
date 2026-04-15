// Report collects diagnostics during a compilation pass and renders them.
// This is the primary interface for emitting errors.

use crate::{Diagnostic, Level, SourceMap};

/// Collects diagnostics and tracks whether any errors occurred.
#[derive(Debug)]
pub struct Report {
    diagnostics: Vec<Diagnostic>,
    error_count: u32,
    warning_count: u32,
}

impl Report {
    pub fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
            error_count: 0,
            warning_count: 0,
        }
    }

    pub fn add(&mut self, diag: Diagnostic) {
        match diag.level {
            Level::Error => self.error_count += 1,
            Level::Warning => self.warning_count += 1,
            Level::Note => {}
        }
        self.diagnostics.push(diag);
    }

    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    pub fn error_count(&self) -> u32 {
        self.error_count
    }

    pub fn warning_count(&self) -> u32 {
        self.warning_count
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Render all diagnostics to a string.
    /// Format: level[CODE]: message
    ///   --> file:line:col
    ///   |
    /// N | source line
    ///   | ^^^^^ label message
    pub fn render(&self, sources: &SourceMap) -> String {
        let mut output = String::new();
        for diag in &self.diagnostics {
            // Header: error[E0001]: message
            output.push_str(diag.level.as_str());
            if let Some(code) = &diag.code {
                output.push('[');
                output.push_str(code);
                output.push(']');
            }
            output.push_str(": ");
            output.push_str(&diag.message);
            output.push('\n');

            // Labels
            for label in &diag.labels {
                let (line, col) = sources.line_col(label.span.source, label.span.start);
                let file = sources.name(label.span.source);
                let line_text = sources.line_content(label.span.source, line);
                let line_str = line.to_string();
                let padding = " ".repeat(line_str.len());

                // --> file:line:col
                output.push_str(&format!("{padding} --> {file}:{line}:{col}\n"));
                // N | source line
                output.push_str(&format!("{padding} |\n"));
                output.push_str(&format!("{line_str} | {line_text}\n"));
                // Underline
                let offset_in_line = (col - 1) as usize;
                let underline_len = (label.span.len() as usize).max(1);
                output.push_str(&format!(
                    "{padding} | {}{} {}\n",
                    " ".repeat(offset_in_line),
                    "^".repeat(underline_len),
                    label.message,
                ));
            }

            // Notes
            for note in &diag.notes {
                output.push_str(&format!("  = note: {note}\n"));
            }

            output.push('\n');
        }

        // Summary
        if self.error_count > 0 || self.warning_count > 0 {
            let mut parts = Vec::new();
            if self.error_count > 0 {
                parts.push(format!(
                    "{} error{}",
                    self.error_count,
                    if self.error_count == 1 { "" } else { "s" }
                ));
            }
            if self.warning_count > 0 {
                parts.push(format!(
                    "{} warning{}",
                    self.warning_count,
                    if self.warning_count == 1 { "" } else { "s" }
                ));
            }
            output.push_str(&parts.join(", "));
            output.push_str(" emitted\n");
        }

        output
    }
}

impl Default for Report {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Diagnostic, Label, SourceMap, Span};

    #[test]
    fn render_single_error() {
        let mut sources = SourceMap::new();
        let id = sources.add("test.tyra".into(), "let x = 10\nlet y =\n".into());

        let mut report = Report::new();
        report.add(
            Diagnostic::error("expected expression")
                .with_code("E0001")
                .with_label(Label::new(
                    Span::new(id, 18, 18),
                    "expected expression here",
                )),
        );

        let output = report.render(&sources);
        assert!(output.contains("error[E0001]: expected expression"));
        assert!(output.contains("test.tyra:2:"));
        assert!(output.contains("1 error emitted"));
    }

    #[test]
    fn report_tracks_counts() {
        let mut report = Report::new();
        report.add(Diagnostic::error("e1"));
        report.add(Diagnostic::error("e2"));
        report.add(Diagnostic::warning("w1"));

        assert!(report.has_errors());
        assert_eq!(report.error_count(), 2);
        assert_eq!(report.warning_count(), 1);
    }

    #[test]
    fn empty_report() {
        let report = Report::new();
        let sources = SourceMap::new();
        assert!(!report.has_errors());
        assert_eq!(report.render(&sources), "");
    }
}
