//! `textDocument/implementation`: from a rule to the function behind it.

use std::path::Path;

use lsp_types::{Location, Range};
use starlark_cst::ast::{AstNode, CallExpr, File, Stmt};
use starlark_cst::{SyntaxKind, TextRange, parse};

use super::cursor::{classify_file, file_uri};
use crate::line_index::LineIndex;

/// The `def` named by the `implementation` of the rule under the cursor.
///
/// A `rule()` call says what it does only by naming a function, so the jump
/// worth having is `implementation = _foo` to `def _foo`. The cursor may be on
/// either the rule's own name or the identifier it names.
///
/// One file only. A `.bzl` that loads its implementation from elsewhere needs
/// name resolution across `load()`, which is a symbol table this server does
/// not have and `starlark-cst` deliberately declines to build.
#[must_use]
pub fn implementation(
    text: &str,
    file: &Path,
    root: &Path,
    position: lsp_types::Position,
) -> Vec<Location> {
    let (dialect, _) = classify_file(file, root);
    let lines = LineIndex::new(text);
    let Ok(offset) = u32::try_from(lines.offset(text, position)) else {
        return Vec::new();
    };
    let parsed = parse(text, dialect);
    let syntax = parsed.syntax();

    let Some(wanted) = implementation_name(&syntax, offset) else {
        tracing::debug!("the cursor names no rule implementation");
        return Vec::new();
    };
    let Some(range) = definition_of(&syntax, &wanted) else {
        tracing::debug!(function = wanted, "no `def` of that name in this file");
        return Vec::new();
    };
    let Some(uri) = file_uri(file) else {
        return Vec::new();
    };

    vec![Location {
        uri,
        range: Range {
            start: lines.position(text, range.start().into()),
            end: lines.position(text, range.end().into()),
        },
    }]
}

/// The function name a rule points at, from a cursor on a name that means it.
///
/// Three identifiers do: the name the rule is assigned to, the `rule` callee
/// itself, and the function named by `implementation`. Anywhere else inside the
/// call is an attribute or a value, and jumping from those would fire whenever
/// the cursor happened to be in the rule at all.
fn implementation_name(syntax: &starlark_cst::SyntaxNode, offset: u32) -> Option<String> {
    for node in syntax.descendants() {
        let Some(call) = CallExpr::cast(node.clone()) else {
            continue;
        };
        if call.callee_name().as_deref() != Some("rule") {
            continue;
        }
        let named = call
            .args()
            .find(|arg| arg.name().as_deref() == Some("implementation"))
            .and_then(|arg| {
                arg.syntax()
                    .descendants_with_tokens()
                    .filter_map(starlark_cst::SyntaxElement::into_token)
                    .find(|token| {
                        token.kind() == SyntaxKind::IDENT && token.text() != "implementation"
                    })
            })?;

        let mut triggers = vec![named.text_range()];
        if let Some(callee) = call
            .syntax()
            .children()
            .find(|child| child.kind() == SyntaxKind::IDENT_EXPR)
        {
            triggers.push(callee.text_range());
        }
        if let Some(assigned) = node
            .parent()
            .filter(|parent| parent.kind() == SyntaxKind::ASSIGN_STMT)
            .and_then(|parent| {
                parent
                    .children()
                    .find(|child| child.kind() == SyntaxKind::IDENT_EXPR)
            })
        {
            triggers.push(assigned.text_range());
        }

        if triggers
            .iter()
            .any(|range| (u32::from(range.start())..=u32::from(range.end())).contains(&offset))
        {
            return Some(named.text().to_string());
        }
    }
    None
}

/// The span of a top-level `def` of that name.
fn definition_of(syntax: &starlark_cst::SyntaxNode, wanted: &str) -> Option<TextRange> {
    File::cast(syntax.clone())?.stmts().find_map(|stmt| {
        let Stmt::Def(def) = stmt else { return None };
        (def.name().as_deref() == Some(wanted)).then(|| def.range())
    })
}

#[cfg(test)]
mod tests {
    use super::super::fixture::Fixture;
    use super::*;

    const BZL: &str = "\
def _beacon_impl(ctx):
    return []

beacon_component = rule(
    implementation = _beacon_impl,
    attrs = {},
)
";

    fn jump(text: &str, needle: &str) -> Option<String> {
        let root = Fixture::workspace().root;
        let file = root.join("tools/scratch.bzl");
        let lines = LineIndex::new(text);
        let at = text.find(needle).expect("needle") + needle.len() / 2;
        let found = implementation(text, &file, &root, lines.position(text, at));
        found.first().map(|location| {
            let start = lines.offset(text, location.range.start);
            let end = lines.offset(text, location.range.end);
            text[start..end]
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        })
    }

    #[test]
    fn the_identifier_leads_to_its_def() {
        assert_eq!(
            jump(BZL, "_beacon_impl,"),
            Some("def _beacon_impl(ctx):".to_string())
        );
    }

    #[test]
    fn a_name_with_no_def_here_yields_nothing() {
        let loaded = "load(\"//other:defs.bzl\", \"_impl\")\n\nr = rule(implementation = _impl)\n";
        assert_eq!(jump(loaded, "_impl)"), None);
    }

    #[test]
    fn the_rules_exported_name_leads_to_its_def() {
        assert_eq!(
            jump(BZL, "beacon_component ="),
            Some("def _beacon_impl(ctx):".to_string())
        );
    }

    /// An attribute inside the call is not a name that means the rule, so a
    /// cursor resting there does not fire.
    #[test]
    fn a_cursor_elsewhere_in_the_call_yields_nothing() {
        assert_eq!(jump(BZL, "attrs"), None);
    }
}
