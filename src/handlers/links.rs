//! `textDocument/documentLink`: every label in a file, as something to click.
//!
//! Go-to-definition answers for the one string under the cursor; this answers
//! for all of them at once, which is what a client wants when it underlines a
//! whole buffer. The resolution is `definition`'s, so the two agree by
//! construction: a link exists exactly where a jump would have gone somewhere.

use std::path::Path;

use lsp_types::{DocumentLink, Range};
use starlark_cst::{SyntaxElement, SyntaxKind};

use super::cursor::{enclosing_package, file_uri, string_at};
use super::definition::{file_site, target_site};
use crate::document::Document;
use crate::label::parse_label;

/// A link for every label and `load()` path that resolves.
///
/// A string that resolves to nothing produces no link rather than a dead one:
/// an underline that goes nowhere when clicked is worse than plain text, and
/// external repositories and macro-generated targets are both unresolvable
/// here by design.
#[must_use]
pub fn document_links(
    document: &Document,
    root: &Path,
    index: &crate::index::Index,
) -> Vec<DocumentLink> {
    let text = document.text();
    let package = enclosing_package(root, document.path());
    let lines = document.line_index();
    let syntax = document.parse().syntax();

    let mut resolved = Vec::new();
    for token in syntax
        .descendants_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .filter(|token| token.kind() == SyntaxKind::STRING)
    {
        let offset = u32::from(token.text_range().start()) + 1;
        let Some(found) = string_at(&syntax, offset, document.kind()) else {
            continue;
        };
        let Some(label) = parse_label(&found.value, package.as_deref()) else {
            continue;
        };
        let Some(site) = target_site(index, &label).or_else(|| file_site(root, &label)) else {
            continue;
        };
        let Some(target) = file_uri(site.path()) else {
            continue;
        };
        resolved.push(DocumentLink {
            range: Range {
                start: lines.position(text, found.range.start as usize),
                end: lines.position(text, found.range.end as usize),
            },
            target: Some(target),
            tooltip: Some(label.key()),
            data: None,
        });
    }

    resolved.sort_by_key(|at| (at.range.start.line, at.range.start.character));
    resolved.dedup_by_key(|at| (at.range.start.line, at.range.start.character));
    resolved
}

#[cfg(test)]
mod tests {
    use crate::line_index::LineIndex;

    use super::super::fixture::Fixture;
    use super::*;

    fn links_in(relative: &str) -> Vec<(String, String)> {
        let fixture = Fixture::workspace();
        let file = fixture.root.join(relative);
        let text = std::fs::read_to_string(&file).expect("fixture");
        let lines = LineIndex::new(&text);

        document_links(&fixture.open(relative), &fixture.root, &fixture.index)
            .into_iter()
            .map(|link| {
                let start = lines.offset(&text, link.range.start);
                let end = lines.offset(&text, link.range.end);
                (
                    text[start..end].to_string(),
                    link.tooltip.unwrap_or_default(),
                )
            })
            .collect()
    }

    #[test]
    fn a_resolving_label_becomes_a_link_over_the_label_alone() {
        let links = links_in("lib/BUILD.bazel");
        assert!(!links.is_empty());
        assert!(
            links
                .iter()
                .any(|(text, key)| text == ":srcs" && key == "//lib:srcs"),
            "{links:?}"
        );
        for (text, _) in &links {
            assert!(
                !text.contains('"'),
                "the link excludes the quotes: {text:?}"
            );
        }
    }

    #[test]
    fn a_label_inside_a_command_is_linked() {
        let links = links_in("lib/BUILD.bazel");
        let inside = links
            .iter()
            .filter(|(text, key)| text == ":srcs" && key == "//lib:srcs")
            .count();
        assert!(inside >= 2, "the genrule cmd contributes one: {links:?}");
    }

    #[test]
    fn an_unresolvable_string_produces_no_link() {
        let links = links_in("lib/BUILD.bazel");
        assert!(
            !links.iter().any(|(_, key)| key.starts_with('@')),
            "external repositories are not linked: {links:?}"
        );
    }
}
