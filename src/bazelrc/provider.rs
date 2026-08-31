//! LSP boundary for Bazelrc documents.

use crate::document::{Document, Documents};
use lsp_server::{Request, Response};
use lsp_types::{Diagnostic, DiagnosticSeverity, LspRequestMethod, Range, Uri};

/// Route a text-document request here when its buffer is Bazelrc.
///
/// Structural and semantic answers are added inside this provider. Returning
/// typed empty results keeps an rc buffer out of the Starlark handlers while a
/// capability has no rc implementation.
#[must_use]
pub fn respond(request: &Request, docs: &Documents) -> Option<Response> {
    let document = request_document(request, docs)?;
    if !document.is_bazelrc() {
        return None;
    }
    let method: LspRequestMethod<'_> = request.method.as_str().into();
    let value = match method {
        LspRequestMethod::TextDocumentDocumentSymbol
        | LspRequestMethod::TextDocumentReferences
        | LspRequestMethod::TextDocumentDocumentHighlight
        | LspRequestMethod::TextDocumentFormatting
        | LspRequestMethod::TextDocumentFoldingRange
        | LspRequestMethod::TextDocumentSelectionRange
        | LspRequestMethod::TextDocumentDocumentLink
        | LspRequestMethod::TextDocumentImplementation
        | LspRequestMethod::TextDocumentCodeLens
        | LspRequestMethod::TextDocumentInlayHint => serde_json::json!([]),
        LspRequestMethod::TextDocumentDefinition
        | LspRequestMethod::TextDocumentHover
        | LspRequestMethod::TextDocumentRename
        | LspRequestMethod::TextDocumentPrepareRename
        | LspRequestMethod::TextDocumentSemanticTokensFull => serde_json::Value::Null,
        _ => return None,
    };
    Some(Response::new_ok(request.id.clone(), value))
}

fn request_document<'a>(request: &Request, docs: &'a Documents) -> Option<&'a Document> {
    let uri: Uri = request
        .params
        .pointer("/textDocument/uri")?
        .as_str()?
        .parse()
        .ok()?;
    docs.get(&uri)
}

/// Structural errors in one Bazelrc buffer.
#[must_use]
pub fn syntax_diagnostics(document: &Document) -> Vec<Diagnostic> {
    let Some(parsed) = document.bazelrc() else {
        return Vec::new();
    };
    parsed
        .errors
        .iter()
        .map(|error| Diagnostic {
            range: Range {
                start: document
                    .line_index()
                    .position(document.text(), error.range.start),
                end: document
                    .line_index()
                    .position(document.text(), error.range.end),
            },
            severity: Some(DiagnosticSeverity::Error),
            source: Some("bazel-language-server".to_owned()),
            message: error.message.clone().into(),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{IndexHandle, Tier};
    use lsp_server::RequestId;
    use lsp_types::{DocumentFormattingRequest, Request as _};

    fn documents(text: &str) -> (Documents, Uri) {
        let root = std::path::PathBuf::from("/ws");
        let index = IndexHandle::new();
        index.store_disk(Tier::default());
        let mut docs = Documents::new(Some(root.clone()), index);
        let uri: Uri = "file:///ws/.bazelrc".parse().unwrap();
        docs.set(uri.clone(), root.join(".bazelrc"), 1, text.to_owned());
        (docs, uri)
    }

    #[test]
    fn formatting_never_reaches_buildifier() {
        let (docs, uri) = documents("build --config=dev\n");
        let request = Request {
            id: RequestId::from(1),
            method: DocumentFormattingRequest::METHOD.to_string(),
            params: serde_json::json!({"textDocument": {"uri": uri}}),
        };
        assert_eq!(
            respond(&request, &docs).unwrap().response_result.unwrap(),
            serde_json::json!([])
        );
    }

    #[test]
    fn malformed_directives_use_bazelrc_diagnostics() {
        let (docs, uri) = documents("import one two\n");
        let document = docs.get(&uri).unwrap();
        let diagnostics = syntax_diagnostics(document);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].message,
            "`import` expects 1 argument(s)".to_owned().into()
        );
    }
}
