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

use lsp_types::Position;
use starlark_cst::{Dialect, FileKind, Parse, classify, parse};

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
