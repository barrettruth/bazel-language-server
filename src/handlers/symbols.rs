//! `textDocument/documentSymbol` and `workspace/symbol`: the targets a file
//! declares, and the targets the whole workspace declares.

use lsp_types::{
    BaseSymbolInformation, DocumentSymbol, Location, Position, Range, SymbolKind, WorkspaceSymbol,
};
use starlark_cst::FileKind;
use starlark_cst::ast::{AstNode, Expr, File, Stmt};

use super::cursor::file_uri;
use crate::document::Document;

/// A target declared in a BUILD file, with the ranges an editor needs.
pub(super) struct Declaration {
    pub(super) name: String,
    pub(super) rule: String,
    /// The whole rule call.
    pub(super) full: Range,
    /// Just the name string's content, quotes excluded.
    pub(super) selection: Range,
}

/// Every target a BUILD file declares.
///
/// Only BUILD files declare targets. `MODULE.bazel` is full of top-level calls
/// carrying a `name` — `bazel_dep(name = "rules_shell")` — and reporting those
/// as targets invents labels that resolve to nothing.
///
/// Legacy macros are invisible here by construction: `legacy_macro(name = "x")`
/// yields `x`, but the `x_0`, `x_1` it actually declares are computed at
/// evaluation time and only Bazel knows them.
#[must_use]
pub(super) fn declarations(document: &Document) -> Vec<Declaration> {
    if document.kind() != FileKind::Build {
        return Vec::new();
    }
    let text = document.text();
    let lines = document.line_index();
    let Some(file) = File::cast(document.parse().syntax()) else {
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
pub fn document_symbols(document: &Document) -> Vec<DocumentSymbol> {
    declarations(document)
        .into_iter()
        .map(|d| DocumentSymbol {
            name: format!(":{}", d.name),
            kind: symbol_kind(&d.rule),
            detail: Some(d.rule),
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range: d.full,
            selection_range: d.selection,
            children: None,
        })
        .collect()
}

/// A `SymbolKind` chosen so a picker's kind column says something.
///
/// LSP has no kind for "build target", so every target sharing one renders as a
/// column of identical `[Object]` — noise in a list of hundreds. Grouping by
/// what the rule *does* makes tests and binaries findable at a glance. The rule
/// name is still carried exactly, in `containerName`.
fn symbol_kind(rule: &str) -> SymbolKind {
    match rule {
        r if r.ends_with("_test") || r == "test_suite" => SymbolKind::Event,
        r if r.ends_with("_binary") => SymbolKind::Function,
        r if r.ends_with("_library") || r.ends_with("_module") => SymbolKind::Module,
        "alias" => SymbolKind::Interface,
        "filegroup" | "exports_files" | "pkg_files" => SymbolKind::File,
        "genrule" | "run_binary" => SymbolKind::Constructor,
        r if r.ends_with("_setting") || r.ends_with("_flag") => SymbolKind::Constant,
        _ => SymbolKind::Struct,
    }
}

/// Workspace symbols from the static index.
///
/// Undercounts until the graph tier lands, which is why the caller must not
/// present this as exhaustive. See `ROADMAP.md` G4.
#[must_use]
pub fn workspace_symbols(index: &crate::index::Index, query: &str) -> Vec<WorkspaceSymbol> {
    index
        .targets()
        .filter(|(label, _)| contains_case_insensitive(label, query))
        .take(512)
        .filter_map(|(label, target)| {
            let uri = file_uri(&target.file)?;
            let at = Position {
                line: target.line,
                character: target.character,
            };
            Some(WorkspaceSymbol {
                location: Location {
                    uri,
                    range: Range { start: at, end: at },
                }
                .into(),
                data: None,
                base_symbol_information: BaseSymbolInformation {
                    name: label.to_string(),
                    kind: symbol_kind(&target.rule),
                    tags: None,
                    container_name: Some(target.rule.to_string()),
                },
            })
        })
        .collect()
}

fn contains_case_insensitive(text: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if text.is_ascii() && query.is_ascii() {
        return text
            .as_bytes()
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query.as_bytes()));
    }
    text.to_lowercase().contains(&query.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::fixture::document;

    const BUILD: &str = "\
filegroup(\n    name = \"srcs\",\n    srcs = [],\n)\n\ncc_library(name = \"core\")\n";

    #[test]
    fn finds_every_declaration() {
        let found = declarations(&document("BUILD.bazel", BUILD));
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
        let symbols = document_symbols(&document("BUILD.bazel", BUILD));
        assert_eq!(symbols[0].name, ":srcs");
        assert_eq!(symbols[0].detail.as_deref(), Some("filegroup"));
    }

    #[test]
    fn module_bazel_declares_no_targets() {
        let module = "bazel_dep(name = \"rules_shell\", version = \"0.3.0\")\n";
        assert!(declarations(&document("MODULE.bazel", module)).is_empty());
        // The same text read as a BUILD file would look like a target.
        assert_eq!(declarations(&document("BUILD.bazel", module)).len(), 1);
    }

    #[test]
    fn workspace_queries_ignore_case() {
        assert!(contains_case_insensitive("//lib:HTTPServer", "http"));
        assert!(contains_case_insensitive("//münchen:Straße", "straße"));
        assert!(!contains_case_insensitive("//lib:server", "client"));
    }
}
