use std::collections::HashMap;

use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tyra_diagnostics::{Level, SourceMap};
use tyra_driver::{DefIndex, SourceId, SymbolList, TypeIndex};

mod completion;
use completion::{
    build_completion_items, detect_member_receiver, lookup_receiver_ty,
    module_member_completions, type_method_completions,
};

mod keywords;
mod outline;
mod references;
mod rename;
mod signature;
mod tokens;

const DIAG_SOURCE: &str = "tyra";

/// Cached analysis result for one open document.
pub(crate) struct DocState {
    pub(crate) text: String,
    pub(crate) sources: SourceMap,
    pub(crate) type_index: TypeIndex,
    pub(crate) def_index: DefIndex,
    pub(crate) symbols: SymbolList,
    pub(crate) source_id: SourceId,
    pub(crate) ast: tyra_driver::SourceFile,
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
            Ok(Ok(tyra_driver::CheckResult { report, sources, type_index, def_index, symbols, source_id, ast })) => {
                let diags = report
                    .diagnostics()
                    .iter()
                    .map(|d| to_lsp_diagnostic(d, &sources))
                    .collect();

                self.documents.lock().await.insert(
                    uri.clone(),
                    DocState { text, sources, type_index, def_index, symbols, source_id, ast },
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
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string()]),
                    ..Default::default()
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            work_done_progress_options: Default::default(),
                            legend: tokens::legend(),
                            range: Some(false),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                        },
                    ),
                ),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    work_done_progress_options: Default::default(),
                }),
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
        let pos = params.text_document_position.position;
        let docs = self.documents.lock().await;
        let state = match docs.get(uri) {
            Some(s) => s,
            None => return Ok(None),
        };

        // If the cursor is immediately after `<ident>.`, switch to member-access
        // completion mode and return only members of that receiver.
        if let Some(receiver) = detect_member_receiver(&state.text, pos) {
            let mut items = module_member_completions(&receiver, state);
            if let Some(ty) = lookup_receiver_ty(&receiver, state) {
                items.extend(type_method_completions(&ty));
            }
            return Ok(Some(CompletionResponse::Array(items)));
        }

        Ok(Some(CompletionResponse::Array(build_completion_items(state))))
    }

    async fn references(
        &self,
        params: ReferenceParams,
    ) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let include_decl = params.context.include_declaration;

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

        let def_span = match references::find_def_span_at_cursor(state, offset) {
            Some(s) => s,
            None => return Ok(None),
        };

        let mut use_spans =
            references::find_uses_for_def(&state.def_index, def_span, state.source_id);
        if include_decl {
            use_spans.push(def_span);
        }

        let locations = use_spans
            .into_iter()
            .map(|s| Location {
                uri: uri.clone(),
                range: span_to_lsp_range(s, &state.sources),
            })
            .collect();
        Ok(Some(locations))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let new_name = &params.new_name;

        if !rename::is_valid_identifier(new_name) {
            return Err(tower_lsp::jsonrpc::Error::invalid_params(format!(
                "`{new_name}` is not a valid Tyra identifier"
            )));
        }

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

        let old_name = match rename::extract_identifier_at(&state.text, offset) {
            Some(n) => n,
            None => return Ok(None),
        };

        let def_span = match references::find_def_span_at_cursor(state, offset) {
            Some(s) => s,
            None => return Ok(None),
        };

        let use_spans =
            references::find_uses_for_def(&state.def_index, def_span, state.source_id);
        let mut edits: Vec<TextEdit> = use_spans
            .into_iter()
            .map(|s| TextEdit {
                range: span_to_lsp_range(s, &state.sources),
                new_text: new_name.clone(),
            })
            .collect();

        if let Some(name_span) =
            rename::find_binding_name_span(&state.text, def_span, &old_name)
        {
            edits.push(TextEdit {
                range: span_to_lsp_range(name_span, &state.sources),
                new_text: new_name.clone(),
            });
        }

        // Sort edits by position so clients that apply them in order behave correctly.
        edits.sort_by(|a, b| {
            a.range.start.line.cmp(&b.range.start.line)
                .then(a.range.start.character.cmp(&b.range.start.character))
        });

        let mut changes = HashMap::new();
        changes.insert(uri.clone(), edits);
        Ok(Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.lock().await;
        let state = match docs.get(uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        let symbols =
            outline::build_document_symbols(state.source_id, &state.ast, &state.sources);
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.lock().await;
        let state = match docs.get(uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        let toks = tokens::build_full(
            &state.text,
            &state.ast,
            &state.def_index,
            state.source_id,
            &state.sources,
        );
        Ok(Some(SemanticTokensResult::Tokens(toks)))
    }

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>> {
        let pos = params.text_document_position_params.position;
        let uri = &params.text_document_position_params.text_document.uri;
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
        let (callee, active_parameter) =
            match signature::find_active_call(&state.text, offset as usize) {
                Some(c) => (c.callee.to_string(), c.active_parameter),
                None => return Ok(None),
            };
        let sig = state
            .ast
            .items
            .iter()
            .find_map(|it| match it {
                tyra_ast::Item::FnDef(f) if f.name == callee => {
                    Some(signature::build_signature_for_fn(f))
                }
                _ => None,
            })
            .or_else(|| signature::prelude_signature(&callee));
        let sig = match sig {
            Some(s) => s,
            None => return Ok(None),
        };
        Ok(Some(SignatureHelp {
            signatures: vec![sig],
            active_signature: Some(0),
            active_parameter: Some(active_parameter),
        }))
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
mod tests;

