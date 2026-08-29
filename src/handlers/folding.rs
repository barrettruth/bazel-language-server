//! `textDocument/foldingRange`: what a reader can collapse.
//!
//! Purely syntactic, so it answers for a broken buffer too: the parser recovers
//! and whatever nodes survived still fold.

use lsp_types::{FoldingRange, FoldingRangeKind};
use starlark_cst::{SyntaxKind, SyntaxNode};

use crate::document::Document;

use crate::line_index::LineIndex;

/// The foldable regions of a document.
///
/// A rule call spanning several lines is the unit a BUILD file is read in, so
/// calls, the collections inside them, and definition bodies all fold. A run of
/// consecutive comment lines folds as one region, which is how a licence header
/// gets out of the way.
///
/// A region that begins and ends on one line is dropped: collapsing it hides
/// nothing and clients draw a useless marker for it.
#[must_use]
pub fn folding_ranges(document: &Document) -> Vec<FoldingRange> {
    let text = document.text();
    let lines = document.line_index();
    let root = document.parse().syntax();
    let mut ranges = Vec::new();

    for node in root.descendants() {
        if !foldable(node.kind()) {
            continue;
        }
        let start = lines
            .position(text, usize::from(node.text_range().start()))
            .line;
        let end = lines
            .position(text, usize::from(node.text_range().end()))
            .line;
        push(&mut ranges, start, end, None);
    }

    for (start, end) in comment_runs(&root, text, lines) {
        push(&mut ranges, start, end, Some(FoldingRangeKind::Comment));
    }

    ranges.sort_by_key(|range| (range.start_line, range.end_line));
    ranges.dedup_by_key(|range| (range.start_line, range.end_line));
    ranges
}

fn foldable(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::CALL_EXPR
            | SyntaxKind::LIST_EXPR
            | SyntaxKind::DICT_EXPR
            | SyntaxKind::TUPLE_EXPR
            | SyntaxKind::LIST_COMP
            | SyntaxKind::DICT_COMP
            | SyntaxKind::SUITE
            | SyntaxKind::LOAD_STMT
    )
}

/// Consecutive comment lines, as one region each.
///
/// Consecutive means no blank line between them: a blank line is how a writer
/// separates one remark from the next, so folding across it would join two
/// things the file says are apart.
fn comment_runs(root: &SyntaxNode, text: &str, lines: &LineIndex) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let mut open: Option<(u32, u32)> = None;

    for token in root
        .descendants_with_tokens()
        .filter_map(starlark_cst::SyntaxElement::into_token)
        .filter(|token| token.kind() == SyntaxKind::COMMENT)
    {
        let line = lines
            .position(text, usize::from(token.text_range().start()))
            .line;
        match open {
            Some((start, last)) if line == last + 1 => open = Some((start, line)),
            Some((start, last)) => {
                runs.push((start, last));
                open = Some((line, line));
            }
            None => open = Some((line, line)),
        }
    }
    runs.extend(open);
    runs
}

fn push(ranges: &mut Vec<FoldingRange>, start: u32, end: u32, kind: Option<FoldingRangeKind>) {
    if end <= start {
        return;
    }
    ranges.push(FoldingRange {
        start_line: start,
        end_line: end,
        kind,
        ..Default::default()
    });
}

#[cfg(test)]
mod tests {
    use super::super::fixture::document;
    use super::*;

    const BUILD: &str = "\
# a licence
# spanning three
# comment lines

# a separate remark
load(\"//tools:defs.bzl\", \"thing\")

filegroup(
    name = \"srcs\",
    srcs = [
        \"a.txt\",
        \"b.txt\",
    ],
)

filegroup(name = \"one_line\")
";

    fn fold(text: &str) -> Vec<(u32, u32, bool)> {
        folding_ranges(&document("BUILD.bazel", text))
            .into_iter()
            .map(|range| {
                (
                    range.start_line,
                    range.end_line,
                    range.kind == Some(FoldingRangeKind::Comment),
                )
            })
            .collect()
    }

    #[test]
    fn a_blank_line_ends_a_comment_run() {
        let folds = fold(BUILD);
        assert!(folds.contains(&(0, 2, true)), "{folds:?}");
        assert!(
            !folds.iter().any(|&(start, end, _)| start == 0 && end == 4),
            "the run stops at the blank line: {folds:?}"
        );
    }

    #[test]
    fn multi_line_calls_and_their_lists_fold() {
        let folds = fold(BUILD);
        assert!(folds.contains(&(7, 13, false)), "the filegroup: {folds:?}");
        assert!(folds.contains(&(9, 12, false)), "its srcs: {folds:?}");
    }

    #[test]
    fn a_single_line_region_is_dropped() {
        let folds = fold(BUILD);
        assert!(
            folds.iter().all(|&(start, end, _)| end > start),
            "{folds:?}"
        );
        assert!(fold("filegroup(name = \"x\")\n").is_empty());
    }

    #[test]
    fn a_broken_buffer_still_folds() {
        let broken = "filegroup(\n    name = \"a\",\n\ncc_library(name = \"b\")\n";
        assert!(!fold(broken).is_empty());
    }
}
