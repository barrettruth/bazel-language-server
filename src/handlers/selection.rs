//! `textDocument/selectionRange`: expand the selection along the syntax tree.

use lsp_types::{Position, Range, SelectionRange};
use starlark_cst::SyntaxNode;

use crate::document::Document;

use crate::line_index::LineIndex;

/// For each requested position, the chain of ranges enclosing it.
///
/// The chain runs from the token under the cursor outward to the file, so a
/// caret inside a label grows to the string, then the list entry, then the
/// attribute, then the whole rule call — which is how a BUILD file nests, and
/// each step is a thing a reader would want to select.
///
/// A position outside the document still answers, with the file itself, since
/// the protocol expects one result per position and dropping one would
/// misalign the client's array.
#[must_use]
pub fn selection_ranges(document: &Document, positions: &[Position]) -> Vec<SelectionRange> {
    let text = document.text();
    let lines = document.line_index();
    let root = document.parse().syntax();

    positions
        .iter()
        .map(|position| {
            let offset = lines.offset(text, *position);
            chain(&root, text, &lines, offset)
        })
        .collect()
}

fn chain(root: &SyntaxNode, text: &str, lines: &LineIndex, offset: usize) -> SelectionRange {
    let mut widening: Vec<Range> = Vec::new();
    for node in root.descendants() {
        let range = node.text_range();
        if usize::from(range.start()) <= offset && offset <= usize::from(range.end()) {
            widening.push(Range {
                start: lines.position(text, usize::from(range.start())),
                end: lines.position(text, usize::from(range.end())),
            });
        }
    }

    let mut current: Option<SelectionRange> = None;
    for range in widening {
        if current.as_ref().is_some_and(|inner| inner.range == range) {
            continue;
        }
        current = Some(SelectionRange {
            range,
            parent: current.map(Box::new),
        });
    }

    current.unwrap_or_else(|| SelectionRange {
        range: Range {
            start: Position::new(0, 0),
            end: lines.position(text, text.len()),
        },
        parent: None,
    })
}

#[cfg(test)]
mod tests {
    use super::super::fixture::document;
    use super::*;
    use crate::line_index::LineIndex;

    const BUILD: &str = "filegroup(\n    name = \"srcs\",\n    srcs = [\"a.txt\"],\n)\n";

    fn widening(text: &str, needle: &str) -> Vec<String> {
        let lines = LineIndex::new(text);
        let at = text.find(needle).expect("needle") + 1;
        let ranges = selection_ranges(&document("BUILD.bazel", text), &[lines.position(text, at)]);
        assert_eq!(ranges.len(), 1);

        let mut out = Vec::new();
        let mut node = Some(&ranges[0]);
        while let Some(current) = node {
            let start = lines.offset(text, current.range.start);
            let end = lines.offset(text, current.range.end);
            out.push(text[start..end].to_string());
            node = current.parent.as_deref();
        }
        out
    }

    #[test]
    fn the_chain_widens_from_the_token_to_the_file() {
        let steps = widening(BUILD, "a.txt");
        assert_eq!(steps.first().map(String::as_str), Some("\"a.txt\""));
        assert_eq!(steps.last().map(String::as_str), Some(BUILD));
        assert!(
            steps.iter().any(|step| step == "[\"a.txt\"]"),
            "the list is a step: {steps:?}"
        );
        assert!(
            steps.iter().any(|step| step == "srcs = [\"a.txt\"]"),
            "the attribute is a step: {steps:?}"
        );
    }

    #[test]
    fn each_step_contains_the_one_before_it() {
        let steps = widening(BUILD, "srcs\",");
        for pair in steps.windows(2) {
            assert!(
                pair[1].contains(&pair[0]),
                "{:?} does not contain {:?}",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn one_answer_per_position() {
        let lines = LineIndex::new(BUILD);
        let positions = [
            lines.position(BUILD, 0),
            lines.position(BUILD, BUILD.len()),
            Position::new(999, 0),
        ];
        assert_eq!(
            selection_ranges(&document("BUILD.bazel", BUILD), &positions).len(),
            positions.len()
        );
    }
}
