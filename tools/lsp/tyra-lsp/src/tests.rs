use std::collections::HashMap;

use tokio::sync::Mutex;
use tower_lsp::lsp_types::*;
use tower_lsp::{LspService};
use tyra_diagnostics::{Diagnostic as TyraDiag, Label, SourceMap, Span};
use tyra_driver::{CompletionKind, SymbolList};

use crate::{DocState, TyraLsp, DIAG_SOURCE, to_lsp_diagnostic};
use crate::completion::{
    build_completion_items, detect_member_receiver, lookup_receiver_ty,
    module_member_completions, type_method_completions,
};

fn make_source() -> (SourceMap, tyra_diagnostics::SourceId) {
    let mut sources = SourceMap::new();
    // "hello\nworld\n" → line 1 = "hello", line 2 = "world"
    let id = sources.add("test.tyra".into(), "hello\nworld\n".into());
    (sources, id)
}

#[test]
fn to_lsp_diagnostic_range_conversion() {
    let (sources, id) = make_source();
    // "hello" is at bytes 0..5 on line 1, col 1..6 (1-based)
    // LSP expects line 0, char 0..5 (0-based)
    let diag = TyraDiag::error("test error")
        .with_code("E0001")
        .with_label(Label::new(Span::new(id, 0, 5), "here"));
    let lsp = to_lsp_diagnostic(&diag, &sources);
    assert_eq!(lsp.range.start.line, 0);
    assert_eq!(lsp.range.start.character, 0);
    assert_eq!(lsp.range.end.line, 0);
    assert_eq!(lsp.range.end.character, 5);
}

#[test]
fn to_lsp_diagnostic_second_line() {
    let (sources, id) = make_source();
    let diag = TyraDiag::error("msg")
        .with_label(Label::new(Span::new(id, 6, 11), "second line"));
    let lsp = to_lsp_diagnostic(&diag, &sources);
    assert_eq!(lsp.range.start.line, 1);
    assert_eq!(lsp.range.start.character, 0);
}

#[test]
fn to_lsp_diagnostic_no_label_falls_back_to_origin() {
    let (sources, _) = make_source();
    let diag = TyraDiag::error("no span here");
    let lsp = to_lsp_diagnostic(&diag, &sources);
    assert_eq!(lsp.range, Range::default());
    assert_eq!(lsp.message, "no span here");
}

#[test]
fn to_lsp_diagnostic_message_combines_label() {
    let (sources, id) = make_source();
    let diag = TyraDiag::error("primary")
        .with_label(Label::new(Span::new(id, 0, 1), "label text"));
    let lsp = to_lsp_diagnostic(&diag, &sources);
    assert_eq!(lsp.message, "primary — label text");
}

#[test]
fn to_lsp_diagnostic_severity_mapping() {
    let (sources, _) = make_source();
    let err = to_lsp_diagnostic(&TyraDiag::error("e"), &sources);
    let warn = to_lsp_diagnostic(&TyraDiag::warning("w"), &sources);
    let note = to_lsp_diagnostic(&TyraDiag::note("n"), &sources);
    assert_eq!(err.severity, Some(DiagnosticSeverity::ERROR));
    assert_eq!(warn.severity, Some(DiagnosticSeverity::WARNING));
    assert_eq!(note.severity, Some(DiagnosticSeverity::INFORMATION));
}

#[test]
fn to_lsp_diagnostic_source_is_tyra() {
    let (sources, _) = make_source();
    let lsp = to_lsp_diagnostic(&TyraDiag::error("e"), &sources);
    assert_eq!(lsp.source.as_deref(), Some(DIAG_SOURCE));
}

// ── Hover ─────────────────────────────────────────────────────────────────────

#[test]
fn hover_type_index_lookup() {
    let src = "let x: Int = 1\n";
    let (report, sources, type_index, _, _, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    assert!(!report.has_errors(), "unexpected errors: {:?}", report.diagnostics());

    let offset = sources.offset_at(source_id, 0, 4).expect("offset_at failed");

    let best = type_index
        .iter()
        .filter(|(span, _)| {
            span.source == source_id && span.start <= offset && offset < span.end
        })
        .min_by_key(|(span, _)| span.end - span.start);

    let (_span, ty) = best.expect("no type found at offset");
    assert_eq!(ty.display_name(), "Int");
}

// ── Go to definition ──────────────────────────────────────────────────────────

#[test]
fn goto_definition_def_index_lookup() {
    let src = "let x: Int = 1\nlet y = x + 1\n";
    let (report, sources, _, def_index, _, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    assert!(!report.has_errors(), "unexpected errors: {:?}", report.diagnostics());

    let ref_offset = sources.offset_at(source_id, 1, 8).expect("offset_at failed");

    let best = def_index
        .iter()
        .filter(|(span, _)| {
            span.source == source_id && span.start <= ref_offset && ref_offset < span.end
        })
        .min_by_key(|(span, _)| span.end - span.start);

    let (_, def_span) = best.expect("no definition found for x reference");
    assert_eq!(def_span.start, 0, "expected definition at start of `let x` stmt");
}

#[tokio::test(flavor = "current_thread")]
async fn goto_definition_returns_location() {
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp::jsonrpc::Request;

    let (mut service, _socket) = LspService::new(|client| TyraLsp {
        client,
        documents: Mutex::new(HashMap::new()),
    });

    let init = Request::build("initialize")
        .params(json!({"capabilities": {}}))
        .id(1)
        .finish();
    let _ = service.ready().await.unwrap().call(init).await.unwrap();

    let src = "let x: Int = 1\nlet y = x + 1\n";
    let did_open = Request::build("textDocument/didOpen")
        .params(json!({
            "textDocument": {
                "uri": "file:///tmp/def_test.tyra",
                "languageId": "tyra",
                "version": 1,
                "text": src
            }
        }))
        .finish();
    let _ = service.ready().await.unwrap().call(did_open).await.unwrap();

    let def_req = Request::build("textDocument/definition")
        .params(json!({
            "textDocument": { "uri": "file:///tmp/def_test.tyra" },
            "position": { "line": 1, "character": 8 }
        }))
        .id(2)
        .finish();
    let resp = service.ready().await.unwrap().call(def_req).await.unwrap();
    let body = serde_json::to_value(&resp).unwrap();

    assert!(body["result"].is_object(), "expected a Location object, got: {body}");
    assert!(
        body["result"]["uri"]
            .as_str()
            .map(|s| s.contains("def_test.tyra"))
            .unwrap_or(false),
        "expected def_test.tyra in uri, got: {body}"
    );
}

// ── Completion ────────────────────────────────────────────────────────────────

#[test]
fn completion_returns_prelude_and_locals() {
    let src = "let xs = [1]\n";
    let (report, sources, _, _, symbols, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    assert!(!report.has_errors(), "unexpected errors: {:?}", report.diagnostics());

    let state = DocState {
        text: src.to_string(),
        sources,
        type_index: Default::default(),
        def_index: Default::default(),
        symbols,
        source_id,
    };
    let items = build_completion_items(&state);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    assert!(labels.contains(&"xs"), "missing user-defined `xs`");
    assert!(labels.contains(&"println"), "missing prelude `println`");
    assert!(labels.contains(&"Some"), "missing prelude constructor `Some`");
    assert!(labels.contains(&"Int"), "missing prelude type `Int`");
    assert!(labels.contains(&"let"), "missing keyword `let`");
}

#[test]
fn completion_excludes_intrinsics() {
    let src = "let x = 1\n";
    let (_, sources, _, _, symbols, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    let state = DocState {
        text: src.to_string(),
        sources,
        type_index: Default::default(),
        def_index: Default::default(),
        symbols,
        source_id,
    };
    let items = build_completion_items(&state);
    assert!(
        !items.iter().any(|i| i.label.starts_with("__")),
        "intrinsic names should be excluded"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn completion_returns_array_with_println() {
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp::jsonrpc::Request;

    let (mut service, _socket) = LspService::new(|client| TyraLsp {
        client,
        documents: Mutex::new(HashMap::new()),
    });

    let init = Request::build("initialize")
        .params(json!({"capabilities": {}}))
        .id(1)
        .finish();
    let _ = service.ready().await.unwrap().call(init).await.unwrap();

    let src = "let x: Int = 1\n";
    let did_open = Request::build("textDocument/didOpen")
        .params(json!({
            "textDocument": {
                "uri": "file:///tmp/completion_test.tyra",
                "languageId": "tyra",
                "version": 1,
                "text": src
            }
        }))
        .finish();
    let _ = service.ready().await.unwrap().call(did_open).await.unwrap();

    let comp_req = Request::build("textDocument/completion")
        .params(json!({
            "textDocument": { "uri": "file:///tmp/completion_test.tyra" },
            "position": { "line": 0, "character": 0 }
        }))
        .id(2)
        .finish();
    let resp = service.ready().await.unwrap().call(comp_req).await.unwrap();
    let body = serde_json::to_value(&resp).unwrap();

    assert!(body["result"].is_array(), "expected array response, got: {body}");
    let labels: Vec<&str> = body["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(
        labels.contains(&"println"),
        "expected `println` in completion items, got: {labels:?}"
    );
}

// ── Member-access completion ───────────────────────────────────────────────────

#[test]
fn detect_member_receiver_unit() {
    let mk = |line: &str, ch: u32| -> Option<String> {
        detect_member_receiver(line, Position { line: 0, character: ch })
    };

    assert_eq!(mk("string.", 7), Some("string".into()));
    assert_eq!(mk("string.tri", 10), Some("string".into()));
    assert_eq!(mk("string", 6), None);
    let _ = mk("1.", 2);
    assert_eq!(mk("  .", 3), None);
    assert_eq!(mk("foo.bar.", 8), Some("bar".into()));
    assert_eq!(
        detect_member_receiver("let x = 1\nstring.", Position { line: 1, character: 7 }),
        Some("string".into())
    );
    assert_eq!(mk("1.", 2), None);
}

#[test]
fn completion_after_module_dot_returns_module_members() {
    let src = "let x = 1\n";
    let (_, sources, type_index, def_index, _, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    let symbols: SymbolList = vec![
        ("mymod__foo".into(), CompletionKind::Function),
        ("mymod__bar".into(), CompletionKind::Function),
        ("other".into(), CompletionKind::Variable),
    ];
    let state = DocState {
        text: "mymod.".to_string(),
        sources,
        type_index,
        def_index,
        symbols,
        source_id,
    };

    let pos = Position { line: 0, character: 6 };
    let receiver = detect_member_receiver(&state.text, pos).expect("should detect receiver");
    assert_eq!(receiver, "mymod");

    let items = module_member_completions(&receiver, &state);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"foo"), "expected `foo` in members: {labels:?}");
    assert!(labels.contains(&"bar"), "expected `bar` in members: {labels:?}");
    assert!(!labels.contains(&"other"), "`other` should not appear");
}

#[test]
fn completion_after_dot_no_match_returns_empty() {
    let src = "let x = 1\nlet r = unknown_module.\n";
    let (_, sources, type_index, def_index, symbols, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    let state = DocState {
        text: src.to_string(),
        sources,
        type_index,
        def_index,
        symbols,
        source_id,
    };
    let pos = Position { line: 1, character: 23 };
    let receiver = detect_member_receiver(src, pos).expect("should detect receiver");
    assert_eq!(receiver, "unknown_module");

    let module_items = module_member_completions(&receiver, &state);
    assert!(module_items.is_empty(), "expected no module members for unknown receiver");

    let ty_items = match lookup_receiver_ty(&receiver, &state) {
        Some(ty) => type_method_completions(&ty),
        None => vec![],
    };
    assert!(ty_items.is_empty(), "expected no type methods for unknown receiver");
}

// ── Find References ───────────────────────────────────────────────────────────

#[test]
fn references_finds_uses_from_def_site() {
    use crate::references::{find_def_span_at_cursor, find_uses_for_def};

    let src = "let x: Int = 1\nlet y = x + 1\nlet z = x * 2\n";
    let (report, sources, _, def_index, _, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    assert!(!report.has_errors(), "unexpected errors: {:?}", report.diagnostics());

    let state = DocState {
        text: src.to_string(),
        sources,
        type_index: Default::default(),
        def_index,
        symbols: Default::default(),
        source_id,
    };

    let offset = state.sources.offset_at(source_id, 0, 4).expect("offset_at");
    let def_span = find_def_span_at_cursor(&state, offset).expect("should find def_span");

    let uses = find_uses_for_def(&state.def_index, def_span, source_id);
    assert_eq!(uses.len(), 2, "expected 2 use-spans for `x`, got: {uses:?}");
}

#[test]
fn references_finds_uses_from_use_site() {
    use crate::references::{find_def_span_at_cursor, find_uses_for_def};

    let src = "let x: Int = 1\nlet y = x + 1\nlet z = x * 2\n";
    let (report, sources, _, def_index, _, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    assert!(!report.has_errors(), "unexpected errors: {:?}", report.diagnostics());

    let state = DocState {
        text: src.to_string(),
        sources,
        type_index: Default::default(),
        def_index,
        symbols: Default::default(),
        source_id,
    };

    let offset = state.sources.offset_at(source_id, 1, 8).expect("offset_at");
    let def_span = find_def_span_at_cursor(&state, offset).expect("should find def_span");

    let uses = find_uses_for_def(&state.def_index, def_span, source_id);
    assert_eq!(uses.len(), 2, "expected 2 use-spans for `x`, got: {uses:?}");
}

#[test]
fn references_includes_declaration_when_requested() {
    use crate::references::{find_def_span_at_cursor, find_uses_for_def};

    let src = "let x: Int = 1\nlet y = x + 1\n";
    let (report, sources, _, def_index, _, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    assert!(!report.has_errors(), "unexpected errors: {:?}", report.diagnostics());

    let state = DocState {
        text: src.to_string(),
        sources,
        type_index: Default::default(),
        def_index,
        symbols: Default::default(),
        source_id,
    };

    let offset = state.sources.offset_at(source_id, 1, 8).expect("offset_at");
    let def_span = find_def_span_at_cursor(&state, offset).expect("should find def_span");

    let mut spans = find_uses_for_def(&state.def_index, def_span, source_id);
    spans.push(def_span);

    assert_eq!(spans.len(), 2, "expected use + declaration, got: {spans:?}");
    assert!(spans.contains(&def_span), "declaration span should be present");
}

#[tokio::test(flavor = "current_thread")]
async fn references_returns_location_array() {
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp::jsonrpc::Request;

    let (mut service, _socket) = LspService::new(|client| TyraLsp {
        client,
        documents: Mutex::new(HashMap::new()),
    });

    let init = Request::build("initialize")
        .params(json!({"capabilities": {}}))
        .id(1)
        .finish();
    let _ = service.ready().await.unwrap().call(init).await.unwrap();

    let src = "let x: Int = 1\nlet y = x + 1\n";
    let did_open = Request::build("textDocument/didOpen")
        .params(json!({
            "textDocument": {
                "uri": "file:///tmp/refs_test.tyra",
                "languageId": "tyra",
                "version": 1,
                "text": src
            }
        }))
        .finish();
    let _ = service.ready().await.unwrap().call(did_open).await.unwrap();

    let refs_req = Request::build("textDocument/references")
        .params(json!({
            "textDocument": { "uri": "file:///tmp/refs_test.tyra" },
            "position": { "line": 1, "character": 8 },
            "context": { "includeDeclaration": false }
        }))
        .id(2)
        .finish();
    let resp = service.ready().await.unwrap().call(refs_req).await.unwrap();
    let body = serde_json::to_value(&resp).unwrap();

    assert!(body["result"].is_array(), "expected array of locations, got: {body}");
    assert!(
        !body["result"].as_array().unwrap().is_empty(),
        "expected at least one reference location, got: {body}"
    );
}

/// JSON-RPC smoke: `includeDeclaration: true` returns use-spans + the def-span (2 total).
#[tokio::test(flavor = "current_thread")]
async fn references_include_declaration_returns_def_and_use() {
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp::jsonrpc::Request;

    let (mut service, _socket) = LspService::new(|client| TyraLsp {
        client,
        documents: Mutex::new(HashMap::new()),
    });

    let init = Request::build("initialize")
        .params(json!({"capabilities": {}}))
        .id(1)
        .finish();
    let _ = service.ready().await.unwrap().call(init).await.unwrap();

    // One declaration (`let x`), one use (`x + 1`).
    let src = "let x: Int = 1\nlet y = x + 1\n";
    let did_open = Request::build("textDocument/didOpen")
        .params(json!({
            "textDocument": {
                "uri": "file:///tmp/refs_incl_test.tyra",
                "languageId": "tyra",
                "version": 1,
                "text": src
            }
        }))
        .finish();
    let _ = service.ready().await.unwrap().call(did_open).await.unwrap();

    // Cursor on use-site; includeDeclaration: true → 1 use + 1 decl = 2 locations.
    let refs_req = Request::build("textDocument/references")
        .params(json!({
            "textDocument": { "uri": "file:///tmp/refs_incl_test.tyra" },
            "position": { "line": 1, "character": 8 },
            "context": { "includeDeclaration": true }
        }))
        .id(2)
        .finish();
    let resp = service.ready().await.unwrap().call(refs_req).await.unwrap();
    let body = serde_json::to_value(&resp).unwrap();

    let locations = body["result"].as_array().expect("expected array result");
    assert_eq!(
        locations.len(),
        2,
        "expected 1 use + 1 declaration = 2 locations, got: {body}"
    );
    assert!(
        locations.iter().all(|loc| loc["uri"]
            .as_str()
            .map(|s| s.contains("refs_incl_test.tyra"))
            .unwrap_or(false)),
        "all locations should reference refs_incl_test.tyra, got: {body}"
    );
}

// ── Rename ────────────────────────────────────────────────────────────────────

/// Pure helper test: rename `x` → `xx` produces 3 edits (1 decl + 2 uses).
#[test]
fn rename_renames_all_uses_and_declaration() {
    use crate::references::{find_def_span_at_cursor, find_uses_for_def};
    use crate::rename::{extract_identifier_at, find_binding_name_span};

    let src = "let x: Int = 1\nlet y = x + 1\nlet z = x * 2\n";
    let (report, sources, _, def_index, _, source_id) =
        tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
    assert!(!report.has_errors(), "unexpected errors: {:?}", report.diagnostics());

    let state = DocState {
        text: src.to_string(),
        sources,
        type_index: Default::default(),
        def_index,
        symbols: Default::default(),
        source_id,
    };

    // Cursor on `x` in "let y = x + 1" (line 1, col 8).
    let offset = state.sources.offset_at(source_id, 1, 8).expect("offset_at");
    let old_name = extract_identifier_at(src, offset).expect("extract_identifier_at");
    assert_eq!(old_name, "x");

    let def_span = find_def_span_at_cursor(&state, offset).expect("find_def_span_at_cursor");
    let use_spans = find_uses_for_def(&state.def_index, def_span, source_id);
    assert_eq!(use_spans.len(), 2, "expected 2 use-spans");

    let name_span = find_binding_name_span(src, def_span, &old_name)
        .expect("find_binding_name_span");
    // name_span should cover the 'x' in "let x: Int = 1" (byte 4..5)
    assert_eq!(&src[name_span.start as usize..name_span.end as usize], "x");

    // Total edits: 2 uses + 1 declaration = 3.
    let total = use_spans.len() + 1;
    assert_eq!(total, 3, "expected 3 edits (1 decl + 2 uses)");
}

/// JSON-RPC smoke: `textDocument/rename` returns a WorkspaceEdit with non-empty changes.
#[tokio::test(flavor = "current_thread")]
async fn rename_returns_workspace_edit() {
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp::jsonrpc::Request;

    let (mut service, _socket) = LspService::new(|client| TyraLsp {
        client,
        documents: Mutex::new(HashMap::new()),
    });

    let init = Request::build("initialize")
        .params(json!({"capabilities": {}}))
        .id(1)
        .finish();
    let _ = service.ready().await.unwrap().call(init).await.unwrap();

    let src = "let x: Int = 1\nlet y = x + 1\n";
    let did_open = Request::build("textDocument/didOpen")
        .params(json!({
            "textDocument": {
                "uri": "file:///tmp/rename_test.tyra",
                "languageId": "tyra",
                "version": 1,
                "text": src
            }
        }))
        .finish();
    let _ = service.ready().await.unwrap().call(did_open).await.unwrap();

    // Rename `x` at line 1 col 8 (use-site) to `renamed`.
    let rename_req = Request::build("textDocument/rename")
        .params(json!({
            "textDocument": { "uri": "file:///tmp/rename_test.tyra" },
            "position": { "line": 1, "character": 8 },
            "newName": "renamed"
        }))
        .id(2)
        .finish();
    let resp = service.ready().await.unwrap().call(rename_req).await.unwrap();
    let body = serde_json::to_value(&resp).unwrap();

    let changes = &body["result"]["changes"];
    assert!(changes.is_object(), "expected changes object, got: {body}");
    let file_edits = changes["file:///tmp/rename_test.tyra"]
        .as_array()
        .expect("expected edits array for the file");
    assert!(!file_edits.is_empty(), "expected at least one edit, got: {body}");
    assert!(
        file_edits.iter().all(|e| e["newText"] == "renamed"),
        "all edits should use new name 'renamed', got: {body}"
    );
}

/// Rename from the declaration site (cursor on `x` in `let x: ...`) also
/// produces a valid WorkspaceEdit covering declaration + all uses.
#[tokio::test(flavor = "current_thread")]
async fn rename_from_def_site_returns_workspace_edit() {
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp::jsonrpc::Request;

    let (mut service, _socket) = LspService::new(|client| TyraLsp {
        client,
        documents: Mutex::new(HashMap::new()),
    });

    let init = Request::build("initialize")
        .params(json!({"capabilities": {}}))
        .id(1)
        .finish();
    let _ = service.ready().await.unwrap().call(init).await.unwrap();

    // One declaration, one use.
    let src = "let x: Int = 1\nlet y = x + 1\n";
    let did_open = Request::build("textDocument/didOpen")
        .params(json!({
            "textDocument": {
                "uri": "file:///tmp/rename_def_test.tyra",
                "languageId": "tyra",
                "version": 1,
                "text": src
            }
        }))
        .finish();
    let _ = service.ready().await.unwrap().call(did_open).await.unwrap();

    // Cursor at line 0 col 4 = 'x' in "let x: Int = 1" (def-site).
    let rename_req = Request::build("textDocument/rename")
        .params(json!({
            "textDocument": { "uri": "file:///tmp/rename_def_test.tyra" },
            "position": { "line": 0, "character": 4 },
            "newName": "renamed"
        }))
        .id(2)
        .finish();
    let resp = service.ready().await.unwrap().call(rename_req).await.unwrap();
    let body = serde_json::to_value(&resp).unwrap();

    let edits = body["result"]["changes"]["file:///tmp/rename_def_test.tyra"]
        .as_array()
        .expect("expected edits array");
    // 1 declaration edit + 1 use edit = 2 total.
    assert_eq!(edits.len(), 2, "expected 2 edits (decl + use), got: {body}");
    assert!(
        edits.iter().all(|e| e["newText"] == "renamed"),
        "all edits should use 'renamed', got: {body}"
    );
    // Edits should be sorted: declaration (line 0) before use (line 1).
    assert_eq!(edits[0]["range"]["start"]["line"], 0, "first edit should be declaration");
    assert_eq!(edits[1]["range"]["start"]["line"], 1, "second edit should be use");
}

/// `new_name = \"let\"` (keyword) must produce an invalid_params error.
#[tokio::test(flavor = "current_thread")]
async fn rename_invalid_identifier_returns_error() {
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp::jsonrpc::Request;

    let (mut service, _socket) = LspService::new(|client| TyraLsp {
        client,
        documents: Mutex::new(HashMap::new()),
    });

    let init = Request::build("initialize")
        .params(json!({"capabilities": {}}))
        .id(1)
        .finish();
    let _ = service.ready().await.unwrap().call(init).await.unwrap();

    let src = "let x: Int = 1\n";
    let did_open = Request::build("textDocument/didOpen")
        .params(json!({
            "textDocument": {
                "uri": "file:///tmp/rename_err_test.tyra",
                "languageId": "tyra",
                "version": 1,
                "text": src
            }
        }))
        .finish();
    let _ = service.ready().await.unwrap().call(did_open).await.unwrap();

    let rename_req = Request::build("textDocument/rename")
        .params(json!({
            "textDocument": { "uri": "file:///tmp/rename_err_test.tyra" },
            "position": { "line": 0, "character": 4 },
            "newName": "let"
        }))
        .id(2)
        .finish();
    let resp = service.ready().await.unwrap().call(rename_req).await.unwrap();
    let body = serde_json::to_value(&resp).unwrap();

    assert!(body["error"].is_object(), "expected error object, got: {body}");
}

// ── Diagnostics smoke ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn did_open_publishes_e0110_diagnostic() {
    use futures::StreamExt;
    use serde_json::json;
    use tower::{Service, ServiceExt};
    use tower_lsp::jsonrpc::Request;

    let (mut service, mut socket) = LspService::new(|client| TyraLsp {
        client,
        documents: Mutex::new(HashMap::new()),
    });

    let init = Request::build("initialize")
        .params(json!({"capabilities": {}}))
        .id(1)
        .finish();
    let _ = service.ready().await.unwrap().call(init).await.unwrap();

    let src = "fn f() -> Int\n  import foo\n  0\nend\n";
    let did_open = Request::build("textDocument/didOpen")
        .params(json!({
            "textDocument": {
                "uri": "file:///tmp/smoke.tyra",
                "languageId": "tyra",
                "version": 1,
                "text": src
            }
        }))
        .finish();
    let _ = service.ready().await.unwrap().call(did_open).await.unwrap();

    let msg = socket.next().await.expect("expected publishDiagnostics notification");
    let body = serde_json::to_value(&msg).unwrap();
    assert_eq!(body["method"], "textDocument/publishDiagnostics");
    let diags = body["params"]["diagnostics"].as_array().unwrap();
    assert!(
        diags.iter().any(|d| d["code"] == "E0110"),
        "expected E0110 diagnostic, got: {body}"
    );
}
