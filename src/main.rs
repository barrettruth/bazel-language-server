//! Language server for Bazel build files.
//!
//! The main loop owns the open documents and never blocks. Slow work belongs on
//! the Bazel thread, which publishes an index the handlers read as a snapshot.
//!
//! stdout is the LSP transport. Everything human-readable goes to stderr.

mod bazel;
mod document;
mod format;
mod handlers;
mod index;
mod label;
mod line_index;

use std::path::{Path, PathBuf};

use crate::bazel::{BazelClient, BazelConfig};
use crate::document::Document;
use crate::index::IndexHandle;
use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use lsp_server::{Connection, Message, Response};
use lsp_types::{
    CodeLensRequest, Definition, DefinitionRequest, DefinitionResponse,
    DidChangeTextDocumentNotification, DidChangeTextDocumentParams,
    DidCloseTextDocumentNotification, DidCloseTextDocumentParams, DidOpenTextDocumentNotification,
    DidOpenTextDocumentParams, DocumentFormattingRequest, DocumentHighlightRequest,
    DocumentLinkRequest, DocumentSymbolRequest, ExecuteCommandRequest, FoldingRangeRequest,
    HoverRequest, ImplementationRequest, InitializeParams, InlayHintRequest, Location,
    LocationLink, LspNotificationMethod, LspRequestMethod, Notification, PrepareRenameRequest,
    PrepareRenameResult, PublishDiagnosticsNotification, PublishDiagnosticsParams,
    ReferencesRequest, RenameOptions, RenameRequest, Request as _, SelectionRangeRequest,
    SemanticTokensRequest, ServerCapabilities, TextDocumentSync, TextDocumentSyncKind, TextEdit,
    Uri, WorkspaceSymbolRequest,
};

type FxHashMap<K, V> = std::collections::HashMap<K, V>;

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
    let index = crate::index::build_static(&root);
    println!(
        "indexed {} BUILD files, {} targets in {:.2}s",
        index.files.len(),
        index.len(),
        started.elapsed().as_secs_f64()
    );
    for (label, target) in index.targets.iter().take(10) {
        println!("  {:<40} {}", label, target.rule);
    }
    if !index.graph_loaded {
        println!("\nstatic tier only: targets from legacy macros are not counted");
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
        Ok(version) => println!("bazel      {version}"),
        Err(err) => println!("bazel      unavailable: {err:#}"),
    }
}

/// Open documents. Text is retained so ranges can be computed without re-reading.
struct Documents {
    texts: FxHashMap<Uri, Document>,
}

fn run_server() -> Result<()> {
    tracing::info!("bazel-language-server {}", env!("CARGO_PKG_VERSION"));
    let (connection, io_threads) = Connection::stdio();

    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSync::Kind(TextDocumentSyncKind::Full)),
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
            commands: vec![handlers::RUN_COMMAND.to_string()],
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
    };
    // `Connection::initialize` wraps its argument in `{"capabilities": …}`, so
    // passing a whole InitializeResult nests it twice; the client then sees no
    // `textDocumentSync`, never sends `didOpen`, and every document request
    // comes back empty. Drive the handshake directly to send `serverInfo` too.
    let (id, params) = connection.initialize_start()?;
    connection.initialize_finish(
        id,
        serde_json::json!({
            "capabilities": capabilities,
            "serverInfo": {
                "name": "bazel-language-server",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
    )?;
    let init: InitializeParams = serde_json::from_value(params)?;

    let root = workspace_root(&init);
    let link_support = supports_definition_links(&init);
    let index = IndexHandle::new();
    if let Some(root) = root.clone() {
        // Synchronous: ~1.4 s on a 74k-package repo, which is cheaper than the
        // machinery to report progress on it would be.
        index.store(crate::index::build_static(&root));
    }
    tracing::info!(targets = index.load().len(), "ready");

    let mut docs = Documents {
        texts: FxHashMap::default(),
    };

    for message in &connection.receiver {
        match message {
            // A message the server cannot make sense of fails that message.
            // Letting the error out of the loop would exit the process and take
            // every open buffer's language support with it, over one bad URI.
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    break;
                }
                let id = request.id.clone();
                let response = match respond(&request, &docs, &index, root.as_deref(), link_support)
                {
                    Ok(response) => response,
                    Err(err) => {
                        tracing::error!(method = request.method, "{err:#}");
                        Response::new_err(
                            id,
                            lsp_server::ErrorCode::InvalidParams as i32,
                            err.to_string(),
                        )
                    }
                };
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(note) => match apply(&note, &mut docs, root.as_deref()) {
                Ok(Some(uri)) => publish(&connection, &docs, &uri)?,
                Ok(None) => {}
                // A notification has no reply, so this is the only report.
                Err(err) => tracing::error!(method = note.method, "{err:#}"),
            },
            Message::Response(_) => {}
        }
    }

    io_threads.join()?;
    tracing::info!("shut down");
    Ok(())
}

fn respond(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &IndexHandle,
    root: Option<&Path>,
    link_support: bool,
) -> Result<Response> {
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
        Response::new_ok(id, formatting(request, docs)?)
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
    } else if method == ExecuteCommandRequest::METHOD {
        Response::new_ok(id, execute_command(request, root)?)
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
    index: &IndexHandle,
    root: Option<&Path>,
) -> Result<Vec<lsp_types::DocumentHighlight>> {
    let params: lsp_types::DocumentHighlightParams =
        serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let (Some(document), Some(root)) = (docs.texts.get(&uri), root) else {
        return Ok(Vec::new());
    };
    let highlights = handlers::document_highlight(document, root, &index.load(), position.position);
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
    index: &IndexHandle,
    root: Option<&Path>,
) -> Result<Option<lsp_types::Hover>> {
    let params: lsp_types::HoverParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let (Some(document), Some(root)) = (docs.texts.get(&uri), root) else {
        return Ok(None);
    };
    let card = handlers::hover(document, root, &index.load(), position.position);
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
fn formatting(request: &lsp_server::Request, docs: &Documents) -> Result<Vec<TextEdit>> {
    let params: lsp_types::DocumentFormattingParams =
        serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let Some(document) = docs.texts.get(&uri) else {
        return Ok(Vec::new());
    };
    let edits = format::format(document.text(), document.kind())?;
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
        .texts
        .get(&uri)
        .map_or_else(Vec::new, handlers::document_symbols);
    tracing::debug!(?uri, count = symbols.len(), "documentSymbol");
    Ok(symbols)
}

/// Where the string under the cursor is declared.
fn definition(
    request: &lsp_server::Request,
    docs: &Documents,
    index: &IndexHandle,
    root: Option<&Path>,
    link_support: bool,
) -> Result<Option<DefinitionResponse>> {
    let params: lsp_types::DefinitionParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let links = match (docs.texts.get(&uri), root) {
        (Some(document), Some(root)) => {
            handlers::definition(document, root, &index.load(), position.position)
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
    index: &IndexHandle,
    root: Option<&Path>,
) -> Result<Vec<Location>> {
    let params: lsp_types::ReferenceParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let locations = match (docs.texts.get(&uri), root) {
        (Some(document), Some(root)) => handlers::references(
            document,
            root,
            &index.load(),
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
    index: &IndexHandle,
) -> Result<Vec<lsp_types::WorkspaceSymbol>> {
    let params: lsp_types::WorkspaceSymbolParams = serde_json::from_value(request.params.clone())?;
    let symbols = handlers::workspace_symbols(&index.load(), &params.query);
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
    index: &IndexHandle,
    root: Option<&Path>,
) -> Result<Vec<lsp_types::InlayHint>> {
    let params: lsp_types::InlayHintParams = serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let hints = match (docs.texts.get(&uri), root) {
        (Some(document), Some(root)) => {
            handlers::inlay_hints(document, root, &index.load(), params.range)
        }
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
        .texts
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
    let ranges = docs.texts.get(&uri).map_or_else(Vec::new, |document| {
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
    let lenses = match (docs.texts.get(&uri), root) {
        (Some(document), Some(root)) => handlers::code_lenses(document, root),
        _ => Vec::new(),
    };
    tracing::debug!(?uri, count = lenses.len(), "codeLens");
    Ok(lenses)
}

/// Run the Bazel invocation a lens offered.
///
/// This is the one place the server starts Bazel, and it is not a request
/// handler answering about a buffer: the user clicked "test //x", so the
/// invocation *is* the answer. It is detached rather than awaited, because a
/// build takes minutes and the request loop serves every other document.
///
/// Nothing is reported back beyond that it started. LSP has no channel for a
/// subprocess's output, and inventing one here would be a worse terminal than
/// the one the user already has.
fn execute_command(
    request: &lsp_server::Request,
    root: Option<&Path>,
) -> Result<Option<serde_json::Value>> {
    let params: lsp_types::ExecuteCommandParams = serde_json::from_value(request.params.clone())?;
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
    let Some(root) = root else {
        anyhow::bail!("no workspace to run in");
    };

    tracing::info!(%verb, %label, "bazel");
    std::process::Command::new("bazel")
        .arg(verb)
        .arg(label)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("running `bazel {verb} {label}`"))?;
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
    let found = match (docs.texts.get(&uri), root) {
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
        .texts
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
    index: &IndexHandle,
    root: Option<&Path>,
) -> Result<Vec<lsp_types::DocumentLink>> {
    let params: lsp_types::DocumentLinkParams = serde_json::from_value(request.params.clone())?;
    let uri = params.text_document.uri;
    let links = match (docs.texts.get(&uri), root) {
        (Some(document), Some(root)) => handlers::document_links(document, root, &index.load()),
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
    index: &IndexHandle,
    root: Option<&Path>,
) -> Result<Option<lsp_types::WorkspaceEdit>> {
    let params: lsp_types::RenameParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let (Some(document), Some(root)) = (docs.texts.get(&uri), root) else {
        return Ok(None);
    };
    let edit = handlers::rename(
        document,
        root,
        &index.load(),
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
    index: &IndexHandle,
    root: Option<&Path>,
) -> Result<Option<PrepareRenameResult>> {
    let params: lsp_types::PrepareRenameParams = serde_json::from_value(request.params.clone())?;
    let position = params.text_document_position_params;
    let uri = position.text_document.uri;
    let (Some(document), Some(root)) = (docs.texts.get(&uri), root) else {
        return Ok(None);
    };
    let range = handlers::prepare_rename(document, root, &index.load(), position.position);
    tracing::debug!(?uri, renameable = range.is_some(), "prepareRename");
    Ok(range.map(PrepareRenameResult::Range))
}

/// Apply a notification to the open-document map.
///
/// Returns the document whose diagnostics are now stale, if any.
fn apply(
    note: &lsp_server::Notification,
    docs: &mut Documents,
    root: Option<&Path>,
) -> Result<Option<Uri>> {
    let method: LspNotificationMethod<'_> = note.method.as_str().into();
    if method == DidOpenTextDocumentNotification::METHOD {
        let params: DidOpenTextDocumentParams = serde_json::from_value(note.params.clone())?;
        let uri = params.text_document.uri;
        docs.texts.insert(
            uri.clone(),
            Document::new(uri_to_path(&uri).into(), params.text_document.text, root),
        );
        return Ok(Some(uri));
    }
    if method == DidChangeTextDocumentNotification::METHOD {
        let params: DidChangeTextDocumentParams = serde_json::from_value(note.params.clone())?;
        let uri = params.text_document.text_document_identifier.uri;
        if let Some(change) = params.content_changes.into_iter().next() {
            let text = match change {
                lsp_types::TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(whole) => whole.text,
                lsp_types::TextDocumentContentChangeEvent::TextDocumentContentChangePartial(partial) => partial.text,
            };
            docs.texts.insert(
                uri.clone(),
                Document::new(uri_to_path(&uri).into(), text, root),
            );
        }
        return Ok(Some(uri));
    }
    if method == DidCloseTextDocumentNotification::METHOD {
        let params: DidCloseTextDocumentParams = serde_json::from_value(note.params.clone())?;
        docs.texts.remove(&params.text_document.uri);
        return Ok(None);
    }
    tracing::trace!(method = note.method, "unhandled notification");
    Ok(None)
}

/// Whether the client understands `LocationLink`.
///
/// A client that does not gets `Location`s instead. Sending links to one that
/// never asked for them is a response it cannot parse, which reads as a broken
/// server rather than as a missing capability.
fn supports_definition_links(init: &InitializeParams) -> bool {
    init.capabilities
        .text_document
        .as_ref()
        .and_then(|caps| caps.definition.as_ref())
        .and_then(|definition| definition.link_support)
        .unwrap_or(false)
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

fn publish(connection: &Connection, docs: &Documents, uri: &Uri) -> Result<()> {
    let Some(document) = docs.texts.get(uri) else {
        return Ok(());
    };
    let mut diagnostics = handlers::syntax_diagnostics(document);
    if diagnostics.is_empty() {
        diagnostics.extend(format::lint(document.text(), document.kind()));
    }
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    };
    connection
        .sender
        .send(Message::Notification(lsp_server::Notification {
            method: PublishDiagnosticsNotification::METHOD.to_string(),
            params: serde_json::to_value(params)?,
        }))?;
    Ok(())
}

/// The filesystem path of a `file://` URI.
///
/// `Uri::path()` yields a percent-encoded `EStr`, so decode rather than taking
/// the raw bytes: a workspace under a path with a space would otherwise index
/// nothing and say nothing.
fn uri_to_path(uri: &Uri) -> String {
    uri.path().decode().to_string_lossy().into_owned()
}

/// Prefer a real Bazel root over whatever the editor called the workspace.
///
/// Clients disagree about which field carries the root. `workspaceFolders` is
/// the current one, `rootUri` and `rootPath` are deprecated but still what
/// several clients actually send, and reading only the first leaves the index
/// silently empty — the server starts, attaches, answers every request with
/// nothing, and logs no error. So try all of them, then the working directory,
/// and record which one won.
// rootUri and rootPath are deprecated in favour of workspaceFolders, and are
// read anyway: the deprecation describes the spec's intent, not what clients
// send.
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
