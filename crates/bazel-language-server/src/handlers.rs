//! Request handling against in-memory state.
//!
//! Everything here is pure and fast: parse, walk, convert. No Bazel, no IO.
//! That is invariant 1, expressed as a module boundary.

use lsp_types::{
    BaseSymbolInformation, Diagnostic, DiagnosticSeverity, DocumentSymbol, Location, Range,
    SymbolKind, WorkspaceSymbol,
};
use starlark_cst::ast::{AstNode, Expr, File, Stmt};
use starlark_cst::{Dialect, parse};

use crate::line_index::LineIndex;

/// A target declared in a BUILD file, with the ranges an editor needs.
pub struct Declaration {
    pub name: String,
    pub rule: String,
    /// The whole rule call.
    pub full: Range,
    /// Just the name string's content, quotes excluded.
    pub selection: Range,
}

/// Every target a BUILD file declares.
///
/// Legacy macros are invisible here by construction: `legacy_macro(name = "x")`
/// yields `x`, but the `x_0`, `x_1` it actually declares are computed at
/// evaluation time and only Bazel knows them.
#[must_use]
pub fn declarations(text: &str, dialect: Dialect) -> Vec<Declaration> {
    let lines = LineIndex::new(text);
    let parsed = parse(text, dialect);
    let Some(file) = File::cast(parsed.syntax()) else {
        return Vec::new();
    };

    file.stmts()
        .filter_map(|stmt| match stmt {
            Stmt::Expr(expr) => expr.expr(),
            _ => None,
        })
        .filter_map(|expr| match expr {
            Expr::Call(call) => Some(call),
            _ => None,
        })
        .filter_map(|call| {
            let rule = call.callee_name()?;
            let Expr::Literal(name) = call.arg("name")? else {
                return None;
            };
            let value = name.string_value()?;
            let span = call.range();
            let full = Range {
                start: lines.position(text, usize::from(span.start())),
                end: lines.position(text, usize::from(span.end())),
            };
            let selection = name.string_value_range().map_or(full, |r| Range {
                start: lines.position(text, usize::from(r.start())),
                end: lines.position(text, usize::from(r.end())),
            });
            Some(Declaration {
                name: value,
                rule,
                full,
                selection,
            })
        })
        .collect()
}

#[must_use]
pub fn document_symbols(text: &str, dialect: Dialect) -> Vec<DocumentSymbol> {
    declarations(text, dialect)
        .into_iter()
        .map(|d| DocumentSymbol {
            name: format!(":{}", d.name),
            detail: Some(d.rule),
            kind: SymbolKind::Object,
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range: d.full,
            selection_range: d.selection,
            children: None,
        })
        .collect()
}

/// Syntax errors, as diagnostics.
///
/// The parser always returns a tree, so this never suppresses other features;
/// a file with errors still yields whatever symbols survived recovery.
#[must_use]
pub fn syntax_diagnostics(text: &str, dialect: Dialect) -> Vec<Diagnostic> {
    let lines = LineIndex::new(text);
    parse(text, dialect)
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

/// Workspace symbols from the static index.
///
/// Undercounts until the graph tier lands, which is why the caller must not
/// present this as exhaustive. See `ROADMAP.md` G4.
#[must_use]
pub fn workspace_symbols(index: &bls_index::Index, query: &str) -> Vec<WorkspaceSymbol> {
    let needle = query.to_lowercase();
    index
        .targets
        .iter()
        .filter(|(label, _)| needle.is_empty() || label.to_lowercase().contains(&needle))
        .take(512)
        .filter_map(|(label, target)| {
            let path = index.path(target.file)?;
            let uri: lsp_types::Uri = format!("file://{}", path.display()).parse().ok()?;
            Some(WorkspaceSymbol {
                // Offsets need the file's text to become positions, and the
                // index deliberately does not retain it. Resolved lazily by
                // `workspaceSymbol/resolve` once that lands.
                location: Location {
                    uri,
                    range: Range::default(),
                }
                .into(),
                data: None,
                base_symbol_information: BaseSymbolInformation {
                    name: label.clone(),
                    kind: SymbolKind::Object,
                    tags: None,
                    container_name: Some(target.rule.to_string()),
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = "\
filegroup(\n    name = \"srcs\",\n    srcs = [],\n)\n\ncc_library(name = \"core\")\n";

    #[test]
    fn finds_every_declaration() {
        let found = declarations(BUILD, Dialect::Bazel);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "srcs");
        assert_eq!(found[0].rule, "filegroup");
        assert_eq!(found[1].name, "core");

        // The selection range covers the name only, not the whole call.
        assert_eq!(found[0].selection.start.line, 1);
        assert!(found[0].selection.start.character > 0);
    }

    #[test]
    fn symbols_are_prefixed_like_labels() {
        let symbols = document_symbols(BUILD, Dialect::Bazel);
        assert_eq!(symbols[0].name, ":srcs");
        assert_eq!(symbols[0].detail.as_deref(), Some("filegroup"));
    }

    #[test]
    fn broken_input_still_yields_symbols() {
        let broken = "filegroup(name = \"a\",\n\ncc_library(name = \"b\")\n";
        assert!(!syntax_diagnostics(broken, Dialect::Bazel).is_empty());
        // Recovery is local, so the file is not written off entirely.
        assert!(
            !parse(broken, Dialect::Bazel)
                .syntax()
                .text()
                .to_string()
                .is_empty()
        );
    }
}
