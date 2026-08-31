//! Catalog-independent structure for Bazelrc editor requests.

use lsp_types::{FoldingRange, FoldingRangeKind, Position, Range, SelectionRange, SemanticTokens};

use super::syntax::{Directive, Span, Statement};
use crate::document::Document;
use crate::line_index::encode_semantic_tokens;

const NAMESPACE: u32 = 2;
const KEYWORD: u32 = 4;
const PROPERTY: u32 = 5;
const COMMENT: u32 = 6;
const OPERATOR: u32 = 7;

#[must_use]
pub fn semantic_tokens(document: &Document) -> SemanticTokens {
    let Some(parsed) = document.bazelrc() else {
        return SemanticTokens::default();
    };
    let mut absolute = Vec::new();
    for line in &parsed.lines {
        match &line.statement {
            Some(Statement::Directive(directive)) => {
                push_span(document, line.tokens[0].range, KEYWORD, &mut absolute);
                if matches!(directive, Directive::ConditionalImport(_)) {
                    push_span(document, line.tokens[1].range, OPERATOR, &mut absolute);
                    push_span(document, line.tokens[2].range, NAMESPACE, &mut absolute);
                } else {
                    push_span(document, line.tokens[1].range, NAMESPACE, &mut absolute);
                }
            }
            Some(Statement::Entry) => {
                push_span(document, line.tokens[0].range, KEYWORD, &mut absolute);
                for option in line.options() {
                    push_span(document, option.range, PROPERTY, &mut absolute);
                }
            }
            Some(Statement::InvalidDirective) | None => {}
        }
        if let Some(comment) = line.comment {
            push_span(document, comment, COMMENT, &mut absolute);
        }
    }
    absolute.sort_unstable();
    SemanticTokens {
        result_id: None,
        data: encode_semantic_tokens(&absolute),
    }
}

fn push_span(
    document: &Document,
    span: Span,
    token_type: u32,
    out: &mut Vec<(u32, u32, u32, u32)>,
) {
    let text = document.text();
    let mut start = span.start;
    while start < span.end {
        let end = text[start..span.end]
            .find('\n')
            .map_or(span.end, |newline| start + newline);
        let end = end - usize::from(end > start && text.as_bytes()[end - 1] == b'\r');
        if end > start {
            let at = document.line_index().position(text, start);
            let after = document.line_index().position(text, end);
            out.push((
                at.line,
                at.character,
                after.character - at.character,
                token_type,
            ));
        }
        start = text[start..span.end]
            .find('\n')
            .map_or(span.end, |newline| start + newline + 1);
    }
}

#[must_use]
pub fn folding_ranges(document: &Document) -> Vec<FoldingRange> {
    let Some(parsed) = document.bazelrc() else {
        return Vec::new();
    };
    let text = document.text();
    let lines = document.line_index();
    let mut ranges = Vec::new();
    for line in &parsed.lines {
        let start = lines.position(text, line.range.start).line;
        let end = lines.position(text, line.range.end).line;
        push_fold(&mut ranges, start, end, None);
    }

    let mut comments: Vec<u32> = parsed
        .lines
        .iter()
        .filter_map(|line| {
            let comment = line.comment?;
            text[line.range.start..comment.start]
                .trim()
                .is_empty()
                .then(|| lines.position(text, comment.start).line)
        })
        .collect();
    comments.sort_unstable();
    let mut run = None;
    for line in comments {
        match run {
            Some((start, last)) if line == last + 1 => run = Some((start, line)),
            Some((start, last)) => {
                push_fold(&mut ranges, start, last, Some(FoldingRangeKind::Comment));
                run = Some((line, line));
            }
            None => run = Some((line, line)),
        }
    }
    if let Some((start, end)) = run {
        push_fold(&mut ranges, start, end, Some(FoldingRangeKind::Comment));
    }
    ranges.sort_by_key(|range| (range.start_line, range.end_line));
    ranges
}

fn push_fold(ranges: &mut Vec<FoldingRange>, start: u32, end: u32, kind: Option<FoldingRangeKind>) {
    if end > start {
        ranges.push(FoldingRange {
            start_line: start,
            end_line: end,
            kind,
            ..Default::default()
        });
    }
}

#[must_use]
pub fn selection_ranges(document: &Document, positions: &[Position]) -> Vec<SelectionRange> {
    let Some(parsed) = document.bazelrc() else {
        return Vec::new();
    };
    let text = document.text();
    let lines = document.line_index();
    let file = Range {
        start: Position::new(0, 0),
        end: lines.position(text, text.len()),
    };
    positions
        .iter()
        .map(|position| {
            let offset = lines.offset(text, *position);
            let line = parsed
                .lines
                .iter()
                .find(|line| contains(line.range, offset));
            let line_range = line.map_or(file, |line| range(document, line.range));
            let parent = SelectionRange {
                range: file,
                parent: None,
            };
            let parent = SelectionRange {
                range: line_range,
                parent: (line_range != file).then(|| Box::new(parent)),
            };
            let token = line.and_then(|line| {
                line.tokens
                    .iter()
                    .map(|token| token.range)
                    .chain(line.comment)
                    .find(|span| contains(*span, offset))
            });
            token.map_or(parent.clone(), |span| {
                let token = range(document, span);
                if token == parent.range {
                    parent
                } else {
                    SelectionRange {
                        range: token,
                        parent: Some(Box::new(parent)),
                    }
                }
            })
        })
        .collect()
}

fn contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

fn range(document: &Document, span: Span) -> Range {
    Range {
        start: document.line_index().position(document.text(), span.start),
        end: document.line_index().position(document.text(), span.end),
    }
}
