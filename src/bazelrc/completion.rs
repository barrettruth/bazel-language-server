//! Catalog-independent command, directive, and config completion.

use std::collections::{BTreeMap, BTreeSet};

use lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, Position};

use super::ConfigurationSnapshot;
use super::commands;
use super::syntax::{Statement, config_references};
use crate::document::{Document, Documents};

const DIRECTIVES: &[(&str, &str)] = &[
    ("import", "Required Bazelrc import"),
    ("try-import", "Optional Bazelrc import"),
    (
        "try-import-if-bazel-version",
        "Version-gated optional Bazelrc import",
    ),
];

#[must_use]
pub fn completions(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    position: Position,
) -> CompletionResponse {
    let offset = document.line_index().offset(document.text(), position);
    let Some(line) = document.bazelrc().and_then(|parsed| {
        parsed
            .lines
            .iter()
            .find(|line| line.range.start <= offset && offset <= line.range.end)
    }) else {
        return Vec::new().into();
    };
    if line.comment.is_some_and(|comment| offset >= comment.start) {
        return Vec::new().into();
    }
    let Some(key) = line.key() else {
        return command_items().into();
    };
    if offset <= key.range.end {
        return command_items().into();
    }
    if !matches!(line.statement, Some(Statement::Entry)) {
        return Vec::new().into();
    }
    let command = key
        .text
        .split_once(':')
        .map_or(key.text.as_str(), |(command, _)| command);
    let completing_config = config_references(line, document.text())
        .iter()
        .any(|reference| reference.range.start <= offset && offset <= reference.range.end)
        || line
            .options()
            .last()
            .is_some_and(|option| option.text == "--config" && offset >= option.range.end);
    if completing_config {
        config_items(documents, configuration, command).into()
    } else {
        Vec::new().into()
    }
}

fn command_items() -> Vec<CompletionItem> {
    let mut items: Vec<_> = commands::NAMES
        .iter()
        .map(|name| CompletionItem {
            label: (*name).to_owned(),
            kind: Some(CompletionItemKind::Keyword),
            detail: Some(if matches!(*name, "always" | "common" | "startup") {
                "Bazel 8.7 rc scope".to_owned()
            } else {
                "Bazel 8.7 command".to_owned()
            }),
            ..Default::default()
        })
        .collect();
    items.extend(DIRECTIVES.iter().map(|(name, detail)| CompletionItem {
        label: (*name).to_owned(),
        kind: Some(CompletionItemKind::Keyword),
        detail: Some((*detail).to_owned()),
        ..Default::default()
    }));
    items
}

fn config_items(
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    command: &str,
) -> Vec<CompletionItem> {
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for site in configuration
        .declarations
        .iter()
        .filter(|site| commands::applies(command, &site.command))
        .filter(|site| {
            documents
                .iter()
                .all(|(_, open)| open.path() != site.file.as_ref())
        })
    {
        found
            .entry(site.name.to_string())
            .or_default()
            .insert(site.command.to_string());
    }
    for (_, document) in documents
        .iter()
        .filter(|(_, document)| document.is_bazelrc())
    {
        let Some(parsed) = document.bazelrc() else {
            continue;
        };
        for key in parsed.lines.iter().filter_map(|line| line.key()) {
            let Some((defined_command, name)) = key.text.split_once(':') else {
                continue;
            };
            if commands::applies(command, defined_command) {
                found
                    .entry(name.to_owned())
                    .or_default()
                    .insert(defined_command.to_owned());
            }
        }
    }
    found
        .into_iter()
        .map(|(name, commands)| CompletionItem {
            label: name,
            kind: Some(CompletionItemKind::Reference),
            detail: Some(format!(
                "Bazelrc config ({})",
                commands.into_iter().collect::<Vec<_>>().join(", ")
            )),
            ..Default::default()
        })
        .collect()
}
