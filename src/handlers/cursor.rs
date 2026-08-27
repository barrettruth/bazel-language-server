//! What the cursor is on, and where else that name is written.
//!
//! The machinery several requests share: resolving a caret to the string it
//! sits in, deciding what that string names, and listing every site the index
//! records for it.

use std::path::{Path, PathBuf};

use crate::label::{Label, make_variable_labels, parse_label};
use lsp_types::{Position, Range, Uri};
use starlark_cst::ast::{Arg, AstNode, LiteralExpr, LoadItem, LoadStmt};
use starlark_cst::{FileKind, SyntaxElement, SyntaxKind, SyntaxNode};

pub(super) fn file_uri(path: &Path) -> Option<Uri> {
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        // RFC 3986 unreserved, plus the separators a path needs to keep. Bytes
        // outside that set are escaped, so a workspace under a directory with a
        // space in it produces a URI a client can parse. `uri_to_path` decodes
        // the same way on the way back in.
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b':'
            | b'@' => uri.push(*byte as char),
            other => {
                use std::fmt::Write as _;
                let _ = write!(uri, "%{other:02X}");
            }
        }
    }
    uri.parse().ok()
}

/// What a string in a build file refers to, decided by where it sits.
#[derive(Debug)]
pub(super) enum StringRole {
    /// The first item of a `load()`: the `.bzl` file being loaded from.
    LoadModule,
    /// A symbol in a `load()`, carrying the module it comes from.
    ///
    /// Jumping to the symbol's own `def` would mean parsing the module and
    /// resolving the name in it, which is a second file and a second index;
    /// this jumps to the file that defines it instead. Landing in the right
    /// file is most of the value and none of the risk.
    LoadSymbol(String),
    /// The `name` of a top-level rule call: the target being declared rather
    /// than a label pointing at one. `name = "srcs"` in `//lib` is `//lib:srcs`
    /// and nothing else, where a label of the same text would first have to be
    /// resolved against the package that wrote it.
    TargetName,
    /// Anything else. In a build file that means a label.
    Label,
}

/// A string literal's content and what it is for.
#[derive(Debug)]
pub(super) struct StringAt {
    pub(super) value: String,
    /// Byte range of the content, quotes and prefix excluded.
    pub(super) range: std::ops::Range<u32>,
    pub(super) role: StringRole,
}

/// The string literal whose content contains `offset`.
///
/// `None` when the cursor is anywhere else — on a quote, an identifier, or in
/// the whitespace between them.
pub(super) fn string_at(root: &SyntaxNode, offset: u32, kind: FileKind) -> Option<StringAt> {
    // `token_at_offset` answers with whatever token is at the offset, and with
    // two of them on a boundary. The kind test is the real query, so the scan
    // states it directly — O(tokens) against a parse that is O(bytes) and has
    // just happened anyway, so it is not the cost that matters here.
    let token = root
        .descendants_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|token| {
            token.kind() == SyntaxKind::STRING
                && (u32::from(token.text_range().start())..u32::from(token.text_range().end()))
                    .contains(&offset)
        })?;
    let parent = token.parent()?;

    // A `load()` item holds its string token directly; every other string is
    // wrapped in a literal expression.
    let (value, range, role) = if let Some(item) = LoadItem::cast(parent.clone()) {
        let load = LoadStmt::cast(parent.parent()?)?;
        let module = load.module()?;
        let role = if module == item {
            StringRole::LoadModule
        } else {
            StringRole::LoadSymbol(module.value()?)
        };
        (item.value()?, item.value_range()?, role)
    } else {
        let literal = LiteralExpr::cast(parent)?;
        // Only a BUILD file declares targets. `MODULE.bazel` and `.bzl` files
        // are full of top-level calls carrying a `name` — `module(name = "x")`,
        // every `bazel_dep` — and such a string is neither a declaration nor a
        // label. Reading it as either resolves it against the enclosing package
        // and lands on whichever target happens to share the spelling, which
        // rename would then rewrite.
        let role = match (declares_a_target(&literal), kind) {
            (true, FileKind::Build) => StringRole::TargetName,
            (true, _) => return None,
            (false, _) => StringRole::Label,
        };
        (literal.string_value()?, literal.string_value_range()?, role)
    };

    let range = u32::from(range.start())..u32::from(range.end());
    // The content range excludes the quotes, so a cursor on one is not on the
    // label. Its far end counts: that is where the caret sits after typing.
    if !(range.start..=range.end).contains(&offset) {
        return None;
    }

    // A `cmd` is not a label, but `$(location :srcs)` inside it holds one, and
    // that is what the cursor is on. The index already reads these, so without
    // this the same label is findable by references and not by navigation.
    if let Some(inner) = expansion_at(&value, range.start, offset) {
        return Some(inner);
    }

    Some(StringAt { value, range, role })
}

/// The label of a make-variable expansion under the cursor, in file coordinates.
///
/// `content` is where the string's content begins, and `string_value` returns
/// the raw slice rather than an unescaped copy, so an offset within the value
/// is that offset within the file.
fn expansion_at(value: &str, content: u32, offset: u32) -> Option<StringAt> {
    make_variable_labels(value).find_map(|(label, at)| {
        let start = content + u32::try_from(at).ok()?;
        let end = start + u32::try_from(label.len()).ok()?;
        (start..=end).contains(&offset).then_some(StringAt {
            value: label,
            range: start..end,
            role: StringRole::Label,
        })
    })
}

/// Whether a string is the `name` of a rule call at the top level of the file.
///
/// Only the top level declares targets: `name = "x"` inside a `select()` or a
/// nested call is an argument that happens to be called `name`, and treating it
/// as a declaration would attribute every reference to the wrong label.
fn declares_a_target(literal: &LiteralExpr) -> bool {
    let Some(arg) = literal.syntax().parent().and_then(Arg::cast) else {
        return false;
    };
    if arg.name().as_deref() != Some("name") {
        return false;
    }
    // ARG -> ARG_LIST -> CALL_EXPR -> EXPR_STMT -> FILE
    arg.syntax()
        .parent()
        .and_then(|list| list.parent())
        .filter(|call| call.kind() == SyntaxKind::CALL_EXPR)
        .and_then(|call| call.parent())
        .filter(|stmt| stmt.kind() == SyntaxKind::EXPR_STMT)
        .and_then(|stmt| stmt.parent())
        .is_some_and(|file| file.kind() == SyntaxKind::FILE)
}

/// The package a file belongs to, as a workspace-relative directory.
///
/// The package is the nearest ancestor holding a BUILD file, not simply the
/// file's own directory: `app/nested/data.txt` belongs to `//app`, and calling
/// it `//app/nested` would resolve every relative label in it to a package
/// that does not exist.
///
/// `None` when no ancestor up to the workspace root has a BUILD file. Relative
/// labels are then unresolvable, which is correct — there is no package for
/// them to be relative to — while `//pkg:target` still works.
pub(super) fn enclosing_package(root: &Path, file: &Path) -> Option<String> {
    if !file.starts_with(root) {
        return None;
    }
    let mut dir = file.parent()?;
    loop {
        if dir.join("BUILD.bazel").is_file() || dir.join("BUILD").is_file() {
            return Some(dir.strip_prefix(root).ok()?.to_str()?.replace('\\', "/"));
        }
        if dir == root {
            return None;
        }
        dir = dir.parent()?;
    }
}

/// The target a string names, whether it declares the target or points at it.
///
/// A declaration names its own package; a label has to be resolved against the
/// package that wrote it. A `.bzl` file is not a target at all: finding the
/// files that `load()` it is a different question, and answering this one with
/// those would be a wrong answer rather than none.
pub(super) fn target_label(found: &StringAt, package: Option<&str>) -> Option<Label> {
    match &found.role {
        StringRole::TargetName => Some(Label::new(package?, &found.value)),
        StringRole::Label => parse_label(&found.value, package),
        StringRole::LoadModule | StringRole::LoadSymbol(_) => None,
    }
}

/// Every place a target's name is written, as `(file, range of the name)`.
///
/// The range covers the name alone — `srcs` inside `"//lib:srcs"` — which is
/// what a client highlights and what a rename replaces.
///
/// Sorted, so a picker does not reshuffle between calls and a file's edits
/// arrive in the order they appear in it.
pub(super) fn name_sites(
    index: &crate::index::Index,
    key: &str,
    include_declaration: bool,
) -> Vec<(PathBuf, Range)> {
    let mut sites: Vec<(PathBuf, Range)> = index
        .references(key)
        .iter()
        .map(|reference| {
            (
                reference.file.to_path_buf(),
                name_range(reference.line, reference.character, reference.length),
            )
        })
        .collect();

    if include_declaration && let Some(site) = declaration_site(index, key) {
        sites.push(site);
    }

    sites.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.start.line.cmp(&b.1.start.line))
            .then(a.1.start.character.cmp(&b.1.start.character))
    });
    sites.dedup();
    sites
}

/// Where the target's own `name` is written, in the same shape as the sites
/// referring to it, so the declaration is recognisable among them.
pub(super) fn declaration_site(index: &crate::index::Index, key: &str) -> Option<(PathBuf, Range)> {
    let target = index.target(key)?;
    Some((
        target.file.to_path_buf(),
        name_range(target.line, target.character, target.length),
    ))
}

/// A name's range, from the UTF-16 columns the index recorded. A name never
/// spans a line, so its end is its start plus its length.
fn name_range(line: u32, character: u32, length: u32) -> Range {
    Range {
        start: Position { line, character },
        end: Position {
            line,
            character: character + length,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::fixture::{Open, fixture_root};
    use crate::handlers::{document_highlight, prepare_rename, references, rename};
    use starlark_cst::{Dialect, parse};

    /// A path a client can parse, for every path a filesystem allows.
    ///
    /// `path.display()` is raw, so a space produced `file:///ws with space/…`,
    /// which fluent-uri rejects; the request then failed and, before the loop
    /// caught its own errors, took the server with it.
    #[test]
    fn file_uri_escapes_what_a_uri_cannot_hold() {
        let cases = [
            ("/ws/lib/BUILD.bazel", "file:///ws/lib/BUILD.bazel"),
            ("/ws with space/BUILD", "file:///ws%20with%20space/BUILD"),
            ("/ws/a#b?c/BUILD", "file:///ws/a%23b%3Fc/BUILD"),
            ("/ws/100%/BUILD", "file:///ws/100%25/BUILD"),
        ];
        for (path, expected) in cases {
            let uri = file_uri(Path::new(path)).expect("a parseable uri");
            assert_eq!(uri.as_str(), expected, "encoding {path}");
        }
    }

    /// `name` identifies a declaration only at the top level. Nested, it is an
    /// argument that happens to share the spelling, and counting it as a
    /// declaration would hang every reference off the wrong label.
    #[test]
    fn only_a_top_level_name_declares_a_target() {
        let role = |text: &str, needle: &str| {
            let offset = u32::try_from(text.find(needle).expect("needle") + 1).unwrap();
            string_at(
                &parse(text, Dialect::Bazel).syntax(),
                offset,
                FileKind::Build,
            )
            .map(|found| found.role)
        };

        assert!(matches!(
            role("filegroup(name = \"srcs\")\n", "srcs"),
            Some(StringRole::TargetName)
        ));
        // An argument called `name` on a call nested inside another.
        assert!(matches!(
            role("filegroup(srcs = glob(name = \"inner\"))\n", "inner"),
            Some(StringRole::Label)
        ));
        // Any other attribute, at the top level.
        assert!(matches!(
            role("filegroup(srcs = [\"//lib:a\"])\n", "//lib:a"),
            Some(StringRole::Label)
        ));
    }

    #[test]
    fn a_package_is_the_nearest_build_file() {
        let root = fixture_root();
        assert_eq!(
            enclosing_package(&root, &root.join("lib/BUILD.bazel")).as_deref(),
            Some("lib")
        );
        assert_eq!(
            enclosing_package(&root, &root.join("lib/sub/BUILD.bazel")).as_deref(),
            Some("lib/sub")
        );
        // `app/nested` has no BUILD file, so its contents belong to `//app`.
        assert_eq!(
            enclosing_package(&root, &root.join("app/nested/data.txt")).as_deref(),
            Some("app")
        );
        // The torture root has a MODULE.bazel and no BUILD file, so it is not
        // a package at all.
        assert_eq!(enclosing_package(&root, &root.join("MODULE.bazel")), None);
    }

    /// `module(name = "x")` is a top-level call with a `name`, and so is every
    /// `bazel_dep`. Reading one as a target declaration attributes the cursor
    /// to whichever BUILD target happens to share the spelling — and rename
    /// then rewrites that target and every label pointing at it, which is a
    /// corrupted workspace rather than a missing answer.
    ///
    /// Every handler that resolves a target name has to decline here, so this
    /// covers all of them rather than the one that happened to find it.
    #[test]
    fn only_a_build_file_declares_targets() {
        let root = PathBuf::from("/ws");
        let text = "module(name = \"beacon\")\n";
        let module = crate::handlers::fixture::document("MODULE.bazel", text);
        let position = Position {
            line: 0,
            character: 15,
        };
        let index = crate::index::Index::default();

        // The same cursor in a BUILD file is a declaration, so the position is
        // right and the file kind is what decides.
        assert!(matches!(
            string_at(&parse(text, Dialect::Bazel).syntax(), 15, FileKind::Build)
                .map(|found| found.role),
            Some(StringRole::TargetName)
        ));
        assert!(
            string_at(&parse(text, Dialect::Bazel).syntax(), 15, FileKind::Module).is_none(),
            "a module's name is neither a declaration nor a label"
        );

        // `/ws/MODULE.bazel` classifies as a module, so every handler that
        // resolves a target name declines on it.
        assert!(references(&module, &root, &index, position, true).is_empty());
        assert!(document_highlight(&module, &root, &index, position).is_empty());
        assert!(prepare_rename(&module, &root, &index, position).is_none());
        assert!(
            rename(&module, &root, &index, &Open::none(), position, "renamed")
                .expect("a legal name")
                .is_none()
        );
    }
}
