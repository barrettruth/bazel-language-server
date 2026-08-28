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
//! Readers never block. An [`IndexHandle`] hands out an [`Index`] snapshot
//! that stays consistent for the life of a request while a writer swaps in a
//! freshly built one.

use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use lsp_types::Position;
use rustc_hash::{FxHashMap, FxHashSet};
use starlark_cst::ast::{AstNode, CallExpr, Expr, File, LiteralExpr, Stmt};
use starlark_cst::{Parse, SyntaxElement, SyntaxKind, TextRange, classify, parse};

use crate::label::{Label, make_variable_labels, parse_label};
use crate::line_index::utf16_len;
use crate::repos::Repos;

/// Where a target was declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Package-relative name, without the leading colon.
    pub name: Box<str>,
    /// The rule or macro that declared it, as written at the call site.
    pub rule: Box<str>,
    pub file: Arc<Path>,
    /// Zero-based line of the target's name, and its column in UTF-16 code
    /// units — the encoding LSP positions use.
    ///
    /// Resolved here rather than on demand because the alternative is re-reading
    /// and re-scanning the file for every symbol a picker displays.
    pub line: u32,
    pub character: u32,
    /// The name's width in UTF-16 code units, so the name has a range and not
    /// just a start. A name never spans a line.
    ///
    /// Zero where no name is written in the file at all, which is every target
    /// a macro computed: Bazel places those at the macro call, and there is no
    /// span there to cover.
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
/// Path first, so that order is the same from one run to the next: the walk
/// that finds these visits directories in whatever order the filesystem hands
/// back.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub file: Arc<Path>,
    /// Zero-based line of the name within the label string, and its column in
    /// UTF-16 code units — the same convention as [`Target`].
    pub line: u32,
    pub character: u32,
    /// The name's width in UTF-16 code units.
    pub length: u32,
}

/// What one writer knows. Immutable once built; never mutated in place.
#[derive(Debug, Default)]
pub struct Tier {
    /// How many BUILD files this was built from, including those that declare
    /// nothing. A count rather than a list: the paths live on the entries.
    pub files: u32,
    /// Keyed by `//package:name`, the form a label resolves to.
    pub targets: FxHashMap<String, Target>,
    /// Every mention of a label in a rule-call argument, under the same key as
    /// [`Tier::targets`]. A key here need not be a key there: a label may name
    /// a source file, an output file, or nothing at all.
    pub references: FxHashMap<String, Vec<Reference>>,
    /// The files this tier is the whole truth about, so that a target it does
    /// *not* declare in one of them is a target that does not exist.
    ///
    /// Empty for a tier that only adds to what the ones after it know, which is
    /// every tier but the buffers: a file the disk tier parsed may still hold
    /// targets only Bazel can name.
    pub speaks_for: FxHashSet<Arc<Path>>,
}

impl Tier {
    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }
}

/// A consistent view of every tier, for the life of one request.
///
/// Cheap: three `Arc`s. Composed rather than merged, because merging needs one
/// writer holding what the others last published, and that is the shape that
/// goes out of step. Precedence is the order [`Index::tiers`] returns them in —
/// the buffer the user is typing into, then the disk under it, then what only
/// Bazel can name.
#[derive(Debug, Clone, Default)]
pub struct Index {
    buffer: Arc<Tier>,
    disk: Arc<Tier>,
    graph: Arc<Tier>,
    repos: Arc<Repos>,
}

impl Index {
    /// A view of the disk alone, for a test that has no buffers and no Bazel.
    /// Every caller that runs has an [`IndexHandle`] to load from instead.
    #[cfg(test)]
    #[must_use]
    pub fn of_disk(disk: Tier) -> Self {
        Self {
            disk: Arc::new(disk),
            ..Self::default()
        }
    }

    /// The tiers that read the source, in precedence order.
    ///
    /// Coverage means something only between these two, because both are
    /// readings of the same files and a fresher reading replaces an older one.
    /// It says nothing about the graph: no parser can see a name a macro
    /// computes, so a buffer that has just been re-read has not contradicted
    /// Bazel about anything.
    fn parsed(&self) -> [&Tier; 2] {
        [&self.buffer, &self.disk]
    }

    /// Whether a parsed tier ahead of `nth` has already said everything true
    /// about `file`.
    ///
    /// This is what makes a deletion visible. A target the user removes from an
    /// open buffer is absent from the buffer tier and still present on disk;
    /// without this it would go on resolving to a declaration that is no longer
    /// written anywhere.
    fn covered(&self, nth: usize, file: &Path) -> bool {
        self.parsed()[..nth]
            .iter()
            .any(|ahead| ahead.speaks_for.contains(file))
    }

    /// The declaration of `label`, from the source if any parser has seen it
    /// and from Bazel otherwise.
    ///
    /// The graph is consulted only for a label no parsed tier holds at all,
    /// which is exactly what it is for: the targets a legacy macro computes.
    /// That is also what keeps it from overriding a field — its rule class is
    /// the private name Bazel instantiated, `_local_rule` where the source says
    /// `local_helper`, and the reader recognises the one they wrote.
    #[must_use]
    pub fn target(&self, label: &str) -> Option<&Target> {
        if self
            .parsed()
            .iter()
            .any(|tier| tier.targets.contains_key(label))
        {
            return self.parsed_target(label);
        }
        self.graph.targets.get(label)
    }

    fn parsed_target(&self, label: &str) -> Option<&Target> {
        self.parsed()
            .into_iter()
            .enumerate()
            .find_map(|(nth, tier)| {
                let target = tier.targets.get(label)?;
                (!self.covered(nth, &target.file)).then_some(target)
            })
    }

    /// Every recorded mention of `label`, in source order per file.
    ///
    /// The source alone. The query that feeds the graph is pruned of attributes
    /// and so carries no edges, which means the graph knows that a target
    /// exists and nothing whatever about what points at it.
    #[must_use]
    pub fn references(&self, label: &str) -> Vec<&Reference> {
        self.parsed()
            .into_iter()
            .enumerate()
            .flat_map(|(nth, tier)| {
                tier.references
                    .get(label)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .filter(move |reference| !self.covered(nth, &reference.file))
            })
            .collect()
    }

    /// Every target any tier knows, each named once.
    pub fn targets(&self) -> impl Iterator<Item = (&str, &Target)> {
        let parsed = self
            .parsed()
            .into_iter()
            .enumerate()
            .flat_map(move |(nth, tier)| {
                tier.targets.iter().filter_map(move |(label, target)| {
                    let ahead = self.parsed()[..nth]
                        .iter()
                        .any(|tier| tier.targets.contains_key(label));
                    (!ahead && !self.covered(nth, &target.file)).then_some((label.as_str(), target))
                })
            });
        let only_bazel_knows = self
            .graph
            .targets
            .iter()
            .filter_map(move |(label, target)| {
                self.parsed()
                    .iter()
                    .all(|tier| !tier.targets.contains_key(label))
                    .then_some((label.as_str(), target))
            });
        parsed.chain(only_bazel_knows)
    }

    /// Whether `label` is one only Bazel can name.
    ///
    /// Derived rather than recorded on the target, because it already is: a
    /// label no parser holds is one no parser saw, which is exactly what a
    /// macro computing a name means. A handler that cannot act on such a target
    /// — rename above all, since there is no `name = "…"` to rewrite — asks
    /// here and says so.
    #[must_use]
    pub fn only_bazel_knows(&self, label: &str) -> bool {
        self.graph.targets.contains_key(label)
            && self
                .parsed()
                .iter()
                .all(|tier| !tier.targets.contains_key(label))
    }

    /// What this workspace's apparent repository names mean.
    #[must_use]
    pub fn repos(&self) -> &Repos {
        &self.repos
    }

    /// How many BUILD files the walk over the disk read.
    #[must_use]
    pub fn files(&self) -> u32 {
        self.disk.files
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.targets().count()
    }
}

/// Publishes tiers. Clone freely; all clones share one slot per tier.
///
/// One slot per writer and no writer touching another's is what removes the
/// question of who wins a race: there is no race. The main loop writes the
/// buffers, the watch thread the disk, the Bazel actor the graph.
#[derive(Debug, Clone, Default)]
pub struct IndexHandle {
    buffer: Arc<ArcSwap<Tier>>,
    disk: Arc<ArcSwap<Tier>>,
    graph: Arc<ArcSwap<Tier>>,
    repos: Arc<ArcSwap<Repos>>,
}

impl IndexHandle {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A consistent view for the life of a request. Unaffected by later swaps.
    #[must_use]
    pub fn load(&self) -> Index {
        Index {
            buffer: self.buffer.load_full(),
            disk: self.disk.load_full(),
            graph: self.graph.load_full(),
            repos: self.repos.load_full(),
        }
    }

    pub fn store_disk(&self, tier: Tier) {
        self.disk.store(Arc::new(tier));
    }

    pub fn store_buffer(&self, tier: Tier) {
        self.buffer.store(Arc::new(tier));
    }

    pub fn store_graph(&self, tier: Tier) {
        self.graph.store(Arc::new(tier));
    }

    pub fn store_repos(&self, repos: Repos) {
        self.repos.store(Arc::new(repos));
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
pub fn build_static(root: &Path) -> Tier {
    let mut index = Tier::default();
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
        if collect_file(
            root,
            &Arc::from(path),
            &parse(&text, dialect),
            &text,
            &mut index.targets,
            &mut index.references,
        ) {
            index.files += 1;
        }
    }

    tracing::info!(
        files = index.files,
        targets = index.targets.len(),
        labels_referenced = index.references.len(),
        "built static index"
    );
    index
}

/// Everything one BUILD file contributes, appended to `targets` and
/// `references`.
///
/// The single place a file becomes index entries, so the walk over the disk and
/// a buffer the client holds open cannot disagree about the package a
/// declaration lands in — the two derive it from the same call rather than from
/// two copies of the same rule.
///
/// The tree is supplied rather than parsed here so that a caller who needs to
/// know whether it parsed cleanly — the buffer tier does, to decide whether it
/// may claim a file — reads the errors off the same pass this reads the
/// declarations off.
///
/// `false` where the path lies outside `root`, which is the one way a file can
/// have no package to be indexed under.
pub fn collect_file(
    root: &Path,
    path: &Arc<Path>,
    parsed: &Parse,
    text: &str,
    targets: &mut FxHashMap<String, Target>,
    references: &mut FxHashMap<String, Vec<Reference>>,
) -> bool {
    let Some(package) = package_dir(root, path) else {
        return false;
    };
    collect(parsed, text, path, &package, targets, references);
    true
}

/// Everything one BUILD file contributes: the targets it declares and the
/// labels it names.
///
/// Both come out of the same parse and the same walk of top-level calls. A
/// second traversal would double the only cost this tier has.
fn collect(
    parsed: &Parse,
    text: &str,
    file: &Arc<Path>,
    package: &str,
    targets: &mut FxHashMap<String, Target>,
    references: &mut FxHashMap<String, Vec<Reference>>,
) {
    let Some(root) = File::cast(parsed.syntax()) else {
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
                repo: None,
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
                    file: Arc::clone(file),
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
                file: Arc::clone(file),
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
    use starlark_cst::Dialect;

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

    fn at(file: &str, line: u32, rule: &str) -> Target {
        Target {
            name: "t".into(),
            rule: rule.into(),
            file: Arc::from(Path::new(file)),
            line,
            character: 0,
            length: 1,
        }
    }

    fn tier(entries: &[(&str, Target)], speaks_for: &[&str]) -> Arc<Tier> {
        Arc::new(Tier {
            targets: entries
                .iter()
                .map(|(label, target)| ((*label).to_string(), target.clone()))
                .collect(),
            speaks_for: speaks_for
                .iter()
                .map(|path| Arc::from(Path::new(path)))
                .collect(),
            ..Tier::default()
        })
    }

    /// The four precedence rules, on one arrangement.
    ///
    /// A buffer open on `lib/BUILD.bazel` that declares `kept` and no longer
    /// declares `gone`; a disk that still has both plus `//app:other`; and a
    /// graph that knows a macro-generated target and disagrees with the source
    /// about a rule class.
    #[test]
    fn a_buffer_speaks_for_its_file_and_bazel_for_what_no_parser_sees() {
        let open = "/ws/lib/BUILD.bazel";
        let index = Index {
            buffer: tier(&[("//lib:kept", at(open, 9, "filegroup"))], &[open]),
            disk: tier(
                &[
                    ("//lib:kept", at(open, 1, "filegroup")),
                    ("//lib:gone", at(open, 2, "filegroup")),
                    ("//app:other", at("/ws/app/BUILD.bazel", 3, "filegroup")),
                ],
                &[],
            ),
            graph: tier(
                &[
                    ("//lib:from_macro", at(open, 7, "filegroup")),
                    ("//lib:kept", at(open, 1, "_private_rule")),
                ],
                &[],
            ),
            repos: Arc::default(),
        };

        // The buffer wins where both have it, so the position is the one the
        // user can see.
        assert_eq!(index.target("//lib:kept").map(|t| t.line), Some(9));
        // Deleted in the buffer, and the buffer speaks for that file, so the
        // disk entry does not resurrect it.
        assert_eq!(index.target("//lib:gone"), None);
        // A file no buffer covers is answered from disk as before.
        assert_eq!(index.target("//app:other").map(|t| t.line), Some(3));
        // Bazel supplies what no parser can see, in that same covered file:
        // re-reading a buffer says nothing about what a macro computes.
        assert_eq!(index.target("//lib:from_macro").map(|t| t.line), Some(7));
        // And overrides nothing: `_private_rule` appears in no source anyone
        // wrote, so the call-site spelling stands.
        assert_eq!(
            index.target("//lib:kept").map(|t| &*t.rule),
            Some("filegroup")
        );

        // Which is also how a handler tells the two apart: the macro's target
        // has no `name = "…"` anywhere to rewrite, and `kept` does.
        assert!(index.only_bazel_knows("//lib:from_macro"));
        assert!(!index.only_bazel_knows("//lib:kept"));
        assert!(!index.only_bazel_knows("//app:other"));
        assert!(!index.only_bazel_knows("//lib:nothing_at_all"));

        let mut listed: Vec<_> = index.targets().map(|(label, _)| label).collect();
        listed.sort_unstable();
        assert_eq!(
            listed,
            vec!["//app:other", "//lib:from_macro", "//lib:kept"],
            "each target once, and the deleted one not at all"
        );
    }

    #[test]
    fn snapshots_are_stable_across_swaps() {
        let handle = IndexHandle::new();
        let before = handle.load();

        let mut next = Tier::default();
        next.targets.insert(
            "//lib:srcs".into(),
            Target {
                name: "srcs".into(),
                rule: "filegroup".into(),
                file: Arc::from(Path::new("/ws/lib/BUILD.bazel")),
                line: 0,
                character: 0,
                length: 4,
            },
        );
        handle.store_disk(next);

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

        let index = Index::of_disk(build_static(&root));
        assert_eq!(index.len(), 1, "each target must appear exactly once");
        assert!(index.target("//lib:srcs").is_some());
        assert!(
            index
                .targets()
                .all(|(label, _)| !label.contains("bazel-out")),
            "generated output must not be indexed: {:?}",
            index.targets().map(|(label, _)| label).collect::<Vec<_>>()
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

        let index = Index::of_disk(build_static(&root));
        let mut labels: Vec<_> = index
            .targets()
            .map(|(label, _)| label.to_string())
            .collect();
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

        let index = Index::of_disk(build_static(&root));
        let labels: Vec<_> = index
            .targets()
            .map(|(label, _)| label.to_string())
            .collect();
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
            &parse(text, Dialect::Bazel),
            text,
            &Arc::from(Path::new("/ws/lib/BUILD.bazel")),
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

    /// A pattern names a set and prose is not a label, so neither is recorded.
    /// A label into another repository is a label, and is.
    #[test]
    fn patterns_are_not_recorded_and_other_repositories_are() {
        let found = refs(
            "genrule(\n    name = \"g\",\n    srcs = [\"@platforms//os:linux\", \"//lib:all\", \"//lib/...\"],\n    outs = [],\n)\n",
            "lib",
        );
        assert_eq!(
            found,
            vec![("@platforms//os:linux".to_string(), vec![(2, 28, 5)])]
        );
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
