//! `textDocument/semanticTokens`: the colouring a grammar cannot produce.
//!
//! A `TextMate` grammar sees `cc_library` and `srcs` as identifiers, because
//! telling a rule from an attribute from a local variable needs the tree. The
//! tokens here are exactly the distinctions that need it; everything a grammar
//! already gets right is left alone.

use lsp_types::{SemanticToken, SemanticTokens};
use starlark_cst::SyntaxKind;
use starlark_cst::ast::{Arg, AstNode, CallExpr, LiteralExpr, LoadStmt};

use crate::document::Document;

use crate::line_index::LineIndex;

/// The token types this server emits, in the order the protocol indexes them.
///
/// A client resolves a token's type by its position in this list, so the order
/// is part of the wire format: appending is safe, reordering silently recolours
/// every buffer.
pub const LEGEND: [&str; 4] = ["function", "parameter", "namespace", "string"];

const FUNCTION: u32 = 0;
const PARAMETER: u32 = 1;
const NAMESPACE: u32 = 2;
const STRING: u32 = 3;

/// One token per rule name, attribute name, `load()` path and label.
#[must_use]
pub fn semantic_tokens(document: &Document) -> SemanticTokens {
    let text = document.text();
    let lines = document.line_index();
    let root = document.parse().syntax();
    let mut absolute: Vec<(u32, u32, u32, u32)> = Vec::new();

    for node in root.descendants() {
        if let Some(load) = LoadStmt::cast(node.clone()) {
            if let Some(range) = load.module().and_then(|module| module.value_range()) {
                absolute.push(span(
                    text,
                    lines,
                    range.start().into(),
                    range.len().into(),
                    NAMESPACE,
                ));
            }
            continue;
        }
        if let Some(call) = CallExpr::cast(node.clone()) {
            if let Some(callee) = call
                .syntax()
                .children()
                .find(|child| child.kind() == SyntaxKind::IDENT_EXPR)
            {
                let range = callee.text_range();
                absolute.push(span(
                    text,
                    lines,
                    range.start().into(),
                    range.len().into(),
                    FUNCTION,
                ));
            }
            continue;
        }
        if let Some(arg) = Arg::cast(node.clone()) {
            if arg.name().is_some()
                && let Some(ident) = arg
                    .syntax()
                    .children_with_tokens()
                    .filter_map(starlark_cst::SyntaxElement::into_token)
                    .find(|token| token.kind() == SyntaxKind::IDENT)
            {
                let range = ident.text_range();
                absolute.push(span(
                    text,
                    lines,
                    range.start().into(),
                    range.len().into(),
                    PARAMETER,
                ));
            }
            continue;
        }
        if let Some(literal) = LiteralExpr::cast(node.clone())
            && let Some(range) = literal.string_value_range()
            && literal.string_value().is_some_and(|value| {
                value.starts_with("//") || value.starts_with(':') || value.starts_with('@')
            })
        {
            absolute.push(span(
                text,
                lines,
                range.start().into(),
                range.len().into(),
                STRING,
            ));
        }
    }

    absolute.sort_unstable();
    SemanticTokens {
        result_id: None,
        data: encode(&absolute),
    }
}

fn span(
    text: &str,
    lines: &LineIndex,
    start: usize,
    length: usize,
    token_type: u32,
) -> (u32, u32, u32, u32) {
    let at = lines.position(text, start);
    let end = lines.position(text, start + length);
    let width = if end.line == at.line {
        end.character - at.character
    } else {
        u32::try_from(length).unwrap_or(u32::MAX)
    };
    (at.line, at.character, width, token_type)
}

/// Absolute positions to the protocol's deltas.
///
/// Each token is relative to the one before it, and the column resets whenever
/// the line advances. Getting that reset wrong shifts every token after it, so
/// the round trip is what the tests assert rather than the encoded numbers.
fn encode(absolute: &[(u32, u32, u32, u32)]) -> Vec<SemanticToken> {
    let mut out = Vec::with_capacity(absolute.len());
    let mut last_line = 0;
    let mut last_start = 0;
    for &(line, start, length, token_type) in absolute {
        let delta_line = line - last_line;
        out.push(SemanticToken {
            delta_line,
            delta_start: if delta_line == 0 {
                start - last_start
            } else {
                start
            },
            length,
            token_type,
            token_modifiers_bitset: 0,
        });
        last_line = line;
        last_start = start;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::fixture::document;
    use super::*;
    use crate::line_index::LineIndex;

    const BUILD: &str = "\
load(\"//tools:defs.bzl\", \"thing\")

filegroup(
    name = \"srcs\",
    srcs = [\":other\"],
)
";

    /// Undo the delta encoding, which is the part that is easy to get wrong.
    fn decode(tokens: &SemanticTokens) -> Vec<(u32, u32, u32, u32)> {
        let mut out = Vec::new();
        let (mut line, mut start) = (0, 0);
        for token in &tokens.data {
            if token.delta_line == 0 {
                start += token.delta_start;
            } else {
                line += token.delta_line;
                start = token.delta_start;
            }
            out.push((line, start, token.length, token.token_type));
        }
        out
    }

    fn tokens_of(text: &str) -> Vec<(String, u32)> {
        let lines = LineIndex::new(text);
        decode(&semantic_tokens(&document("BUILD.bazel", text)))
            .into_iter()
            .map(|(line, start, length, token_type)| {
                let at = lines.offset(text, lsp_types::Position::new(line, start));
                let end = lines.offset(text, lsp_types::Position::new(line, start + length));
                (text[at..end].to_string(), token_type)
            })
            .collect()
    }

    #[test]
    fn the_delta_encoding_round_trips() {
        let found = tokens_of(BUILD);
        assert!(!found.is_empty());
        assert_eq!(
            found,
            vec![
                ("//tools:defs.bzl".to_string(), NAMESPACE),
                ("filegroup".to_string(), FUNCTION),
                ("name".to_string(), PARAMETER),
                ("srcs".to_string(), PARAMETER),
                (":other".to_string(), STRING),
            ]
        );
    }

    #[test]
    fn a_plain_string_is_not_a_label() {
        let found = tokens_of("filegroup(name = \"srcs\", out = \"not a label\")\n");
        assert!(
            found.iter().all(|(text, _)| text != "not a label"),
            "{found:?}"
        );
    }

    #[test]
    fn the_legend_covers_every_type_emitted() {
        let highest = tokens_of(BUILD)
            .iter()
            .map(|&(_, token_type)| token_type)
            .max()
            .unwrap_or(0);
        assert!((highest as usize) < LEGEND.len());
    }
}
