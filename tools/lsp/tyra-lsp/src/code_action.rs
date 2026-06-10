use std::collections::HashMap;

use tower_lsp::lsp_types::Url;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, NumberOrString, TextEdit,
    WorkspaceEdit,
};
use tyra_driver::{PRELUDE_CONSTRUCTORS, PRELUDE_FUNCTIONS, PRELUDE_TYPES, SymbolList};

/// Build code-action suggestions for the given diagnostics.
///
/// `only`: when `Some`, restricts results to actions whose kind is prefixed
/// by one of the requested strings (LSP contract: `context.only`).
///
/// v1: E0200 "undefined name `…`" only — Levenshtein typo correction.
pub(crate) fn build_actions(
    uri: &Url,
    diags: &[Diagnostic],
    symbols: &SymbolList,
    only: Option<&[CodeActionKind]>,
) -> Vec<CodeActionOrCommand> {
    // If the client restricts to specific kinds and QUICKFIX is not among them,
    // return nothing — our only action kind is quickfix.
    if let Some(kinds) = only {
        let want_quickfix = kinds
            .iter()
            .any(|k| CodeActionKind::QUICKFIX.as_str().starts_with(k.as_str()));
        if !want_quickfix {
            return vec![];
        }
    }

    let mut out = Vec::new();
    for diag in diags {
        if let Some(NumberOrString::String(code)) = &diag.code {
            // Restrict to diagnostics that come from undefined-name resolution
            // (message starts with "undefined name `…`"). E0200 is also emitted
            // for missing-module imports; those have a different message prefix
            // and a default 0:0 range, so they must be excluded here.
            if code == "E0200" && diag.message.starts_with("undefined name `") {
                out.extend(e0200_actions(uri, diag, symbols));
            }
        }
    }
    out
}

fn e0200_actions(uri: &Url, diag: &Diagnostic, symbols: &SymbolList) -> Vec<CodeActionOrCommand> {
    let bad = match extract_backtick_name(&diag.message) {
        Some(n) => n,
        None => return vec![],
    };

    let candidates = collect_candidates(symbols);

    let mut scored: Vec<(usize, &str)> = candidates
        .iter()
        .filter_map(|cand| {
            let d = levenshtein(bad, cand);
            if d == 0 || d > 2 {
                None
            } else {
                Some((d, cand.as_str()))
            }
        })
        .collect();

    // Sort by distance then name for determinism.
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
    scored.truncate(3);

    scored
        .into_iter()
        .map(|(dist, good)| {
            let edit = TextEdit {
                range: diag.range,
                new_text: good.to_string(),
            };
            let mut changes = HashMap::new();
            changes.insert(uri.clone(), vec![edit]);
            let action = CodeAction {
                title: format!("Replace `{bad}` with `{good}`"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag.clone()]),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                is_preferred: Some(dist == 1),
                ..Default::default()
            };
            CodeActionOrCommand::CodeAction(action)
        })
        .collect()
}

fn collect_candidates(symbols: &SymbolList) -> Vec<String> {
    let mut names: Vec<String> = symbols.iter().map(|(n, _)| n.clone()).collect();
    for &n in PRELUDE_FUNCTIONS {
        names.push(n.to_string());
    }
    for &n in PRELUDE_TYPES {
        names.push(n.to_string());
    }
    for &n in PRELUDE_CONSTRUCTORS {
        names.push(n.to_string());
    }
    names.sort();
    names.dedup();
    names
}

/// Extract the first backtick-quoted name from a message like
/// `undefined name \`foo\``.
fn extract_backtick_name(msg: &str) -> Option<&str> {
    let start = msg.find('`')? + 1;
    let end = msg[start..].find('`')? + start;
    Some(&msg[start..end])
}

/// Classic DP Levenshtein distance on Unicode chars.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    let mut row: Vec<usize> = (0..=n).collect();
    for i in 1..=m {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=n {
            let next = row[j];
            row[j] = if a[i - 1] == b[j - 1] {
                prev
            } else {
                1 + prev.min(row[j]).min(row[j - 1])
            };
            prev = next;
        }
    }
    row[n]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_e0200(msg: &str) -> Diagnostic {
        Diagnostic {
            message: msg.to_string(),
            code: Some(NumberOrString::String("E0200".into())),
            ..Default::default()
        }
    }

    fn make_diag_with_code(code: &str) -> Diagnostic {
        Diagnostic {
            code: Some(NumberOrString::String(code.into())),
            message: "undefined name `pirnt`".into(),
            ..Default::default()
        }
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("foo", "fooo"), 1);
        assert_eq!(levenshtein("foo", "fo"), 1);
        assert_eq!(levenshtein("abc", "xyz"), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
    }

    #[test]
    fn extracts_bad_name_from_e0200_message() {
        assert_eq!(
            extract_backtick_name("undefined name `pirnt`"),
            Some("pirnt")
        );
        assert_eq!(extract_backtick_name("undefined name `foo`"), Some("foo"));
        assert_eq!(extract_backtick_name("no backticks here"), None);
    }

    #[test]
    fn suggests_typo_correction_for_e0200() {
        let symbols: SymbolList =
            vec![("myvar".to_string(), tyra_driver::CompletionKind::Function)];
        let diag = make_e0200("undefined name `pirnt`");
        let actions = build_actions(
            &Url::parse("file:///test.ty").unwrap(),
            &[diag],
            &symbols,
            None,
        );
        let titles: Vec<String> = actions
            .iter()
            .filter_map(|a| match a {
                CodeActionOrCommand::CodeAction(ca) => Some(ca.title.clone()),
                _ => None,
            })
            .collect();
        assert!(
            titles.iter().any(|t| t.contains("print")),
            "expected `print` suggestion, got: {titles:?}"
        );
    }

    #[test]
    fn no_suggestion_when_distance_too_large() {
        // "zzzzqqqq" is far from every symbol and every prelude name.
        let symbols: SymbolList =
            vec![("xyzzy".to_string(), tyra_driver::CompletionKind::Function)];
        let diag = make_e0200("undefined name `zzzzqqqq`");
        let actions = build_actions(
            &Url::parse("file:///test.ty").unwrap(),
            &[diag],
            &symbols,
            None,
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn ignores_non_e0200_diagnostics() {
        let symbols: SymbolList = vec![];
        let diag = make_diag_with_code("E0309");
        let actions = build_actions(
            &Url::parse("file:///test.ty").unwrap(),
            &[diag],
            &symbols,
            None,
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn ignores_non_undefined_name_e0200() {
        // E0200 from a missing-module import has a different message prefix.
        let symbols: SymbolList = vec![];
        let diag = make_e0200("cannot import `math`: module not found");
        let actions = build_actions(
            &Url::parse("file:///test.ty").unwrap(),
            &[diag],
            &symbols,
            None,
        );
        assert!(
            actions.is_empty(),
            "expected no actions for import error, got: {actions:?}"
        );
    }

    #[test]
    fn respects_context_only_excluding_quickfix() {
        let symbols: SymbolList = vec![];
        let diag = make_e0200("undefined name `pirnt`");
        // Client only wants source actions — quick fixes should be excluded.
        let only = vec![CodeActionKind::SOURCE];
        let actions = build_actions(
            &Url::parse("file:///test.ty").unwrap(),
            &[diag],
            &symbols,
            Some(&only),
        );
        assert!(
            actions.is_empty(),
            "expected no quickfix when only=source, got: {actions:?}"
        );
    }
}
