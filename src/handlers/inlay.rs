//! `textDocument/inlayHint`: the package a shorthand label leaves out.

use std::path::Path;

use lsp_types::{InlayHint, InlayHintKind, Label, Range};
use starlark_cst::{SyntaxElement, SyntaxKind};

use crate::document::Document;

use super::cursor::{enclosing_package, string_at};
use crate::label::parse_label;

/// The resolved package in front of every relative label in range.
///
/// `":srcs"` and `"srcs"` mean something only once you know the file they are
/// written in, and that is the one piece of a BUILD file a reader cannot see
/// locally. An absolute label already says it, so hinting there would repeat
/// what is on screen.
///
/// A label naming no declared target gets no hint: the package it would resolve
/// against is a guess, and a guess rendered as fact is worse than nothing.
#[must_use]
pub fn inlay_hints(
    document: &Document,
    root: &Path,
    index: &crate::index::Index,
    range: Range,
) -> Vec<InlayHint> {
    let text = document.text();
    let Some(package) = enclosing_package(root, document.path()) else {
        return Vec::new();
    };
    let lines = document.line_index();
    let from = lines.offset(text, range.start);
    let to = lines.offset(text, range.end);
    let syntax = document.parse().syntax();

    let mut hints = Vec::new();
    for token in syntax
        .descendants_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .filter(|token| token.kind() == SyntaxKind::STRING)
    {
        let start = usize::from(token.text_range().start());
        if start < from || start > to {
            continue;
        }
        let Some(found) = string_at(
            &syntax,
            u32::from(token.text_range().start()) + 1,
            document.kind(),
        ) else {
            continue;
        };
        if found.value.starts_with("//") || found.value.starts_with('@') {
            continue;
        }
        let Some(label) = parse_label(&found.value, Some(&package)) else {
            continue;
        };
        if index.target(&label.key()).is_none() {
            continue;
        }
        hints.push(InlayHint {
            position: lines.position(text, found.range.start as usize),
            label: Label::String(format!("//{package}")),
            kind: Some(InlayHintKind::Type),
            padding_left: Some(false),
            padding_right: Some(true),
            tooltip: None,
            text_edits: None,
            data: None,
        });
    }
    hints
}

#[cfg(test)]
mod tests {
    use crate::line_index::LineIndex;

    use super::super::fixture::Fixture;
    use super::*;

    fn hints(relative: &str) -> Vec<(String, String)> {
        let fixture = Fixture::workspace();
        let file = fixture.root.join(relative);
        let text = std::fs::read_to_string(&file).expect("fixture");
        let lines = LineIndex::new(&text);
        let whole = Range {
            start: lines.position(&text, 0),
            end: lines.position(&text, text.len()),
        };

        inlay_hints(
            &fixture.open(relative),
            &fixture.root,
            &fixture.index,
            whole,
        )
        .into_iter()
        .map(|hint| {
            let at = lines.offset(&text, hint.position);
            let rest = text[at..].split('"').next().unwrap_or_default().to_string();
            let Label::String(label) = hint.label else {
                unreachable!("only string labels are produced")
            };
            (rest, label)
        })
        .collect()
    }

    #[test]
    fn a_relative_label_is_hinted_with_its_package() {
        let found = hints("lib/BUILD.bazel");
        assert!(!found.is_empty());
        assert!(found.iter().all(|(_, label)| label == "//lib"), "{found:?}");
        assert!(found.iter().any(|(text, _)| text == ":srcs"), "{found:?}");
    }

    #[test]
    fn an_absolute_label_says_its_package_already() {
        let found = hints("lib/BUILD.bazel");
        assert!(
            found.iter().all(|(text, _)| !text.starts_with("//")),
            "{found:?}"
        );
    }

    #[test]
    fn a_label_naming_nothing_is_left_alone() {
        let found = hints("lib/BUILD.bazel");
        let fixture = Fixture::workspace();
        for (text, _) in &found {
            let key = format!("//lib:{}", text.trim_start_matches(':'));
            assert!(
                fixture.index.target(&key).is_some(),
                "hinted an unresolved label: {text}"
            );
        }
    }
}
