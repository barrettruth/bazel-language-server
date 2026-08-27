//! `textDocument/references`: every place the target under the cursor is named.

use std::path::Path;

use lsp_types::{Location, Position};

use super::cursor::{enclosing_package, file_uri, name_sites, string_at, target_label};
use crate::document::Document;

/// Every place in the main repository that names the target under the cursor.
///
/// The cursor may be on a label (`"//lib:srcs"`, `":srcs"`) or on the `name` of
/// the rule declaring it; both resolve to the same target.
///
/// **Partial by construction, in two ways a caller must not paper over.**
/// External repositories are not searched, because resolving `@repo//…` needs
/// the repo mapping only Bazel can produce. And the static tier cannot see
/// targets or references that legacy macros compute at evaluation time — a
/// macro emitting `deps = [name + "_lib"]` is invisible here. Both wait on the
/// graph tier; see `ROADMAP.md` G4.
#[must_use]
pub fn references(
    document: &Document,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
    include_declaration: bool,
) -> Vec<Location> {
    let Ok(offset) = u32::try_from(document.offset(position)) else {
        return Vec::new();
    };
    let Some(found) = string_at(&document.parse().syntax(), offset, document.kind()) else {
        return Vec::new();
    };

    let package = enclosing_package(root, document.path());
    let Some(label) = target_label(&found, package.as_deref()) else {
        return Vec::new();
    };

    let key = label.key();
    let sites = name_sites(index, &key, include_declaration);
    tracing::debug!(label = key, count = sites.len(), "references");
    sites
        .into_iter()
        .filter_map(|(path, range)| {
            Some(Location {
                uri: file_uri(&path)?,
                range,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::fixture::Fixture;

    /// From the declaration and from a label pointing at it, the answer is the
    /// same target — and `includeDeclaration` is what decides whether the
    /// declaring line is in it.
    #[test]
    fn references_agree_from_either_end() {
        let fixture = Fixture::workspace();
        let index = crate::index::Index::of_disk(crate::index::build_static(&fixture.root));
        let document = fixture.open("lib/BUILD.bazel");

        let at = |needle: &str, skip: usize| {
            let offset = document.text().find(needle).expect("needle") + skip;
            references(
                &document,
                &fixture.root,
                &index,
                document.position(offset),
                false,
            )
        };

        // `//lib:srcs` is referenced from the alias, the genrule's srcs, a
        // select() branch, a rule attribute, and inside the genrule cmd's
        // `$(location :srcs)`, which is a label a whole-string parse cannot see.
        let from_declaration = at("\"srcs\"", 2);
        assert_eq!(
            from_declaration.len(),
            5,
            "expected every referrer, got {from_declaration:?}"
        );

        // From a label pointing at it: `actual = ":srcs"`.
        let from_label = at("actual = \":srcs\"", 11);
        assert_eq!(
            from_label, from_declaration,
            "a label and its declaration name the same target"
        );

        // The declaration is a separate site, added only when asked for.
        let offset = document.text().find("\"srcs\"").expect("needle") + 2;
        let with_declaration = references(
            &document,
            &fixture.root,
            &index,
            document.position(offset),
            true,
        );
        assert_eq!(with_declaration.len(), from_declaration.len() + 1);
    }

    /// A `.bzl` path is not a target. Answering with the files that `load()` it
    /// would be a different question answered wrongly.
    #[test]
    fn a_load_path_names_no_target() {
        let fixture = Fixture::workspace();
        let index = crate::index::Index::of_disk(crate::index::build_static(&fixture.root));
        let document = fixture.open("lib/BUILD.bazel");
        let offset = document
            .text()
            .find("//macros:legacy.bzl")
            .expect("a load path")
            + 4;

        assert!(
            references(
                &document,
                &fixture.root,
                &index,
                document.position(offset),
                true,
            )
            .is_empty()
        );
        assert!(
            fixture
                .highlights("lib/BUILD.bazel", "//macros:legacy.bzl")
                .is_empty()
        );
    }
}
