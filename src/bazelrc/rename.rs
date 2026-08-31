//! Conservative workspace rename for named configurations.

use std::collections::{BTreeSet, HashMap};

use anyhow::{Result, bail};
use lsp_types::{Position, Range, TextEdit, Uri, WorkspaceEdit};

use super::{ConfigurationSnapshot, ConfigurationView};
use crate::document::{Document, Documents};
use crate::uri::file_uri;

#[must_use]
pub fn prepare(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    position: Position,
) -> Option<Range> {
    let view = ConfigurationView::new(documents, configuration);
    let offset = document.line_index().offset(document.text(), position);
    let (_, occurrence) = view.occurrence_at(document.path(), offset)?;
    view.declarations_named(&occurrence.name).next()?;
    view.range(occurrence)
}

/// Rewrite every workspace declaration and reference of one decoded name.
///
/// # Errors
///
/// When the replacement cannot be inserted literally as one token fragment,
/// or when it would merge two declared configurations.
pub fn rename(
    document: &Document,
    documents: &Documents,
    configuration: &ConfigurationSnapshot,
    position: Position,
    new_name: &str,
) -> Result<Option<WorkspaceEdit>> {
    validate(new_name)?;
    let view = ConfigurationView::new(documents, configuration);
    let offset = document.line_index().offset(document.text(), position);
    let Some((_, occurrence)) = view.occurrence_at(document.path(), offset) else {
        return Ok(None);
    };
    let old_name = occurrence.name.as_ref();
    if view.declarations_named(old_name).next().is_none() {
        return Ok(None);
    }
    if new_name != old_name && view.declarations_named(new_name).next().is_some() {
        bail!(
            "configuration `{new_name}` is already declared; renaming `{old_name}` would merge their option bodies"
        );
    }

    let mut seen = BTreeSet::new();
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for site in view.declarations_named(old_name).chain(
        view.references()
            .filter(|site| site.name.as_ref() == old_name),
    ) {
        if !seen.insert((site.file.to_path_buf(), site.range.start, site.range.end)) {
            continue;
        }
        let (Some(uri), Some(range)) = (file_uri(&site.file), view.range(site)) else {
            continue;
        };
        changes.entry(uri).or_default().push(TextEdit {
            range,
            new_text: new_name.to_owned(),
        });
    }
    for edits in changes.values_mut() {
        edits.sort_unstable_by(|left, right| {
            (left.range.start.line, left.range.start.character)
                .cmp(&(right.range.start.line, right.range.start.character))
        });
    }
    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }))
}

fn validate(name: &str) -> Result<()> {
    if !name.is_empty()
        && !name
            .chars()
            .any(|character| character.is_ascii_whitespace() || "#'\"\\\0".contains(character))
    {
        return Ok(());
    }
    bail!(
        "{name:?} is not a rename-safe Bazelrc configuration name: use a nonempty name without whitespace, #, quotes, backslash, or NUL"
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lsp_types::Uri;

    use super::*;
    use crate::index::IndexHandle;

    fn fixture(text: &str) -> (Documents, Uri, ConfigurationSnapshot) {
        let root = std::path::PathBuf::from("/ws");
        let mut documents = Documents::new(Some(root.clone()), IndexHandle::new());
        let uri: Uri = "file:///ws/.bazelrc".parse().unwrap();
        documents.set(uri.clone(), root.join(".bazelrc"), 1, text.to_owned());
        let configuration = ConfigurationSnapshot {
            root: Some(Arc::from(root.as_path())),
            ..ConfigurationSnapshot::default()
        };
        (documents, uri, configuration)
    }

    #[test]
    fn escaped_existing_spellings_receive_name_only_edits() {
        let text = "'build:de\\v' --jobs=1\nbuild --config=\"de\\v\"\n";
        let (documents, uri, configuration) = fixture(text);
        let document = documents.get(&uri).unwrap();
        let position = document
            .line_index()
            .position(text, text.rfind("de\\v").unwrap());
        let prepared = prepare(document, &documents, &configuration, position).unwrap();
        assert_eq!(
            &text[document.line_index().offset(text, prepared.start)
                ..document.line_index().offset(text, prepared.end)],
            "de\\v"
        );
        let edit = rename(document, &documents, &configuration, position, "prod")
            .unwrap()
            .unwrap();
        let edits = edit.changes.unwrap().remove(&uri).unwrap();
        assert_eq!(edits.len(), 2);
        assert!(edits.iter().all(|edit| edit.new_text == "prod"));
    }

    #[test]
    fn collisions_and_unsafe_fragments_are_refused() {
        let text = "build:dev --jobs=1\nbuild:prod --jobs=2\nbuild --config=dev\n";
        let (documents, uri, configuration) = fixture(text);
        let document = documents.get(&uri).unwrap();
        let position = document.line_index().position(text, 7);
        assert!(
            rename(document, &documents, &configuration, position, "prod")
                .unwrap_err()
                .to_string()
                .contains("merge")
        );
        assert!(rename(document, &documents, &configuration, position, "not safe").is_err());
    }

    #[test]
    fn an_external_only_reference_is_not_renameable() {
        let text = "build --config=personal\n";
        let (documents, uri, configuration) = fixture(text);
        let document = documents.get(&uri).unwrap();
        let position = document.line_index().position(text, 16);
        assert!(prepare(document, &documents, &configuration, position).is_none());
    }

    #[test]
    fn an_empty_declared_name_renames_at_zero_width_ranges() {
        let text = "build: --jobs=1\nbuild --config=\n";
        let (documents, uri, configuration) = fixture(text);
        let document = documents.get(&uri).unwrap();
        let position = document.line_index().position(text, 6);
        let edit = rename(document, &documents, &configuration, position, "default")
            .unwrap()
            .unwrap();
        let edits = edit.changes.unwrap().remove(&uri).unwrap();
        assert_eq!(edits.len(), 2);
        assert!(edits.iter().all(|edit| edit.range.start == edit.range.end));
    }
}
