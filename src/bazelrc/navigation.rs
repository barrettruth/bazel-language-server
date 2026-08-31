//! Import and named-configuration navigation over request snapshots.

use std::path::Path;

use lsp_types::{DocumentLink, LocationLink, Position, Range};

use super::ConfigurationSnapshot;
use super::commands;
use super::index::resolve_import;
use super::syntax::{Directive, Span, Statement, Token, config_declaration, config_references};
use crate::document::{Document, Documents};
use crate::line_index::LineIndex;
use crate::uri::file_uri;

#[must_use]
pub fn document_links(
    document: &Document,
    configuration: &ConfigurationSnapshot,
    root: Option<&Path>,
) -> Vec<DocumentLink> {
    let Some(root) = root else {
        return Vec::new();
    };
    import_tokens(document)
        .filter_map(|token| {
            let target = imported(configuration, document, root, token)?;
            let uri = file_uri(target)?;
            Some(DocumentLink {
                range: range(document, token.range),
                target: Some(uri),
                tooltip: Some(target.display().to_string()),
                data: None,
            })
        })
        .collect()
}

#[must_use]
pub fn definitions(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    root: Option<&Path>,
    position: Position,
) -> Vec<LocationLink> {
    let offset = document.line_index().offset(document.text(), position);
    if let Some(root) = root
        && let Some(token) = import_tokens(document).find(|token| contains(token.range, offset))
        && let Some(target) = imported(configuration, document, root, token)
        && let Some(uri) = file_uri(target)
    {
        let target_range = Range::new(Position::new(0, 0), Position::new(0, 0));
        return vec![LocationLink {
            origin_selection_range: Some(range(document, token.range)),
            target_uri: uri,
            target_range,
            target_selection_range: target_range,
        }];
    }

    let Some((command, name, origin)) = config_reference_at(document, offset) else {
        return Vec::new();
    };
    let origin = range(document, origin);
    let mut links = open_declarations(documents, &command, &name, origin);
    for site in configuration
        .declarations(&name)
        .filter(|site| commands::applies(&command, &site.command))
        .filter(|site| {
            documents
                .iter()
                .all(|(_, open)| open.path() != site.file.as_ref())
        })
    {
        let Some(file) = configuration.files.get(site.file.as_ref()) else {
            continue;
        };
        let Some(uri) = file_uri(&site.file) else {
            continue;
        };
        let line_index = LineIndex::new(&file.text);
        let target = Range {
            start: line_index.position(&file.text, site.range.start),
            end: line_index.position(&file.text, site.range.end),
        };
        links.push(LocationLink {
            origin_selection_range: Some(origin),
            target_uri: uri,
            target_range: target,
            target_selection_range: target,
        });
    }
    links.sort_by(|left, right| {
        left.target_uri
            .as_str()
            .cmp(right.target_uri.as_str())
            .then_with(|| {
                left.target_range
                    .start
                    .line
                    .cmp(&right.target_range.start.line)
            })
            .then_with(|| {
                left.target_range
                    .start
                    .character
                    .cmp(&right.target_range.start.character)
            })
    });
    links.dedup_by(|left, right| {
        left.target_uri == right.target_uri && left.target_range == right.target_range
    });
    links
}

fn open_declarations(
    documents: &Documents,
    command: &str,
    name: &str,
    origin: Range,
) -> Vec<LocationLink> {
    let mut links = Vec::new();
    for (uri, document) in documents
        .iter()
        .filter(|(_, document)| document.is_bazelrc())
    {
        let Some(parsed) = document.bazelrc() else {
            continue;
        };
        for line in &parsed.lines {
            let Some((key, defined_command, defined_name)) = config_declaration(line) else {
                continue;
            };
            if !commands::accepts_config(defined_command)
                || defined_name != name
                || !commands::applies(command, defined_command)
            {
                continue;
            }
            let target = range(document, key.range);
            links.push(LocationLink {
                origin_selection_range: Some(origin),
                target_uri: uri.clone(),
                target_range: target,
                target_selection_range: target,
            });
        }
    }
    links
}

fn config_reference_at(document: &Document, offset: usize) -> Option<(String, String, Span)> {
    let parsed = document.bazelrc()?;
    for line in &parsed.lines {
        let Some(key) = line.key() else {
            continue;
        };
        let command = key
            .text
            .split_once(':')
            .map_or(key.text.as_str(), |(command, _)| command);
        if !commands::accepts_config(command) {
            continue;
        }
        for reference in config_references(line, document.text()) {
            if contains(reference.range, offset) {
                return Some((command.to_owned(), reference.name, reference.range));
            }
        }
    }
    None
}

fn import_tokens(document: &Document) -> impl Iterator<Item = &Token> {
    document
        .bazelrc()
        .into_iter()
        .flat_map(|parsed| &parsed.lines)
        .filter_map(|line| match &line.statement {
            Some(Statement::Directive(Directive::ConditionalImport(_))) => line.tokens.get(2),
            Some(Statement::Directive(_)) => line.tokens.get(1),
            Some(Statement::Entry | Statement::InvalidDirective) | None => None,
        })
}

fn imported<'a>(
    configuration: &'a ConfigurationSnapshot,
    document: &Document,
    root: &Path,
    token: &Token,
) -> Option<&'a Path> {
    let target = resolve_import(root, &token.text);
    if let Some(site) = configuration.imports.iter().find(|site| {
        site.file.as_ref() == document.path() && site.range == token.range && site.target == target
    }) {
        return site.active.then_some(site.loaded.as_deref()).flatten();
    }
    configuration
        .imports
        .iter()
        .find(|site| site.active && site.target == target)?
        .loaded
        .as_deref()
}

fn contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

fn range(document: &Document, span: Span) -> Range {
    Range {
        start: document.line_index().position(document.text(), span.start),
        end: document.line_index().position(document.text(), span.end),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::bazelrc::ImportSite;

    #[test]
    fn an_inactive_exact_import_does_not_borrow_an_active_target() {
        let root = Path::new("/ws");
        let text = "try-import-if-bazel-version <8.7.0 imported.bazelrc\n";
        let document = Document::versioned(root.join(".bazelrc"), 1, text.to_owned(), Some(root));
        let token = import_tokens(&document).next().unwrap();
        let target = resolve_import(root, &token.text);
        let loaded: Arc<Path> = Arc::from(root.join("imported.bazelrc"));
        let configuration = ConfigurationSnapshot {
            imports: vec![
                ImportSite {
                    file: Arc::from(document.path()),
                    range: token.range,
                    target: target.clone(),
                    loaded: None,
                    active: false,
                },
                ImportSite {
                    file: Arc::from(root.join("other.bazelrc")),
                    range: Span::new(0, 1),
                    target,
                    loaded: Some(loaded),
                    active: true,
                },
            ],
            ..ConfigurationSnapshot::default()
        };

        assert!(imported(&configuration, &document, root, token).is_none());
    }
}
