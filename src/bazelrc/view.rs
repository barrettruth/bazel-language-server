//! Request-local configuration sites with open buffers overlaid on disk.

use std::path::Path;
use std::sync::Arc;

use lsp_types::{Location, Range};

use super::commands;
use super::index::{ConfigSite, ConfigurationFile, ConfigurationSnapshot};
use super::syntax::{Directive, Parse, Statement, config_declaration, config_references};
use crate::document::{Buffers, Document, Documents};
use crate::line_index::LineIndex;
use crate::uri::file_uri;

pub struct ConfigurationView<'a> {
    documents: &'a Documents,
    snapshot: &'a ConfigurationSnapshot,
    ready: bool,
    declarations: Vec<ConfigSite>,
    references: Vec<ConfigSite>,
}

impl<'a> ConfigurationView<'a> {
    #[must_use]
    pub fn new(documents: &'a Documents, snapshot: &'a ConfigurationSnapshot) -> Self {
        let mut collector = Collector::new(documents, snapshot);
        if let Some(root_file) = snapshot.root_file.as_deref() {
            if !collector.visit(root_file, None) {
                collector.valid = false;
            }
        } else if snapshot.problems.is_empty()
            && let Some(document) = workspace_root_document(documents, snapshot)
        {
            if !collector.visit(document.path(), Some(document)) {
                collector.valid = false;
            }
        }
        collector.finish()
    }

    #[must_use]
    pub fn for_document(
        document: &Document,
        documents: &'a Documents,
        snapshot: &'a ConfigurationSnapshot,
    ) -> Self {
        if snapshot.includes(document.path())
            || workspace_root_document(documents, snapshot)
                .is_some_and(|root| root.path() == document.path())
        {
            return Self::new(documents, snapshot);
        }
        let mut declarations = Vec::new();
        let mut references = Vec::new();
        collect(
            document.bazelrc(),
            Arc::from(document.path()),
            &mut declarations,
            &mut references,
        );
        Self {
            documents,
            snapshot,
            ready: ready(snapshot),
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

    pub fn declarations_named<'view>(
        &'view self,
        name: &'view str,
    ) -> impl Iterator<Item = &'view ConfigSite> {
        self.declarations
            .iter()
            .filter(move |site| site.name.as_ref() == name)
    }

    pub fn applicable_declarations<'view>(
        &'view self,
        command: &'view str,
        name: &'view str,
    ) -> impl Iterator<Item = &'view ConfigSite> {
        self.declarations_named(name)
            .filter(move |site| commands::applies(command, &site.command))
    }

    pub fn references(&self) -> impl Iterator<Item = &ConfigSite> {
        self.references.iter()
    }

    pub fn occurrence_at(&self, path: &Path, offset: usize) -> Option<(bool, &ConfigSite)> {
        self.declarations
            .iter()
            .find(|site| site.file.as_ref() == path && contains(site.range, offset))
            .map(|site| (true, site))
            .or_else(|| {
                self.references
                    .iter()
                    .find(|site| site.file.as_ref() == path && contains(site.range, offset))
                    .map(|site| (false, site))
            })
    }

    pub fn range(&self, site: &ConfigSite) -> Option<Range> {
        self.span_range(&site.file, site.range)
    }

    pub fn line_range(&self, site: &ConfigSite) -> Option<Range> {
        self.span_range(&site.file, site.line)
    }

    pub fn location(&self, site: &ConfigSite) -> Option<Location> {
        Some(Location {
            uri: file_uri(&site.file)?,
            range: self.range(site)?,
        })
    }

    fn span_range(&self, path: &Path, span: super::syntax::Span) -> Option<Range> {
        if let Some(document) = self.documents.at(path) {
            return Some(Range {
                start: document.line_index().position(document.text(), span.start),
                end: document.line_index().position(document.text(), span.end),
            });
        }
        let file = self.snapshot.files.get(path)?;
        let lines = LineIndex::new(&file.text);
        Some(Range {
            start: lines.position(&file.text, span.start),
            end: lines.position(&file.text, span.end),
        })
    }
}

struct Collector<'a> {
    documents: &'a Documents,
    snapshot: &'a ConfigurationSnapshot,
    active: Vec<std::path::PathBuf>,
    valid: bool,
    declarations: Vec<ConfigSite>,
    references: Vec<ConfigSite>,
}

impl<'a> Collector<'a> {
    fn new(documents: &'a Documents, snapshot: &'a ConfigurationSnapshot) -> Self {
        Self {
            documents,
            snapshot,
            active: Vec::new(),
            valid: true,
            declarations: Vec::new(),
            references: Vec::new(),
        }
    }

    fn finish(mut self) -> ConfigurationView<'a> {
        if !self.valid {
            self.declarations.clear();
            self.references.clear();
        }
        ConfigurationView {
            documents: self.documents,
            snapshot: self.snapshot,
            ready: self.valid && ready(self.snapshot),
            declarations: self.declarations,
            references: self.references,
        }
    }

    fn visit(&mut self, identity: &Path, direct: Option<&Document>) -> bool {
        if self.active.iter().any(|path| path == identity) {
            return true;
        }
        let source = if let Some(document) = direct {
            let Some(document) = self.documents.shared_at(document.path()) else {
                return false;
            };
            Source::Open(document)
        } else if let Some(document) = self.open_document(identity) {
            Source::Open(document)
        } else if let Some(file) = self.snapshot.files.get(identity) {
            Source::Saved(Arc::clone(file))
        } else {
            return false;
        };
        let Some(parsed) = source.parsed() else {
            return false;
        };
        if !parsed.errors.is_empty() {
            return false;
        }
        let site_file = source.site_file();
        let root = self.snapshot.root.as_deref().map(Path::to_path_buf);
        self.active.push(identity.to_path_buf());
        for line in &parsed.lines {
            match &line.statement {
                Some(Statement::Entry) => collect_line(
                    line,
                    &site_file,
                    &mut self.declarations,
                    &mut self.references,
                ),
                Some(Statement::Directive(directive)) => {
                    if matches!(directive, Directive::ConditionalImport(condition) if !condition.matches("8.7.0"))
                    {
                        continue;
                    }
                    let Some(root) = root.as_deref() else {
                        continue;
                    };
                    let path = if matches!(directive, Directive::ConditionalImport(_)) {
                        &line.tokens[2]
                    } else {
                        &line.tokens[1]
                    };
                    let target = super::index::resolve_import(root, &path.text);
                    let loaded = self
                        .snapshot
                        .imports
                        .iter()
                        .find_map(|site| {
                            (site.file.as_ref() == identity && site.active && site.target == target)
                                .then_some(site.loaded.as_deref())
                                .flatten()
                        })
                        .map(Path::to_path_buf);
                    if let Some(loaded) = loaded {
                        if !self.visit(&loaded, None) {
                            self.active.pop();
                            self.valid = false;
                            return false;
                        }
                    }
                }
                Some(Statement::InvalidDirective) | None => {}
            }
        }
        self.active.pop();
        true
    }

    fn open_document(&self, identity: &Path) -> Option<Arc<Document>> {
        self.documents.iter().find_map(|(uri, document)| {
            self.snapshot
                .identity(document.path())
                .is_some_and(|known| known == identity)
                .then(|| self.documents.shared(uri))
                .flatten()
        })
    }
}

enum Source {
    Open(Arc<Document>),
    Saved(Arc<ConfigurationFile>),
}

impl Source {
    fn parsed(&self) -> Option<&Parse> {
        match self {
            Self::Open(document) => document.bazelrc(),
            Self::Saved(file) => Some(&file.parsed),
        }
    }

    fn site_file(&self) -> Arc<Path> {
        match self {
            Self::Open(document) => Arc::from(document.path()),
            Self::Saved(file) => Arc::clone(&file.path),
        }
    }
}

fn workspace_root_document<'a>(
    documents: &'a Documents,
    snapshot: &ConfigurationSnapshot,
) -> Option<&'a Document> {
    let root = snapshot.root.as_deref()?.join(".bazelrc");
    documents
        .iter()
        .find_map(|(_, document)| (document.path() == root).then_some(document))
}

fn ready(snapshot: &ConfigurationSnapshot) -> bool {
    snapshot.root.is_some() && (snapshot.root_file.is_some() || snapshot.files.is_empty())
}

const fn contains(span: super::syntax::Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

fn collect(
    parsed: Option<&Parse>,
    file: Arc<Path>,
    declarations: &mut Vec<ConfigSite>,
    references: &mut Vec<ConfigSite>,
) {
    let Some(parsed) = parsed else { return };
    for line in parsed
        .lines
        .iter()
        .filter(|line| matches!(line.statement, Some(Statement::Entry)))
    {
        collect_line(line, &file, declarations, references);
    }
}

fn collect_line(
    line: &super::syntax::Line,
    file: &Arc<Path>,
    declarations: &mut Vec<ConfigSite>,
    references: &mut Vec<ConfigSite>,
) {
    let Some(key) = line.key() else { return };
    let declaration = config_declaration(line)
        .filter(|declaration| commands::accepts_config(declaration.command));
    if let Some(declaration) = declaration {
        declarations.push(ConfigSite {
            name: declaration.name.into(),
            command: declaration.command.into(),
            file: Arc::clone(file),
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
        return;
    }
    for reference in config_references(line) {
        references.push(ConfigSite {
            name: reference.name.into(),
            command: command.into(),
            file: Arc::clone(file),
            range: reference.range,
            line: line.range,
            owner: declaration.map(|declaration| declaration.name.into()),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::bazelrc::syntax::Span;
    use crate::index::IndexHandle;
    use lsp_types::Uri;

    static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

    struct Workspace(std::path::PathBuf);

    impl Workspace {
        fn new() -> Self {
            let unique = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
            let root = std::path::PathBuf::from("/tmp")
                .join(format!("bls-bazelrc-view-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn write(&self, relative: &str, text: &str) {
            std::fs::write(self.0.join(relative), text).unwrap();
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.0));
        }
    }

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

    #[test]
    fn unrelated_open_files_stay_local() {
        let workspace = Workspace::new();
        workspace.write(".bazelrc", "build --config=missing\n");
        let snapshot = ConfigurationSnapshot::build(&workspace.0);
        let mut documents = Documents::new(Some(workspace.0.clone()), IndexHandle::new());
        let root_uri: Uri = "file:///tmp/root.bazelrc".parse().unwrap();
        documents.set(
            root_uri,
            workspace.0.join(".bazelrc"),
            1,
            "build --config=missing\n".to_owned(),
        );
        let unused_uri: Uri = "file:///tmp/unused.bazelrc".parse().unwrap();
        documents.set(
            unused_uri.clone(),
            workspace.0.join("unused.bazelrc"),
            1,
            "build:missing --jobs=1\n".to_owned(),
        );

        let workspace_view = ConfigurationView::new(&documents, &snapshot);
        assert!(
            workspace_view
                .declarations_named("missing")
                .next()
                .is_none()
        );
        let unused = documents.get(&unused_uri).unwrap();
        let local = ConfigurationView::for_document(unused, &documents, &snapshot);
        assert_eq!(local.declarations_named("missing").count(), 1);
    }

    #[test]
    fn open_graph_files_keep_import_order_and_multiplicity() {
        let workspace = Workspace::new();
        workspace.write(
            ".bazelrc",
            "build --config=before\nimport child\nbuild --config=after\nimport child\n",
        );
        workspace.write("child", "build --config=inside\n");
        let snapshot = ConfigurationSnapshot::build(&workspace.0);
        let mut documents = Documents::new(Some(workspace.0.clone()), IndexHandle::new());
        let root_uri: Uri = "file:///tmp/root.bazelrc".parse().unwrap();
        documents.set_classified(
            root_uri,
            workspace.0.join(".bazelrc"),
            1,
            "build --config=before\nimport child\nbuild --config=after\nimport child\n".to_owned(),
            true,
        );
        let child_uri: Uri = "file:///tmp/child.bazelrc".parse().unwrap();
        documents.set_classified(
            child_uri,
            workspace.0.join("child"),
            1,
            "build --config=inside\n".to_owned(),
            true,
        );

        let view = ConfigurationView::new(&documents, &snapshot);
        assert_eq!(
            view.references()
                .map(|site| site.name.as_ref())
                .collect::<Vec<_>>(),
            ["before", "inside", "after", "inside"]
        );
    }

    #[test]
    fn lexical_tmp_paths_overlay_their_canonical_graph_file() {
        let workspace = Workspace::new();
        workspace.write(".bazelrc", "build:saved --jobs=1\n");
        let snapshot = ConfigurationSnapshot::build(&workspace.0);
        let mut documents = Documents::new(Some(workspace.0.clone()), IndexHandle::new());
        let uri: Uri = "file:///tmp/alias.bazelrc".parse().unwrap();
        documents.set(
            uri,
            workspace.0.join(".bazelrc"),
            1,
            "build:open --jobs=1\n".to_owned(),
        );
        let view = ConfigurationView::new(&documents, &snapshot);
        assert!(view.declarations_named("saved").next().is_none());
        assert_eq!(view.declarations_named("open").count(), 1);
    }

    #[test]
    fn malformed_open_import_invalidates_the_request_view() {
        let workspace = Workspace::new();
        workspace.write(".bazelrc", "import child\nbuild:after --jobs=1\n");
        workspace.write("child", "build:inside --jobs=1\n");
        let snapshot = ConfigurationSnapshot::build(&workspace.0);
        let mut documents = Documents::new(Some(workspace.0.clone()), IndexHandle::new());
        let child_uri: Uri = "file:///tmp/child.bazelrc".parse().unwrap();
        documents.set_classified(
            child_uri,
            workspace.0.join("child"),
            1,
            "import one two\n".to_owned(),
            true,
        );

        let view = ConfigurationView::new(&documents, &snapshot);
        assert!(!view.ready());
        assert_eq!(view.declarations().count(), 0);
        assert_eq!(view.references().count(), 0);
    }
}
