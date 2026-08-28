//! `textDocument/definition`: where the string under the cursor is declared.

use std::path::{Path, PathBuf};

use lsp_types::{LocationLink, Position, Range};

use super::cursor::{StringRole, enclosing_package, file_uri, string_at};
use crate::document::Document;
use crate::label::{Label, parse_label};
use crate::repos::Resolved;

/// Where a definition lives, and the position to reveal in it.
pub(super) struct Site {
    path: PathBuf,
    at: Position,
}

impl Site {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

/// The declaring rule call, from the index snapshot or from the buffer holding
/// that file.
///
/// An open buffer outranks the index, which records where the target sat on
/// disk. Where the buffer no longer declares it, the answer is nothing:
/// pointing at where it used to be sends the user somewhere the target is not.
pub(super) fn target_site(index: &crate::index::Index, label: &Label) -> Option<Site> {
    let target = index.target(&label.key())?;
    // The index carries where the name starts and not where the call ends, so
    // the range is empty. Clients reveal the line either way, and re-reading
    // the file to widen it would put IO in the request path.
    Some(Site {
        path: target.file.to_path_buf(),
        at: Position {
            line: target.line,
            character: target.character,
        },
    })
}

/// The source file a label names, for the `srcs = ["main.sh"]` case.
///
/// A source file is a target in its own right, so this is a definition and not
/// a consolation prize. It is tried after the index because a rule and a source
/// file cannot share a name, and the rule is what a label with that name means.
pub(super) fn file_site(root: &Path, label: &Label) -> Option<Site> {
    let path = root.join(label.path());
    path.is_file().then_some(Site {
        path,
        at: Position {
            line: 0,
            character: 0,
        },
    })
}

/// The tree a label's package sits in: this workspace, or the repository the
/// label names.
///
/// `None` where the repository cannot be placed, which is the only thing
/// standing between a label and a wrong answer — resolving `@repo//lib:srcs`
/// against this workspace finds our `lib/srcs` and offers it, and a file from
/// the wrong repository is exactly the jump invariant 4 rules out.
pub(super) fn tree(root: &Path, index: &crate::index::Index, label: &Label) -> Option<PathBuf> {
    match label.repo.as_deref() {
        None => Some(root.to_path_buf()),
        Some(repo) => match index.repos().locate(repo) {
            Resolved::Main => Some(root.to_path_buf()),
            Resolved::At(at) => Some(at),
            Resolved::Unfetched(_) | Resolved::Unknown | Resolved::Unavailable => None,
        },
    }
}

/// The BUILD file of a label's package, wherever that package lives.
pub(super) fn package_site(tree: &Path, label: &Label) -> Option<Site> {
    ["BUILD.bazel", "BUILD"]
        .into_iter()
        .map(|name| tree.join(&label.package).join(name))
        .find(|path| path.is_file())
        .map(|path| Site {
            path,
            at: Position {
                line: 0,
                character: 0,
            },
        })
}

/// Goto-definition for the string under the cursor.
///
/// A string is read as a `load()` path, a symbol in a `load()`, or a label,
/// decided by where it sits. A label resolves to the declaring rule call in the
/// index, or failing that to the source file it names; a `load()` resolves to
/// the `.bzl` file, and so does a symbol inside one — following the symbol to
/// its own `def` is out of scope.
///
/// Main repo only. Everything is answered from the index snapshot and the
/// document text; nothing here can invoke Bazel, and an unresolvable label
/// yields nothing rather than a guess.
#[must_use]
pub fn definition(
    document: &Document,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Vec<LocationLink> {
    let text = document.text();
    let lines = document.line_index();
    let Ok(offset) = u32::try_from(lines.offset(text, position)) else {
        return Vec::new();
    };
    let Some(found) = string_at(&document.parse().syntax(), offset, document.kind()) else {
        return Vec::new();
    };

    let package = enclosing_package(root, document.path());
    let tree = |label: &Label| tree(root, index, label);
    let site =
        match &found.role {
            // A load path names a file, never a target, so the index is not
            // consulted: a rule that happened to be called `defs.bzl` is not it.
            StringRole::LoadModule => parse_label(&found.value, package.as_deref())
                .and_then(|label| file_site(&tree(&label)?, &label)),
            StringRole::LoadSymbol(module) => parse_label(module, package.as_deref())
                .and_then(|label| file_site(&tree(&label)?, &label)),
            // The cursor is on the declaration already, so there is nowhere to go.
            // Jumping to the line it is sitting on reads as the server having
            // failed. The variant earns its keep in `references`.
            StringRole::TargetName => None,
            StringRole::Label => {
                parse_label(&found.value, package.as_deref()).and_then(|label| {
                    let tree = tree(&label)?;
                    target_site(index, &label)
                .or_else(|| file_site(&tree, &label))
                // Another repository's targets are not indexed, so a label
                // naming one lands on the BUILD file that declares it: reading
                // that file for the exact line would put IO in the request
                // path, and the package is true where the line is merely
                // better. In this repository a miss is a real miss, and
                // offering the package would be a wrong jump.
                .or_else(|| label.repo.as_ref().and_then(|_| package_site(&tree, &label)))
                .or_else(|| {
                    tracing::debug!(
                        label = label.key(),
                        "no such target in the static index and no source file at its path; \
                         legacy macros and external repositories need the graph tier"
                    );
                    None
                })
                })
            }
        };

    let Some(site) = site else {
        return Vec::new();
    };
    let Some(uri) = file_uri(&site.path) else {
        return Vec::new();
    };
    let target = Range {
        start: site.at,
        end: site.at,
    };
    vec![LocationLink {
        // Highlight the label text alone, without its quotes.
        origin_selection_range: Some(Range {
            start: lines.position(text, found.range.start as usize),
            end: lines.position(text, found.range.end as usize),
        }),
        target_uri: uri,
        target_range: target,
        target_selection_range: target,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::fixture::{Fixture, fixture_root};

    /// The buffer holding a target's declaration outranks the index, which
    /// records where that declaration sat when the file was last read.
    #[test]
    fn a_moved_declaration_is_found_where_the_buffer_has_it() {
        let fixture = Fixture::workspace();
        let file = fixture.root.join("lib/BUILD.bazel");
        let shifted = format!(
            "# a line the file on disk does not have\n{}",
            std::fs::read_to_string(&file).expect("fixture file")
        );
        let label = parse_label("//lib:srcs", None).expect("a label");

        let indexed = target_site(&fixture.index, &label).expect("an indexed target");
        let editing = fixture.editing("lib/BUILD.bazel", &shifted);
        let edited = target_site(&editing, &label).expect("the buffer still declares it");

        assert_eq!(edited.at.line, indexed.at.line + 1);
    }

    /// A target the buffer has renamed away is one no position can be true
    /// about, so there is nothing to offer.
    #[test]
    fn a_declaration_the_buffer_dropped_resolves_to_nothing() {
        let fixture = Fixture::workspace();
        let file = fixture.root.join("lib/BUILD.bazel");
        let renamed = std::fs::read_to_string(&file)
            .expect("fixture file")
            .replace("\"srcs\"", "\"sources\"");
        let label = parse_label("//lib:srcs", None).expect("a label");

        let editing = fixture.editing("lib/BUILD.bazel", &renamed);
        assert!(target_site(&editing, &label).is_none());
    }

    /// Goto-definition on a declaration must not jump to the line the cursor is
    /// already on; that reads as the server having misfired.
    #[test]
    fn definition_on_a_declaration_goes_nowhere() {
        let fixture = Fixture::workspace();
        let document = fixture.open("lib/BUILD.bazel");
        let offset = document
            .text()
            .find("\"srcs\"")
            .expect("the srcs declaration")
            + 2;

        let jumps = definition(
            &document,
            &fixture.root,
            &crate::index::Index::default(),
            document.position(offset),
        );
        assert!(jumps.is_empty(), "got {jumps:?}");
    }

    /// The index reads labels out of `$(location …)`, so navigation has to as
    /// well: a label that find-references reports and go-to-definition shrugs at
    /// looks like the definition is missing rather than the reader.
    #[test]
    fn a_label_inside_a_command_is_navigable() {
        let fixture = Fixture::workspace();
        let document = fixture.open("lib/BUILD.bazel");
        let text = document.text();
        let lines = document.line_index();
        let cmd = text.find("$(location :srcs)").expect("the genrule cmd");

        let on = |offset: usize| {
            definition(
                &document,
                &fixture.root,
                &fixture.index,
                document.position(offset),
            )
        };

        // Inside the label, navigation resolves it.
        let jumps = on(cmd + "$(location :".len() + 1);
        assert_eq!(jumps.len(), 1, "got {jumps:?}");
        let origin = jumps[0].origin_selection_range.expect("an origin range");
        assert_eq!(
            &text[lines.offset(text, origin.start)..lines.offset(text, origin.end)],
            ":srcs",
            "the origin covers the label alone, not the command around it"
        );

        // On the prose around it, there is no label and nothing to offer.
        assert!(on(cmd.saturating_sub(2)).is_empty());
    }

    #[test]
    fn labels_resolve_through_the_index() {
        let fixture = Fixture::workspace();

        // An absolute label into another package.
        assert_eq!(
            fixture
                .go("lib/BUILD.bazel", "//lib/sub:sub_srcs")
                .as_deref(),
            Some("lib/sub/BUILD.bazel:3:12")
        );
        // A relative one, against the package the file is in.
        assert_eq!(
            fixture.go("lib/BUILD.bazel", ":aliased").as_deref(),
            Some("lib/BUILD.bazel:34:12")
        );
        // A label pointing back up out of a subpackage.
        assert_eq!(
            fixture
                .go("lib/sub/BUILD.bazel", "//lib:exported.txt")
                .as_deref(),
            Some("lib/exported.txt:0:0")
        );
    }

    /// `srcs = ["tool.sh"]` names a source file, and a source file is a target.
    #[test]
    fn source_files_are_definitions() {
        let fixture = Fixture::workspace();
        assert_eq!(
            fixture.go("app/BUILD.bazel", "tool.sh").as_deref(),
            Some("app/tool.sh:0:0")
        );
        // A file in a subdirectory of the package that owns it. The quotes are
        // part of the needle because a comment above names the same label, and
        // a cursor in a comment is a cursor in no string at all.
        assert_eq!(
            fixture
                .go("lib/BUILD.bazel", "\"//app:nested/data.txt\"")
                .as_deref(),
            Some("app/nested/data.txt:0:0")
        );
    }

    #[test]
    fn load_paths_and_their_symbols_reach_the_file() {
        let fixture = Fixture::workspace();
        assert_eq!(
            fixture
                .go("lib/BUILD.bazel", "//macros:legacy.bzl")
                .as_deref(),
            Some("macros/legacy.bzl:0:0")
        );
        // The symbol jumps to the file that defines it, not to the `def`.
        assert_eq!(
            fixture.go("lib/BUILD.bazel", "legacy_macro").as_deref(),
            Some("macros/legacy.bzl:0:0")
        );
        // A load path relative to the current package.
        assert_eq!(
            fixture.go("lib/BUILD.bazel", ":local.bzl").as_deref(),
            Some("lib/local.bzl:0:0")
        );
        // An aliased symbol: the string is the original name, and the alias
        // token beside it is not a string at all.
        assert_eq!(
            fixture.go("lib/BUILD.bazel", "renamed_in_load").as_deref(),
            Some("lib/local.bzl:0:0")
        );
    }

    #[test]
    fn external_labels_yield_nothing() {
        let fixture = Fixture::workspace();
        assert!(
            fixture
                .links("lib/BUILD.bazel", "@platforms//os:linux")
                .is_empty()
        );
        assert!(
            fixture
                .links("lib/BUILD.bazel", "@bazel_skylib//rules:write_file.bzl")
                .is_empty()
        );
    }

    #[test]
    fn a_label_naming_nothing_yields_nothing() {
        let fixture = Fixture::workspace();
        // The torture workspace has this deliberately dangling label.
        assert!(
            fixture
                .links("lib/sub/BUILD.bazel", "//lib:does_not_exist")
                .is_empty()
        );
        // And these are pseudo-labels that never name a target.
        assert!(
            fixture
                .links("lib/BUILD.bazel", "//visibility:public")
                .is_empty()
        );
        assert!(
            fixture
                .links("lib/BUILD.bazel", "//conditions:default")
                .is_empty()
        );
    }

    #[test]
    fn a_cursor_outside_a_string_yields_nothing() {
        let text = "filegroup(\n    name = \"srcs\",\n    srcs = [\"//lib:a\"],\n)\n";
        let root = fixture_root();
        let document = Document::new(root.join("lib/BUILD.bazel"), text.to_string(), Some(&root));
        let index = crate::index::Index::default();

        for needle in ["filegroup", "name", "srcs = [", ")"] {
            let at = text.find(needle).unwrap();
            let found = definition(&document, &root, &index, document.position(at));
            assert!(found.is_empty(), "cursor on {needle:?} found {found:?}");
        }

        // The quotes are not part of the label either.
        let quote = text.find("\"//lib:a\"").unwrap();
        assert!(definition(&document, &root, &index, document.position(quote)).is_empty());
    }

    /// The origin range is what the editor underlines. It has to be the label
    /// alone: including the quotes highlights punctuation the user did not
    /// point at.
    #[test]
    fn the_origin_range_is_the_label_without_its_quotes() {
        let fixture = Fixture::workspace();
        let link = fixture
            .links("lib/BUILD.bazel", "//lib/sub:sub_srcs")
            .into_iter()
            .next()
            .expect("the label resolves");
        let document = fixture.open("lib/BUILD.bazel");
        let text = document.text();
        let lines = document.line_index();
        let origin = link.origin_selection_range.expect("an origin range");
        let start = lines.offset(text, origin.start);
        let end = lines.offset(text, origin.end);
        assert_eq!(&text[start..end], "//lib/sub:sub_srcs");
    }
}
