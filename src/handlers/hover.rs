//! `textDocument/hover`: a card saying what the string under the cursor names.

use std::path::Path;

use lsp_types::{Hover, MarkupContent, MarkupKind, Position, Range};

use super::cursor::{StringRole, enclosing_package, string_at};
use super::definition::file_site;
use crate::document::Document;
use crate::label::{Label, parse_label};
use crate::repos::Resolved;

/// What the string under the cursor names.
///
/// Three strings get a card. A label is resolved to the target it names, or to
/// the source file where no rule declares one. A target's own `name` gets the
/// same card plus how many references the index holds. A `load()` path is
/// resolved to the `.bzl` file it reads.
///
/// Everything else declines — a symbol inside a `load()`, a `name` in a file
/// that declares no targets, prose, a string that is no label at all — because
/// saying anything more would mean opening a second file and resolving a name
/// in it, and there is no symbol table here to do that with. Rule and
/// attribute documentation is the same answer: it is per-workspace, comes from
/// `--proto:rule_classes`, and waits on the graph tier. See `ROADMAP.md` G5.
///
/// The label is always shown resolved — `//lib:srcs` where the author wrote
/// `":srcs"` — which is most of what makes the card worth reading.
#[must_use]
pub fn hover(
    document: &Document,
    root: &Path,
    index: &crate::index::Index,
    position: Position,
) -> Option<Hover> {
    let text = document.text();
    let lines = document.line_index();
    let offset = u32::try_from(lines.offset(text, position)).ok()?;
    let found = string_at(&document.parse().syntax(), offset, document.kind())?;
    let package = enclosing_package(root, document.path());

    let markdown = match &found.role {
        // A load path names a file, so the index is not consulted, exactly as
        // it is not in `definition`.
        StringRole::LoadModule => {
            let label = parse_label(&found.value, package.as_deref())?;
            if let Some(elsewhere) = external_card(index, &label) {
                Some(elsewhere)
            } else {
                let site = file_site(root, &label)?;
                Some(card(
                    &label.key(),
                    &format!("Starlark file `{}`", relative(root, site.path())),
                ))
            }
        }
        StringRole::LoadSymbol(_) => None,
        StringRole::TargetName => {
            let label = Label::new(package.as_deref()?, &found.value);
            let declared = declared_card(index, root, &label)?;
            Some(format!("{declared}\n\n{}", tally(index, &label)))
        }
        StringRole::Label => {
            let label = parse_label(&found.value, package.as_deref())?;
            if let Some(elsewhere) = external_card(index, &label) {
                Some(elsewhere)
            } else {
                declared_card(index, root, &label)
                    .or_else(|| {
                        let site = file_site(root, &label)?;
                        Some(card(
                            &label.key(),
                            &format!("source file `{}`", relative(root, site.path())),
                        ))
                    })
                    .or_else(|| {
                        tracing::debug!(
                            label = label.key(),
                            "no such target in the static index and no source file at its path, so \
                         there is nothing true to say about it; legacy macros and external \
                         repositories need the graph tier"
                        );
                        None
                    })
            }
        }
    }?;

    Some(Hover {
        contents: MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }
        .into(),
        // The label alone, without its quotes, so the client underlines what
        // the card is about.
        range: Some(Range {
            start: lines.position(text, found.range.start as usize),
            end: lines.position(text, found.range.end as usize),
        }),
    })
}

/// The card for a label naming another repository, where it does not simply
/// resolve.
///
/// Which of the four states it is decides what the reader does next — fetch,
/// fix a typo, wait for Bazel, or nothing — so the card says which. An empty
/// answer here reads as a feature that was never written.
fn external_card(index: &crate::index::Index, label: &Label) -> Option<String> {
    let repo = label.repo.as_deref()?;
    let detail = match index.repos().locate(repo) {
        Resolved::Main => return None,
        Resolved::At(at) => format!(
            "`{}` in `{}`",
            label.path().display(),
            at.join(label.path()).display()
        ),
        Resolved::Unfetched(canonical) => format!(
            "`@{repo}` is `{canonical}` here and has not been fetched; \
             `bazel fetch @{repo}` brings it down"
        ),
        Resolved::Unknown => format!("`@{repo}` is not a repository this workspace declares"),
        Resolved::Unavailable => "the repository mapping has not been read, so an apparent name \
             cannot be turned into a place on disk"
            .to_string(),
    };
    Some(card(&label.key(), &detail))
}

/// A card: the resolved label, fenced, and one line saying what it is.
///
/// The fence carries no language, because its contents are a label rather than
/// code and a Starlark highlighter renders `//lib:srcs` as punctuation.
fn card(label: &str, detail: &str) -> String {
    format!("```\n{label}\n```\n{detail}")
}

/// The card for a target the index has seen declared.
fn declared_card(index: &crate::index::Index, root: &Path, label: &Label) -> Option<String> {
    let target = index.target(&label.key())?;
    let file = &target.file;
    Some(card(
        &label.key(),
        &format!("`{}` declared in `{}`", target.rule, relative(root, file)),
    ))
}

/// How many references the index holds, said as what it is.
///
/// A floor, and permanently so. The count is a property of the static index
/// rather than of the target: labels a legacy macro computes are invisible to a
/// parser, and the graph tier cannot raise the number either, because the query
/// feeding it is pruned of attributes and so carries no edges at all. The true
/// figure is this one or larger, and a bare "5 references" would be read as the
/// answer.
fn tally(index: &crate::index::Index, label: &Label) -> String {
    let counted = match index.references(&label.key()).len() {
        0 => "No references".to_string(),
        1 => "1 reference".to_string(),
        n => format!("{n} references"),
    };
    format!("{counted} in the static index, which does not see macro-generated labels")
}

/// A path as a label spells it: relative to the workspace, forward slashes.
///
/// A path from outside the workspace keeps its own spelling rather than being
/// dropped — an absolute path is still true, and a card with a hole in it is
/// not.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::fixture::Fixture;

    /// A label's card says what the label resolves to: the target, the rule
    /// that declares it, and the file that holds it.
    #[test]
    fn hover_on_a_label_says_what_it_resolves_to() {
        let fixture = Fixture::workspace();

        assert_eq!(
            fixture
                .card("lib/BUILD.bazel", "//lib/sub:sub_srcs")
                .as_deref(),
            Some(
                "```\n//lib/sub:sub_srcs\n```\n\
                 `filegroup` declared in `lib/sub/BUILD.bazel`"
            )
        );
        // Written `":srcs"`, shown resolved: the package the file is in is
        // what the reader does not have to work out.
        assert_eq!(
            fixture.card("lib/BUILD.bazel", "\":srcs\",").as_deref(),
            Some("```\n//lib:srcs\n```\n`filegroup` declared in `lib/BUILD.bazel`")
        );
        // A label naming a source file rather than a declared target says so,
        // rather than claiming a rule it does not have.
        assert_eq!(
            fixture
                .card("lib/sub/BUILD.bazel", "//lib:exported.txt")
                .as_deref(),
            Some("```\n//lib:exported.txt\n```\nsource file `lib/exported.txt`")
        );
    }

    /// An external label that cannot be placed is a limitation, not an
    /// absence, and the card has to distinguish the two: an empty answer here
    /// is what a user reads as the feature never having been written.
    ///
    /// The fixture has read no mapping, which is its own answer and not the
    /// same as the repository being unknown.
    #[test]
    fn an_external_label_says_why_it_cannot_be_placed() {
        let fixture = Fixture::workspace();
        let card = fixture
            .card("lib/BUILD.bazel", "@platforms//os:linux")
            .expect("a card naming the limitation");
        assert!(
            card.contains("repository mapping has not been read"),
            "got {card:?}"
        );
        assert!(card.contains("@platforms//os:linux"), "got {card:?}");
    }

    /// The count is a floor whatever else has loaded, so the caveat is not
    /// conditional on anything: no tier this server has carries the edges that
    /// would make the number exact.
    #[test]
    fn the_count_is_hedged_even_with_nothing_to_count() {
        let index = crate::index::Index::default();
        let label = crate::label::parse_label("//lib:srcs", None).expect("a label");
        assert_eq!(
            tally(&index, &label),
            "No references in the static index, which does not see macro-generated labels"
        );
    }

    /// On the declaration, the card carries the reference count — worded so
    /// the number is read as what the index holds and not as the truth about
    /// the target, which only the graph tier can give.
    #[test]
    fn hover_on_a_declaration_counts_what_the_index_holds() {
        let fixture = Fixture::workspace();
        assert_eq!(
            fixture.card("lib/BUILD.bazel", "\"srcs\"").as_deref(),
            Some(
                "```\n//lib:srcs\n```\n\
                 `filegroup` declared in `lib/BUILD.bazel`\n\n\
                 5 references in the static index, which does not see macro-generated labels"
            )
        );
        // Nothing names `//lib:double_aliased`, and none is not a failure.
        assert_eq!(
            fixture
                .card("lib/BUILD.bazel", "\"double_aliased\"")
                .as_deref(),
            Some(
                "```\n//lib:double_aliased\n```\n\
                 `alias` declared in `lib/BUILD.bazel`\n\n\
                 No references in the static index, which does not see macro-generated labels"
            )
        );
    }

    /// A `load()` path resolves to the file it reads, and says nothing about
    /// what is in it: that would need a second parse and a symbol table.
    #[test]
    fn hover_on_a_load_path_resolves_the_file() {
        let fixture = Fixture::workspace();
        assert_eq!(
            fixture
                .card("lib/BUILD.bazel", "//macros:legacy.bzl")
                .as_deref(),
            Some("```\n//macros:legacy.bzl\n```\nStarlark file `macros/legacy.bzl`")
        );
        // Relative to the package doing the loading.
        assert_eq!(
            fixture.card("lib/BUILD.bazel", ":local.bzl").as_deref(),
            Some("```\n//lib:local.bzl\n```\nStarlark file `lib/local.bzl`")
        );
    }

    /// Everything the index cannot answer for declines. A card reading
    /// "unknown" is a claim about the target, when the only true claim is
    /// about the index.
    ///
    /// An external label is the exception and is covered by
    /// `an_external_label_says_what_it_needs`: there the true claim about the
    /// tier is worth making, because the target does exist and only the
    /// repository mapping is missing.
    #[test]
    fn hover_declines_wherever_it_would_have_to_guess() {
        let fixture = Fixture::workspace();
        let nothing = [
            // The torture workspace's deliberately dangling label.
            ("lib/sub/BUILD.bazel", "//lib:does_not_exist"),
            // A pseudo-label that never names a target.
            ("lib/BUILD.bazel", "//visibility:public"),
            // A symbol inside a `load()`. The file it comes from is known and
            // what the symbol *is* is not, and the second is what was asked.
            ("lib/BUILD.bazel", "\"local_helper\""),
            // A cursor on an identifier, a comment, or a rule name.
            ("lib/BUILD.bazel", "filegroup("),
            ("lib/BUILD.bazel", "# Cross-package"),
            ("lib/BUILD.bazel", "cc_library_placeholder"),
        ];
        for (file, needle) in nothing {
            let found = fixture.card(file, needle);
            assert_eq!(found, None, "cursor on {needle:?} in {file}");
        }
    }

    /// Only a BUILD file declares targets. `module(name = "beacon")` names a
    /// module, and a repository with a `//:beacon` alias in it would otherwise
    /// get a card describing that alias — an answer about something the cursor
    /// is not on, which invariant 4 rates worse than no answer.
    #[test]
    fn a_name_outside_a_build_file_declares_no_target() {
        let root = std::env::temp_dir().join("bls-hover-module-name");
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let module = "module(name = \"beacon\")\n";
        std::fs::write(root.join("MODULE.bazel"), module).unwrap();
        std::fs::write(
            root.join("BUILD.bazel"),
            "alias(name = \"beacon\", actual = \"//lib:srcs\")\n",
        )
        .unwrap();
        // The same text under two names: what decides is the kind of file, and
        // the kind comes from the path, so the test has to use real ones.
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        std::fs::write(root.join("pkg/BUILD.bazel"), module).unwrap();
        let index = crate::index::Index::of_disk(crate::index::build_static(&root));
        let at = module.find("beacon").expect("the module's name");
        let card = |relative: &str| {
            let document = Document::new(root.join(relative), module.to_string(), Some(&root));
            let position = document.position(at);
            hover(&document, &root, &index, position)
        };

        assert!(card("MODULE.bazel").is_none(), "a module is not a target");
        assert!(card("pkg/BUILD.bazel").is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    /// The range is what the client highlights while the card is up. It has to
    /// be the label alone: including the quotes paints punctuation the user
    /// did not point at.
    #[test]
    fn the_hover_range_is_the_label_without_its_quotes() {
        let fixture = Fixture::workspace();
        let (document, position) = fixture.cursor("lib/BUILD.bazel", "//lib/sub:sub_srcs");
        let hovered =
            hover(&document, &fixture.root, &fixture.index, position).expect("the label resolves");

        let text = document.text();
        let lines = document.line_index();
        let range = hovered.range.expect("a range");
        let start = lines.offset(text, range.start);
        let end = lines.offset(text, range.end);
        assert_eq!(&text[start..end], "//lib/sub:sub_srcs");
    }
}
