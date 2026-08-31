//! LSP boundary for Bazelrc documents.

use std::path::Path;

use anyhow::Result;
use lsp_server::{Request, Response};
use lsp_types::{
    CompletionParams, CompletionRequest, DefinitionParams, DefinitionRequest, Diagnostic,
    DiagnosticSeverity, DocumentLinkParams, DocumentLinkRequest, FoldingRangeParams,
    FoldingRangeRequest, HoverParams, HoverRequest, LspRequestMethod, Range, Request as _,
    SelectionRangeParams, SelectionRangeRequest, SemanticTokensParams, SemanticTokensRequest, Uri,
};

use super::{
    ConfigurationSnapshot, FlagCatalog, ProblemSeverity, completion, hover, navigation, structural,
};
use crate::document::{Document, Documents};

/// Route a text-document request here when its buffer is Bazelrc.
#[must_use]
pub fn respond(
    request: &Request,
    docs: &Documents,
    configuration: &ConfigurationSnapshot,
    catalog: Option<&FlagCatalog>,
    root: Option<&Path>,
    link_support: bool,
) -> Option<Result<Response>> {
    let document = request_document(request, docs)?;
    if !document.is_bazelrc() {
        return None;
    }
    Some(
        answer(
            request,
            document,
            docs,
            configuration,
            catalog,
            root,
            link_support,
        )
        .map(|value| Response::new_ok(request.id.clone(), value)),
    )
}

fn answer(
    request: &Request,
    document: &Document,
    docs: &Documents,
    configuration: &ConfigurationSnapshot,
    catalog: Option<&FlagCatalog>,
    root: Option<&Path>,
    link_support: bool,
) -> Result<serde_json::Value> {
    let method: LspRequestMethod<'_> = request.method.as_str().into();
    if method == CompletionRequest::METHOD {
        let params: CompletionParams = serde_json::from_value(request.params.clone())?;
        return Ok(serde_json::to_value(completion::completions(
            document,
            docs,
            configuration,
            catalog,
            params.text_document_position_params.position,
        ))?);
    }
    if method == DefinitionRequest::METHOD {
        let params: DefinitionParams = serde_json::from_value(request.params.clone())?;
        let links = navigation::definitions(
            document,
            docs,
            configuration,
            root,
            params.text_document_position_params.position,
        );
        return Ok(serde_json::to_value(crate::definition_response(
            links,
            link_support,
        ))?);
    }
    if method == HoverRequest::METHOD {
        let params: HoverParams = serde_json::from_value(request.params.clone())?;
        let value = catalog.and_then(|catalog| {
            hover::hover(
                document,
                catalog,
                params.text_document_position_params.position,
            )
        });
        return Ok(serde_json::to_value(value)?);
    }
    if method == FoldingRangeRequest::METHOD {
        let _: FoldingRangeParams = serde_json::from_value(request.params.clone())?;
        return Ok(serde_json::to_value(structural::folding_ranges(document))?);
    }
    if method == SelectionRangeRequest::METHOD {
        let params: SelectionRangeParams = serde_json::from_value(request.params.clone())?;
        return Ok(serde_json::to_value(structural::selection_ranges(
            document,
            &params.positions,
        ))?);
    }
    if method == DocumentLinkRequest::METHOD {
        let _: DocumentLinkParams = serde_json::from_value(request.params.clone())?;
        return Ok(serde_json::to_value(navigation::document_links(
            document,
            configuration,
            root,
        ))?);
    }
    if method == SemanticTokensRequest::METHOD {
        let _: SemanticTokensParams = serde_json::from_value(request.params.clone())?;
        return Ok(serde_json::to_value(structural::semantic_tokens(document))?);
    }

    Ok(match method {
        LspRequestMethod::TextDocumentDocumentSymbol
        | LspRequestMethod::TextDocumentReferences
        | LspRequestMethod::TextDocumentDocumentHighlight
        | LspRequestMethod::TextDocumentFormatting
        | LspRequestMethod::TextDocumentImplementation
        | LspRequestMethod::TextDocumentCodeLens
        | LspRequestMethod::TextDocumentInlayHint => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
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

/// Structural and import-graph findings in one current Bazelrc buffer.
#[must_use]
pub fn diagnostics(document: &Document, configuration: &ConfigurationSnapshot) -> Vec<Diagnostic> {
    let Some(parsed) = document.bazelrc() else {
        return Vec::new();
    };
    let mut diagnostics: Vec<_> = parsed
        .errors
        .iter()
        .map(|error| Diagnostic {
            range: span(document, error.range),
            severity: Some(DiagnosticSeverity::Error),
            source: Some("bazel-language-server".to_owned()),
            message: error.message.clone().into(),
            ..Default::default()
        })
        .collect();
    let saved_is_current = configuration
        .files
        .get(document.path())
        .is_some_and(|file| file.text.as_ref() == document.text());
    if saved_is_current {
        diagnostics.extend(
            configuration
                .problems
                .iter()
                .filter(|problem| problem.file.as_ref() == document.path())
                .map(|problem| Diagnostic {
                    range: span(document, problem.range),
                    severity: Some(match problem.severity {
                        ProblemSeverity::Error => DiagnosticSeverity::Error,
                        ProblemSeverity::Warning => DiagnosticSeverity::Warning,
                    }),
                    source: Some("bazel-language-server".to_owned()),
                    message: problem.message.to_string().into(),
                    ..Default::default()
                }),
        );
    }
    diagnostics
}

fn span(document: &Document, range: super::syntax::Span) -> Range {
    Range {
        start: document.line_index().position(document.text(), range.start),
        end: document.line_index().position(document.text(), range.end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{IndexHandle, Tier};
    use lsp_server::RequestId;
    use lsp_types::DocumentFormattingRequest;

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
            respond(
                &request,
                &docs,
                &ConfigurationSnapshot::default(),
                None,
                Some(Path::new("/ws")),
                true,
            )
            .unwrap()
            .unwrap()
            .response_result
            .unwrap(),
            serde_json::json!([])
        );
    }

    #[test]
    fn malformed_directives_use_bazelrc_diagnostics() {
        let (docs, uri) = documents("import one two\n");
        let document = docs.get(&uri).unwrap();
        let diagnostics = diagnostics(document, &ConfigurationSnapshot::default());
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            &diagnostics[0].message,
            lsp_types::Message::String(message) if message.contains("expects 1 argument")
        ));
    }
}
