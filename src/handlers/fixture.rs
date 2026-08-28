//! The torture workspace, driven the way the server drives a handler.

use std::path::{Path, PathBuf};

use lsp_types::{LocationLink, MarkupKind, Position};

use super::definition::definition;
use super::highlight::document_highlight;
use super::hover::hover;
use crate::document::{Buffers, Document};

/// The buffers a request arrives against.
///
/// A handler that resolves into a file the client is editing reads it from
/// here, so a test that cares which text answers says so rather than leaving it
/// to what is on disk.
pub(super) struct Open(pub(super) Vec<Document>);

impl Open {
    /// Nothing open, so every answer comes from the index.
    pub(super) fn none() -> Self {
        Self(Vec::new())
    }
}

impl Buffers for Open {
    fn at(&self, path: &Path) -> Option<&Document> {
        self.0.iter().find(|document| document.path() == path)
    }
}

pub(super) fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/workspace")
        .canonicalize()
        .expect("the test workspace is checked in")
}

/// A buffer that is not on disk, under a name that classifies it the way the
/// request under test needs.
///
/// The kind of a file comes from its path, so a test asking about a BUILD file
/// has to name one even when the bytes never leave memory.
pub(super) fn document(name: &str, text: &str) -> Document {
    Document::new(Path::new("/ws").join(name), text.to_string(), None)
}

/// Drives a handler the way the server does: read the file, put the cursor
/// in the middle of `needle`, and report where it lands.
pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) index: crate::index::Index,
}

impl Fixture {
    pub(super) fn workspace() -> Self {
        let root = fixture_root();
        let index = crate::index::Index::of_disk(crate::index::build_static(&root));
        Self { root, index }
    }

    /// A file of the workspace, as the server holds it once opened.
    pub(super) fn open(&self, relative: &str) -> Document {
        let file = self.root.join(relative);
        let text = std::fs::read_to_string(&file).expect("fixture file");
        Document::new(file, text, Some(&self.root))
    }

    /// The same workspace with `relative` held open at `text`, published the
    /// way the server publishes it — through `Documents`, so a test exercises
    /// the path that actually runs rather than one shaped like it.
    pub(super) fn editing(&self, relative: &str, text: &str) -> crate::index::Index {
        let handle = crate::index::IndexHandle::new();
        handle.store_disk(crate::index::build_static(&self.root));
        let file = self.root.join(relative);
        let uri = super::cursor::file_uri(&file).expect("a uri for a fixture path");
        let mut docs = crate::document::Documents::new(Some(self.root.clone()), handle.clone());
        docs.set(uri, file, text.to_string());
        handle.load()
    }

    /// The document, and the cursor in the middle of `needle`.
    pub(super) fn cursor(&self, relative: &str, needle: &str) -> (Document, Position) {
        let document = self.open(relative);
        let at = document.text().find(needle).unwrap_or_else(|| {
            panic!("{needle:?} is not in {relative}");
        }) + needle.len() / 2;
        let position = document.position(at);
        (document, position)
    }

    pub(super) fn links(&self, relative: &str, needle: &str) -> Vec<LocationLink> {
        let (document, position) = self.cursor(relative, needle);
        definition(&document, &self.root, &self.index, position)
    }

    /// Every highlight, as `kind line:character text`, so a test reads the
    /// range's own contents rather than taking its word for them.
    pub(super) fn highlights(&self, relative: &str, needle: &str) -> Vec<String> {
        let (document, position) = self.cursor(relative, needle);
        let text = document.text();
        let lines = document.line_index();
        document_highlight(&document, &self.root, &self.index, position)
            .into_iter()
            .map(|highlight| {
                let start = lines.offset(text, highlight.range.start);
                let end = lines.offset(text, highlight.range.end);
                format!(
                    "{:?} {}:{} {}",
                    highlight.kind.expect("a kind"),
                    highlight.range.start.line,
                    highlight.range.start.character,
                    &text[start..end]
                )
            })
            .collect()
    }

    /// The hover card, as the client would render it.
    pub(super) fn card(&self, relative: &str, needle: &str) -> Option<String> {
        let (document, position) = self.cursor(relative, needle);
        let hovered = hover(&document, &self.root, &self.index, position)?;
        match hovered.contents {
            lsp_types::Contents::MarkupContent(markup) => {
                assert_eq!(markup.kind, MarkupKind::Markdown, "markdown, not marked-up");
                Some(markup.value)
            }
            other => panic!("a card is markup content, got {other:?}"),
        }
    }

    /// Where the cursor lands, as `path:line:character` relative to the
    /// workspace root.
    pub(super) fn go(&self, relative: &str, needle: &str) -> Option<String> {
        let link = self.links(relative, needle).into_iter().next()?;
        let path = link.target_uri.path().as_str().to_string();
        let path = path
            .strip_prefix(self.root.to_str().unwrap())?
            .trim_start_matches('/')
            .to_string();
        Some(format!(
            "{path}:{}:{}",
            link.target_range.start.line, link.target_range.start.character
        ))
    }
}
