use tower_lsp::lsp_types::*;
use tyra_driver::{CompletionKind, PRELUDE_CONSTRUCTORS, PRELUDE_FUNCTIONS, PRELUDE_TYPES, Ty};

use crate::DocState;
use crate::keywords::TYRA_KEYWORDS;

/// Builtin method names for the `String` type, sourced from the stdlib.
static STRING_METHODS: &[&str] = &[
    "byte_at",
    "substring",
    "from_byte",
    "parse_int",
    "parse_errno",
    "starts_with",
    "ends_with",
    "to_upper",
    "to_lower",
    "is_empty",
    "trim",
    "len",
];

/// Builtin method names for the `List<T>` type.
static LIST_METHODS: &[&str] = &["push", "pop", "len", "get", "is_empty"];

/// Detect whether the cursor is positioned immediately after `<ident>.`
/// (with an optional partial identifier already typed).  Returns the receiver
/// name when the pattern matches.
///
/// Examples (cursor shown as `|`):
///   `string.|`     → Some("string")
///   `string.tri|`  → Some("string")
///   `xs|`          → None (no dot)
///   `1.|`          → None (digit-only identifiers are rejected)
///   `foo.bar.|`    → Some("bar")  (rightmost dot receiver)
pub(crate) fn detect_member_receiver(text: &str, pos: Position) -> Option<String> {
    let line = text.lines().nth(pos.line as usize)?;

    // Convert UTF-16 character index to byte offset within the line.
    let mut byte_off = 0usize;
    let mut utf16_seen = 0u32;
    for ch in line.chars() {
        if utf16_seen >= pos.character {
            break;
        }
        utf16_seen += ch.len_utf16() as u32;
        byte_off += ch.len_utf8();
    }

    let before_cursor = &line[..byte_off];

    // Find the last `.` to the left of the cursor.
    let dot_byte = before_cursor.rfind('.')?;
    let before_dot = before_cursor[..dot_byte].trim_end();

    // Extract the trailing identifier (ASCII alnum + `_`) from before_dot.
    let bytes = before_dot.as_bytes();
    let ident_end = bytes.len();
    let mut ident_start = ident_end;
    while ident_start > 0 {
        let b = bytes[ident_start - 1];
        if b.is_ascii_alphanumeric() || b == b'_' {
            ident_start -= 1;
        } else {
            break;
        }
    }

    let receiver = &before_dot[ident_start..ident_end];
    if receiver.is_empty() {
        return None;
    }

    // Reject numeric-only tokens (e.g. float literal `1.5` with cursor after dot).
    if receiver.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    Some(receiver.to_string())
}

/// Return completion items for `<receiver>.<Tab>` where `receiver` is a module
/// name.  Candidates are extracted from `state.symbols` by matching the
/// `<receiver>__<member>` mangling produced by `resolve_imports`.
pub(crate) fn module_member_completions(receiver: &str, state: &DocState) -> Vec<CompletionItem> {
    let prefix = format!("{receiver}__");
    let mut items = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (name, kind) in &state.symbols {
        if let Some(member) = name.strip_prefix(&prefix) {
            if seen.insert(member) {
                items.push(CompletionItem {
                    label: member.to_string(),
                    kind: Some(match kind {
                        CompletionKind::Function => CompletionItemKind::FUNCTION,
                        CompletionKind::Variable => CompletionItemKind::VARIABLE,
                        CompletionKind::TypeDef => CompletionItemKind::CLASS,
                        CompletionKind::Module => CompletionItemKind::MODULE,
                    }),
                    ..Default::default()
                });
            }
        }
    }
    items
}

/// Return completion items for `<receiver>.<Tab>` based on the receiver's `Ty`.
/// Uses a hardcoded method table per primitive type.
pub(crate) fn type_method_completions(ty: &Ty) -> Vec<CompletionItem> {
    let methods: &[&str] = match ty {
        Ty::String => STRING_METHODS,
        Ty::Generic(name, _) if name == "List" => LIST_METHODS,
        _ => &[],
    };
    methods
        .iter()
        .map(|&m| CompletionItem {
            label: m.to_string(),
            kind: Some(CompletionItemKind::METHOD),
            ..Default::default()
        })
        .collect()
}

/// Resolve the `Ty` of a named receiver by scanning `type_index` for a span
/// whose text in the source matches `receiver`.  Returns the first match found.
///
/// NOTE: when multiple spans share the same source text (e.g. a variable and a
/// same-named type annotation), the returned `Ty` is from an arbitrary span due
/// to `HashMap` iteration order.  Scope-aware lookup is deferred to a future pass.
pub(crate) fn lookup_receiver_ty(receiver: &str, state: &DocState) -> Option<Ty> {
    let text = state.text.as_bytes();
    state
        .type_index
        .iter()
        .filter(|(span, _)| span.source == state.source_id)
        .find(|(span, _)| {
            let (s, e) = (span.start as usize, span.end as usize);
            e <= text.len() && std::str::from_utf8(&text[s..e]).ok() == Some(receiver)
        })
        .map(|(_, ty)| ty.clone())
}

/// Build the full completion item list from cached document state.
///
/// Combines four sources:
/// 1. User-defined names from the resolver (`state.symbols`)
/// 2. Public prelude functions (excludes `__`-prefixed intrinsics)
/// 3. Prelude constructors + types
/// 4. Language keywords
///
/// Position-independent: every name defined anywhere in the file is offered
/// regardless of cursor scope.
pub(crate) fn build_completion_items(state: &DocState) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = Vec::new();
    // Track emitted labels so user-defined names that shadow prelude names
    // (e.g. `fn println()`) don't produce duplicate completion entries.
    let mut emitted: std::collections::HashSet<&str> = std::collections::HashSet::new();

    // 1. User-defined symbols
    for (name, kind) in &state.symbols {
        emitted.insert(name.as_str());
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(match kind {
                CompletionKind::Function => CompletionItemKind::FUNCTION,
                CompletionKind::Variable => CompletionItemKind::VARIABLE,
                CompletionKind::TypeDef => CompletionItemKind::CLASS,
                CompletionKind::Module => CompletionItemKind::MODULE,
            }),
            ..Default::default()
        });
    }

    // 2. Prelude functions (skip internal `__`-prefixed intrinsics and user-shadowed names)
    for &name in PRELUDE_FUNCTIONS {
        if !name.starts_with("__") && emitted.insert(name) {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FUNCTION),
                ..Default::default()
            });
        }
    }

    // 3. Prelude constructors + types
    for &name in PRELUDE_CONSTRUCTORS {
        if emitted.insert(name) {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::CONSTRUCTOR),
                ..Default::default()
            });
        }
    }
    for &name in PRELUDE_TYPES {
        if emitted.insert(name) {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::CLASS),
                ..Default::default()
            });
        }
    }

    // 4. Keywords (no overlap with prelude constants; no dedup needed)
    for &kw in TYRA_KEYWORDS {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        });
    }

    items
}
