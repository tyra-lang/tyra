use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tyra_diagnostics::{Level, SourceMap};
use tyra_driver::{DefIndex, SourceId, SymbolList, TypeIndex};

mod completion;
use completion::{
    build_completion_items, detect_member_receiver, lookup_receiver_ty, module_member_completions,
    type_method_completions,
};

mod call_hierarchy;
mod code_action;
mod code_lens;
mod declaration;
mod document_link;
mod file_delete;
mod file_rename;
mod folding;
mod implementation;
mod inlay;
mod keywords;
mod outline;
mod references;
mod rename;
mod selection_range;
mod signature;
mod tokens;
mod type_definition;
mod type_hierarchy;
mod workspace_symbol;

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
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) version: i32,
}

struct TyraLsp {
    client: Client,
    // Per-document state: text + type index for hover, updated on every edit.
    // NOTE: writes and `analyze` calls are not atomic — if two `did_change`
    // events race, the store may briefly hold a stale state. A version guard
    // should be added when incremental sync lands.
    documents: Mutex<HashMap<Url, DocState>>,
    // Whether the client supports dynamic registration for type hierarchy.
    // Set during initialize; read in initialized to decide whether to register.
    type_hierarchy_dynamic: AtomicBool,
    // Whether the client supports dynamic registration for watched files.
    did_change_watched_files_dynamic: AtomicBool,
}

impl TyraLsp {
    /// Analyze `text` as the content of `uri` and publish diagnostics.
    ///
    /// `workspace_dir` is forwarded to `check_in_memory` to enable filesystem
    /// import resolution (`resolve_imports`).  Pass `None` for single-file
    /// analysis (didOpen / didChange); pass the document's parent directory
    /// when re-analyzing after external file changes so that imported modules
    /// are re-read from disk.
    async fn analyze(
        &self,
        uri: Url,
        text: String,
        version: i32,
        workspace_dir: Option<std::path::PathBuf>,
    ) {
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
                tyra_driver::check_in_memory(file_name, text_clone, workspace_dir.as_deref())
            }))
        })
        .await;

        let lsp_diags = match result {
            Ok(Ok(tyra_driver::CheckResult {
                report,
                sources,
                type_index,
                def_index,
                symbols,
                source_id,
                ast,
            })) => {
                let diags: Vec<Diagnostic> = report
                    .diagnostics()
                    .iter()
                    .map(|d| to_lsp_diagnostic(d, &sources))
                    .collect();

                self.documents.lock().await.insert(
                    uri.clone(),
                    DocState {
                        text,
                        sources,
                        type_index,
                        def_index,
                        symbols,
                        source_id,
                        ast,
                        diagnostics: diags.clone(),
                        version,
                    },
                );

                diags
            }
            Ok(Err(_panic_payload)) => {
                // The compiler panicked — report it as an internal error so
                // the user knows something went wrong and can file a bug.
                let diags = vec![Diagnostic {
                    range: Range::default(),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String("E9999".into())),
                    source: Some(DIAG_SOURCE.into()),
                    message: "internal compiler error — please report this bug".into(),
                    ..Default::default()
                }];
                // Keep pull diagnostics in sync: update the cached state so
                // textDocument/diagnostic returns the same E9999 as the push path.
                if let Some(state) = self.documents.lock().await.get_mut(&uri) {
                    state.diagnostics = diags.clone();
                    state.version = version;
                }
                diags
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

    let message = first_label.filter(|l| !l.message.is_empty()).map_or_else(
        || d.message.clone(),
        |l| format!("{} — {}", d.message, l.message),
    );

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
pub(crate) fn span_to_lsp_range(span: tyra_diagnostics::Span, sources: &SourceMap) -> Range {
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
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let supports_dynamic = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|td| td.type_hierarchy.as_ref())
            .and_then(|th| th.dynamic_registration)
            == Some(true);
        self.type_hierarchy_dynamic
            .store(supports_dynamic, Ordering::Relaxed);

        let supports_watched_files_dynamic = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.did_change_watched_files.as_ref())
            .and_then(|d| d.dynamic_registration)
            == Some(true);
        self.did_change_watched_files_dynamic
            .store(supports_watched_files_dynamic, Ordering::Relaxed);

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
                implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
                declaration_provider: Some(DeclarationCapability::Simple(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string()]),
                    ..Default::default()
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
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
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(
                    DiagnosticOptions {
                        identifier: Some("tyra".to_string()),
                        inter_file_dependencies: false,
                        workspace_diagnostics: true,
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                    },
                )),
                inlay_hint_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
                linked_editing_range_provider: Some(LinkedEditingRangeServerCapabilities::Simple(
                    true,
                )),
                workspace: Some(WorkspaceServerCapabilities {
                    file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                        will_rename: Some(FileOperationRegistrationOptions {
                            filters: vec![FileOperationFilter {
                                scheme: Some("file".to_string()),
                                pattern: FileOperationPattern {
                                    glob: "**/*.tyra".to_string(),
                                    matches: Some(FileOperationPatternKind::File),
                                    options: None,
                                },
                            }],
                        }),
                        will_delete: Some(FileOperationRegistrationOptions {
                            filters: vec![FileOperationFilter {
                                scheme: Some("file".to_string()),
                                pattern: FileOperationPattern {
                                    glob: "**/*.tyra".to_string(),
                                    matches: Some(FileOperationPatternKind::File),
                                    options: None,
                                },
                            }],
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
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

        // lsp-types 0.94.1 lacks type_hierarchy_provider on ServerCapabilities.
        // Register dynamically only when the client declared support for it;
        // static-only clients do not process client/registerCapability.
        if self.type_hierarchy_dynamic.load(Ordering::Relaxed) {
            let reg = Registration {
                id: "tyra-type-hierarchy".to_string(),
                method: "textDocument/prepareTypeHierarchy".to_string(),
                register_options: Some(serde_json::json!({
                    "documentSelector": [{ "language": "tyra" }]
                })),
            };
            if let Err(e) = self.client.register_capability(vec![reg]).await {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("type hierarchy registration failed: {e}"),
                    )
                    .await;
            }
        }

        if self
            .did_change_watched_files_dynamic
            .load(Ordering::Relaxed)
        {
            let opts = DidChangeWatchedFilesRegistrationOptions {
                watchers: vec![FileSystemWatcher {
                    glob_pattern: GlobPattern::String("**/*.tyra".into()),
                    kind: None,
                }],
            };
            let reg = Registration {
                id: "tyra-watched-files".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: serde_json::to_value(opts).ok(),
            };
            if let Err(e) = self.client.register_capability(vec![reg]).await {
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("watched files registration failed: {e}"),
                    )
                    .await;
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;
        self.analyze(uri, text, version, None).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        // TextDocumentSyncKind::FULL → changes[0].text is the entire new content.
        match params.content_changes.into_iter().next() {
            Some(change) => {
                self.analyze(uri, change.text, version, None).await;
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

    async fn goto_type_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(offset) = state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
        else {
            return Ok(None);
        };
        let Some(span) = type_definition::find_type_def_span(
            &state.ast,
            &state.type_index,
            state.source_id,
            offset,
        ) else {
            return Ok(None);
        };
        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range: span_to_lsp_range(span, &state.sources),
        })))
    }

    async fn goto_implementation(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(offset) = state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
        else {
            return Ok(None);
        };
        let spans = implementation::find_implementations(&state.ast, &state.text, offset);
        if spans.is_empty() {
            return Ok(None);
        }
        let locs: Vec<Location> = spans
            .into_iter()
            .map(|s| Location {
                uri: uri.clone(),
                range: span_to_lsp_range(s, &state.sources),
            })
            .collect();
        Ok(Some(GotoDefinitionResponse::Array(locs)))
    }

    async fn goto_declaration(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(offset) = state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
        else {
            return Ok(None);
        };
        let Some(span) = declaration::find_declaration(&state.ast, &state.text, offset) else {
            return Ok(None);
        };
        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range: span_to_lsp_range(span, &state.sources),
        })))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
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

        Ok(Some(CompletionResponse::Array(build_completion_items(
            state,
        ))))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(offset) = state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
        else {
            return Ok(None);
        };
        let Some(def_span) = references::find_def_span_at_cursor(state, offset) else {
            return Ok(None);
        };
        let mut spans = references::find_uses_for_def(&state.def_index, def_span, state.source_id);
        // Narrow def_span to just the binding name token (def_span covers the
        // whole declaration, e.g. an entire `fn ... end` block).
        let def_name_span = rename::extract_identifier_at(&state.text, offset)
            .and_then(|name| rename::find_binding_name_span(&state.text, def_span, &name))
            .unwrap_or(def_span);
        spans.push(def_name_span);
        let highlights = spans
            .into_iter()
            .map(|s| DocumentHighlight {
                range: span_to_lsp_range(s, &state.sources),
                kind: Some(DocumentHighlightKind::TEXT),
            })
            .collect();
        Ok(Some(highlights))
    }

    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(offset) = state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
        else {
            return Ok(None);
        };
        let Some(def_span) = references::find_def_span_at_cursor(state, offset) else {
            return Ok(None);
        };
        // Is the cursor on a use-site ref-span (rather than inside the declaration)?
        let at_use_site = state.def_index.iter().any(|(&ref_span, &ds)| {
            ds == def_span
                && ref_span.source == state.source_id
                && ref_span.start <= offset
                && offset < ref_span.end
        });
        // Narrow def_span to the identifier name token; bail if narrowing fails
        // so all returned ranges have identical length (LSP requirement).
        let Some(name) = rename::extract_identifier_at(&state.text, offset) else {
            return Ok(None);
        };
        let Some(def_name_span) = rename::find_binding_name_span(&state.text, def_span, &name)
        else {
            return Ok(None);
        };
        // At a def site the cursor must land specifically on the binding name
        // token (not on `let`, a type annotation, or the RHS expression).
        if !at_use_site && !(def_name_span.start <= offset && offset < def_name_span.end) {
            return Ok(None);
        }
        let mut spans = references::find_uses_for_def(&state.def_index, def_span, state.source_id);
        spans.push(def_name_span);
        let ranges = spans
            .into_iter()
            .map(|s| span_to_lsp_range(s, &state.sources))
            .collect();
        Ok(Some(LinkedEditingRanges {
            ranges,
            word_pattern: None,
        }))
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(offset) = state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
        else {
            return Ok(None);
        };
        Ok(call_hierarchy::prepare(state, uri, offset).map(|item| vec![item]))
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let uri = &params.item.uri;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        Ok(Some(call_hierarchy::incoming(state, uri, &params.item)))
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let uri = &params.item.uri;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        Ok(Some(call_hierarchy::outgoing(state, uri, &params.item)))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let mut out = Vec::with_capacity(params.positions.len());
        for pos in &params.positions {
            let Some(offset) =
                state
                    .sources
                    .offset_at_utf16(state.source_id, pos.line, pos.character)
            else {
                return Ok(None);
            };
            let Some(sr) =
                selection_range::compute(&state.ast, &state.sources, state.source_id, offset)
            else {
                return Ok(None);
            };
            out.push(sr);
        }
        Ok(Some(out))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
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

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let pos = params.position;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let Some(offset) = state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
        else {
            return Ok(None);
        };

        let Some(name_span) =
            rename::extract_identifier_span_at(&state.text, state.source_id, offset)
        else {
            return Ok(None);
        };
        let name = state.text[name_span.start as usize..name_span.end as usize].to_string();

        if !rename::is_valid_identifier(&name) {
            return Err(tower_lsp::jsonrpc::Error::invalid_params(format!(
                "`{name}` is not renameable"
            )));
        }

        let Some(def_span) = references::find_def_span_at_cursor(state, offset) else {
            return Ok(None);
        };

        // Guard: the identifier under the cursor must actually be the rename
        // target — either a recorded use-span in def_index or the binding-name
        // token of the definition.  Without this check, any identifier that
        // happens to fall inside a definition span (e.g. `Int` in `let x: Int`)
        // would be falsely advertised as renameable.
        let is_use_span = state
            .def_index
            .iter()
            .any(|(s, _)| s.source == state.source_id && *s == name_span);
        let is_binding_name = rename::find_binding_name_span(&state.text, def_span, &name)
            .map(|s| s == name_span)
            .unwrap_or(false);
        if !is_use_span && !is_binding_name {
            return Ok(None);
        }

        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: span_to_lsp_range(name_span, &state.sources),
            placeholder: name,
        }))
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

        let use_spans = references::find_uses_for_def(&state.def_index, def_span, state.source_id);
        let mut edits: Vec<TextEdit> = use_spans
            .into_iter()
            .map(|s| TextEdit {
                range: span_to_lsp_range(s, &state.sources),
                new_text: new_name.clone(),
            })
            .collect();

        if let Some(name_span) = rename::find_binding_name_span(&state.text, def_span, &old_name) {
            edits.push(TextEdit {
                range: span_to_lsp_range(name_span, &state.sources),
                new_text: new_name.clone(),
            });
        }

        // Sort edits by position so clients that apply them in order behave correctly.
        edits.sort_by(|a, b| {
            a.range
                .start
                .line
                .cmp(&b.range.start.line)
                .then(a.range.start.character.cmp(&b.range.start.character))
        });

        let mut changes = HashMap::new();
        changes.insert(uri.clone(), edits);
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }))
    }

    async fn will_rename_files(&self, params: RenameFilesParams) -> Result<Option<WorkspaceEdit>> {
        let docs = self.documents.lock().await;
        Ok(file_rename::compute_edits(&docs, &params.files))
    }

    async fn will_delete_files(&self, params: DeleteFilesParams) -> Result<Option<WorkspaceEdit>> {
        let docs = self.documents.lock().await;
        Ok(file_delete::compute_edits(&docs, &params.files))
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
        let symbols = outline::build_document_symbols(state.source_id, &state.ast, &state.sources);
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let docs = self.documents.lock().await;
        let syms = workspace_symbol::collect(&params.query, &docs);
        if syms.is_empty() {
            Ok(None)
        } else {
            Ok(Some(syms))
        }
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

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
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

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let lenses = code_lens::build_code_lenses(state);
        if lenses.is_empty() {
            Ok(None)
        } else {
            Ok(Some(lenses))
        }
    }

    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let pos = params.text_document_position_params.position;
        let uri = &params.text_document_position_params.text_document.uri;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let offset = state
            .sources
            .offset_at_utf16(state.source_id, pos.line, pos.character)
            .unwrap_or(0);
        let items = type_hierarchy::prepare(&state.ast, &state.text, &state.sources, uri, offset);
        Ok(if items.is_empty() { None } else { Some(items) })
    }

    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let uri = params.item.uri.clone();
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(Some(vec![]));
        };
        let items =
            type_hierarchy::supertypes(&state.ast, &state.text, &state.sources, &uri, &params.item);
        Ok(Some(items))
    }

    async fn subtypes(
        &self,
        params: TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let uri = params.item.uri.clone();
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(Some(vec![]));
        };
        let items =
            type_hierarchy::subtypes(&state.ast, &state.text, &state.sources, &uri, &params.item);
        Ok(Some(items))
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = &params.text_document.uri;
        let docs = self.documents.lock().await;
        let items = docs
            .get(uri)
            .map(|s| s.diagnostics.clone())
            .unwrap_or_default();
        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items,
                },
            }),
        ))
    }

    async fn workspace_diagnostic(
        &self,
        _params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let docs = self.documents.lock().await;
        let items: Vec<WorkspaceDocumentDiagnosticReport> = docs
            .iter()
            .map(|(uri, state)| {
                WorkspaceDocumentDiagnosticReport::Full(WorkspaceFullDocumentDiagnosticReport {
                    uri: uri.clone(),
                    version: Some(state.version as i64),
                    full_document_diagnostic_report: FullDocumentDiagnosticReport {
                        result_id: None,
                        items: state.diagnostics.clone(),
                    },
                })
            })
            .collect();
        Ok(WorkspaceDiagnosticReportResult::Report(
            WorkspaceDiagnosticReport { items },
        ))
    }

    async fn did_change_watched_files(&self, _params: DidChangeWatchedFilesParams) {
        // Any .tyra file created/changed/deleted outside the editor may affect
        // import resolution for all open docs. Re-analyze everything with the
        // document's parent directory so resolve_imports re-reads modules from
        // disk and picks up the external change.
        let entries: Vec<(Url, String, i32)> = {
            let docs = self.documents.lock().await;
            docs.iter()
                .map(|(uri, state)| (uri.clone(), state.text.clone(), state.version))
                .collect()
        };
        for (uri, text, version) in entries {
            let workspace_dir = uri
                .to_file_path()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()));
            self.analyze(uri, text, version, workspace_dir).await;
        }
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let main_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let links =
            document_link::collect(&state.ast, &state.text, &state.sources, main_dir.as_deref());
        Ok(if links.is_empty() { None } else { Some(links) })
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(&uri) else {
            return Ok(None);
        };
        let actions = code_action::build_actions(
            &uri,
            &params.context.diagnostics,
            &state.symbols,
            params.context.only.as_deref(),
        );
        Ok(if actions.is_empty() {
            None
        } else {
            Some(actions)
        })
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let hints = inlay::build_hints(
            &state.ast,
            &state.type_index,
            &state.sources,
            state.source_id,
            params.range,
        );
        Ok(Some(hints))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.lock().await;
        let Some(state) = docs.get(uri) else {
            return Ok(None);
        };
        let ranges = folding::build_ranges(&state.ast, &state.sources, state.source_id);
        Ok(Some(ranges))
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
            .filter(|(span, _)| {
                span.source == state.source_id && span.start <= offset && offset < span.end
            })
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
        type_hierarchy_dynamic: AtomicBool::new(false),
        did_change_watched_files_dynamic: AtomicBool::new(false),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests;
