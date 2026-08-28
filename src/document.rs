//! An open buffer and the facts every request asks of it.
//!
//! A document is classified once, where the client hands over its text, so a
//! buffer is the same kind of file to every request: `documentSymbol` and
//! `definition` cannot disagree about whether it is a BUILD file.
//!
//! It is also the seam the sync model lives behind. A handler asks for text,
//! ranges and a tree, never for how the buffer got that way, so moving from
//! whole-document to incremental `didChange` is a change to this type.
//!
//! The parse and the line index are computed per call rather than stored.
//! Either one cached beside the text is state to keep in step with it, and the
//! only cost this project has measured worth optimising is Bazel. Measure
//! before caching, and key what you cache on the text itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::{Position, Uri};
use rustc_hash::FxHashMap;
use starlark_cst::{Dialect, FileKind, Parse, classify, parse};

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
pub struct Documents {
    texts: FxHashMap<Uri, Document>,
    root: Option<PathBuf>,
    index: IndexHandle,
}

impl Buffers for Documents {
    /// Scanned rather than indexed by path: a client holds tens of buffers
    /// open, and a second map keyed on path is state to keep in step with this
    /// one for a lookup that never shows up in a profile.
    fn at(&self, path: &Path) -> Option<&Document> {
        self.texts.values().find(|document| document.path() == path)
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
        self.texts.get(uri)
    }

    /// Hold `text` as the current contents of `uri`, opened or edited.
    pub fn set(&mut self, uri: Uri, path: PathBuf, text: String) {
        let document = Document::new(path, text, self.root.as_deref());
        self.texts.insert(uri, document);
        self.republish();
    }

    /// Forget `uri`. The disk answers for that file again from here.
    pub fn forget(&mut self, uri: &Uri) {
        self.texts.remove(uri);
        self.republish();
    }

    /// Rebuild the buffer tier from every open BUILD file.
    ///
    /// Whole rather than patched: only BUILD documents contribute, a session
    /// holds a handful of those, and one is tens of microseconds at the
    /// measured 110.8 MB/s. A cache keyed on text we already hold would buy a
    /// cost nobody has measured.
    ///
    /// Synchronous, on the loop that owns the map. That is what makes the tier
    /// current for every later request rather than merely soon: the loop reads
    /// the next message only once this has returned.
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
    dialect: Dialect,
    kind: FileKind,
}

impl Document {
    /// Hold `text` as the contents of `path`, classified against the workspace.
    ///
    /// `root` resolves the one file addressed by path rather than by name,
    /// `tools/build_rules/prelude_bazel`, so a server that found a workspace
    /// reads it as Bazel's prelude.
    ///
    /// A path Bazel recognises as nothing is read as Starlark: the client
    /// opened it against this server, and a syntactic answer is the closest
    /// true one available.
    #[must_use]
    pub fn new(path: PathBuf, text: String, root: Option<&Path>) -> Self {
        let (dialect, kind) = classify(&path, root).unwrap_or((Dialect::Standard, FileKind::Bzl));
        Self {
            text,
            path,
            dialect,
            kind,
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
        self.kind
    }

    /// Line starts, for a request converting more than one offset.
    #[must_use]
    pub fn line_index(&self) -> LineIndex {
        LineIndex::new(&self.text)
    }

    /// The syntax tree, in the dialect the path implies.
    #[must_use]
    pub fn parse(&self) -> Parse {
        parse(&self.text, self.dialect)
    }

    /// Add what this buffer declares to `tier`, and claim the file if it can.
    ///
    /// **A buffer the parser had to recover in adds and never claims.** Claiming
    /// a file is what lets the tier say a target has been *deleted*, and a file
    /// caught mid-edit cannot tell a deletion from a declaration the user has
    /// not finished typing. Claiming it anyway would make every target in the
    /// file blink out of the index between keystrokes, taking navigation with
    /// them; leaving the disk to answer keeps them, stale by a few characters.
    /// Invariant 4 wants the stale one.
    fn contribute(&self, root: &Path, tier: &mut Tier) {
        if !matches!(self.kind, FileKind::Build) {
            return;
        }
        let path: Arc<Path> = Arc::from(self.path.as_path());
        let parsed = self.parse();
        if !collect_file(
            root,
            &path,
            &parsed,
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

    /// Where this buffer declares `name`, as it stands now.
    ///
    /// `None` where the buffer declares no such target, which is the answer
    /// when an edit has renamed or deleted one the index still lists.
    #[must_use]
    pub fn declaration_of(&self, name: &str) -> Option<Position> {
        crate::index::declaration_in(&self.text, self.dialect, name)
    }

    /// The byte offset of a line and UTF-16 column.
    #[must_use]
    /// The position of a byte offset in this document.
    ///
    /// Rebuilds the line index on every call, which is why handlers hold a
    /// [`Document::line_index`] instead: this is a convenience for a one-off
    /// conversion, and in a loop it is quadratic.
    #[cfg(test)]
    pub fn position(&self, offset: usize) -> Position {
        self.line_index().position(&self.text, offset)
    }

    /// The byte offset of a position in this document.
    pub fn offset(&self, position: Position) -> usize {
        self.line_index().offset(&self.text, position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
