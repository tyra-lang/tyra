use std::collections::HashMap;

use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tyra_diagnostics::Level;

const DIAG_SOURCE: &str = "tyra";

struct TyraLsp {
    client: Client,
    // Kept for future hover / completion support, where handlers need to
    // re-read the latest source text without going back to disk.
    // NOTE: writes and `analyze` calls are not atomic — if two `did_change`
    // events race, `documents` may hold the newer text while an older
    // diagnostic result is still in flight. A version guard should be added
    // once hover/completion land (compare `version` before publishing).
    documents: Mutex<HashMap<Url, String>>,
}

impl TyraLsp {
    async fn analyze(&self, uri: Url, text: String, version: i32) {
        let file_name: String = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "untitled.tyra".to_string());

        // Run the compiler pipeline on a blocking thread so we don't stall
        // the async executor, and catch any unexpected panics so a compiler
        // bug never brings down the LSP process.
        let result = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tyra_driver::check_in_memory(file_name, text, None)
            }))
        })
        .await;

        let lsp_diags = match result {
            Ok(Ok((report, sources))) => report
                .diagnostics()
                .iter()
                .map(|d| to_lsp_diagnostic(d, &sources))
                .collect(),
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
                // Don't publish anything; the client is going away anyway.
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
        .map(|label| {
            let (sl, sc) = sources.line_col(label.span.source, label.span.start);
            let (el, ec) = sources.line_col(label.span.source, label.span.end);
            Range {
                start: Position {
                    line: sl.saturating_sub(1),
                    character: sc.saturating_sub(1),
                },
                end: Position {
                    line: el.saturating_sub(1),
                    character: ec.saturating_sub(1),
                },
            }
        })
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

#[tower_lsp::async_trait]
impl LanguageServer for TyraLsp {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
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
        self.documents.lock().await.insert(uri.clone(), text.clone());
        self.analyze(uri, text, version).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        // TextDocumentSyncKind::FULL → changes[0].text is the entire new content.
        match params.content_changes.into_iter().next() {
            Some(change) => {
                self.documents.lock().await.insert(uri.clone(), change.text.clone());
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
        // Sending `initialized` would fire log_message, filling the capacity-1
        // channel before analyze() can send publishDiagnostics, causing a deadlock
        // while service.call(did_open) is still driving analyze() inline.
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
