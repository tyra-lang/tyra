//! tyra-fmt: source code formatter for the Tyra language.
//!
//! Public API:
//! - `fmt_source(src: &str) -> Result<String, String>` — format a source string.
//!
//! Formatting rules (v0.2.0):
//! - Indentation: 2 spaces
//! - Trailing newline always present
//! - Comment lines are preserved in their original position
//! - Blank lines between top-level items are normalised to exactly one

mod printer;

use tyra_diagnostics::{Report, SourceMap};

/// Format Tyra source code. Returns the formatted string, or an error
/// message if the source cannot be parsed.
pub fn fmt_source(src: &str) -> Result<String, String> {
    let mut sources = SourceMap::new();
    let sid = sources.add("<fmt>".into(), src.into());
    let mut report = Report::new();
    let ast = tyra_parser::parse(sid, &sources, &mut report);
    if report.has_errors() {
        return Err("parse error: cannot format invalid source".to_string());
    }
    let (comments, inline_comments) = printer::extract_comments(src);
    let out = printer::Printer::new(src, sid, &sources, comments, inline_comments).print_file(&ast);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_source_roundtrip() {
        let out = fmt_source("").unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn single_fn_idempotent() {
        let src = "fn main() -> Unit\n  print(\"hello\")\nend\n";
        let first = fmt_source(src).unwrap();
        let second = fmt_source(&first).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn comment_header_preserved() {
        let src = "# header comment\n\nfn main() -> Unit\n  ()\nend\n";
        let out = fmt_source(src).unwrap();
        assert!(out.starts_with("# header comment"), "comment must be first: {out:?}");
    }

    #[test]
    fn comment_only_file_preserved() {
        let src = "# just a comment\n# second line\n";
        let out = fmt_source(src).unwrap();
        assert_eq!(out, "# just a comment\n# second line\n");
    }

    #[test]
    fn trailing_comment_preserved() {
        let src = "fn main() -> Unit\n  ()\nend\n\n# trailing note\n";
        let out = fmt_source(src).unwrap();
        assert!(out.contains("# trailing note"), "trailing comment must survive: {out:?}");
    }

    #[test]
    fn inline_comment_preserved() {
        let src = "fn main() -> Unit\n  let x = 1 # inline note\nend\n";
        let out = fmt_source(src).unwrap();
        assert!(out.contains("# inline note"), "inline comment must survive: {out:?}");
    }

    #[test]
    fn inline_comment_on_fn_header_preserved() {
        let src = "fn main() -> Unit # entry point\n  ()\nend\n";
        let out = fmt_source(src).unwrap();
        assert!(
            out.contains("# entry point"),
            "fn header inline comment must survive: {out:?}"
        );
        let second = fmt_source(&out).unwrap();
        assert_eq!(out, second, "must be idempotent");
    }

    #[test]
    fn inline_comment_on_import_preserved() {
        let src = "import core.io # for println\n\nfn main() -> Unit\n  ()\nend\n";
        let out = fmt_source(src).unwrap();
        assert!(
            out.contains("# for println"),
            "import inline comment must survive: {out:?}"
        );
        let second = fmt_source(&out).unwrap();
        assert_eq!(out, second, "must be idempotent");
    }

    #[test]
    fn constructor_pattern_shorthand_preserved() {
        // `Ok(v)` must not expand to `Ok(v: v)`
        let src = "fn f(r: Result<Int, String>) -> Unit\n  match r\n  when Ok(v)\n    ()\n  when Err(msg)\n    ()\n  end\nend\n";
        let out = fmt_source(src).unwrap();
        assert!(
            out.contains("when Ok(v)"),
            "shorthand Ok(v) must not expand to Ok(v: v): {out:?}"
        );
        assert!(
            !out.contains("Ok(v: v)"),
            "expanded form must not appear: {out:?}"
        );
        let second = fmt_source(&out).unwrap();
        assert_eq!(out, second, "must be idempotent");
    }

    #[test]
    fn constructor_pattern_wildcard_preserved() {
        // `Ok(_)` must not expand to `Ok(: _)` or `Ok(_: _)`
        let src = "fn f(r: Result<Int, String>) -> Unit\n  match r\n  when Ok(_)\n    ()\n  when Err(_)\n    ()\n  end\nend\n";
        let out = fmt_source(src).unwrap();
        assert!(
            out.contains("when Ok(_)"),
            "wildcard Ok(_) must be preserved: {out:?}"
        );
        assert!(
            !out.contains("Ok(_: _)") && !out.contains("Ok(: _)"),
            "mangled wildcard form must not appear: {out:?}"
        );
        let second = fmt_source(&out).unwrap();
        assert_eq!(out, second, "must be idempotent");
    }

    #[test]
    fn constructor_pattern_explicit_form_preserved() {
        // `Card(last4: binding)` where field_name != binding must stay explicit
        let src = "fn f(c: Card) -> Unit\n  match c\n  when Card(last4: digits)\n    ()\n  end\nend\n";
        let out = fmt_source(src).unwrap();
        assert!(
            out.contains("when Card(last4: digits)"),
            "explicit field:binding form must be preserved: {out:?}"
        );
        let second = fmt_source(&out).unwrap();
        assert_eq!(out, second, "must be idempotent");
    }

    #[test]
    fn inline_comment_on_alias_type_def_preserved() {
        let src = "type UserId = Int # stable wire id\n";
        let out = fmt_source(src).unwrap();
        assert!(
            out.contains("# stable wire id"),
            "alias type inline comment must survive: {out:?}"
        );
        let second = fmt_source(&out).unwrap();
        assert_eq!(out, second, "must be idempotent");
    }

    #[test]
    fn inline_comment_on_adt_type_def_preserved() {
        let src = "type Color = # primary colors\n  | Red\n  | Green\n  | Blue\n";
        let out = fmt_source(src).unwrap();
        assert!(
            out.contains("# primary colors"),
            "ADT type header inline comment must survive: {out:?}"
        );
        let second = fmt_source(&out).unwrap();
        assert_eq!(out, second, "must be idempotent");
    }
}
