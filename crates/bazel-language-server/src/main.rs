//! Language server for Bazel build files.
//!
//! The main loop owns the open documents and never blocks. Slow work belongs on
//! the Bazel thread, which publishes an index the handlers read as a snapshot.
//!
//! stdout is the LSP transport. Everything human-readable goes to stderr.

mod handlers;
mod line_index;

use std::path::{Path, PathBuf};

use anyhow::Result;
use bls_bazel::{BazelClient, BazelConfig};
use bls_index::IndexHandle;
use clap::{Parser, Subcommand};
use lsp_server::{Connection, Message, Response};
use lsp_types::{
    Definition, DefinitionRequest, DefinitionResponse, DidChangeTextDocumentNotification,
    DidChangeTextDocumentParams, DidCloseTextDocumentNotification, DidCloseTextDocumentParams,
    DidOpenTextDocumentNotification, DidOpenTextDocumentParams, DocumentHighlightRequest,
    DocumentSymbolRequest, InitializeParams, Location, LocationLink, LspNotificationMethod,
    LspRequestMethod, Notification, PrepareRenameRequest, PrepareRenameResult,
    PublishDiagnosticsNotification, PublishDiagnosticsParams, ReferencesRequest, RenameOptions,
    RenameRequest, Request as _, ServerCapabilities, TextDocumentSync, TextDocumentSyncKind, Uri,
    WorkspaceSymbolRequest,
};
use starlark_cst::{Dialect, FileKind, classify};

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
    let root = bls_bazel::find_workspace(path).map_or_else(
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
    let index = bls_index::build_static(&root);
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
    let workspace = bls_bazel::find_workspace(path);
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
    texts: FxHashMap<Uri, String>,
}

impl Documents {
    fn classify_uri(uri: &Uri) -> (Dialect, FileKind) {
        let path = PathBuf::from(uri_to_path(uri));
        classify(&path, None).unwrap_or((Dialect::Standard, FileKind::Bzl))
    }
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
        index.store(bls_index::build_static(&root));
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
            Message::Notification(note) => match apply(&note, &mut docs) {
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
        let params: lsp_types::DocumentSymbolParams =
            serde_json::from_value(request.params.clone())?;
        let uri = params.text_document.uri;
        let (dialect, kind) = Documents::classify_uri(&uri);
        let symbols = docs.texts.get(&uri).map_or_else(Vec::new, |text| {
            handlers::document_symbols(text, dialect, kind)
        });
        tracing::debug!(?uri, count = symbols.len(), "documentSymbol");
        Response::new_ok(id, symbols)
    } else if method == DefinitionRequest::METHOD {
        let params: lsp_types::DefinitionParams = serde_json::from_value(request.params.clone())?;
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let (dialect, _) = Documents::classify_uri(&uri);
        let links = match (docs.texts.get(&uri), root) {
            (Some(text), Some(root)) => handlers::definition(
                text,
                dialect,
                Path::new(&uri_to_path(&uri)),
                root,
                &index.load(),
                position.position,
            ),
            _ => Vec::new(),
        };
        tracing::debug!(?uri, count = links.len(), "definition");
        Response::new_ok(id, definition_response(links, link_support))
    } else if method == ReferencesRequest::METHOD {
        let params: lsp_types::ReferenceParams = serde_json::from_value(request.params.clone())?;
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let (dialect, _) = Documents::classify_uri(&uri);
        let locations = match (docs.texts.get(&uri), root) {
            (Some(text), Some(root)) => handlers::references(
                text,
                dialect,
                Path::new(&uri_to_path(&uri)),
                root,
                &index.load(),
                position.position,
                params.context.include_declaration,
            ),
            _ => Vec::new(),
        };
        tracing::debug!(?uri, count = locations.len(), "references");
        Response::new_ok(id, locations)
    } else if method == DocumentHighlightRequest::METHOD {
        Response::new_ok(id, document_highlight(request, docs, index, root)?)
    } else if method == RenameRequest::METHOD {
        Response::new_ok(id, rename(request, docs, index, root)?)
    } else if method == PrepareRenameRequest::METHOD {
        Response::new_ok(id, prepare_rename(request, docs, index, root)?)
    } else if method == WorkspaceSymbolRequest::METHOD {
        let params: lsp_types::WorkspaceSymbolParams =
            serde_json::from_value(request.params.clone())?;
        let symbols = handlers::workspace_symbols(&index.load(), &params.query);
        tracing::debug!(
            query = params.query,
            count = symbols.len(),
            "workspaceSymbol"
        );
        Response::new_ok(id, symbols)
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
    let (dialect, _) = Documents::classify_uri(&uri);
    let (Some(text), Some(root)) = (docs.texts.get(&uri), root) else {
        return Ok(Vec::new());
    };
    let highlights = handlers::document_highlight(
        text,
        dialect,
        Path::new(&uri_to_path(&uri)),
        root,
        &index.load(),
        position.position,
    );
    tracing::debug!(?uri, count = highlights.len(), "documentHighlight");
    Ok(highlights)
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
    let (dialect, _) = Documents::classify_uri(&uri);
    let (Some(text), Some(root)) = (docs.texts.get(&uri), root) else {
        return Ok(None);
    };
    let edit = handlers::rename(
        text,
        dialect,
        Path::new(&uri_to_path(&uri)),
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
    let (dialect, _) = Documents::classify_uri(&uri);
    let (Some(text), Some(root)) = (docs.texts.get(&uri), root) else {
        return Ok(None);
    };
    let range = handlers::prepare_rename(
        text,
        dialect,
        Path::new(&uri_to_path(&uri)),
        root,
        &index.load(),
        position.position,
    );
    tracing::debug!(?uri, renameable = range.is_some(), "prepareRename");
    Ok(range.map(PrepareRenameResult::Range))
}

/// Apply a notification to the open-document map.
///
/// Returns the document whose diagnostics are now stale, if any.
fn apply(note: &lsp_server::Notification, docs: &mut Documents) -> Result<Option<Uri>> {
    let method: LspNotificationMethod<'_> = note.method.as_str().into();
    if method == DidOpenTextDocumentNotification::METHOD {
        let params: DidOpenTextDocumentParams = serde_json::from_value(note.params.clone())?;
        let uri = params.text_document.uri;
        docs.texts.insert(uri.clone(), params.text_document.text);
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
            docs.texts.insert(uri.clone(), text);
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
    let Some(text) = docs.texts.get(uri) else {
        return Ok(());
    };
    let (dialect, _) = Documents::classify_uri(uri);
    let diagnostics = handlers::syntax_diagnostics(text, dialect);
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

    let root = bls_bazel::find_workspace(&candidate).map_or(candidate, |w| w.root);
    tracing::info!(?root, source, "workspace root");
    Some(root)
}
