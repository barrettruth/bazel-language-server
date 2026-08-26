//! `textDocument/documentHighlight`: the target under the cursor, in this file.

use std::path::Path;

use lsp_types::{DocumentHighlight, DocumentHighlightKind, Position};
use starlark_cst::parse;

use super::cursor::{
    classify_file, declaration_site, enclosing_package, name_sites, string_at, target_label,
};
use crate::line_index::LineIndex;

/// Every occurrence of the target under the cursor, within one file.
///
/// `references` narrowed to the document the cursor is in, which is what an
/// editor paints as you rest on a label. The declaration is a `Write` and the
/// labels naming it are `Read`s, so a client can colour the definition apart
/// from its uses.
///
/// Partial in the same two ways `references` is: external repositories and the
/// targets legacy macros compute at evaluation time wait on the graph tier.
#[must_use]
pub fn document_highlight(
    text: &str,
    file: &Path,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Vec<DocumentHighlight> {
    let lines = LineIndex::new(text);
    let label = u32::try_from(lines.offset(text, position))
        .ok()
        .and_then(|offset| {
            string_at(
                &parse(text, classify_file(file, root).0).syntax(),
                offset,
                classify_file(file, root).1,
            )
        })
        .and_then(|found| target_label(&found, enclosing_package(root, file).as_deref()));
    let Some(label) = label else {
        tracing::debug!("the cursor is on no label and no target name, so nothing is highlighted");
        return Vec::new();
    };

    let key = label.key();
    let declaration = declaration_site(index, &key);
    let highlights: Vec<DocumentHighlight> = name_sites(index, &key, true)
        .into_iter()
        .filter(|site| site.0 == file)
        .map(|site| DocumentHighlight {
            range: site.1,
            kind: Some(if declaration.as_ref() == Some(&site) {
                DocumentHighlightKind::Write
            } else {
                DocumentHighlightKind::Read
            }),
        })
        .collect();
    tracing::debug!(label = key, count = highlights.len(), "documentHighlight");
    highlights
}

#[cfg(test)]
mod tests {
    use crate::handlers::fixture::Fixture;

    /// The declaration is the write and every label naming it is a read, so an
    /// editor can colour the definition apart from its uses. Both ends of the
    /// question agree, as they do for references.
    #[test]
    fn document_highlight_writes_the_declaration_and_reads_its_labels() {
        let fixture = Fixture::workspace();
        let expected = [
            "Write 11:12 srcs",
            "Read 35:15 srcs",
            "Read 58:10 srcs",
            "Read 63:27 srcs",
            "Read 78:24 srcs",
            "Read 88:12 srcs",
        ];

        let from_declaration = fixture.highlights("lib/BUILD.bazel", "\"srcs\"");
        assert_eq!(from_declaration, expected);
        // From a label pointing at it: `actual = ":srcs"`.
        assert_eq!(
            fixture.highlights("lib/BUILD.bazel", "\":srcs\","),
            expected
        );
    }

    /// Only this document. `//lib/sub:sub_srcs` is named three times in
    /// `//lib` and declared in `//lib/sub`, and neither file sees the other's
    /// occurrences — a highlight in a buffer the user is not looking at is a
    /// range the client would paint over the wrong text.
    #[test]
    fn document_highlight_stops_at_the_file_it_was_asked_about() {
        let fixture = Fixture::workspace();
        assert_eq!(
            fixture.highlights("lib/BUILD.bazel", "//lib/sub:sub_srcs"),
            [
                "Read 23:23 sub_srcs",
                "Read 59:19 sub_srcs",
                "Read 63:55 sub_srcs"
            ]
        );
        // The declaring file holds the write and none of the reads.
        assert_eq!(
            fixture.highlights("lib/sub/BUILD.bazel", "\"sub_srcs\""),
            ["Write 3:12 sub_srcs"]
        );
    }

    /// A cursor on an identifier, a comment or bare punctuation is on no
    /// target, and the empty answer is logged rather than silent.
    #[test]
    fn document_highlight_declines_off_a_string() {
        let fixture = Fixture::workspace();
        for needle in ["filegroup(", "# Cross-package", "cc_library_placeholder"] {
            let found = fixture.highlights("lib/BUILD.bazel", needle);
            assert!(found.is_empty(), "cursor on {needle:?} found {found:?}");
        }
    }
}
