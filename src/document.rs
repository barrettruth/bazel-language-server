//! Immutable, versioned snapshots of open buffers.
//!
//! Text, syntax and line offsets are derived together whenever a document
//! changes, then shared by request workers through `Arc`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::{Position, Uri};
use rustc_hash::FxHashMap;
use starlark_cst::{Dialect, FileKind, Parse, classify, parse};

use crate::bazelrc;
use crate::index::{IndexHandle, Tier, collect_file};
use crate::line_index::LineIndex;

/// The buffers the client has open, addressed by the path the index knows.
///
/// The index records the tree as it stands on disk. A request that resolves
/// into a file the user is editing asks the buffer through this, so an answer
/// points at text the user can actually see.
pub trait Buffers {
    /// The open buffer for `path`, if the client has one.
    fn at(&self, path: &Path) -> Option<&Document>;
}

/// Every buffer the client has open, and the index tier derived from them.
///
/// The tier is a function of this map and of nothing else, so it is rebuilt
/// here, wherever the map changes. Publishing it from the caller instead would
/// make every future path that touches a document a chance to leave the index
/// disagreeing with the editor.
#[derive(Clone)]
pub struct Documents {
    texts: FxHashMap<Uri, Arc<Document>>,
    root: Option<PathBuf>,
    index: IndexHandle,
}

impl Buffers for Documents {
    fn at(&self, path: &Path) -> Option<&Document> {
        self.texts
            .values()
            .find(|document| document.path() == path)
            .map(Arc::as_ref)
    }
}

impl Documents {
    #[must_use]
    pub fn new(root: Option<PathBuf>, index: IndexHandle) -> Self {
        Self {
            texts: FxHashMap::default(),
            root,
            index,
        }
    }

    #[must_use]
    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.texts.get(uri).map(Arc::as_ref)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Uri, &Document)> {
        self.texts
            .iter()
            .map(|(uri, document)| (uri, document.as_ref()))
    }

    #[must_use]
    pub fn shared(&self, uri: &Uri) -> Option<Arc<Document>> {
        self.texts.get(uri).cloned()
    }

    #[must_use]
    pub fn is_current(&self, uri: &Uri, document: &Arc<Document>) -> bool {
        self.texts
            .get(uri)
            .is_some_and(|current| Arc::ptr_eq(current, document))
    }

    /// Hold `text` as the current contents of `uri`, opened or edited.
    pub fn set(&mut self, uri: Uri, path: PathBuf, version: i32, text: String) {
        let document = Document::versioned(path, version, text, self.root.as_deref());
        self.texts.insert(uri, Arc::new(document));
        self.republish();
    }

    pub fn change(
        &mut self,
        uri: &Uri,
        version: i32,
        changes: Vec<lsp_types::TextDocumentContentChangeEvent>,
    ) -> anyhow::Result<()> {
        let current = self
            .texts
            .get(uri)
            .ok_or_else(|| anyhow::anyhow!("change for a document that is not open"))?;
        anyhow::ensure!(
            version > current.version(),
            "document version {version} does not follow {}",
            current.version()
        );
        let text = apply_changes(current.text(), changes)?;
        let document = Document::versioned(
            current.path().to_path_buf(),
            version,
            text,
            self.root.as_deref(),
        );
        self.texts.insert(uri.clone(), Arc::new(document));
        self.republish();
        Ok(())
    }

    /// Forget `uri`. The disk answers for that file again from here.
    pub fn forget(&mut self, uri: &Uri) {
        self.texts.remove(uri);
        self.republish();
    }

    /// Republish facts derived from all open BUILD files.
    fn republish(&self) {
        let mut tier = Tier::default();
        if let Some(root) = self.root.as_deref() {
            for document in self.texts.values() {
                document.contribute(root, &mut tier);
            }
        }
        self.index.store_buffer(tier);
    }
}

/// One open buffer: its text, where it lives, and what Bazel reads it as.
pub struct Document {
    text: String,
    path: PathBuf,
    version: i32,
    syntax: Syntax,
    lines: LineIndex,
}

enum Syntax {
    Starlark { kind: FileKind, parsed: Parse },
    Bazelrc(bazelrc::syntax::Parse),
}

impl Document {
    /// Hold `text` as the contents of `path`, classified against the workspace.
    #[cfg(test)]
    #[must_use]
    pub fn new(path: PathBuf, text: String, root: Option<&Path>) -> Self {
        Self::versioned(path, 0, text, root)
    }

    #[must_use]
    pub fn versioned(path: PathBuf, version: i32, text: String, root: Option<&Path>) -> Self {
        let syntax = if is_bazelrc(&path) {
            Syntax::Bazelrc(bazelrc::syntax::parse(&text))
        } else {
            let (dialect, kind) =
                classify(&path, root).unwrap_or((Dialect::Standard, FileKind::Bzl));
            Syntax::Starlark {
                kind,
                parsed: parse(&text, dialect),
            }
        };
        let lines = LineIndex::new(&text);
        Self {
            text,
            path,
            version,
            syntax,
            lines,
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn kind(&self) -> FileKind {
        match &self.syntax {
            Syntax::Starlark { kind, .. } => *kind,
            Syntax::Bazelrc(_) => unreachable!("Bazelrc is routed to its own provider"),
        }
    }

    #[must_use]
    pub fn version(&self) -> i32 {
        self.version
    }

    /// Line starts, for a request converting more than one offset.
    #[must_use]
    pub fn line_index(&self) -> &LineIndex {
        &self.lines
    }

    /// The syntax tree, in the dialect the path implies.
    #[must_use]
    pub fn parse(&self) -> &Parse {
        match &self.syntax {
            Syntax::Starlark { parsed, .. } => parsed,
            Syntax::Bazelrc(_) => unreachable!("Bazelrc is routed to its own provider"),
        }
    }

    #[must_use]
    pub const fn is_bazelrc(&self) -> bool {
        matches!(self.syntax, Syntax::Bazelrc(_))
    }

    #[must_use]
    pub fn bazelrc(&self) -> Option<&bazelrc::syntax::Parse> {
        match &self.syntax {
            Syntax::Starlark { .. } => None,
            Syntax::Bazelrc(parsed) => Some(parsed),
        }
    }

    /// Add declarations to `tier`. Only a clean parse supersedes disk facts;
    /// recovered syntax cannot distinguish deletion from an unfinished edit.
    fn contribute(&self, root: &Path, tier: &mut Tier) {
        let Syntax::Starlark { kind, parsed } = &self.syntax else {
            return;
        };
        if !matches!(kind, FileKind::Build) {
            return;
        }
        let path: Arc<Path> = Arc::from(self.path.as_path());
        if !collect_file(
            root,
            &path,
            parsed,
            &self.text,
            &mut tier.targets,
            &mut tier.references,
        ) {
            return;
        }
        if parsed.errors().is_empty() {
            tier.speaks_for.insert(path);
        }
    }

    /// The position of a byte offset in this document.
    #[must_use]
    #[cfg(test)]
    pub fn position(&self, offset: usize) -> Position {
        self.line_index().position(&self.text, offset)
    }

    /// The byte offset of a position in this document.
    pub fn offset(&self, position: Position) -> usize {
        self.lines.offset(&self.text, position)
    }
}

fn is_bazelrc(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == ".bazelrc" || name.ends_with(".bazelrc"))
}

fn apply_changes(
    original: &str,
    changes: Vec<lsp_types::TextDocumentContentChangeEvent>,
) -> anyhow::Result<String> {
    let mut text = original.to_owned();
    for change in changes {
        match change {
            lsp_types::TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(
                whole,
            ) => text = whole.text,
            lsp_types::TextDocumentContentChangeEvent::TextDocumentContentChangePartial(
                partial,
            ) => {
                let lines = LineIndex::new(&text);
                let start = lines.offset(&text, partial.range.start);
                let end = lines.offset(&text, partial.range.end);
                anyhow::ensure!(start <= end, "change range ends before it starts");
                anyhow::ensure!(
                    lines.position(&text, start) == partial.range.start
                        && lines.position(&text, end) == partial.range.end,
                    "change range is outside the document"
                );
                text.replace_range(start..end, &partial.text);
            }
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn buffer(name: &str, text: &str) -> (PathBuf, Document) {
        let path = PathBuf::from("/ws").join(name);
        let document = Document::new(path.clone(), text.to_string(), Some(Path::new("/ws")));
        (path, document)
    }

    fn contributed(name: &str, text: &str) -> (PathBuf, Tier) {
        let (path, document) = buffer(name, text);
        let mut tier = Tier::default();
        document.contribute(Path::new("/ws"), &mut tier);
        (path, tier)
    }

    #[test]
    fn a_buffer_that_parses_claims_the_file_it_read() {
        let (path, tier) = contributed("lib/BUILD.bazel", "filegroup(name = \"a\", srcs = [])\n");
        assert!(tier.targets.contains_key("//lib:a"));
        assert!(tier.speaks_for.contains(path.as_path()));
    }

    /// Mid-edit the parser recovers, so what it found is a floor rather than
    /// the whole file. Claiming the file on that basis would delete every
    /// target the recovery dropped, which is most of them, between keystrokes.
    #[test]
    fn a_buffer_caught_mid_edit_adds_and_claims_nothing() {
        let (path, tier) = contributed(
            "lib/BUILD.bazel",
            "filegroup(name = \"a\", srcs = [])\n\nfilegroup(name = \"b\",\n",
        );
        assert!(
            tier.targets.contains_key("//lib:a"),
            "what did parse is still offered"
        );
        assert!(
            !tier.speaks_for.contains(path.as_path()),
            "a recovered parse cannot tell a deletion from half a keystroke"
        );
    }

    /// Only BUILD files declare targets, so nothing else can claim a file
    /// either — `MODULE.bazel` is full of top-level calls carrying a `name`.
    #[test]
    fn only_a_build_file_contributes() {
        for name in ["MODULE.bazel", "lib/defs.bzl", "README.md"] {
            let (path, tier) = contributed(name, "filegroup(name = \"a\", srcs = [])\n");
            assert!(tier.targets.is_empty(), "{name} declared something");
            assert!(!tier.speaks_for.contains(path.as_path()), "{name} claimed");
        }
    }

    #[test]
    fn bazelrc_uses_its_own_syntax_and_contributes_no_targets() {
        let (path, document) = buffer("config/build.bazelrc", "build --config=dev\n");
        assert!(document.is_bazelrc());
        assert!(document.bazelrc().is_some());
        let mut tier = Tier::default();
        document.contribute(Path::new("/ws"), &mut tier);
        assert!(tier.targets.is_empty());
        assert!(!tier.speaks_for.contains(path.as_path()));
    }

    #[test]
    fn incremental_changes_apply_in_order_and_in_utf16() {
        let changes = serde_json::from_value(json!([
            {
                "range": {
                    "start": {"line": 0, "character": 1},
                    "end": {"line": 0, "character": 3}
                },
                "text": "x"
            },
            {
                "range": {
                    "start": {"line": 0, "character": 2},
                    "end": {"line": 0, "character": 3}
                },
                "text": "y"
            }
        ]))
        .unwrap();
        assert_eq!(apply_changes("a😀b\n", changes).unwrap(), "axy\n");
    }

    #[test]
    fn an_invalid_incremental_range_is_rejected() {
        let changes = serde_json::from_value(json!([{
            "range": {
                "start": {"line": 9, "character": 0},
                "end": {"line": 9, "character": 0}
            },
            "text": "x"
        }]))
        .unwrap();
        assert!(apply_changes("a\n", changes).is_err());
    }
}
