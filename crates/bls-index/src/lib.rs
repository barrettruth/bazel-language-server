//! The target index.
//!
//! Two tiers, per invariant 6 in `ROADMAP.md`:
//!
//! - **static** — every target this crate can see by parsing BUILD files. Cheap:
//!   measured at 219 MB/s and 69 bytes per target, so ~1.4 s and ~13 MB for a
//!   74k-package repo. Rebuilt outright rather than incrementally.
//! - **graph** — targets only Bazel knows about, because legacy macros compute
//!   names at evaluation time. Not implemented yet; see `ROADMAP.md` G4.
//!
//! Readers never block. An [`IndexHandle`] hands out an [`Arc<Index>`] snapshot
//! that stays consistent for the life of a request while a writer swaps in a
//! freshly built one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use rustc_hash::FxHashMap;
use starlark_cst::ast::{AstNode, Expr, File, Stmt};
use starlark_cst::{Dialect, classify, parse};

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
    /// Whether the graph tier has run. False means the counts undercount, and
    /// callers must say so rather than imply completeness.
    pub graph_loaded: bool,
}

impl Index {
    #[must_use]
    pub fn target(&self, label: &str) -> Option<&Target> {
        self.targets.get(label)
    }

    #[must_use]
    pub fn path(&self, id: FileId) -> Option<&Path> {
        self.files.get(id.0 as usize).map(PathBuf::as_path)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
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

/// Package label for a BUILD file, e.g. `//lib/sub`, relative to `root`.
fn package_label(root: &Path, build_file: &Path) -> Option<String> {
    let dir = build_file.parent()?;
    let rel = dir.strip_prefix(root).ok()?;
    let package = rel.to_str()?.replace('\\', "/");
    Some(if package.is_empty() {
        "//".to_string()
    } else {
        format!("//{package}")
    })
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
/// names are computed at evaluation time. That is what the graph tier is for.
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
        let Some(package) = package_label(root, path) else {
            continue;
        };

        #[allow(clippy::cast_possible_truncation)]
        let file = FileId(index.files.len() as u32);
        index.files.push(path.to_path_buf());
        collect_targets(&text, dialect, file, &package, &mut index.targets);
    }

    tracing::info!(
        files = index.files.len(),
        targets = index.targets.len(),
        "built static index"
    );
    index
}

fn collect_targets(
    text: &str,
    dialect: Dialect,
    file: FileId,
    package: &str,
    out: &mut FxHashMap<String, Target>,
) {
    let Some(root) = File::cast(parse(text, dialect).syntax()) else {
        return;
    };
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let position = |offset: usize| -> (u32, u32) {
        let line = line_starts.partition_point(|&s| s <= offset) - 1;
        let column: usize = text
            .get(line_starts[line]..offset)
            .map_or(0, |s| s.chars().map(char::len_utf16).sum());
        #[allow(clippy::cast_possible_truncation)]
        (line as u32, column as u32)
    };

    for stmt in root.stmts() {
        let Stmt::Expr(expr) = stmt else { continue };
        let Some(Expr::Call(call)) = expr.expr() else {
            continue;
        };
        let (Some(rule), Some(Expr::Literal(name))) = (call.callee_name(), call.arg("name")) else {
            continue;
        };
        let Some(value) = name.string_value() else {
            continue;
        };
        let label = if package == "//" {
            format!("//:{value}")
        } else {
            format!("{package}:{value}")
        };
        // Point at the name string, not the call: jumping to `cc_library(` is
        // less useful than landing on the target you searched for.
        let anchor = match name.string_value_range() {
            Some(range) => range.start(),
            None => call.range().start(),
        };
        let (line, character) = position(usize::from(anchor));
        out.insert(
            label,
            Target {
                name: value.into_boxed_str(),
                rule: rule.into_boxed_str(),
                file,
                offset: u32::from(call.range().start()),
                line,
                character,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn package_labels() {
        let root = Path::new("/ws");
        assert_eq!(
            package_label(root, Path::new("/ws/lib/BUILD.bazel")).as_deref(),
            Some("//lib")
        );
        assert_eq!(
            package_label(root, Path::new("/ws/BUILD.bazel")).as_deref(),
            Some("//")
        );
    }
}
