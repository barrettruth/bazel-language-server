//! Request-local configuration sites with open buffers overlaid on disk.

use std::path::Path;
use std::sync::Arc;

use super::commands;
use super::index::{ConfigSite, ConfigurationSnapshot};
use super::syntax::{Statement, config_declaration, config_references};
use crate::document::{Document, Documents};

pub struct ConfigurationView {
    ready: bool,
    declarations: Vec<ConfigSite>,
    references: Vec<ConfigSite>,
}

impl ConfigurationView {
    #[must_use]
    pub fn new(documents: &Documents, snapshot: &ConfigurationSnapshot) -> Self {
        let is_open = |path: &Path| {
            documents
                .iter()
                .any(|(_, document)| document.path() == path)
        };
        let mut declarations: Vec<_> = snapshot
            .declarations
            .iter()
            .filter(|site| !is_open(&site.file))
            .cloned()
            .collect();
        let mut references: Vec<_> = snapshot
            .references
            .iter()
            .filter(|site| !is_open(&site.file))
            .cloned()
            .collect();

        for (_, document) in documents
            .iter()
            .filter(|(_, document)| document.is_bazelrc())
        {
            collect(document, &mut declarations, &mut references);
        }

        Self {
            ready: snapshot.root.is_some(),
            declarations,
            references,
        }
    }

    #[must_use]
    pub const fn ready(&self) -> bool {
        self.ready
    }

    pub fn declarations(&self) -> impl Iterator<Item = &ConfigSite> {
        self.declarations.iter()
    }

    pub fn declarations_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a ConfigSite> {
        self.declarations
            .iter()
            .filter(move |site| site.name.as_ref() == name)
    }

    pub fn applicable_declarations<'a>(
        &'a self,
        command: &'a str,
        name: &'a str,
    ) -> impl Iterator<Item = &'a ConfigSite> {
        self.declarations_named(name)
            .filter(move |site| commands::applies(command, &site.command))
    }

    pub fn references(&self) -> impl Iterator<Item = &ConfigSite> {
        self.references.iter()
    }
}

fn collect(
    document: &Document,
    declarations: &mut Vec<ConfigSite>,
    references: &mut Vec<ConfigSite>,
) {
    let Some(parsed) = document.bazelrc() else {
        return;
    };
    let file: Arc<Path> = Arc::from(document.path());
    for line in parsed
        .lines
        .iter()
        .filter(|line| matches!(line.statement, Some(Statement::Entry)))
    {
        let Some(key) = line.key() else { continue };
        let declaration = config_declaration(line)
            .filter(|declaration| commands::accepts_config(declaration.command));
        if let Some(declaration) = declaration {
            declarations.push(ConfigSite {
                name: declaration.name.into(),
                command: declaration.command.into(),
                file: Arc::clone(&file),
                range: declaration.range,
                line: line.range,
                owner: None,
            });
        }

        let command = key
            .text
            .split_once(':')
            .map_or(key.text.as_str(), |(command, _)| command);
        if !commands::accepts_config(command) {
            continue;
        }
        for reference in config_references(line) {
            references.push(ConfigSite {
                name: reference.name.into(),
                command: command.into(),
                file: Arc::clone(&file),
                range: reference.range,
                line: line.range,
                owner: declaration.map(|declaration| declaration.name.into()),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bazelrc::syntax::Span;
    use crate::index::IndexHandle;
    use lsp_types::Uri;

    #[test]
    fn open_sites_replace_disk_sites_and_keep_owners() {
        let root = std::path::PathBuf::from("/ws");
        let path: Arc<Path> = Arc::from(root.join(".bazelrc"));
        let snapshot = ConfigurationSnapshot {
            root: Some(Arc::from(root.as_path())),
            declarations: vec![ConfigSite {
                name: "saved".into(),
                command: "build".into(),
                file: Arc::clone(&path),
                range: Span::new(6, 11),
                line: Span::new(0, 20),
                owner: None,
            }],
            ..ConfigurationSnapshot::default()
        };
        let mut documents = Documents::new(Some(root.clone()), IndexHandle::new());
        let uri: Uri = "file:///ws/.bazelrc".parse().unwrap();
        documents.set(
            uri,
            root.join(".bazelrc"),
            1,
            "build:open --config=nested\n".to_owned(),
        );

        let view = ConfigurationView::new(&documents, &snapshot);
        assert!(view.declarations_named("saved").next().is_none());
        assert_eq!(view.declarations_named("open").count(), 1);
        let reference = view.references().next().unwrap();
        assert_eq!(reference.name.as_ref(), "nested");
        assert_eq!(reference.owner.as_deref(), Some("open"));
    }
}
