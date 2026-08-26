//! Label parsing and normalisation.
//!
//! A label is only meaningful relative to the package that writes it, so this
//! is where `":srcs"` in `//lib` becomes the `//lib:srcs` key both the target
//! table and the reference table are stored under. Everything the index and
//! the handlers agree about labels lives here, and nothing here touches Bazel.

use std::path::{Path, PathBuf};

/// A label resolved against the package that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// Workspace-relative package directory. Empty for the root package.
    pub package: String,
    /// The target name. For a source file this is a package-relative path,
    /// which is why it may contain slashes.
    pub name: String,
}

impl Label {
    /// A label naming `name` in `package`, already resolved.
    #[must_use]
    pub fn new(package: &str, name: &str) -> Self {
        Self {
            package: package.to_string(),
            name: name.to_string(),
        }
    }

    /// The index key: `//pkg:name`, and `//:name` at the root.
    #[must_use]
    pub fn key(&self) -> String {
        format!("//{}:{}", self.package, self.name)
    }

    /// Where a source file of this name would sit, relative to the workspace.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        Path::new(&self.package).join(&self.name)
    }
}

/// Normalise a label written in `package` to its absolute form.
///
/// Handled: `//pkg:target`, `//:target`, `//pkg` (shorthand for
/// `//pkg:<last component>`), `:target`, and a bare `target`. `@//pkg:target`
/// and `@@//pkg:target` name the main repository explicitly and are the same
/// thing.
///
/// Refused, because a guess here is worse than nothing: any other `@repo//` or
/// `@@canonical//` label, which needs the repo mapping only Bazel can produce,
/// and every target *pattern* — `...`, `//pkg:all`, `//pkg:*` — which names a
/// set rather than a target.
#[must_use]
pub fn parse_label(raw: &str, package: Option<&str>) -> Option<Label> {
    let raw = if let Some(rest) = raw.strip_prefix("@@").or_else(|| raw.strip_prefix('@')) {
        if !rest.starts_with("//") {
            tracing::debug!(
                label = raw,
                "external repository: the apparent name maps to a canonical one only Bazel knows, \
                 and the repo may not be fetched at all"
            );
            return None;
        }
        rest
    } else {
        raw
    };

    if raw.contains("...") {
        tracing::debug!(label = raw, "target pattern, not a label");
        return None;
    }

    let (package, name) = if let Some(rest) = raw.strip_prefix("//") {
        match rest.split_once(':') {
            Some((package, name)) => (package, name),
            // `//pkg` means `//pkg:pkg`, taking the last component.
            None => (rest, rest.rsplit('/').next()?),
        }
    } else if let Some(name) = raw.strip_prefix(':') {
        (package?, name)
    } else {
        // A bare name is relative to the current package. A colon anywhere but
        // the front does not make a label at all: `pkg:target` is not one.
        if raw.contains(':') {
            return None;
        }
        (package?, raw)
    };

    if name.is_empty() || package.starts_with('/') || package.ends_with('/') {
        return None;
    }
    if matches!(name, "all" | "*" | "all-targets") {
        tracing::debug!(label = raw, "target pattern, not a label");
        return None;
    }
    // Bazel allows a wide alphabet in target names but no whitespace, so this
    // rejects prose that merely happens to sit in a string.
    if raw.chars().any(char::is_whitespace) {
        return None;
    }

    Some(Label {
        package: package.to_string(),
        name: name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(raw: &str, package: Option<&str>) -> Option<String> {
        parse_label(raw, package).map(|label| label.key())
    }

    #[test]
    fn absolute_labels_are_already_keys() {
        assert_eq!(key("//lib:srcs", None).as_deref(), Some("//lib:srcs"));
        assert_eq!(
            key("//lib/sub:sub_srcs", None).as_deref(),
            Some("//lib/sub:sub_srcs")
        );
        // The root package: the index writes it `//:name`, not `//name`.
        assert_eq!(key("//:beacon", None).as_deref(), Some("//:beacon"));
        // A source file in a subdirectory of its package keeps the slashes.
        assert_eq!(
            key("//app:nested/data.txt", None).as_deref(),
            Some("//app:nested/data.txt")
        );
    }

    #[test]
    fn a_package_alone_names_its_last_component() {
        assert_eq!(
            key("//lib/config", None).as_deref(),
            Some("//lib/config:config")
        );
        assert_eq!(key("//lib", None).as_deref(), Some("//lib:lib"));
    }

    #[test]
    fn relative_labels_take_the_enclosing_package() {
        assert_eq!(key(":srcs", Some("lib")).as_deref(), Some("//lib:srcs"));
        assert_eq!(key("srcs", Some("lib")).as_deref(), Some("//lib:srcs"));
        assert_eq!(
            key("main.sh", Some("app/cli")).as_deref(),
            Some("//app/cli:main.sh")
        );
        assert_eq!(key(":beacon", Some("")).as_deref(), Some("//:beacon"));

        // No package means no answer. A file in no package has nothing for a
        // relative label to be relative to.
        assert_eq!(key(":srcs", None), None);
        assert_eq!(key("srcs", None), None);
    }

    #[test]
    fn the_main_repository_may_be_named_explicitly() {
        assert_eq!(key("@//lib:srcs", None).as_deref(), Some("//lib:srcs"));
        assert_eq!(key("@@//lib:srcs", None).as_deref(), Some("//lib:srcs"));
    }

    /// An apparent repo name maps to a canonical one that only `bazel mod
    /// dump_repo_mapping` knows, and the repo may not be fetched at all.
    /// Guessing `@rules_go` is `rules_go+` was already wrong once, when the
    /// format changed in Bazel 8.
    #[test]
    fn external_repositories_are_refused() {
        assert_eq!(key("@platforms//os:linux", Some("lib")), None);
        assert_eq!(
            key("@bazel_skylib//rules:write_file.bzl", Some("lib")),
            None
        );
        assert_eq!(key("@@rules_go+//go:def.bzl", Some("lib")), None);
        assert_eq!(key("@sh//:__subpackages__", Some("lib")), None);
    }

    /// A pattern names a set. Picking one of its members would be a guess, and
    /// invariant 4 rates a wrong jump worse than no jump.
    #[test]
    fn target_patterns_are_not_labels() {
        assert_eq!(key("//lib:all", None), None);
        assert_eq!(key("//lib:*", None), None);
        assert_eq!(key("//lib:all-targets", None), None);
        assert_eq!(key("//...", None), None);
        assert_eq!(key("//lib/...", None), None);
        assert_eq!(key("...", Some("lib")), None);
        assert_eq!(key(":all", Some("lib")), None);
    }

    #[test]
    fn strings_that_are_not_labels_resolve_to_nothing() {
        assert_eq!(key("", Some("lib")), None);
        assert_eq!(key(":", Some("lib")), None);
        assert_eq!(key("//", None), None);
        assert_eq!(key("//lib:", None), None);
        assert_eq!(key("///lib:srcs", None), None);
        // Prose, and a shell command with a label buried in it. Neither is one.
        assert_eq!(key("hello world", Some("lib")), None);
        assert_eq!(key("cat $(location :srcs) > $@", Some("lib")), None);
        // A relative label may not carry a package: `pkg:target` is not legal.
        assert_eq!(key("lib:srcs", Some("app")), None);
    }
}
