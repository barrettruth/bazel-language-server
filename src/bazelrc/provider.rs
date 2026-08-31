//! LSP boundary for Bazelrc documents.

use std::path::Path;

use anyhow::Result;
use lsp_server::{Request, Response};
use lsp_types::{
    CompletionParams, CompletionRequest, DefinitionParams, DefinitionRequest,
    DocumentHighlightParams, DocumentHighlightRequest, DocumentLinkParams, DocumentLinkRequest,
    DocumentSymbolParams, DocumentSymbolRequest, FoldingRangeParams, FoldingRangeRequest,
    HoverParams, HoverRequest, LspRequestMethod, PrepareRenameParams, PrepareRenameRequest,
    PrepareRenameResult, ReferenceParams, ReferencesRequest, RenameParams, RenameRequest,
    Request as _, SelectionRangeParams, SelectionRangeRequest, SemanticTokensParams,
    SemanticTokensRequest, Uri,
};

use super::{
    ConfigurationSnapshot, FlagCatalog, completion, hover, navigation, occurrences, rename,
    structural,
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
    if !document.is_bazelrc() || !supports(request.method.as_str().into()) {
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
        let value = hover::hover(
            document,
            docs,
            configuration,
            catalog,
            root,
            params.text_document_position_params.position,
        );
        return Ok(serde_json::to_value(value)?);
    }
    if method == ReferencesRequest::METHOD {
        let params: ReferenceParams = serde_json::from_value(request.params.clone())?;
        return Ok(serde_json::to_value(occurrences::references(
            document,
            docs,
            configuration,
            params.text_document_position_params.position,
            params.context.include_declaration,
        ))?);
    }
    if method == DocumentHighlightRequest::METHOD {
        let params: DocumentHighlightParams = serde_json::from_value(request.params.clone())?;
        return Ok(serde_json::to_value(occurrences::highlights(
            document,
            docs,
            configuration,
            params.text_document_position_params.position,
        ))?);
    }
    if method == DocumentSymbolRequest::METHOD {
        let _: DocumentSymbolParams = serde_json::from_value(request.params.clone())?;
        return Ok(serde_json::to_value(occurrences::document_symbols(
            document,
            docs,
            configuration,
        ))?);
    }
    if method == PrepareRenameRequest::METHOD {
        let params: PrepareRenameParams = serde_json::from_value(request.params.clone())?;
        let value = rename::prepare(
            document,
            docs,
            configuration,
            params.text_document_position_params.position,
        )
        .map(PrepareRenameResult::Range);
        return Ok(serde_json::to_value(value)?);
    }
    if method == RenameRequest::METHOD {
        let params: RenameParams = serde_json::from_value(request.params.clone())?;
        let value = rename::rename(
            document,
            docs,
            configuration,
            params.text_document_position_params.position,
            &params.new_name,
        )?;
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
        LspRequestMethod::TextDocumentFormatting
        | LspRequestMethod::TextDocumentImplementation
        | LspRequestMethod::TextDocumentCodeLens
        | LspRequestMethod::TextDocumentInlayHint => serde_json::json!([]),
        _ => unreachable!("unsupported methods are rejected before dispatch"),
    })
}

fn supports(method: LspRequestMethod<'_>) -> bool {
    method == CompletionRequest::METHOD
        || method == DefinitionRequest::METHOD
        || method == HoverRequest::METHOD
        || method == ReferencesRequest::METHOD
        || method == DocumentHighlightRequest::METHOD
        || method == DocumentSymbolRequest::METHOD
        || method == PrepareRenameRequest::METHOD
        || method == RenameRequest::METHOD
        || method == FoldingRangeRequest::METHOD
        || method == SelectionRangeRequest::METHOD
        || method == DocumentLinkRequest::METHOD
        || method == SemanticTokensRequest::METHOD
        || matches!(
            method,
            LspRequestMethod::TextDocumentFormatting
                | LspRequestMethod::TextDocumentImplementation
                | LspRequestMethod::TextDocumentCodeLens
                | LspRequestMethod::TextDocumentInlayHint
        )
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
        let diagnostics =
            super::super::diagnostics(document, &docs, &ConfigurationSnapshot::default(), None);
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            &diagnostics[0].message,
            lsp_types::Message::String(message) if message.contains("expects 1 argument")
        ));
    }

    #[test]
    fn unknown_methods_are_left_for_the_main_dispatcher() {
        let (docs, uri) = documents("build --jobs=1\n");
        let request = Request {
            id: RequestId::from(1),
            method: "textDocument/notARealMethod".to_owned(),
            params: serde_json::json!({"textDocument": {"uri": uri}}),
        };
        assert!(
            respond(
                &request,
                &docs,
                &ConfigurationSnapshot::default(),
                None,
                Some(Path::new("/ws")),
                true,
            )
            .is_none()
        );
    }
}
