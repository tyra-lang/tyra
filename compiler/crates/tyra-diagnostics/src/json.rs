//! NDJSON rendering for `--error-format json` (ADR-0026).
//!
//! Contract: with `--error-format json`, stderr carries NDJSON records
//! ONLY, on every code path. Three record types:
//!
//! - `{"type":"diagnostic", code?, severity, message, spans:[…], help?, notes:[…]}`
//! - `{"type":"error", kind, message}` — non-diagnostic failures
//! - `{"type":"summary", errors, warnings}` — always the last line
//!
//! The schema is append-only after v0.11.0: new fields may be added,
//! existing fields are never renamed or removed. `code`/`type`/`kind`
//! are locale-independent; `message`/`label`/`help` honour `TYRA_LANG`.
//!
//! Hand-rolled serialization (no serde dependency): the shapes are flat
//! and small, and this crate is foundational for every other crate.

use crate::diagnostic::{Diagnostic, Level};
use crate::report::Report;
use crate::source::SourceMap;

/// Escape a string for inclusion in a JSON string literal.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// One NDJSON line for a single diagnostic (no trailing newline).
fn diagnostic_record(diag: &Diagnostic, sources: &SourceMap) -> String {
    let mut line = String::from("{\"type\":\"diagnostic\"");
    if let Some(code) = &diag.code {
        line.push_str(&format!(",\"code\":\"{}\"", esc(code)));
    }
    line.push_str(&format!(",\"severity\":\"{}\"", diag.level.as_str()));
    line.push_str(&format!(",\"message\":\"{}\"", esc(&diag.message)));
    line.push_str(",\"spans\":[");
    for (i, label) in diag.labels.iter().enumerate() {
        if i > 0 {
            line.push(',');
        }
        let (l, c) = sources.line_col(label.span.source, label.span.start);
        let (el, ec) = sources.line_col(label.span.source, label.span.end);
        let file = sources.name(label.span.source);
        line.push_str(&format!(
            "{{\"file\":\"{}\",\"line\":{l},\"col\":{c},\"end_line\":{el},\"end_col\":{ec},\"label\":\"{}\"}}",
            esc(file),
            esc(&label.message)
        ));
    }
    line.push(']');
    if let Some(help) = &diag.help {
        line.push_str(&format!(",\"help\":\"{}\"", esc(help)));
    }
    line.push_str(",\"notes\":[");
    for (i, note) in diag.notes.iter().enumerate() {
        if i > 0 {
            line.push(',');
        }
        line.push_str(&format!("\"{}\"", esc(note)));
    }
    line.push_str("]}");
    line
}

/// A non-diagnostic failure record (usage error, file not found,
/// dependency-resolution failure, internal error). `kind` is a stable
/// lower-kebab-case discriminator.
pub fn json_error_record(kind: &str, message: &str) -> String {
    format!(
        "{{\"type\":\"error\",\"kind\":\"{}\",\"message\":\"{}\"}}",
        esc(kind),
        esc(message)
    )
}

/// The terminating summary record. Its presence tells consumers the
/// stream ended normally (not truncated).
pub fn json_summary_record(errors: usize, warnings: usize) -> String {
    format!("{{\"type\":\"summary\",\"errors\":{errors},\"warnings\":{warnings}}}")
}

impl Report {
    /// Render every diagnostic as NDJSON followed by the summary record.
    /// Each line (including the last) is newline-terminated.
    pub fn render_json(&self, sources: &SourceMap) -> String {
        let mut out = String::new();
        let mut errors = 0usize;
        let mut warnings = 0usize;
        for diag in self.diagnostics() {
            match diag.level {
                Level::Error => errors += 1,
                Level::Warning => warnings += 1,
                Level::Note => {}
            }
            out.push_str(&diagnostic_record(diag, sources));
            out.push('\n');
        }
        out.push_str(&json_summary_record(errors, warnings));
        out.push('\n');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Label;
    use crate::Span;

    #[test]
    fn escapes_quotes_backslashes_and_control_chars() {
        assert_eq!(esc("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
        assert_eq!(esc("\u{0001}"), "\\u0001");
    }

    #[test]
    fn error_and_summary_records_are_single_json_objects() {
        let e = json_error_record("file-not-found", "cannot read `x.ty`");
        assert!(e.starts_with("{\"type\":\"error\""));
        assert!(e.contains("\"kind\":\"file-not-found\""));
        let s = json_summary_record(2, 1);
        assert_eq!(s, "{\"type\":\"summary\",\"errors\":2,\"warnings\":1}");
    }

    #[test]
    fn report_render_json_ends_with_summary() {
        let mut sources = SourceMap::new();
        let sid = sources.add("t.ty".into(), "let x = 1\n".into());
        let mut report = Report::new();
        report.add(
            Diagnostic::error("boom".to_string())
                .with_code("E9999")
                .with_label(Label::new(Span::new(sid, 0, 3), "here"))
                .with_help("try harder"),
        );
        let out = report.render_json(&sources);
        let lines: Vec<&str> = out.trim_end().lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"type\":\"diagnostic\""));
        assert!(lines[0].contains("\"code\":\"E9999\""));
        assert!(lines[0].contains("\"file\":\"t.ty\""));
        assert!(lines[0].contains("\"help\":\"try harder\""));
        assert_eq!(lines[1], "{\"type\":\"summary\",\"errors\":1,\"warnings\":0}");
    }
}
