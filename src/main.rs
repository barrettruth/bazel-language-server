//! Language server for Bazel build files.
//!
//! The main loop owns the open documents and never blocks. Slow work belongs on
//! the Bazel thread, which publishes an index the handlers read as a snapshot.
//!
//! stdout is the LSP transport. Everything human-readable goes to stderr.

mod actor;
mod bazel;
pub mod bazelrc;
mod document;
mod format;
mod graph;
mod handlers;
mod index;
mod label;
mod line_index;
mod repos;
mod watch;
mod worker;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::actor::Bazel;
use crate::bazel::{BazelClient, BazelConfig};
use crate::document::{Document, Documents};
use crate::index::{Index, IndexHandle};
use anyhow::Result;
use clap::{Parser, Subcommand};
use lsp_server::{Connection, Message, ReqQueue, RequestId, Response};
use lsp_types::{
    CancelNotification, CancelParams, CodeLensRequest, Definition, DefinitionRequest,
    DefinitionResponse, DidChangeConfigurationNotification, DidChangeConfigurationParams,
    DidChangeTextDocumentNotification, DidChangeTextDocumentParams,
    DidCloseTextDocumentNotification, DidCloseTextDocumentParams, DidOpenTextDocumentNotification,
    DidOpenTextDocumentParams, DocumentFormattingRequest, DocumentHighlightRequest,
    DocumentLinkRequest, DocumentSymbolRequest, ExecuteCommandRequest, FoldingRangeRequest,
    HoverRequest, ImplementationRequest, InitializeParams, InlayHintRequest, Location,
    LocationLink, LspNotificationMethod, LspRequestMethod, Notification, PrepareRenameRequest,
    PrepareRenameResult, ProgressNotification, ProgressParams, ProgressToken,
    PublishDiagnosticsNotification, PublishDiagnosticsParams, ReferencesRequest, RenameOptions,
    RenameRequest, Request as _, SelectionRangeRequest, SemanticTokensRequest, ServerCapabilities,
    TextDocumentSync, TextDocumentSyncKind, TextDocumentSyncOptions, TextEdit, Uri,
    WorkDoneProgressBegin, WorkDoneProgressCreateParams, WorkDoneProgressCreateRequest,
    WorkDoneProgressEnd, WorkspaceSymbolRequest,
};

const SERVER_CANCELLED: i32 = -32802;

enum Completed {
    Response(Response),
    Diagnostics {
        uri: Uri,
        document: Arc<Document>,
        diagnostics: Vec<lsp_types::Diagnostic>,
    },
}

enum Outgoing {
    IndexProgress,
}

struct IndexProgress {
    token: ProgressToken,
    created: bool,
    ready: Option<watch::Ready>,
}

impl IndexProgress {
    fn created(&mut self, connection: &Connection) -> Result<()> {
        self.created = true;
        send_progress(
            connection,
            &self.token,
            WorkDoneProgressBegin {
                title: "Indexing Bazel workspace".to_owned(),
                cancellable: Some(false),
                message: None,
                percentage: None,
            },
        )?;
        if let Some(ready) = self.ready.take() {
            self.finish(connection, &ready)?;
        }
        Ok(())
    }

    fn ready(&mut self, connection: &Connection, ready: watch::Ready) -> Result<()> {
        if self.created {
            self.finish(connection, &ready)
        } else {
            self.ready = Some(ready);
            Ok(())
        }
    }

    fn finish(&self, connection: &Connection, ready: &watch::Ready) -> Result<()> {
        send_progress(
            connection,
            &self.token,
            WorkDoneProgressEnd {
                message: Some(format!(
                    "{} BUILD files, {} targets in {:.2}s",
                    ready.files,
                    ready.targets,
                    ready.elapsed.as_secs_f64()
                )),
            },
        )
    }
}

struct RequestContext<'a> {
    connection: &'a Connection,
    workers: &'a worker::Pool<Completed>,
    index: &'a IndexHandle,
    configuration: &'a bazelrc::ConfigurationHandle,
    root: Option<&'a Path>,
    bazel: &'a Bazel,
    watch: Option<&'a watch::Watch>,
    link_support: bool,
}

impl RequestContext<'_> {
    fn dispatch(
        &self,
        requests: &mut ReqQueue<worker::Cancellation, Outgoing>,
        docs: &Documents,
        request: lsp_server::Request,
    ) -> Result<bool> {
        if self.connection.handle_shutdown(&request)? {
            return Ok(true);
        }

        let method: LspRequestMethod<'_> = request.method.as_str().into();
        if method == ExecuteCommandRequest::METHOD {
            let response = match execute_command(&request, self.root, self.bazel, self.watch) {
                Ok(value) => Response::new_ok(request.id.clone(), value),
                Err(err) => request_error(&request, &err),
            };
            self.connection.sender.send(Message::Response(response))?;
            return Ok(false);
        }

        let cancellation = worker::Cancellation::default();
        requests
            .incoming
            .register(request.id.clone(), cancellation.clone());
        let queued_id = request.id.clone();
        let snapshot = docs.clone();
        let index = self.index.load();
        let configuration = self.configuration.load();
        let root = self.root.map(Path::to_path_buf);
        let link_support = self.link_support;
        let admitted = self.workers.execute(move || {
            let id = request.id.clone();
            let method = request.method.clone();
            if cancellation.is_cancelled() {
                return Completed::Response(Response::new_err(
                    id,
                    lsp_server::ErrorCode::RequestCanceled as i32,
                    "canceled by client".to_owned(),
                ));
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                respond(
                    &request,
                    &snapshot,
                    &index,
                    &configuration,
                    root.as_deref(),
                    link_support,
                    &cancellation,
                )
            }));
            let response = if let Ok(result) = result {
                request_result(&request, result)
            } else {
                tracing::error!(%method, "request handler panicked");
                Response::new_err(
                    id,
                    lsp_server::ErrorCode::InternalError as i32,
                    "request handler panicked".to_owned(),
                )
            };
            Completed::Response(response)
        });
        if !admitted {
            requests.incoming.complete(&queued_id);
            self.connection
                .sender
                .send(Message::Response(Response::new_err(
                    queued_id,
                    SERVER_CANCELLED,
                    "server request queue is full".to_owned(),
                )))?;
        }
        Ok(false)
    }
}

#[derive(Parser)]
#[command(name = "bazel-language-server", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Speak LSP over stdio. The default.
    Server,
    /// Index a workspace and report what was found, without an editor.
    ///
    /// Writes to stdout because it is not speaking the protocol.
    Index {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Report whether the Bazel subsystem can run here.
    Doctor {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("BLS_LOG")
                .unwrap_or_else(|_| "bazel_language_server=info".into()),
        )
        .init();

    match Cli::parse().command {
        Some(Command::Index { path }) => {
            cmd_index(&path);
            Ok(())
        }
        Some(Command::Doctor { path }) => {
            cmd_doctor(&path);
            Ok(())
        }
        Some(Command::Server) | None => run_server(),
    }
}

fn cmd_index(path: &std::path::Path) {
    let root = crate::bazel::find_workspace(path).map_or_else(
        || path.to_path_buf(),
        |workspace| {
            println!(
                "workspace {} (via {})",
                workspace.root.display(),
                workspace.marker
            );
            workspace.root
        },
    );
    let started = std::time::Instant::now();
    let handle = IndexHandle::new();
    handle.store_disk(crate::index::build_static(&root));
    let parsed = handle.load();
    println!(
        "indexed {} BUILD files, {} targets in {:.2}s",
        parsed.files(),
        parsed.len(),
        started.elapsed().as_secs_f64()
    );
    for (label, target) in parsed.targets().take(10) {
        println!("  {:<40} {}", label, target.rule);
    }
    if !cmd_graph(&root, &handle) {
        return;
    }
    let index = handle.load();
    // Bazel answers with real paths, so shortening one against the root the
    // user typed only works if that root is a real path too.
    let base = root.canonicalize();
    let base = base.as_deref().unwrap_or(&root);
    let mut only_bazel: Vec<_> = index
        .targets()
        .filter(|(label, _)| index.only_bazel_knows(label))
        .collect();
    only_bazel.sort_unstable_by_key(|(label, _)| *label);
    println!(
        "\n{} of {} targets are named by a macro, so no parser can see them:",
        only_bazel.len(),
        index.len()
    );
    for (label, target) in only_bazel.iter().take(10) {
        println!(
            "  {:<40} {} at {}:{}",
            label,
            target.rule,
            target
                .file
                .strip_prefix(base)
                .unwrap_or(&target.file)
                .display(),
            target.line + 1
        );
    }
}

/// Publish the Bazel graph when available.
fn cmd_graph(root: &Path, handle: &IndexHandle) -> bool {
    let client = BazelClient::new(BazelConfig::default(), root.to_path_buf());
    if let Err(err) = client.probe() {
        println!("\nstatic tier only: {err:#}");
        println!("targets from legacy macros are not counted");
        return false;
    }
    match crate::graph::query(&client, |_| {}) {
        Ok(query) if query.outcome.ok() => {
            handle.store_graph(query.tier);
            true
        }
        Ok(query) => {
            println!(
                "\nbazel query declined: {}",
                query.outcome.stderr.lines().next_back().unwrap_or_default()
            );
            false
        }
        Err(err) => {
            println!("\nbazel query could not run: {err:#}");
            false
        }
    }
}

fn cmd_doctor(path: &std::path::Path) {
    let workspace = crate::bazel::find_workspace(path);
    match &workspace {
        Some(w) => println!("workspace  {} (via {})", w.root.display(), w.marker),
        None => println!("workspace  not found - static features only"),
    }
    let root = workspace.map_or_else(|| path.to_path_buf(), |w| w.root);
    let client = BazelClient::new(BazelConfig::default(), root);
    match client.probe() {
        Ok(probe) => {
            println!(
                "bazel      {} ({} or newer)",
                probe.version,
                crate::bazel::FLOOR
            );
            let offered = |yes: bool| if yes { "yes" } else { "no" };
            let oracles = probe.capabilities;
            // Both, because a wrapper that rewrites the line is the reason a
            // version ever reads wrong, and this is where you look.
            println!("  reported           {}", probe.reported);
            println!("  rule schemas       {}", offered(oracles.rule_classes));
            println!("  repository mapping {}", offered(oracles.repo_mapping));
            println!(
                "  query to a file    {}",
                offered(oracles.query_output_file)
            );
            match crate::repos::Repos::read(&client) {
                Ok(repos) => {
                    println!("repos      {} apparent names", repos.len());
                    println!(
                        "  external tree      {}",
                        repos.output_base().join("external").display()
                    );
                }
                Err(err) => println!("repos      unavailable: {err:#}"),
            }
        }
        Err(err) => println!("bazel      unavailable: {err:#}"),
    }
}

/// Read the `bazel` section used by initialization and configuration changes.
fn bazel_settings(sent: Option<&serde_json::Value>) -> Option<BazelConfig> {
    let section = sent?.get("bazel")?;
    match serde_json::from_value(section.clone()) {
        Ok(config) => Some(config),
        Err(err) => {
            tracing::warn!("the `bazel` settings could not be read, so the defaults stand: {err}");
            None
        }
    }
}

/// The request surface advertised to clients.
fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSync::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::Incremental),
            ..Default::default()
        })),
        document_symbol_provider: Some(lsp_types::DocumentSymbolProvider::Bool(true)),
        workspace_symbol_provider: Some(lsp_types::WorkspaceSymbolProvider::Bool(true)),
        definition_provider: Some(lsp_types::DefinitionProvider::Bool(true)),
        references_provider: Some(lsp_types::ReferencesProvider::Bool(true)),
        document_highlight_provider: Some(lsp_types::DocumentHighlightProvider::Bool(true)),
        hover_provider: Some(lsp_types::HoverProvider::Bool(true)),
        // Advertised whether or not buildifier is installed, the way the rest
        // of the server is advertised without Bazel: a capability withdrawn at
        // startup is one the user cannot get back by installing the tool.
        inlay_hint_provider: Some(lsp_types::InlayHintProvider::Bool(true)),
        code_lens_provider: Some(lsp_types::CodeLensOptions::default()),
        execute_command_provider: Some(lsp_types::ExecuteCommandOptions {
            commands: vec![
                handlers::RUN_COMMAND.to_string(),
                watch::REINDEX_COMMAND.to_string(),
            ],
            ..Default::default()
        }),
        implementation_provider: Some(lsp_types::ImplementationProvider::Bool(true)),
        semantic_tokens_provider: Some(lsp_types::SemanticTokensProvider::SemanticTokensOptions(
            lsp_types::SemanticTokensOptions {
                legend: lsp_types::SemanticTokensLegend {
                    token_types: handlers::LEGEND
                        .iter()
                        .map(|name| (*name).to_string())
                        .collect(),
                    token_modifiers: Vec::new(),
                },
                full: Some(lsp_types::Full::Bool(true)),
                ..Default::default()
            },
        )),
        document_link_provider: Some(lsp_types::DocumentLinkOptions::default()),
        selection_range_provider: Some(lsp_types::SelectionRangeProvider::Bool(true)),
        folding_range_provider: Some(lsp_types::FoldingRangeProvider::Bool(true)),
        document_formatting_provider: Some(lsp_types::DocumentFormattingProvider::Bool(true)),
        rename_provider: Some(lsp_types::RenameProvider::RenameOptions(RenameOptions {
            prepare_provider: Some(true),
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn initialize(connection: &Connection) -> Result<InitializeParams> {
    let (id, params) = connection.initialize_start()?;
    connection.initialize_finish(
        id,
        serde_json::json!({
            "capabilities": capabilities(),
            "serverInfo": {
                "name": "bazel-language-server",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
    )?;
    Ok(serde_json::from_value(params)?)
}

fn start_index_progress(
    connection: &Connection,
    init: &InitializeParams,
    has_workspace: bool,
    requests: &mut ReqQueue<worker::Cancellation, Outgoing>,
) -> Result<Option<IndexProgress>> {
    if !has_workspace || !supports_work_done_progress(init) {
        return Ok(None);
    }
    let token = ProgressToken::String("bazel-language-server-index".to_owned());
    let request = requests.outgoing.register(
        WorkDoneProgressCreateRequest::METHOD.to_string(),
        WorkDoneProgressCreateParams {
            token: token.clone(),
        },
        Outgoing::IndexProgress,
    );
    connection.sender.send(Message::Request(request))?;
    Ok(Some(IndexProgress {
        token,
        created: false,
        ready: None,
    }))
}

fn run_server() -> Result<()> {
    tracing::info!("bazel-language-server {}", env!("CARGO_PKG_VERSION"));
    let (connection, io_threads) = Connection::stdio();
    let init = initialize(&connection)?;

    let root = workspace_root(&init);
    let link_support = supports_definition_links(&init);
    let mut requests = ReqQueue::<worker::Cancellation, Outgoing>::default();
    let mut index_progress =
        start_index_progress(&connection, &init, root.is_some(), &mut requests)?;
    let index = IndexHandle::new();
    let configuration = bazelrc::ConfigurationHandle::new();
    let bazel = std::sync::Arc::new(Bazel::spawn(root.clone(), index.clone()));
    bazel.reconfigure(bazel_settings(init.initialization_options.as_ref()).unwrap_or_default());
    let (watch, mut ready_rx) = if let Some(root) = root.as_deref() {
        let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
        (
            Some(watch::spawn(
                root,
                index.clone(),
                configuration.clone(),
                bazel.clone(),
                ready_tx,
            )),
            ready_rx,
        )
    } else {
        (None, crossbeam_channel::never())
    };
    tracing::info!("ready");

    let mut docs = Documents::new(root.clone(), index.clone());
    let worker_count = std::thread::available_parallelism()
        .map_or(2, std::num::NonZero::get)
        .clamp(2, 8);
    let (completed_tx, completed_rx) = crossbeam_channel::bounded(worker_count * 8);
    let workers = worker::Pool::new(worker_count, &completed_tx);
    let diagnostics = worker::Latest::new(&completed_tx);
    let session = (|| -> Result<()> {
        let request_context = RequestContext {
            connection: &connection,
            workers: &workers,
            index: &index,
            configuration: &configuration,
            root: root.as_deref(),
            bazel: &bazel,
            watch: watch.as_ref(),
            link_support,
        };

        'session: loop {
            crossbeam_channel::select! {
            recv(connection.receiver) -> message => {
                let Ok(message) = message else { break };
                match message {
                    Message::Request(request) => {
                        if request_context.dispatch(&mut requests, &docs, request)? {
                            break 'session;
                        }
                    }
                    Message::Notification(note) => handle_notification(
                        &connection,
                        &diagnostics,
                        &mut requests,
                        &mut docs,
                        &bazel,
                        note,
                    )?,
                    Message::Response(response) => {
                        if let Some(Outgoing::IndexProgress) =
                            requests.outgoing.complete(response.id.clone())
                        {
                            if response.response_result.is_ok() {
                                if let Some(progress) = &mut index_progress {
                                    progress.created(&connection)?;
                                }
                            } else {
                                index_progress = None;
                            }
                        }
                    }
                }
            }
            recv(completed_rx) -> completed => {
                let Ok(completed) = completed else { break };
                handle_completed(&connection, &mut requests, &docs, completed)?;
            }
            recv(ready_rx) -> ready => {
                if let Ok(ready) = ready
                    && let Some(progress) = &mut index_progress
                {
                    progress.ready(&connection, ready)?;
                }
                ready_rx = crossbeam_channel::never();
            }
            }
        }
        Ok(())
    })();

    drop(completed_rx);
    drop(diagnostics);
    drop(workers);
    drop(connection);
    let transport = io_threads.join();
    session?;
    transport?;
    tracing::info!("shut down");
    Ok(())
}

fn handle_completed(
    connection: &Connection,
    requests: &mut ReqQueue<worker::Cancellation, Outgoing>,
    docs: &Documents,
    completed: Completed,
) -> Result<()> {
    match completed {
        Completed::Response(response) => {
            if requests.incoming.complete(&response.id).is_some() {
                connection.sender.send(Message::Response(response))?;
            }
        }
        Completed::Diagnostics {
            uri,
            document,
            diagnostics,
        } if docs.is_current(&uri, &document) => {
            publish_diagnostics(connection, uri, document.version(), diagnostics)?;
        }
        Completed::Diagnostics { .. } => {}
    }
    Ok(())
}

fn respond(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &Index,
    configuration: &bazelrc::ConfigurationSnapshot,
    root: Option<&Path>,
    link_support: bool,
    cancellation: &worker::Cancellation,
) -> Result<Response> {
    if let Some(response) = bazelrc::respond(request, docs, configuration) {
        return Ok(response);
    }
    let id = request.id.clone();
    let method: LspRequestMethod<'_> = request.method.as_str().into();
    Ok(if method == DocumentSymbolRequest::METHOD {
        Response::new_ok(id, document_symbols(request, docs)?)
    } else if method == DefinitionRequest::METHOD {
        Response::new_ok(id, definition(request, docs, index, root, link_support)?)
    } else if method == ReferencesRequest::METHOD {
        Response::new_ok(id, references(request, docs, index, root)?)
    } else if method == DocumentHighlightRequest::METHOD {
        Response::new_ok(id, document_highlight(request, docs, index, root)?)
    } else if method == HoverRequest::METHOD {
        Response::new_ok(id, hover(request, docs, index, root)?)
    } else if method == DocumentFormattingRequest::METHOD {
        Response::new_ok(id, formatting(request, docs, cancellation)?)
    } else if method == RenameRequest::METHOD {
        Response::new_ok(id, rename(request, docs, index, root)?)
    } else if method == PrepareRenameRequest::METHOD {
        Response::new_ok(id, prepare_rename(request, docs, index, root)?)
    } else if method == FoldingRangeRequest::METHOD {
        Response::new_ok(id, folding_ranges(request, docs)?)
    } else if method == SelectionRangeRequest::METHOD {
        Response::new_ok(id, selection_ranges(request, docs)?)
    } else if method == DocumentLinkRequest::METHOD {
        Response::new_ok(id, document_links(request, docs, index, root)?)
    } else if method == SemanticTokensRequest::METHOD {
        Response::new_ok(id, semantic_tokens(request, docs)?)
    } else if method == ImplementationRequest::METHOD {
        Response::new_ok(id, implementation(request, docs, root)?)
    } else if method == CodeLensRequest::METHOD {
        Response::new_ok(id, code_lenses(request, docs, root)?)
    } else if method == InlayHintRequest::METHOD {
        Response::new_ok(id, inlay_hints(request, docs, index, root)?)
    } else if method == WorkspaceSymbolRequest::METHOD {
        Response::new_ok(id, workspace_symbols(request, index)?)
    } else {
        tracing::debug!(method = request.method, "unhandled request");
        Response::new_err(
            id,
            lsp_server::ErrorCode::MethodNotFound as i32,
            format!("unhandled: {}", request.method),
        )
    })
}

/// Every occurrence of the target under the cursor, in that document alone.
fn document_highlight(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &Index,
    root: Option<&Path>,
) -> Result<Vec<lsp_types::DocumentHighlight>> {
    let params: lsp_types::DocumentHighlightParams =
        serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let (Some(document), Some(root)) = (docs.get(&uri), root) else {
        return Ok(Vec::new());
    };
    let highlights = handlers::document_highlight(document, root, index, position.position);
    tracing::debug!(?uri, count = highlights.len(), "documentHighlight");
    Ok(highlights)
}

/// What the label under the cursor names, as a card.
///
/// Nothing to say is `null` rather than an empty card: a client renders the
/// latter as a blank popup that the user has to dismiss.
fn hover(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &Index,
    root: Option<&Path>,
) -> Result<Option<lsp_types::Hover>> {
    let params: lsp_types::HoverParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let (Some(document), Some(root)) = (docs.get(&uri), root) else {
        return Ok(None);
    };
    let card = handlers::hover(document, root, index, position.position);
    tracing::debug!(?uri, answered = card.is_some(), "hover");
    Ok(card)
}

/// buildifier's opinion of the open buffer, as one whole-document edit.
///
/// The buffer is what gets formatted, never the file beside it: a document the
/// user has been editing and has not saved is a different file on disk, and
/// formatting that one would revert their typing.
///
/// A document the server does not hold is not an error — the client may format
/// a file it never opened — and neither is a buildifier that is absent or
/// unhappy. Both are no edits; only the second says so on stderr.
fn formatting(
    request: &lsp_server::Request,
    docs: &Documents,
    cancellation: &worker::Cancellation,
) -> Result<Vec<TextEdit>> {
    let params: lsp_types::DocumentFormattingParams =
        serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let Some(document) = docs.get(&uri) else {
        return Ok(Vec::new());
    };
    let edits = format::format_cancelled(document.text(), document.kind(), cancellation)?;
    tracing::debug!(?uri, count = edits.len(), "formatting");
    Ok(edits)
}

/// The targets a BUILD file declares.
fn document_symbols(
    request: &lsp_server::Request,
    docs: &Documents,
) -> Result<Vec<lsp_types::DocumentSymbol>> {
    let params: lsp_types::DocumentSymbolParams = serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let symbols = docs
        .get(&uri)
        .map_or_else(Vec::new, handlers::document_symbols);
    tracing::debug!(?uri, count = symbols.len(), "documentSymbol");
    Ok(symbols)
}

/// Where the string under the cursor is declared.
fn definition(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &Index,
    root: Option<&Path>,
    link_support: bool,
) -> Result<Option<DefinitionResponse>> {
    let params: lsp_types::DefinitionParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let links = match (docs.get(&uri), root) {
        (Some(document), Some(root)) => {
            handlers::definition(document, root, index, position.position)
        }
        _ => Vec::new(),
    };
    tracing::debug!(?uri, count = links.len(), "definition");
    Ok(definition_response(links, link_support))
}

/// Every label naming the target under the cursor.
fn references(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &Index,
    root: Option<&Path>,
) -> Result<Vec<Location>> {
    let params: lsp_types::ReferenceParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let locations = match (docs.get(&uri), root) {
        (Some(document), Some(root)) => handlers::references(
            document,
            root,
            index,
            position.position,
            params.context.include_declaration,
        ),
        _ => Vec::new(),
    };
    tracing::debug!(?uri, count = locations.len(), "references");
    Ok(locations)
}

/// Every target in the workspace matching a query.
fn workspace_symbols(
    request: &lsp_server::Request,
    index: &Index,
) -> Result<Vec<lsp_types::WorkspaceSymbol>> {
    let params: lsp_types::WorkspaceSymbolParams = serde_json::from_value(request.params.clone())?;
    let symbols = handlers::workspace_symbols(index, &params.query);
    tracing::debug!(
        query = params.query,
        count = symbols.len(),
        "workspaceSymbol"
    );
    Ok(symbols)
}

/// The package a shorthand label resolves against.
fn inlay_hints(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &Index,
    root: Option<&Path>,
) -> Result<Vec<lsp_types::InlayHint>> {
    let params: lsp_types::InlayHintParams = serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let hints = match (docs.get(&uri), root) {
        (Some(document), Some(root)) => handlers::inlay_hints(document, root, index, params.range),
        _ => Vec::new(),
    };
    tracing::debug!(?uri, count = hints.len(), "inlayHint");
    Ok(hints)
}

/// What a reader can collapse.
fn folding_ranges(
    request: &lsp_server::Request,
    docs: &Documents,
) -> Result<Vec<lsp_types::FoldingRange>> {
    let params: lsp_types::FoldingRangeParams = serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let ranges = docs
        .get(&uri)
        .map_or_else(Vec::new, handlers::folding_ranges);
    tracing::debug!(?uri, count = ranges.len(), "foldingRange");
    Ok(ranges)
}

/// The syntax around each requested position.
fn selection_ranges(
    request: &lsp_server::Request,
    docs: &Documents,
) -> Result<Vec<lsp_types::SelectionRange>> {
    let params: lsp_types::SelectionRangeParams = serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let ranges = docs.get(&uri).map_or_else(Vec::new, |document| {
        handlers::selection_ranges(document, &params.positions)
    });
    tracing::debug!(?uri, count = ranges.len(), "selectionRange");
    Ok(ranges)
}

/// The Bazel commands each target line affords.
fn code_lenses(
    request: &lsp_server::Request,
    docs: &Documents,
    root: Option<&Path>,
) -> Result<Vec<lsp_types::CodeLens>> {
    let params: lsp_types::CodeLensParams = serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let lenses = match (docs.get(&uri), root) {
        (Some(document), Some(root)) => handlers::code_lenses(document, root),
        _ => Vec::new(),
    };
    tracing::debug!(?uri, count = lenses.len(), "codeLens");
    Ok(lenses)
}

/// Queue a reindex or start a code-lens Bazel command.
fn execute_command(
    request: &lsp_server::Request,
    root: Option<&Path>,
    bazel: &Bazel,
    watch: Option<&watch::Watch>,
) -> Result<Option<serde_json::Value>> {
    let params: lsp_types::ExecuteCommandParams = serde_json::from_value(request.params.clone())?;
    if params.command == watch::REINDEX_COMMAND {
        let Some(watch) = watch else {
            anyhow::bail!("there is no workspace to reindex");
        };
        watch.reindex();
        return Ok(None);
    }
    if params.command != handlers::RUN_COMMAND {
        anyhow::bail!("unknown command: {}", params.command);
    }
    let arguments: Vec<String> = params
        .arguments
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect();
    let [verb, label] = arguments.as_slice() else {
        anyhow::bail!("expected a verb and a label, got {arguments:?}");
    };
    let Some(_root) = root else {
        anyhow::bail!("no workspace to run in");
    };
    bazel.run_target(verb, label)?;
    Ok(None)
}

/// The function behind the rule under the cursor.
fn implementation(
    request: &lsp_server::Request,
    docs: &Documents,
    root: Option<&Path>,
) -> Result<Vec<Location>> {
    let params: lsp_types::ImplementationParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let found = match (docs.get(&uri), root) {
        (Some(document), Some(_)) => handlers::implementation(document, position.position),
        _ => Vec::new(),
    };
    tracing::debug!(?uri, count = found.len(), "implementation");
    Ok(found)
}

/// The tokens a grammar cannot colour.
fn semantic_tokens(
    request: &lsp_server::Request,
    docs: &Documents,
) -> Result<lsp_types::SemanticTokens> {
    let params: lsp_types::SemanticTokensParams = serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let tokens = docs
        .get(&uri)
        .map(handlers::semantic_tokens)
        .unwrap_or_default();
    tracing::debug!(?uri, count = tokens.data.len(), "semanticTokens");
    Ok(tokens)
}

/// Every label in a document that resolves, as a link.
fn document_links(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &Index,
    root: Option<&Path>,
) -> Result<Vec<lsp_types::DocumentLink>> {
    let params: lsp_types::DocumentLinkParams = serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let links = match (docs.get(&uri), root) {
        (Some(document), Some(root)) => handlers::document_links(document, root, index),
        _ => Vec::new(),
    };
    tracing::debug!(?uri, count = links.len(), "documentLink");
    Ok(links)
}

/// The edits a rename produces, or nothing where there is no target under the
/// cursor.
///
/// An illegal new name comes back as an `Err` and reaches the client as a
/// request error it shows the user, which is the only outcome they can act on:
/// a workspace half-rewritten to a name Bazel cannot load is worse.
fn rename(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &Index,
    root: Option<&Path>,
) -> Result<Option<lsp_types::WorkspaceEdit>> {
    let params: lsp_types::RenameParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let (Some(document), Some(root)) = (docs.get(&uri), root) else {
        return Ok(None);
    };
    let edit = handlers::rename(
        document,
        root,
        index,
        docs,
        position.position,
        &params.new_name,
    )?;
    tracing::debug!(?uri, new_name = params.new_name, "rename");
    Ok(edit)
}

/// The range a rename would rewrite, so a client can offer the request only
/// where it will do something.
fn prepare_rename(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &Index,
    root: Option<&Path>,
) -> Result<Option<PrepareRenameResult>> {
    let params: lsp_types::PrepareRenameParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let (Some(document), Some(root)) = (docs.get(&uri), root) else {
        return Ok(None);
    };
    let range = handlers::prepare_rename(document, root, index, position.position);
    tracing::debug!(?uri, renameable = range.is_some(), "prepareRename");
    Ok(range.map(PrepareRenameResult::Range))
}

enum Applied {
    Changed(Uri),
    Closed { uri: Uri, version: i32 },
    None,
}

fn apply(note: &lsp_server::Notification, docs: &mut Documents) -> Result<Applied> {
    let method: LspNotificationMethod<'_> = note.method.as_str().into();
    if method == DidOpenTextDocumentNotification::METHOD {
        let params: DidOpenTextDocumentParams = serde_json::from_value(note.params.clone())?;
        let uri = params.text_document.uri;
        docs.set(
            uri.clone(),
            uri_to_path(&uri).into(),
            params.text_document.version,
            params.text_document.text,
        );
        return Ok(Applied::Changed(uri));
    }
    if method == DidChangeTextDocumentNotification::METHOD {
        let params: DidChangeTextDocumentParams = serde_json::from_value(note.params.clone())?;
        let uri = params.text_document.text_document_identifier.uri;
        docs.change(&uri, params.text_document.version, params.content_changes)?;
        return Ok(Applied::Changed(uri));
    }
    if method == DidCloseTextDocumentNotification::METHOD {
        let params: DidCloseTextDocumentParams = serde_json::from_value(note.params.clone())?;
        let uri = params.text_document.uri;
        let version = docs.get(&uri).map_or(0, Document::version);
        docs.forget(&uri);
        return Ok(Applied::Closed { uri, version });
    }
    tracing::trace!(method = note.method, "unhandled notification");
    Ok(Applied::None)
}

/// Whether definition responses may use `LocationLink`.
fn supports_definition_links(init: &InitializeParams) -> bool {
    init.capabilities
        .text_document
        .as_ref()
        .and_then(|caps| caps.definition.as_ref())
        .and_then(|definition| definition.link_support)
        .unwrap_or(false)
}

fn supports_work_done_progress(init: &InitializeParams) -> bool {
    init.capabilities
        .window
        .as_ref()
        .and_then(|window| window.work_done_progress)
        .unwrap_or(false)
}

fn send_progress(
    connection: &Connection,
    token: &ProgressToken,
    value: impl serde::Serialize,
) -> Result<()> {
    let params = ProgressParams {
        token: token.clone(),
        value: serde_json::to_value(value)?,
    };
    connection
        .sender
        .send(Message::Notification(lsp_server::Notification {
            method: ProgressNotification::METHOD.to_string(),
            params: serde_json::to_value(params)?,
        }))?;
    Ok(())
}

/// Narrow a link to a plain location, dropping the origin range with it.
fn definition_response(links: Vec<LocationLink>, link_support: bool) -> Option<DefinitionResponse> {
    if links.is_empty() {
        return None;
    }
    Some(if link_support {
        DefinitionResponse::DefinitionLinkList(links)
    } else {
        DefinitionResponse::Definition(Definition::LocationList(
            links
                .into_iter()
                .map(|link| Location {
                    uri: link.target_uri,
                    range: link.target_selection_range,
                })
                .collect(),
        ))
    })
}

fn handle_notification(
    connection: &Connection,
    diagnostics: &worker::Latest<Uri, Completed>,
    requests: &mut ReqQueue<worker::Cancellation, Outgoing>,
    docs: &mut Documents,
    bazel: &Bazel,
    note: lsp_server::Notification,
) -> Result<()> {
    let method: LspNotificationMethod<'_> = note.method.as_str().into();
    if method == CancelNotification::METHOD {
        let params: CancelParams = serde_json::from_value(note.params)?;
        let id = match params.id {
            lsp_types::Id::Int(id) => RequestId::from(id),
            lsp_types::Id::String(id) => RequestId::from(id),
        };
        if let Some(cancellation) = requests.incoming.complete(&id) {
            cancellation.cancel();
            let response = Response::new_err(
                id,
                lsp_server::ErrorCode::RequestCanceled as i32,
                "canceled by client".to_owned(),
            );
            connection.sender.send(Message::Response(response))?;
        }
        return Ok(());
    }
    if method == DidChangeConfigurationNotification::METHOD {
        let sent: Option<DidChangeConfigurationParams> = serde_json::from_value(note.params).ok();
        if let Some(config) = bazel_settings(sent.as_ref().map(|sent| &sent.settings)) {
            bazel.reconfigure(config);
        }
        return Ok(());
    }

    match apply(&note, docs) {
        Ok(Applied::Changed(uri)) => schedule_diagnostics(connection, diagnostics, docs, &uri)?,
        Ok(Applied::Closed { uri, version }) => {
            diagnostics.cancel(&uri);
            publish_diagnostics(connection, uri, version, Vec::new())?;
        }
        Ok(Applied::None) => {}
        Err(err) => tracing::error!(method = note.method, "{err:#}"),
    }
    Ok(())
}

fn schedule_diagnostics(
    connection: &Connection,
    diagnostics: &worker::Latest<Uri, Completed>,
    docs: &Documents,
    uri: &Uri,
) -> Result<()> {
    let Some(document) = docs.get(uri) else {
        return Ok(());
    };
    let version = document.version();
    let syntax = if document.is_bazelrc() {
        bazelrc::syntax_diagnostics(document)
    } else {
        handlers::syntax_diagnostics(document)
    };
    let clean = syntax.is_empty();
    diagnostics.cancel(uri);
    publish_diagnostics(connection, uri.clone(), version, syntax)?;
    if clean && !document.is_bazelrc() {
        let uri = uri.clone();
        let document = docs.shared(&uri).expect("the document scheduled above");
        diagnostics.execute(uri.clone(), move |cancellation| Completed::Diagnostics {
            uri,
            diagnostics: format::lint_cancelled(document.text(), document.kind(), cancellation),
            document,
        });
    }
    Ok(())
}

fn publish_diagnostics(
    connection: &Connection,
    uri: Uri,
    version: i32,
    diagnostics: Vec<lsp_types::Diagnostic>,
) -> Result<()> {
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics,
        version: Some(version),
    };
    connection
        .sender
        .send(Message::Notification(lsp_server::Notification {
            method: PublishDiagnosticsNotification::METHOD.to_string(),
            params: serde_json::to_value(params)?,
        }))?;
    Ok(())
}

fn request_result(request: &lsp_server::Request, result: Result<Response>) -> Response {
    match result {
        Ok(response) => response,
        Err(err) => request_error(request, &err),
    }
}

fn request_error(request: &lsp_server::Request, err: &anyhow::Error) -> Response {
    tracing::error!(method = request.method, "{err:#}");
    let code = if err.downcast_ref::<serde_json::Error>().is_some() {
        lsp_server::ErrorCode::InvalidParams
    } else {
        lsp_server::ErrorCode::RequestFailed
    };
    Response::new_err(request.id.clone(), code as i32, err.to_string())
}

/// The filesystem path of a `file://` URI.
///
/// `Uri::path()` yields a percent-encoded `EStr`, so decode rather than taking
/// the raw bytes: a workspace under a path with a space would otherwise index
/// nothing and say nothing.
fn uri_to_path(uri: &Uri) -> String {
    uri.path().decode().to_string_lossy().into_owned()
}

/// Resolve the workspace from modern fields, legacy fields, then cwd.
#[allow(deprecated)]
fn workspace_root(init: &InitializeParams) -> Option<PathBuf> {
    let from_folders = init
        .workspace_folders_initialize_params
        .workspace_folders
        .as_ref()
        .and_then(|folders| match folders {
            lsp_types::WorkspaceFolders::WorkspaceFolderList(list) => list.first(),
            lsp_types::WorkspaceFolders::Null => None,
        })
        .map(|folder| ("workspaceFolders", PathBuf::from(uri_to_path(&folder.uri))));

    let from_root_uri = init
        .root_uri
        .as_ref()
        .map(|uri| ("rootUri", PathBuf::from(uri_to_path(uri))));

    let from_root_path = init.root_path.as_ref().and_then(|root| match root {
        lsp_types::RootPath::String(path) => Some(("rootPath", PathBuf::from(path))),
        lsp_types::RootPath::Null => None,
    });

    let from_cwd = std::env::current_dir().ok().map(|dir| ("cwd", dir));

    let (source, candidate) = from_folders
        .or(from_root_uri)
        .or(from_root_path)
        .or(from_cwd)?;

    let root = crate::bazel::find_workspace(&candidate).map_or(candidate, |w| w.root);
    tracing::info!(?root, source, "workspace root");
    Some(root)
}
