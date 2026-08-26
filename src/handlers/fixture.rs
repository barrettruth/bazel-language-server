//! The torture workspace, driven the way the server drives a handler.

use std::path::{Path, PathBuf};

use lsp_types::{LocationLink, MarkupKind, Position};

use super::definition::definition;
use super::highlight::document_highlight;
use super::hover::hover;
use crate::line_index::LineIndex;

pub(super) fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/workspace")
        .canonicalize()
        .expect("the test workspace is checked in")
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
        let index = crate::index::build_static(&root);
        Self { root, index }
    }

    /// The document, and the cursor in the middle of `needle`.
    pub(super) fn cursor(&self, relative: &str, needle: &str) -> (PathBuf, String, Position) {
        let file = self.root.join(relative);
        let text = std::fs::read_to_string(&file).expect("fixture file");
        let at = text.find(needle).unwrap_or_else(|| {
            panic!("{needle:?} is not in {relative}");
        }) + needle.len() / 2;
        let position = LineIndex::new(&text).position(&text, at);
        (file, text, position)
    }

    pub(super) fn links(&self, relative: &str, needle: &str) -> Vec<LocationLink> {
        let (file, text, position) = self.cursor(relative, needle);
        definition(&text, &file, &self.root, &self.index, position)
    }

    /// Every highlight, as `kind line:character text`, so a test reads the
    /// range's own contents rather than taking its word for them.
    pub(super) fn highlights(&self, relative: &str, needle: &str) -> Vec<String> {
        let (file, text, position) = self.cursor(relative, needle);
        let lines = LineIndex::new(&text);
        document_highlight(&text, &file, &self.root, &self.index, position)
            .into_iter()
            .map(|highlight| {
                let start = lines.offset(&text, highlight.range.start);
                let end = lines.offset(&text, highlight.range.end);
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
        let (file, text, position) = self.cursor(relative, needle);
        let hovered = hover(&text, &file, &self.root, &self.index, position)?;
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
