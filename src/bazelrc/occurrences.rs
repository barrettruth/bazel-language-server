//! LSP projections of named-configuration occurrences.

use std::collections::BTreeSet;

use lsp_types::{
    BaseSymbolInformation, DocumentHighlight, DocumentHighlightKind, DocumentSymbol, Location,
    Position, SymbolKind, WorkspaceSymbol,
};

use super::{ConfigurationSnapshot, ConfigurationView};
use crate::document::{Document, Documents};

#[must_use]
pub fn references(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    position: Position,
    include_declarations: bool,
) -> Vec<Location> {
    let view = ConfigurationView::new(documents, configuration);
    let offset = document.line_index().offset(document.text(), position);
    let Some((_, occurrence)) = view.occurrence_at(document.path(), offset) else {
        return Vec::new();
    };
    let mut locations: Vec<_> = view
        .references()
        .filter(|site| site.name == occurrence.name)
        .chain(
            include_declarations
                .then(|| {
                    view.declarations()
                        .filter(|site| site.name == occurrence.name)
                })
                .into_iter()
                .flatten(),
        )
        .filter_map(|site| view.location(site))
        .collect();
    sort_locations(&mut locations);
    locations.dedup();
    locations
}

#[must_use]
pub fn highlights(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    position: Position,
) -> Vec<DocumentHighlight> {
    let view = ConfigurationView::new(documents, configuration);
    let offset = document.line_index().offset(document.text(), position);
    let Some((_, occurrence)) = view.occurrence_at(document.path(), offset) else {
        return Vec::new();
    };
    let mut highlights: Vec<_> = view
        .declarations()
        .filter(|site| site.file.as_ref() == document.path() && site.name == occurrence.name)
        .filter_map(|site| {
            Some(DocumentHighlight {
                range: view.range(site)?,
                kind: Some(DocumentHighlightKind::Write),
            })
        })
        .chain(
            view.references()
                .filter(|site| {
                    site.file.as_ref() == document.path() && site.name == occurrence.name
                })
                .filter_map(|site| {
                    Some(DocumentHighlight {
                        range: view.range(site)?,
                        kind: Some(DocumentHighlightKind::Read),
                    })
                }),
        )
        .collect();
    highlights.sort_unstable_by(|left, right| compare_ranges(&left.range, &right.range));
    highlights.dedup();
    highlights
}

#[must_use]
pub fn document_symbols(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
) -> Vec<DocumentSymbol> {
    let view = ConfigurationView::new(documents, configuration);
    let mut symbols: Vec<_> = view
        .declarations()
        .filter(|site| site.file.as_ref() == document.path())
        .filter_map(|site| {
            let selection = view.range(site)?;
            Some(DocumentSymbol {
                name: site.name.to_string(),
                detail: Some(format!("{} configuration", site.command)),
                kind: SymbolKind::Constant,
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                range: selection,
                selection_range: selection,
                children: None,
            })
        })
        .collect();
    symbols.sort_unstable_by(|left, right| {
        compare_ranges(&left.selection_range, &right.selection_range)
    });
    symbols.dedup_by(|left, right| left.selection_range == right.selection_range);
    symbols
}

#[must_use]
pub fn workspace_symbols(
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    query: &str,
) -> Vec<WorkspaceSymbol> {
    let view = ConfigurationView::new(documents, configuration);
    let mut seen = BTreeSet::new();
    let mut symbols: Vec<_> = view
        .declarations()
        .filter(|site| contains_case_insensitive(&site.name, query))
        .filter(|site| {
            seen.insert((
                site.file.to_path_buf(),
                site.range.start,
                site.range.end,
                site.command.to_string(),
                site.name.to_string(),
            ))
        })
        .filter_map(|site| {
            Some(WorkspaceSymbol {
                location: view.location(site)?.into(),
                data: None,
                base_symbol_information: BaseSymbolInformation {
                    name: format!("--config={}", site.name),
                    kind: SymbolKind::Constant,
                    tags: None,
                    container_name: Some(format!("{} Bazelrc", site.command)),
                },
            })
        })
        .collect();
    symbols.sort_unstable_by(|left, right| {
        left.base_symbol_information
            .name
            .cmp(&right.base_symbol_information.name)
            .then_with(|| {
                left.base_symbol_information
                    .container_name
                    .cmp(&right.base_symbol_information.container_name)
            })
    });
    symbols.dedup();
    symbols
}

fn sort_locations(locations: &mut [Location]) {
    locations.sort_unstable_by(|left, right| {
        left.uri
            .as_str()
            .cmp(right.uri.as_str())
            .then_with(|| compare_ranges(&left.range, &right.range))
    });
}

fn compare_ranges(left: &lsp_types::Range, right: &lsp_types::Range) -> std::cmp::Ordering {
    (
        left.start.line,
        left.start.character,
        left.end.line,
        left.end.character,
    )
        .cmp(&(
            right.start.line,
            right.start.character,
            right.end.line,
            right.end.character,
        ))
}

fn contains_case_insensitive(text: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if text.is_ascii() && query.is_ascii() {
        return text
            .as_bytes()
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query.as_bytes()));
    }
    text.to_lowercase().contains(&query.to_lowercase())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lsp_types::Uri;

    use super::*;
    use crate::index::IndexHandle;

    fn fixture() -> (Documents, Uri, ConfigurationSnapshot) {
        let root = std::path::PathBuf::from("/ws");
        let mut documents = Documents::new(Some(root.clone()), IndexHandle::new());
        let uri: Uri = "file:///ws/.bazelrc".parse().unwrap();
        documents.set(
            uri.clone(),
            root.join(".bazelrc"),
            1,
            "build:dev --config=nested\ntest:dev --test_output=errors\n\
             build:nested --jobs=1\nbuild --config=dev\n"
                .to_owned(),
        );
        let configuration = ConfigurationSnapshot {
            root: Some(Arc::from(root.as_path())),
            ..ConfigurationSnapshot::default()
        };
        (documents, uri, configuration)
    }

    #[test]
    fn references_use_one_decoded_identity_and_respect_declarations() {
        let (documents, uri, configuration) = fixture();
        let document = documents.get(&uri).unwrap();
        let position = document
            .line_index()
            .position(document.text(), document.text().rfind("dev").unwrap());
        assert_eq!(
            references(document, &documents, &configuration, position, false).len(),
            1
        );
        assert_eq!(
            references(document, &documents, &configuration, position, true).len(),
            3
        );
    }

    #[test]
    fn highlights_and_symbols_keep_name_only_selection_ranges() {
        let (documents, uri, configuration) = fixture();
        let document = documents.get(&uri).unwrap();
        let position = document.line_index().position(document.text(), 7);
        let highlights = highlights(document, &documents, &configuration, position);
        assert_eq!(highlights.len(), 3);
        assert_eq!(highlights[0].kind, Some(DocumentHighlightKind::Write));
        let symbols = document_symbols(document, &documents, &configuration);
        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].name, "dev");
        assert_eq!(symbols[0].selection_range.start.character, 6);
        assert_eq!(
            workspace_symbols(&documents, &configuration, "DEV").len(),
            2
        );
    }
}
