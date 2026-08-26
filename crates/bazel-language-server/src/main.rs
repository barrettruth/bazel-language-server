//! Language server for Bazel build files.
//!
//! The main loop owns the open documents and never blocks. Slow work belongs on
//! the Bazel thread, which publishes an index the handlers read as a snapshot.
//!
//! stdout is the LSP transport. Everything human-readable goes to stderr.

mod handlers;
mod line_index;

use std::path::PathBuf;

use anyhow::Result;
use bls_bazel::{BazelClient, BazelConfig};
use bls_index::IndexHandle;
use clap::{Parser, Subcommand};
use lsp_server::{Connection, Message, Response};
use lsp_types::{
    DidChangeTextDocumentNotification, DidChangeTextDocumentParams,
    DidCloseTextDocumentNotification, DidCloseTextDocumentParams, DidOpenTextDocumentNotification,
    DidOpenTextDocumentParams, DocumentSymbolRequest, InitializeParams, LspNotificationMethod,
    LspRequestMethod, Notification, PublishDiagnosticsNotification, PublishDiagnosticsParams,
    Request as _, ServerCapabilities, TextDocumentSync, TextDocumentSyncKind, Uri,
    WorkspaceSymbolRequest,
};
use starlark_cst::{Dialect, classify};

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
    fn dialect(uri: &Uri) -> Dialect {
        let path = PathBuf::from(uri_to_path(uri));
        classify(&path, None).map_or(Dialect::Standard, |(dialect, _)| dialect)
    }
}

fn run_server() -> Result<()> {
    tracing::info!("bazel-language-server {}", env!("CARGO_PKG_VERSION"));
    let (connection, io_threads) = Connection::stdio();

    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSync::Kind(TextDocumentSyncKind::Full)),
        document_symbol_provider: Some(lsp_types::DocumentSymbolProvider::Bool(true)),
        workspace_symbol_provider: Some(lsp_types::WorkspaceSymbolProvider::Bool(true)),
        ..Default::default()
    };
    let params = connection.initialize(serde_json::json!({
        "capabilities": capabilities,
        "serverInfo": { "name": "bazel-language-server", "version": env!("CARGO_PKG_VERSION") },
    }))?;
    let init: InitializeParams = serde_json::from_value(params)?;

    let root = workspace_root(&init);
    let index = IndexHandle::new();
    if let Some(root) = root.clone() {
        // Synchronous for now: measured at ~1.4 s for a 74k-package repo, and
        // moving it to the Bazel thread is step one of G3.
        index.store(bls_index::build_static(&root));
    }
    tracing::info!(targets = index.load().len(), "ready");

    let mut docs = Documents {
        texts: FxHashMap::default(),
    };

    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    break;
                }
                let method: LspRequestMethod<'_> = request.method.as_str().into();
                let response = if method == DocumentSymbolRequest::METHOD {
                    let params: lsp_types::DocumentSymbolParams =
                        serde_json::from_value(request.params.clone())?;
                    let uri = params.text_document.uri;
                    let symbols = docs.texts.get(&uri).map_or_else(Vec::new, |text| {
                        handlers::document_symbols(text, Documents::dialect(&uri))
                    });
                    tracing::debug!(?uri, count = symbols.len(), "documentSymbol");
                    Response::new_ok(request.id, symbols)
                } else if method == WorkspaceSymbolRequest::METHOD {
                    let params: lsp_types::WorkspaceSymbolParams =
                        serde_json::from_value(request.params.clone())?;
                    let symbols = handlers::workspace_symbols(&index.load(), &params.query);
                    tracing::debug!(
                        query = params.query,
                        count = symbols.len(),
                        "workspaceSymbol"
                    );
                    Response::new_ok(request.id, symbols)
                } else {
                    tracing::debug!(method = request.method, "unhandled request");
                    Response::new_err(
                        request.id,
                        lsp_server::ErrorCode::MethodNotFound as i32,
                        format!("unhandled: {}", request.method),
                    )
                };
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(note) => {
                let method: LspNotificationMethod<'_> = note.method.as_str().into();
                if method == DidOpenTextDocumentNotification::METHOD {
                    let params: DidOpenTextDocumentParams =
                        serde_json::from_value(note.params.clone())?;
                    let uri = params.text_document.uri;
                    docs.texts.insert(uri.clone(), params.text_document.text);
                    publish(&connection, &docs, &uri)?;
                } else if method == DidChangeTextDocumentNotification::METHOD {
                    let params: DidChangeTextDocumentParams =
                        serde_json::from_value(note.params.clone())?;
                    let uri = params.text_document.text_document_identifier.uri;
                    if let Some(change) = params.content_changes.into_iter().next() {
                        let text = match change {
                            lsp_types::TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(whole) => whole.text,
                            lsp_types::TextDocumentContentChangeEvent::TextDocumentContentChangePartial(partial) => partial.text,
                        };
                        docs.texts.insert(uri.clone(), text);
                    }
                    publish(&connection, &docs, &uri)?;
                } else if method == DidCloseTextDocumentNotification::METHOD {
                    let params: DidCloseTextDocumentParams =
                        serde_json::from_value(note.params.clone())?;
                    docs.texts.remove(&params.text_document.uri);
                } else {
                    tracing::trace!(method = note.method, "unhandled notification");
                }
            }
            Message::Response(_) => {}
        }
    }

    io_threads.join()?;
    tracing::info!("shut down");
    Ok(())
}

fn publish(connection: &Connection, docs: &Documents, uri: &Uri) -> Result<()> {
    let Some(text) = docs.texts.get(uri) else {
        return Ok(());
    };
    let diagnostics = handlers::syntax_diagnostics(text, Documents::dialect(uri));
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
fn workspace_root(init: &InitializeParams) -> Option<PathBuf> {
    let candidate = init
        .workspace_folders_initialize_params
        .workspace_folders
        .as_ref()
        .and_then(|folders| match folders {
            lsp_types::WorkspaceFolders::WorkspaceFolderList(list) => list.first(),
            lsp_types::WorkspaceFolders::Null => None,
        })
        .map(|folder| PathBuf::from(uri_to_path(&folder.uri)))?;
    Some(bls_bazel::find_workspace(&candidate).map_or(candidate, |w| w.root))
}
