//! The target index.
//!
//! Two tiers:
//!
//! - **static** — every target that can be seen by parsing BUILD files. Cheap:
//!   measured at 219 MB/s and 69 bytes per target, so ~1.4 s and ~13 MB for a
//!   74k-package repo. Rebuilt outright rather than incrementally.
//! - **graph** — targets only Bazel knows about, because legacy macros compute
//!   names at evaluation time. Not implemented yet.
//!
//! Readers never block. An [`IndexHandle`] hands out an [`Arc<Index>`] snapshot
//! that stays consistent for the life of a request while a writer swaps in a
//! freshly built one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use lsp_types::Position;
use rustc_hash::FxHashMap;
use starlark_cst::ast::{AstNode, CallExpr, Expr, File, LiteralExpr, Stmt};
use starlark_cst::{Dialect, SyntaxElement, SyntaxKind, TextRange, classify, parse};

use crate::label::{Label, make_variable_labels, parse_label};
use crate::line_index::utf16_len;

/// Where a target was declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Package-relative name, without the leading colon.
    pub name: Box<str>,
    /// The rule or macro that declared it, as written at the call site.
    pub rule: Box<str>,
    pub file: FileId,
    /// Byte offset of the declaring call within its file.
    pub offset: u32,
    /// Zero-based line of the target's name, and its column in UTF-16 code
    /// units — the encoding LSP positions use.
    ///
    /// Resolved here rather than on demand because the alternative is re-reading
    /// and re-scanning the file for every symbol a picker displays.
    pub line: u32,
    pub character: u32,
    /// The name's width in UTF-16 code units, so the name has a range and not
    /// just a start. A name never spans a line.
    pub length: u32,
}

/// Where a label was written, somewhere other than its own declaration.
///
/// The position is the *name* inside the label rather than the start of it:
/// `//lib:srcs` points at `srcs`, which is the span a rename replaces and the
/// span worth highlighting in a list of referrers.
///
/// Ordered by position so a list of these sorts into the order a reader would
/// read them in, which is also the order that keeps a client's list stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub file: FileId,
    /// Zero-based line of the name within the label string, and its column in
    /// UTF-16 code units — the same convention as [`Target`].
    pub line: u32,
    pub character: u32,
    /// The name's width in UTF-16 code units.
    pub length: u32,
}

/// Index into [`Index::files`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// An immutable snapshot. Cheap to clone behind an `Arc`; never mutated in
/// place.
#[derive(Debug, Default)]
pub struct Index {
    pub files: Vec<PathBuf>,
    /// Keyed by `//package:name`, the form a label resolves to.
    pub targets: FxHashMap<String, Target>,
    /// Every mention of a label in a rule-call argument, under the same key as
    /// [`Index::targets`]. A key here need not be a key there: a label may name
    /// a source file, an output file, or nothing at all.
    pub references: FxHashMap<String, Vec<Reference>>,
    /// Whether the graph tier has run. False means the counts undercount, and
    /// callers must say so rather than imply completeness.
    pub graph_loaded: bool,
}

impl Index {
    #[must_use]
    pub fn target(&self, label: &str) -> Option<&Target> {
        self.targets.get(label)
    }

    /// Every recorded mention of `label`, in source order per file.
    #[must_use]
    pub fn references(&self, label: &str) -> &[Reference] {
        self.references.get(label).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn path(&self, id: FileId) -> Option<&Path> {
        self.files.get(id.0 as usize).map(PathBuf::as_path)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }
}

/// Publishes index snapshots. Clone freely; all clones share one slot.
#[derive(Debug, Clone)]
pub struct IndexHandle(Arc<ArcSwap<Index>>);

impl Default for IndexHandle {
    fn default() -> Self {
        Self(Arc::new(ArcSwap::from_pointee(Index::default())))
    }
}

impl IndexHandle {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A consistent view for the life of a request. Unaffected by later swaps.
    #[must_use]
    pub fn load(&self) -> Arc<Index> {
        self.0.load_full()
    }

    pub fn store(&self, index: Index) {
        self.0.store(Arc::new(index));
    }
}

/// The package a BUILD file defines, as a workspace-relative directory:
/// `lib/sub`, and the empty string at the root.
///
/// This is the form [`parse_label`] resolves relative labels against, so a
/// declaration and a reference in the same file agree on their key by
/// construction rather than by two spellings kept in step.
fn package_dir(root: &Path, build_file: &Path) -> Option<String> {
    let dir = build_file.parent()?;
    let rel = dir.strip_prefix(root).ok()?;
    Some(rel.to_str()?.replace('\\', "/"))
}

/// Directories that must never be walked.
///
/// The `bazel-*` convenience symlinks point into the output base, and following
/// them re-enters the source tree through the execroot symlink forest.
///
/// They only ever exist directly in the workspace root, so the check is
/// depth-scoped. Matching the name at any depth would also discard a workspace
/// whose own directory is called something like `bazel-tools` — the root is
/// depth 0 — and take every target with it.
fn is_excluded(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if name == ".git" || name == ".jj" {
        return true;
    }
    entry.depth() == 1 && name.starts_with("bazel-")
}

/// Workspace-relative directories listed in `.bazelignore`.
///
/// Bazel does not load packages under these, so neither may we: indexing them
/// invents targets that no label can resolve to. One path per line, relative to
/// the root, `#` for comments, and no wildcards — Bazel matches literally.
fn read_bazelignore(root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(root.join(".bazelignore")) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.trim_end_matches('/').replace('\\', "/"))
        .collect()
}

fn is_ignored(root: &Path, entry: &walkdir::DirEntry, ignored: &[String]) -> bool {
    if ignored.is_empty() {
        return false;
    }
    let Ok(rel) = entry.path().strip_prefix(root) else {
        return false;
    };
    let rel = rel.to_string_lossy().replace('\\', "/");
    ignored.contains(&rel)
}

/// Build the static tier by parsing every BUILD file under `root`.
///
/// Targets declared by legacy macros are invisible here by construction — the
/// names are computed at evaluation time, and so are the labels that refer to
/// them. That is what the graph tier is for; both tables undercount until it
/// lands.
#[must_use]
pub fn build_static(root: &Path) -> Index {
    let mut index = Index::default();
    let ignored = read_bazelignore(root);

    let walk = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_excluded(e) && !is_ignored(root, e, &ignored));

    for entry in walk.filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let Some((dialect, kind)) = classify(path, Some(root)) else {
            continue;
        };
        if !matches!(kind, starlark_cst::FileKind::Build) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Some(package) = package_dir(root, path) else {
            continue;
        };

        #[allow(clippy::cast_possible_truncation)]
        let file = FileId(index.files.len() as u32);
        index.files.push(path.to_path_buf());
        collect(
            &text,
            dialect,
            file,
            &package,
            &mut index.targets,
            &mut index.references,
        );
    }

    tracing::info!(
        files = index.files.len(),
        targets = index.targets.len(),
        labels_referenced = index.references.len(),
        "built static index"
    );
    index
}

/// Everything one BUILD file contributes: the targets it declares and the
/// labels it names.
///
/// Both come out of the same parse and the same walk of top-level calls. A
/// second traversal would double the only cost this tier has.
fn collect(
    text: &str,
    dialect: Dialect,
    file: FileId,
    package: &str,
    targets: &mut FxHashMap<String, Target>,
    references: &mut FxHashMap<String, Vec<Reference>>,
) {
    let Some(root) = File::cast(parse(text, dialect).syntax()) else {
        return;
    };
    let lines = crate::line_index::LineIndex::new(text);

    for stmt in root.stmts() {
        let Stmt::Expr(expr) = stmt else { continue };
        let Some(Expr::Call(call)) = expr.expr() else {
            continue;
        };

        if let (Some(rule), Some(Expr::Literal(name))) = (call.callee_name(), call.arg("name"))
            && let Some(value) = name.string_value()
        {
            let label = Label {
                package: package.to_string(),
                name: value.clone(),
            };
            // Point at the name string, not the call: jumping to `cc_library(`
            // is less useful than landing on the target you searched for.
            let anchor = name
                .string_value_range()
                .map_or_else(|| call.range().start(), TextRange::start);
            let Position { line, character } = lines.position(text, usize::from(anchor));
            let length = utf16_len(&value);
            targets.insert(
                label.key(),
                Target {
                    name: value.into_boxed_str(),
                    rule: rule.into_boxed_str(),
                    file,
                    offset: u32::from(call.range().start()),
                    line,
                    character,
                    length,
                },
            );
        }

        for (raw, anchor) in label_strings(&call) {
            let Some(label) = parse_label(&raw, Some(package)) else {
                continue;
            };
            // The name, not the whole label: a rename rewrites `srcs` and
            // leaves `//lib:` where the author put it.
            let Position { line, character } =
                lines.position(text, anchor + label.name_offset(&raw));
            references.entry(label.key()).or_default().push(Reference {
                file,
                line,
                character,
                length: utf16_len(&label.name),
            });
        }
    }
}

/// Every string literal in a rule call's arguments, with the byte offset of
/// its content.
///
/// The whole argument subtree, because labels hide in nested structure:
/// `srcs = [...]`, the keys of a `select({...})`, and the arguments of a
/// `glob(...)`. The call's own `name` is excluded — that string declares the
/// target rather than referring to one, and recording it would make every
/// target a reference to itself.
fn label_strings(call: &CallExpr) -> impl Iterator<Item = (String, usize)> + use<> {
    let declared = call.arg("name").map(|name| name.range());
    call.arg_list()
        .into_iter()
        .flat_map(|args| args.syntax().descendants_with_tokens())
        .filter_map(SyntaxElement::into_token)
        .filter(move |token| {
            token.kind() == SyntaxKind::STRING
                && declared.is_none_or(|range| !range.contains_range(token.text_range()))
        })
        .filter_map(|token| {
            let literal = LiteralExpr::cast(token.parent()?)?;
            Some((
                literal.string_value()?,
                usize::from(literal.string_value_range()?.start()),
            ))
        })
        .flat_map(|(value, start)| {
            // A whole string may be a label, and a string may also *contain*
            // labels inside make-variable expansions. Both count: a rename that
            // rewrites `data = [":beacon"]` and leaves
            // `args = ["$(rootpath :beacon)"]` alone produces a workspace that
            // does not build, which is worse than renaming nothing.
            let expansions: Vec<_> = make_variable_labels(&value)
                .map(|(label, offset)| (label, start + offset))
                .collect();
            std::iter::once((value, start)).chain(expansions)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_hide_inside_make_variables() {
        let found = |text: &str| make_variable_labels(text).collect::<Vec<_>>();

        let text = "cat $(location :srcs) > $@";
        assert_eq!(found(text), vec![(":srcs".to_string(), 15)]);
        assert_eq!(&text[15..15 + 5], ":srcs");

        assert_eq!(
            found("$(rootpath //app:bin) $(locations :data)"),
            vec![("//app:bin".to_string(), 11), (":data".to_string(), 34)]
        );
        // Padding shifts the offset; the label still has to be sliceable.
        let padded = "$(location   :srcs )";
        let (label, offset) = found(padded).pop().expect("a label");
        assert_eq!(&padded[offset..offset + label.len()], ":srcs");

        // `$(BINDIR)` and friends take no label, and neither does bare `$@`.
        assert!(found("$(BINDIR)/out $@ $$(cat x)").is_empty());
    }

    #[test]
    fn snapshots_are_stable_across_swaps() {
        let handle = IndexHandle::new();
        let before = handle.load();

        let mut next = Index::default();
        next.targets.insert(
            "//lib:srcs".into(),
            Target {
                name: "srcs".into(),
                rule: "filegroup".into(),
                file: FileId(0),
                offset: 0,
                line: 0,
                character: 0,
                length: 4,
            },
        );
        handle.store(next);

        // The reader that already had a snapshot is unaffected.
        assert_eq!(before.len(), 0);
        assert_eq!(handle.load().len(), 1);
    }

    /// The `bazel-<workspace>` convenience symlink points at the execroot,
    /// whose symlink forest re-enters the source tree. Following it finds
    /// 94,118 BUILD files in a tree that has 74,001, and neither walkdir's
    /// ancestor-loop detection nor rust-analyzer's `path_might_be_cyclic`
    /// notices — rust-analyzer walks Bazel workspaces twice.
    #[test]
    fn bazel_symlinks_are_not_followed() {
        let root = std::env::temp_dir().join("bls-symlink-test");
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(root.join("MODULE.bazel"), "module(name='t')\n").unwrap();
        std::fs::write(
            root.join("lib/BUILD.bazel"),
            "filegroup(name = \"srcs\", srcs = [])\n",
        )
        .unwrap();

        // The execroot link: points back at the tree it lives in. Caught by
        // `follow_links(false)`.
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("bazel-t")).unwrap();

        // A real `bazel-out` directory holding generated BUILD files. Nothing
        // about symlinks helps here; only the `bazel-*` prune keeps these out,
        // and indexing them would invent targets that are not in the source.
        std::fs::create_dir_all(root.join("bazel-out/k8-fastbuild/gen")).unwrap();
        std::fs::write(
            root.join("bazel-out/k8-fastbuild/gen/BUILD.bazel"),
            "filegroup(name = \"generated\", srcs = [])\n",
        )
        .unwrap();

        let index = build_static(&root);
        assert_eq!(index.len(), 1, "each target must appear exactly once");
        assert!(index.target("//lib:srcs").is_some());
        assert!(
            index
                .targets
                .keys()
                .all(|label| !label.contains("bazel-out")),
            "generated output must not be indexed: {:?}",
            index.targets.keys().collect::<Vec<_>>()
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// A workspace may legitimately be named `bazel-something`. Matching the
    /// convenience-symlink prefix at any depth prunes the root itself and the
    /// index comes back empty, with nothing to suggest why.
    #[test]
    fn a_workspace_named_bazel_something_still_indexes() {
        let root = std::env::temp_dir().join("bazel-named-workspace-test");
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::create_dir_all(root.join("tools/bazel-helpers")).unwrap();
        std::fs::write(root.join("MODULE.bazel"), "module(name='t')\n").unwrap();
        for dir in ["lib", "tools/bazel-helpers"] {
            std::fs::write(
                root.join(dir).join("BUILD.bazel"),
                "filegroup(name = \"t\", srcs = [])\n",
            )
            .unwrap();
        }

        let index = build_static(&root);
        let mut labels: Vec<_> = index.targets.keys().cloned().collect();
        labels.sort();
        assert_eq!(
            labels,
            vec!["//lib:t".to_string(), "//tools/bazel-helpers:t".to_string()],
            "the root and a nested bazel-* directory are both real source"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Bazel refuses to load packages under `.bazelignore`, so a target found
    /// there is one no label can resolve to. Offering it is worse than missing
    /// it — invariant 4.
    #[test]
    fn bazelignore_directories_are_skipped() {
        let root = std::env::temp_dir().join("bls-bazelignore-test");
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::create_dir_all(root.join("broken/nested")).unwrap();
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        std::fs::write(root.join("MODULE.bazel"), "module(name='t')\n").unwrap();
        std::fs::write(
            root.join(".bazelignore"),
            "# a comment\n\nbroken\nvendor/\n",
        )
        .unwrap();
        for dir in ["lib", "broken", "broken/nested", "vendor"] {
            std::fs::write(
                root.join(dir).join("BUILD.bazel"),
                "filegroup(name = \"t\", srcs = [])\n",
            )
            .unwrap();
        }

        let index = build_static(&root);
        let labels: Vec<_> = index.targets.keys().cloned().collect();
        assert_eq!(labels, vec!["//lib:t".to_string()], "got {labels:?}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn package_dirs() {
        let root = Path::new("/ws");
        assert_eq!(
            package_dir(root, Path::new("/ws/lib/BUILD.bazel")).as_deref(),
            Some("lib")
        );
        // The root package's directory is the empty string, which `Label::key`
        // renders as `//:name`.
        assert_eq!(
            package_dir(root, Path::new("/ws/BUILD.bazel")).as_deref(),
            Some("")
        );
    }

    /// One file's references as `label -> [(line, character, length), ...]`.
    type Refs = Vec<(String, Vec<(u32, u32, u32)>)>;

    /// Collect one file's references the way `build_static` does, sorted by
    /// label.
    fn refs(text: &str, package: &str) -> Refs {
        let mut targets = FxHashMap::default();
        let mut references = FxHashMap::default();
        collect(
            text,
            Dialect::Bazel,
            FileId(0),
            package,
            &mut targets,
            &mut references,
        );
        let mut found: Vec<_> = references
            .into_iter()
            .map(|(label, at)| {
                (
                    label,
                    at.iter()
                        .map(|r| (r.line, r.character, r.length))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn labels_in_arguments_are_references() {
        let found = refs(
            "filegroup(\n    name = \"b\",\n    srcs = [\"//lib:a\", \":c\"],\n)\n",
            "app",
        );
        assert_eq!(
            found,
            vec![
                // Relative, resolved against the enclosing package.
                ("//app:c".to_string(), vec![(2, 25, 1)]),
                // Absolute, into another package.
                ("//lib:a".to_string(), vec![(2, 19, 1)]),
            ]
        );
    }

    /// The range must cover the name and nothing else: it is what a rename
    /// replaces, so `//lib:` and the quotes around it have to stay put.
    #[test]
    fn a_reference_spans_the_name_inside_the_label() {
        let text = "filegroup(name = \"b\", srcs = [\"//lib:a\"])\n";
        let found = refs(text, "app");
        let (_, at, length) = found[0].1[0];
        let at = at as usize;
        assert_eq!(&text[at..at + length as usize], "a");
    }

    #[test]
    fn the_same_label_twice_in_one_file_is_two_references() {
        let found = refs(
            "filegroup(\n    name = \"b\",\n    srcs = [\":a\"],\n)\n\nalias(\n    name = \"c\",\n    actual = \":a\",\n)\n",
            "lib",
        );
        assert_eq!(
            found,
            vec![("//lib:a".to_string(), vec![(2, 14, 1), (7, 15, 1)])]
        );
    }

    /// A declaration is not a reference to itself. `includeDeclaration` is the
    /// caller's choice, and it cannot unmake one recorded here.
    #[test]
    fn a_targets_own_name_is_not_a_reference() {
        assert!(refs("filegroup(name = \"a\", srcs = [])\n", "lib").is_empty());
    }

    /// Everything `parse_label` refuses: an external repo whose canonical name
    /// only Bazel knows, a pattern that names a set, and prose that merely sits
    /// in a string.
    #[test]
    fn non_labels_are_not_recorded() {
        let found = refs(
            "genrule(\n    name = \"g\",\n    srcs = [\"@platforms//os:linux\", \"//lib:all\", \"//lib/...\"],\n    outs = [],\n)\n",
            "lib",
        );
        assert!(found.is_empty(), "got {found:?}");
    }

    /// A `cmd` is prose, so a whole-string parse rightly refuses it — but the
    /// label inside `$(location …)` is one Bazel resolves, and a rename that
    /// skipped it would leave the command pointing at a target that no longer
    /// exists.
    #[test]
    fn a_label_inside_a_command_is_a_reference() {
        let found = refs(
            "genrule(\n    name = \"g\",\n    cmd = \"cat $(location :srcs) > $@\",\n)\n",
            "lib",
        );
        assert_eq!(found, vec![("//lib:srcs".to_string(), vec![(2, 27, 4)])]);
    }

    /// `select()` keys, `glob()` arguments and dict values are all inside the
    /// rule call, and labels hide in all three.
    #[test]
    fn labels_nested_in_calls_and_dicts_are_found() {
        let found = refs(
            "cc_library(\n    name = \"c\",\n    deps = select({\n        \":is_linux\": [\"//lib:posix\"],\n    }),\n)\n",
            "app",
        );
        assert_eq!(
            found,
            vec![
                ("//app:is_linux".to_string(), vec![(3, 10, 8)]),
                ("//lib:posix".to_string(), vec![(3, 29, 5)]),
            ]
        );
    }

    /// A `load()` is not a rule call, and a `.bzl` module is not a target. The
    /// declared-target case is what `references` answers about.
    #[test]
    fn load_statements_are_not_references() {
        assert!(refs("load(\"//macros:defs.bzl\", \"rule\")\n", "lib").is_empty());
    }
}
