//! `textDocument/rename` and `textDocument/prepareRename`: renaming a target
//! and every label that names it.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, bail};
use lsp_types::{Position, Range, TextEdit, Uri, WorkspaceEdit};

use super::cursor::{enclosing_package, file_uri, name_sites, string_at, target_label};
use crate::document::Document;

/// The punctuation Bazel allows in a target name, alongside `a-zA-Z0-9`.
const NAME_PUNCTUATION: &str = "!%-@^_\"#$&'()*-+,;<=>?[]{|}~/.";

/// Refuse a name Bazel could not load.
///
/// A rename that writes an illegal name breaks every file that mentioned the
/// target, which the user discovers at their next build. An error the editor
/// shows is the one outcome they can act on.
fn validate_name(name: &str) -> Result<()> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || NAME_PUNCTUATION.contains(c);
    if !name.is_empty()
        && !name.starts_with('/')
        && !name.ends_with('/')
        && name.chars().all(allowed)
    {
        return Ok(());
    }
    bail!(
        "{name:?} is not a Bazel target name: a name holds a-zA-Z0-9 and {NAME_PUNCTUATION}, \
         has no whitespace, has at least one character, and neither starts nor ends with /"
    )
}

/// Rename the target under the cursor, rewriting every label that names it.
///
/// The cursor may be on a label (`"//lib:srcs"`, `":srcs"`) or on the `name` of
/// the rule declaring it; both rename the same target. Only the name is
/// rewritten: `":srcs"` becomes `":sources"` and `"//lib:srcs"` becomes
/// `"//lib:sources"`, package and colon as the author wrote them.
///
/// **As complete as the index is, in two ways a caller must not paper over.**
/// External repositories are not searched, because resolving `@repo//…` needs
/// the repo mapping only Bazel can produce. And the static tier cannot see
/// targets or references that legacy macros compute at evaluation time — a
/// macro emitting `deps = [name + "_lib"]` is invisible here, so a label it
/// generates keeps the old name. Both wait on the graph tier; see
/// `ROADMAP.md` G4.
///
/// A new name Bazel could not load is an error rather than an empty result,
/// because an editor shows it and a broken workspace is the worse outcome.
///
/// # Errors
///
/// When `new_name` is not a legal Bazel target name.
pub fn rename(
    document: &Document,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
    new_name: &str,
) -> Result<Option<WorkspaceEdit>> {
    validate_name(new_name)?;

    let Some((_, key)) = renameable(document, root, index, position) else {
        return Ok(None);
    };

    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for (path, range) in name_sites(index, &key, true) {
        let Some(uri) = file_uri(&path) else { continue };
        changes.entry(uri).or_default().push(TextEdit {
            range,
            new_text: new_name.to_string(),
        });
    }

    tracing::debug!(label = key, files = changes.len(), "rename");
    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }))
}

/// The range `textDocument/rename` would replace under the cursor, or nothing
/// where there is no target to rename.
///
/// It is the name alone — `srcs` out of `"//lib:srcs"` — so an editor seeds
/// its prompt with the name the user is changing. Declining tells the editor
/// not to offer a rename that would come back empty.
#[must_use]
pub fn prepare_rename(
    document: &Document,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Option<Range> {
    let (name, _) = renameable(document, root, index, position)?;
    let lines = document.line_index();
    Some(Range {
        start: lines.position(document.text(), name.start as usize),
        end: lines.position(document.text(), name.end as usize),
    })
}

/// The name under the cursor as a byte range, and the target it renames.
///
/// Only a declared target can be renamed. A label naming a source file, an
/// output file or nothing at all has no declaration to rewrite, and rewriting
/// the labels alone would point every one of them at a target that does not
/// exist — invariant 4.
fn renameable(
    document: &Document,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Option<(std::ops::Range<u32>, String)> {
    let offset = u32::try_from(document.offset(position)).ok()?;
    let found = string_at(&document.parse().syntax(), offset, document.kind())?;
    let label = target_label(&found, enclosing_package(root, document.path()).as_deref())?;

    let key = label.key();
    if index.target(&key).is_none() {
        tracing::debug!(
            label = key,
            "no such target in the static index, so there is no declaration to rename; \
             legacy macros and external repositories need the graph tier"
        );
        return None;
    }

    let name_offset = u32::try_from(label.name_offset(&found.value)).ok()?;
    Some((found.range.start + name_offset..found.range.end, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_index::LineIndex;
    use std::path::PathBuf;

    const LIB: &str = r#"filegroup(
    name = "srcs",
    srcs = ["a.txt"],
)

alias(
    name = "aliased",
    actual = ":srcs",
)
"#;

    const APP: &str = r#"filegroup(
    name = "app_srcs",
    srcs = ["//lib:srcs"],
)
"#;

    /// A workspace on disk, so a rename can be indexed, applied, and the
    /// result compared to text in full.
    struct Renaming {
        root: PathBuf,
        index: crate::index::Index,
    }

    impl Renaming {
        /// `//lib:srcs`: declared in `lib`, named relatively from `lib` and
        /// absolutely from `app`.
        fn workspace(name: &str) -> Self {
            let root = std::env::temp_dir().join(name);
            std::fs::remove_dir_all(&root).ok();
            for (relative, text) in [("lib/BUILD.bazel", LIB), ("app/BUILD.bazel", APP)] {
                let path = root.join(relative);
                std::fs::create_dir_all(path.parent().expect("a package directory")).unwrap();
                std::fs::write(path, text).unwrap();
            }
            let index = crate::index::build_static(&root);
            Self { root, index }
        }

        /// The document, and the cursor in the middle of `needle`.
        fn cursor(&self, relative: &str, needle: &str) -> (Document, Position) {
            let file = self.root.join(relative);
            let text = std::fs::read_to_string(&file).expect("fixture file");
            let at = text
                .find(needle)
                .unwrap_or_else(|| panic!("{needle:?} is not in {relative}"))
                + needle.len() / 2;
            let document = Document::new(file, text, Some(&self.root));
            let position = document.position(at);
            (document, position)
        }

        fn rename(
            &self,
            relative: &str,
            needle: &str,
            new_name: &str,
        ) -> Result<Option<WorkspaceEdit>> {
            let (document, position) = self.cursor(relative, needle);
            rename(&document, &self.root, &self.index, position, new_name)
        }

        /// The text `prepareRename` would put in the editor's prompt.
        fn prepared(&self, relative: &str, needle: &str) -> Option<String> {
            let (document, position) = self.cursor(relative, needle);
            let range = prepare_rename(&document, &self.root, &self.index, position)?;
            let text = document.text();
            let lines = document.line_index();
            Some(text[lines.offset(text, range.start)..lines.offset(text, range.end)].to_string())
        }

        /// One file, as an editor applying the edits would leave it.
        fn applied(&self, edit: &WorkspaceEdit, relative: &str) -> String {
            let path = self.root.join(relative);
            let text = std::fs::read_to_string(&path).expect("fixture file");
            let uri = file_uri(&path).expect("a uri");
            let mut edits = edit
                .changes
                .as_ref()
                .and_then(|changes| changes.get(&uri))
                .cloned()
                .unwrap_or_default();
            edits.sort_by_key(|edit| (edit.range.start.line, edit.range.start.character));

            let lines = LineIndex::new(&text);
            let mut applied = text.clone();
            // Back to front, so an applied edit does not move the next one.
            for edit in edits.iter().rev() {
                let start = lines.offset(&text, edit.range.start);
                let end = lines.offset(&text, edit.range.end);
                applied.replace_range(start..end, &edit.new_text);
            }
            applied
        }
    }

    impl Drop for Renaming {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    /// From the declaration and from a label pointing at it, the answer is the
    /// same target and so are the edits.
    #[test]
    fn rename_agrees_from_either_end() {
        let workspace = Renaming::workspace("bls-rename-either-end");
        let from_declaration = workspace
            .rename("lib/BUILD.bazel", "\"srcs\"", "sources")
            .expect("a legal name");
        assert!(from_declaration.is_some());

        for (file, needle) in [
            ("lib/BUILD.bazel", "\":srcs\""),
            ("app/BUILD.bazel", "\"//lib:srcs\""),
        ] {
            assert_eq!(
                workspace
                    .rename(file, needle, "sources")
                    .expect("a legal name"),
                from_declaration,
                "renaming from {needle} in {file}"
            );
        }
    }

    /// Applied, the edits change the name and nothing else: `//lib:` keeps its
    /// package, `:srcs` keeps its colon, and the declaration moves with them.
    #[test]
    fn rename_rewrites_the_name_within_every_label() {
        let workspace = Renaming::workspace("bls-rename-applied");
        let edit = workspace
            .rename("lib/BUILD.bazel", "\"srcs\"", "sources")
            .expect("a legal name")
            .expect("a declared target");

        assert_eq!(
            workspace.applied(&edit, "lib/BUILD.bazel"),
            LIB.replace("name = \"srcs\"", "name = \"sources\"")
                .replace("actual = \":srcs\"", "actual = \":sources\"")
        );
        assert_eq!(
            workspace.applied(&edit, "app/BUILD.bazel"),
            APP.replace("\"//lib:srcs\"", "\"//lib:sources\"")
        );
    }

    /// Nothing refers to `//lib:aliased`, and it still renames: the
    /// declaration is a site like any other.
    #[test]
    fn a_target_with_no_references_renames_its_declaration() {
        let workspace = Renaming::workspace("bls-rename-unreferenced");
        let edit = workspace
            .rename("lib/BUILD.bazel", "\"aliased\"", "alias_target")
            .expect("a legal name")
            .expect("a declared target");

        let changes = edit.changes.as_ref().expect("changes");
        assert_eq!(changes.len(), 1, "one file, got {changes:?}");
        assert_eq!(
            workspace.applied(&edit, "lib/BUILD.bazel"),
            LIB.replace("\"aliased\"", "\"alias_target\"")
        );
    }

    /// A name Bazel could not load is refused rather than written: the user
    /// sees the error, instead of a workspace that stopped building.
    #[test]
    fn an_illegal_new_name_is_refused() {
        let workspace = Renaming::workspace("bls-rename-illegal-name");
        for illegal in [
            "",
            "two words",
            "tab\there",
            "/leading",
            "trailing/",
            "back\\slash",
        ] {
            assert!(
                workspace
                    .rename("lib/BUILD.bazel", "\"srcs\"", illegal)
                    .is_err(),
                "accepted {illegal:?}"
            );
        }
        // Bazel's alphabet is wider than an identifier's, and refusing a name
        // Bazel would take is its own kind of wrong.
        for legal in ["sources", "sub/dir.txt", "a+b", "v1.2.3-rc1"] {
            assert!(
                workspace
                    .rename("lib/BUILD.bazel", "\"srcs\"", legal)
                    .is_ok(),
                "refused {legal:?}"
            );
        }
    }

    /// Only a declared target can be renamed. `a.txt` is a source file: it has
    /// no declaration to rewrite, and rewriting the labels alone would point
    /// them all at nothing.
    #[test]
    fn a_label_naming_no_declaration_renames_nothing() {
        let workspace = Renaming::workspace("bls-rename-undeclared");
        assert_eq!(
            workspace
                .rename("lib/BUILD.bazel", "\"a.txt\"", "b.txt")
                .expect("a legal name"),
            None
        );
        assert_eq!(workspace.prepared("lib/BUILD.bazel", "\"a.txt\""), None);
    }

    /// The prompt an editor opens is seeded with the name, not the label that
    /// carries it.
    #[test]
    fn prepare_rename_selects_the_name_alone() {
        let workspace = Renaming::workspace("bls-prepare-rename");
        assert_eq!(
            workspace.prepared("app/BUILD.bazel", "\"//lib:srcs\""),
            Some("srcs".to_string())
        );
        assert_eq!(
            workspace.prepared("lib/BUILD.bazel", "\":srcs\""),
            Some("srcs".to_string())
        );
        assert_eq!(
            workspace.prepared("lib/BUILD.bazel", "\"srcs\""),
            Some("srcs".to_string())
        );
    }
}
