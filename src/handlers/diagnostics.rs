//! `textDocument/publishDiagnostics`: what the parser could not read.

use crate::document::Document;
use lsp_types::{Diagnostic, DiagnosticSeverity, Range};

/// Syntax errors, as diagnostics.
///
/// The parser always returns a tree, so this never suppresses other features;
/// a file with errors still yields whatever symbols survived recovery.
#[must_use]
pub fn syntax_diagnostics(document: &Document) -> Vec<Diagnostic> {
    let text = document.text();
    let lines = document.line_index();
    document
        .parse()
        .errors()
        .iter()
        .map(|error| Diagnostic {
            range: Range {
                start: lines.position(text, usize::from(error.range.start())),
                end: lines.position(text, usize::from(error.range.end())),
            },
            severity: Some(DiagnosticSeverity::Error),
            source: Some("bazel-language-server".to_string()),
            message: error.message.clone().into(),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::fixture::document;
    use super::*;

    #[test]
    fn broken_input_still_yields_symbols() {
        let broken = "filegroup(name = \"a\",\n\ncc_library(name = \"b\")\n";
        assert!(!syntax_diagnostics(&document("BUILD.bazel", broken)).is_empty());
        // Recovery is local, so the file is not written off entirely.
        assert!(
            !document("BUILD.bazel", broken)
                .parse()
                .syntax()
                .text()
                .to_string()
                .is_empty()
        );
    }
}
