//! `textDocument/codeLens`: the Bazel commands a target line affords.

use std::path::Path;

use lsp_types::{CodeLens, Command, Range};
use starlark_cst::FileKind;

use super::cursor::{classify_file, enclosing_package};

/// The command a lens runs, and what `workspace/executeCommand` answers to.
pub const RUN_COMMAND: &str = "bazel-language-server.run";

/// A lens above every target a BUILD file declares.
///
/// Build for everything, and test as well for anything whose rule name ends in
/// `_test` or is `test_suite` — running `bazel test` on a non-test target is an
/// error, so offering it there would be offering a mistake.
///
/// Only BUILD files: a `.bzl` declares no targets, and `MODULE.bazel`'s
/// top-level calls carry names that are not labels.
#[must_use]
pub fn code_lenses(text: &str, file: &Path, root: &Path) -> Vec<CodeLens> {
    let (dialect, kind) = classify_file(file, root);
    if kind != FileKind::Build {
        return Vec::new();
    }
    let Some(package) = enclosing_package(root, file) else {
        return Vec::new();
    };
    super::symbols::declarations(text, dialect, kind)
        .into_iter()
        .flat_map(|declaration| {
            let label = format!("//{package}:{}", declaration.name);
            let range = Range {
                start: declaration.full.start,
                end: declaration.full.start,
            };
            let mut lenses = vec![lens(range, "build", &label)];
            if declaration.rule.ends_with("_test") || declaration.rule == "test_suite" {
                lenses.push(lens(range, "test", &label));
            }
            lenses
        })
        .collect()
}

fn lens(range: Range, verb: &str, label: &str) -> CodeLens {
    CodeLens {
        range,
        command: Some(Command {
            title: format!("{verb} {label}"),
            command: RUN_COMMAND.to_string(),
            arguments: Some(vec![verb.into(), label.into()]),
            tooltip: None,
        }),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::Fixture;
    use super::*;

    fn titles(relative: &str) -> Vec<String> {
        let fixture = Fixture::workspace();
        let file = fixture.root.join(relative);
        let text = std::fs::read_to_string(&file).expect("fixture");
        code_lenses(&text, &file, &fixture.root)
            .into_iter()
            .filter_map(|lens| lens.command.map(|command| command.title))
            .collect()
    }

    #[test]
    fn every_target_offers_build() {
        let found = titles("lib/BUILD.bazel");
        assert!(!found.is_empty());
        assert!(
            found.iter().any(|title| title == "build //lib:srcs"),
            "{found:?}"
        );
    }

    #[test]
    fn only_a_test_target_offers_test() {
        let found = titles("lib/BUILD.bazel");
        for title in &found {
            if let Some(label) = title.strip_prefix("test ") {
                assert!(
                    label.contains("test"),
                    "test offered on a non-test target: {title}"
                );
            }
        }
    }

    #[test]
    fn a_bzl_file_declares_no_targets() {
        assert!(titles("macros/legacy.bzl").is_empty());
    }
}
