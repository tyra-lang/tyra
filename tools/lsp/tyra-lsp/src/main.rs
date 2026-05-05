use std::collections::HashMap;

use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tyra_diagnostics::{Level, SourceMap};
use tyra_driver::{
    CompletionKind, DefIndex, SourceId, SymbolList, TypeIndex,
    PRELUDE_CONSTRUCTORS, PRELUDE_FUNCTIONS, PRELUDE_TYPES,
};

const DIAG_SOURCE: &str = "tyra";

/// Cached analysis result for one open document.
struct DocState {
    #[allow(dead_code)] // available for future use (e.g. incremental parse)
    text: String,
    sources: SourceMap,
    type_index: TypeIndex,
    def_index: DefIndex,
    symbols: SymbolList,
    source_id: SourceId,
}

struct TyraLsp {
    client: Client,
    // Per-document state: text + type index for hover, updated on every edit.
    // NOTE: writes and `analyze` calls are not atomic — if two `did_change`
    // events race, the store may briefly hold a stale state. A version guard
    // should be added when incremental sync lands.
    documents: Mutex<HashMap<Url, DocState>>,
}

impl TyraLsp {
    async fn analyze(&self, uri: Url, text: String, version: i32) {
        let file_name: String = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "untitled.tyra".to_string());

        let text_clone = text.clone();

        // Run the compiler pipeline on a blocking thread so we don't stall
        // the async executor, and catch any unexpected panics so a compiler
        // bug never brings down the LSP process.
        let result = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tyra_driver::check_in_memory(file_name, text_clone, None)
            }))
        })
        .await;

        let lsp_diags = match result {
            Ok(Ok((report, sources, type_index, def_index, symbols, source_id))) => {
                let diags = report
                    .diagnostics()
                    .iter()
                    .map(|d| to_lsp_diagnostic(d, &sources))
                    .collect();

                self.documents.lock().await.insert(
                    uri.clone(),
                    DocState { text, sources, type_index, def_index, symbols, source_id },
                );

                diags
            }
            Ok(Err(_panic_payload)) => {
                // The compiler panicked — report it as an internal error so
                // the user knows something went wrong and can file a bug.
                vec![Diagnostic {
                    range: Range::default(),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String("E9999".into())),
                    source: Some(DIAG_SOURCE.into()),
                    message: "internal compiler error — please report this bug".into(),
                    ..Default::default()
                }]
            }
            Err(_join_err) => {
                // The spawn_blocking task was cancelled (e.g. LSP shutdown).
                vec![]
            }
        };

        self.client
            .publish_diagnostics(uri, lsp_diags, Some(version))
            .await;
    }
}

/// Convert a Tyra `Diagnostic` to the LSP `Diagnostic` type.
///
/// Span byte offsets come from `SourceMap::line_col` which is 1-based;
/// LSP `Position` is 0-based, so we subtract 1 via `saturating_sub`.
pub(crate) fn to_lsp_diagnostic(
    d: &tyra_diagnostics::Diagnostic,
    sources: &tyra_diagnostics::SourceMap,
) -> Diagnostic {
    let first_label = d.labels.first();

    let range = first_label
        .map(|label| span_to_lsp_range(label.span, sources))
        .unwrap_or_default();

    let message = first_label
        .filter(|l| !l.message.is_empty())
        .map_or_else(|| d.message.clone(), |l| format!("{} — {}", d.message, l.message));

    Diagnostic {
        range,
        severity: Some(match d.level {
            Level::Error => DiagnosticSeverity::ERROR,
            Level::Warning => DiagnosticSeverity::WARNING,
            Level::Note => DiagnosticSeverity::INFORMATION,
        }),
        code: d.code.clone().map(NumberOrString::String),
        source: Some(DIAG_SOURCE.to_string()),
        message,
        ..Default::default()
    }
}

/// Convert a `Span` to an LSP `Range` using the source map.
///
/// Positions use UTF-16 code units for `character`, matching the LSP 3.17
/// default encoding (`positionEncoding: "utf-16"`).
fn span_to_lsp_range(span: tyra_diagnostics::Span, sources: &SourceMap) -> Range {
    let (sl, sc) = sources
        .line_col_utf16(span.source, span.start)
        .unwrap_or((0, 0));
    let (el, ec) = sources
        .line_col_utf16(span.source, span.end)
        .unwrap_or((0, 0));
    Range {
        start: Position {
            line: sl,
            character: sc,
        },
        end: Position {
            line: el,
            character: ec,
        },
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for TyraLsp {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions::default()),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "tyra-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "tyra-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;
        self.analyze(uri, text, version).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        // TextDocumentSyncKind::FULL → changes[0].text is the entire new content.
        match params.content_changes.into_iter().next() {
            Some(change) => {
                self.analyze(uri, change.text, version).await;
            }
            None => {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        "did_change received empty content_changes — skipping analysis",
                    )
                    .await;
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.lock().await.remove(&uri);
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let docs = self.documents.lock().await;
        let state = match docs.get(uri) {
            Some(s) => s,
            None => return Ok(None),
        };

        let offset = match state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
        {
            Some(o) => o,
            None => return Ok(None),
        };

        // Find the smallest ref span in def_index containing the cursor offset.
        let best = state
            .def_index
            .iter()
            .filter(|(span, _)| {
                span.source == state.source_id && span.start <= offset && offset < span.end
            })
            .min_by_key(|(span, _)| span.end - span.start);

        let (_, def_span) = match best {
            Some(entry) => entry,
            None => return Ok(None),
        };

        let def_range = span_to_lsp_range(*def_span, &state.sources);

        // Reconstruct the URI for the file that contains the definition.
        // SourceMap::name returns the name the file was registered with.
        // If it is an absolute path (e.g. from import resolution), build
        // a file:// URL from it; otherwise fall back to the currently-open URI.
        let def_uri = {
            let name = state.sources.name(def_span.source);
            if name.starts_with('/') {
                Url::from_file_path(name).unwrap_or_else(|_| uri.clone())
            } else {
                uri.clone()
            }
        };

        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: def_uri,
            range: def_range,
        })))
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let docs = self.documents.lock().await;
        let state = match docs.get(uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        Ok(Some(CompletionResponse::Array(build_completion_items(state))))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let docs = self.documents.lock().await;
        let state = match docs.get(uri) {
            Some(s) => s,
            None => return Ok(None),
        };

        // Convert LSP 0-based Position (UTF-16 character) to byte offset.
        let offset = match state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
        {
            Some(o) => o,
            None => return Ok(None),
        };

        // Find the span in the type index that contains the cursor offset.
        // Among all matching spans, pick the one with the smallest range
        // (most specific) so hovering inside a complex expression shows the
        // innermost type rather than the enclosing statement's type.
        let best = state
            .type_index
            .iter()
            .filter(|(span, _)| span.source == state.source_id && span.start <= offset && offset < span.end)
            .min_by_key(|(span, _)| span.end - span.start);

        let (span, ty) = match best {
            Some(entry) => entry,
            None => return Ok(None),
        };

        let range = span_to_lsp_range(*span, &state.sources);

        Ok(Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String(ty.display_name())),
            range: Some(range),
        }))
    }
}

/// Tyra language keywords for completion.
static TYRA_KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "if", "else", "end", "when", "match", "for", "in",
    "while", "break", "return", "import", "export", "value", "data", "type",
    "trait", "impl", "true", "false", "and", "or", "not", "defer", "spawn",
    "await", "async",
];

/// Build the full completion item list from cached document state.
///
/// Combines four sources:
/// 1. User-defined names from the resolver (`state.symbols`)
/// 2. Public prelude functions (excludes `__`-prefixed intrinsics)
/// 3. Prelude constructors + types
/// 4. Language keywords
///
/// v0.1 limitation: completion is position-independent — every name defined
/// anywhere in the file is offered regardless of the cursor's scope.
fn build_completion_items(state: &DocState) -> Vec<CompletionItem> {
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

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| TyraLsp {
        client,
        documents: Mutex::new(HashMap::new()),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_diagnostics::{Diagnostic as TyraDiag, Label, SourceMap, Span};

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
        // "world" starts at byte 6 on line 2 col 1 → LSP line 1, char 0
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

    /// Hover over a known binding returns the correct type string.
    #[test]
    fn hover_type_index_lookup() {
        let src = "let x: Int = 1\n";
        let (report, sources, type_index, _, _, source_id) =
            tyra_driver::check_in_memory("test.tyra".into(), src.into(), None);
        assert!(!report.has_errors(), "unexpected errors: {:?}", report.diagnostics());

        // Hover at line 0, col 4 → inside the let statement span.
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

    /// Go-to-definition: reference to `x` in `let y = x + 1` resolves to the
    /// `let x` definition span starting at byte 0.
    #[test]
    fn goto_definition_def_index_lookup() {
        // "let x: Int = 1\n" is 15 bytes; "let y = x + 1\n" follows.
        // 'x' in "let y = x + 1" sits at byte offset 15+8 = 23.
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
        // def_span is the let stmt starting at the beginning of the source
        assert_eq!(def_span.start, 0, "expected definition at start of `let x` stmt");
    }

    /// Smoke test: JSON-RPC `textDocument/definition` returns a `Location`
    /// pointing back into the same document.
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

        // Position line 1 col 8 = 'x' in "let y = x + 1"
        let def_req = Request::build("textDocument/definition")
            .params(json!({
                "textDocument": { "uri": "file:///tmp/def_test.tyra" },
                "position": { "line": 1, "character": 8 }
            }))
            .id(2)
            .finish();
        let resp = service.ready().await.unwrap().call(def_req).await.unwrap();
        let body = serde_json::to_value(&resp).unwrap();

        // GotoDefinitionResponse::Scalar serialises as a single Location object.
        assert!(
            body["result"].is_object(),
            "expected a Location object, got: {body}"
        );
        assert!(
            body["result"]["uri"]
                .as_str()
                .map(|s| s.contains("def_test.tyra"))
                .unwrap_or(false),
            "expected def_test.tyra in uri, got: {body}"
        );
    }

    /// Completion includes user-defined locals and prelude names.
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

    /// `__`-prefixed intrinsics are excluded from completion.
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

    /// JSON-RPC smoke: `textDocument/completion` returns an array containing `println`.
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

        assert!(
            body["result"].is_array(),
            "expected array response, got: {body}"
        );
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

    /// Smoke test: JSON-RPC `textDocument/didOpen` with an E0110-triggering
    /// source must produce a `textDocument/publishDiagnostics` notification
    /// on the client socket that contains the E0110 code.
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

        // initialize — tower-lsp transitions to Initialized on the response, so no
        // separate `initialized` notification is needed for the test to work.
        let init = Request::build("initialize")
            .params(json!({"capabilities": {}}))
            .id(1)
            .finish();
        let _ = service.ready().await.unwrap().call(init).await.unwrap();

        // did_open with import inside fn body (→ E0110)
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

        // publishDiagnostics is sent inline within the did_open call above, so
        // the socket already has exactly one message waiting.
        let msg = socket.next().await.expect("expected publishDiagnostics notification");
        let body = serde_json::to_value(&msg).unwrap();
        assert_eq!(body["method"], "textDocument/publishDiagnostics");
        let diags = body["params"]["diagnostics"].as_array().unwrap();
        assert!(
            diags.iter().any(|d| d["code"] == "E0110"),
            "expected E0110 diagnostic, got: {body}"
        );
    }
}
