//! One recursive watch over the workspace.
//!
//! Exactly one, on the root. Registering a watch per directory is the shape
//! that breaks: at 4,096 directories setup takes 72 s and is quadratic, and at
//! 4,100 every call still returns `Ok(())` while no event is ever delivered
//! again, because macOS `FSEvents` spends a file descriptor per stream path
//! against `RLIMIT_NOFILE`. One recursive watch over a 74k-package tree is
//! 0.002 s and reaches the deepest package in 15.8 ms.
//!
//! We own this rather than registering `workspace/didChangeWatchedFiles`
//! because `DidChangeWatchedFilesRegistrationOptions` carries watchers and no
//! exclude, so "everything but `bazel-out`" cannot be said in the protocol —
//! and during a build that is the difference between a handful of events and a
//! flood.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};

use crate::actor::Bazel;
use crate::index::IndexHandle;

/// How long a burst may keep arriving before the rebuild starts.
///
/// A branch switch rewrites thousands of files and a save rewrites one; both
/// should cost a single rebuild. Long enough to swallow the first, short enough
/// that the second feels immediate.
const SETTLE: Duration = Duration::from_millis(250);

/// Which tiers a changed file can possibly affect.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Invalidates {
    /// The target table, which is built from BUILD files and nothing else.
    targets: bool,
    /// What Bazel would say, which every file Bazel loads can change.
    graph: bool,
}

impl Invalidates {
    const NOTHING: Self = Self {
        targets: false,
        graph: false,
    };
    const GRAPH: Self = Self {
        targets: false,
        graph: true,
    };
    const TARGETS: Self = Self {
        targets: true,
        graph: false,
    };
    const BOTH: Self = Self {
        targets: true,
        graph: true,
    };

    fn with(self, other: Self) -> Self {
        Self {
            targets: self.targets || other.targets,
            graph: self.graph || other.graph,
        }
    }

    fn anything(self) -> bool {
        self.targets || self.graph
    }
}

/// What a change to `path` can reach.
///
/// `build_static` skips every file whose kind is not `Build`
/// (`crate::index`), so a `.bzl` edit cannot move a single entry in the target
/// table no matter what the macro inside it does — the table records what a
/// BUILD file literally declares, and only Bazel evaluates the rest. Rebuilding
/// it anyway costs ~1.4 s on a large repo to arrive at the identical answer.
///
/// `.bazelignore` is the exception among the non-BUILD files: it decides which
/// directories are packages at all, so it changes what the walk may even look
/// at.
fn invalidated(path: &Path) -> Invalidates {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Invalidates::NOTHING;
    };
    match name {
        "BUILD" | "BUILD.bazel" | ".bazelignore" => Invalidates::BOTH,
        "MODULE.bazel" | "MODULE.bazel.lock" | "WORKSPACE" | "WORKSPACE.bazel"
        | "WORKSPACE.bzlmod" | "REPO.bazel" | ".bazelrc" => Invalidates::GRAPH,
        _ if path.extension().is_some_and(|kind| kind == "bzl") => Invalidates::GRAPH,
        _ => Invalidates::NOTHING,
    }
}

/// Whether a path lies somewhere the index never looks.
///
/// The `bazel-*` convenience symlinks point into the output base, whose
/// symlink forest re-enters the source tree — following them finds 94,118
/// BUILD files in a tree that has 74,001. They exist only at the root, so the
/// check is depth-scoped, and a workspace directory of its own called
/// `bazel-tools` keeps its targets.
fn excluded(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    let mut components = relative.components();
    let first = components.next().and_then(|c| c.as_os_str().to_str());
    if first.is_some_and(|name| name.starts_with("bazel-")) {
        return true;
    }
    relative
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .any(|name| name == ".git" || name == ".jj")
}

/// The command that rebuilds everything on demand.
pub const REINDEX_COMMAND: &str = "bazel-language-server.reindex";

/// What wakes the rebuild thread.
enum Wake {
    /// Something changed on disk.
    Fs(notify::Result<notify::Event>),
    /// Somebody asked, through [`REINDEX_COMMAND`].
    Manual,
    Stop,
}

/// The rebuild thread. Dropping it stops the watch and joins the thread.
pub struct Watch {
    tx: Sender<Wake>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Rebuild both tiers, whatever the watcher has or has not noticed.
    ///
    /// Returns without waiting: the rebuild is 0.72 s at 20k packages and
    /// invariant 1 does not care whose request it is.
    pub fn reindex(&self) {
        tracing::info!("reindexing on request");
        drop(self.tx.send(Wake::Manual));
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        drop(self.tx.send(Wake::Stop));
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

/// Rebuild `root`'s index on demand, and whenever something it reads changes.
///
/// **The thread starts even when the watch does not.** A failed watch is
/// exactly the situation the manual reindex exists for, so making the thread
/// conditional on the watcher would remove the escape hatch precisely when it
/// is needed. A server with no watch is one that reindexes only when asked,
/// which is degraded and usable rather than degraded and stuck.
#[must_use]
pub fn spawn(root: &Path, index: IndexHandle, bazel: Arc<Bazel>) -> Watch {
    let (tx, rx) = channel();
    let watching = root.to_path_buf();
    let events = tx.clone();
    let thread = std::thread::Builder::new()
        .name("watch".to_owned())
        .spawn(move || {
            let _watcher = match establish(&watching, events) {
                Ok(watcher) => Some(watcher),
                Err(err) => {
                    tracing::warn!(
                        "the workspace is not being watched, so the index updates only on \
                         `{REINDEX_COMMAND}`: {err:#}"
                    );
                    None
                }
            };
            settle(&watching, &index, &bazel, &rx);
        })
        .expect("spawning the watch thread");
    Watch {
        tx,
        thread: Some(thread),
    }
}

/// One recursive watch on `root`, reporting into `tx`.
fn establish(root: &Path, tx: Sender<Wake>) -> Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event| {
        drop(tx.send(Wake::Fs(event)));
    })
    .context("creating the workspace watcher")?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", root.display()))?;
    Ok(watcher)
}

/// Collapse a burst of events into one rebuild.
fn settle(root: &Path, index: &IndexHandle, bazel: &Bazel, rx: &Receiver<Wake>) {
    rebuild(root, index, bazel, Invalidates::TARGETS, 0);
    let mut nth = 0_u64;
    while let Ok(first) = rx.recv() {
        if matches!(first, Wake::Stop) {
            return;
        }
        let mut reaches = reached(root, &first);
        if !reaches.anything() {
            continue;
        }
        // Everything still arriving is part of the same edit, checkout or
        // build, and the tiers it reaches are the union of what it touched.
        loop {
            match rx.recv_timeout(SETTLE) {
                Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(wake) => reaches = reaches.with(reached(root, &wake)),
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        nth += 1;
        rebuild(root, index, bazel, reaches, nth);
    }
}

/// Which tiers a wake reaches.
///
/// A manual reindex reaches everything by definition: it is asked for when the
/// watcher is not trusted, so believing the watcher about what changed would
/// defeat it.
fn reached(root: &Path, wake: &Wake) -> Invalidates {
    match wake {
        Wake::Manual => Invalidates::BOTH,
        Wake::Fs(event) => wanted(root, event),
        Wake::Stop => Invalidates::NOTHING,
    }
}

/// Whether an event should cost a rebuild.
///
/// `need_rescan` is the backend saying it dropped events — `FSEvents`'
/// `MustScanSubDirs` and its two dropped flags, inotify's `IN_Q_OVERFLOW`,
/// the Windows overflow. What was lost is unknowable, so the answer is to
/// rebuild everything, and not asking is a silent staleness bug.
fn wanted(root: &Path, event: &notify::Result<notify::Event>) -> Invalidates {
    match event {
        Ok(event) if event.need_rescan() => {
            tracing::debug!("the watcher dropped events; rebuilding everything");
            Invalidates::BOTH
        }
        Ok(event) => event
            .paths
            .iter()
            .filter(|path| !excluded(root, path))
            .fold(Invalidates::NOTHING, |reaches, path| {
                reaches.with(invalidated(path))
            }),
        Err(err) => {
            tracing::warn!("watching the workspace: {err}");
            Invalidates::NOTHING
        }
    }
}

/// Refresh whichever tiers the change could have reached.
///
/// `nth` counts settled bursts for the life of the session, so a log says how
/// often editing actually costs a rebuild rather than how long one takes.
fn rebuild(root: &Path, index: &IndexHandle, bazel: &Bazel, reaches: Invalidates, nth: u64) {
    if reaches.targets {
        let started = std::time::Instant::now();
        let built = crate::index::build_static(root);
        let targets = built.len();
        index.store_disk(built);
        tracing::info!(
            nth,
            targets,
            ms = started.elapsed().as_millis(),
            "reindexed"
        );
    } else {
        tracing::info!(nth, "the target table is untouched by this change");
    }
    if reaches.graph {
        bazel.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The target table is built from BUILD files alone, so only they and
    /// `.bazelignore` can move an entry in it.
    #[test]
    fn only_a_build_file_reaches_the_target_table() {
        for path in ["/ws/lib/BUILD", "/ws/lib/BUILD.bazel", "/ws/.bazelignore"] {
            assert_eq!(
                invalidated(Path::new(path)),
                Invalidates::BOTH,
                "{path} reaches both tiers"
            );
        }
    }

    /// Bazel loads these and the target table never reads them, so rebuilding
    /// it would spend ~1.4 s arriving at the identical answer.
    #[test]
    fn what_only_bazel_reads_refreshes_only_the_graph() {
        for path in [
            "/ws/lib/defs.bzl",
            "/ws/MODULE.bazel",
            "/ws/MODULE.bazel.lock",
            "/ws/WORKSPACE",
            "/ws/REPO.bazel",
            "/ws/.bazelrc",
        ] {
            assert_eq!(
                invalidated(Path::new(path)),
                Invalidates::GRAPH,
                "{path} is the graph tier's business alone"
            );
        }
    }

    #[test]
    fn everything_else_costs_nothing() {
        for path in ["/ws/lib/main.cc", "/ws/README.md", "/ws/lib/a.txt"] {
            assert_eq!(invalidated(Path::new(path)), Invalidates::NOTHING, "{path}");
        }
    }

    /// A burst is one rebuild, reaching the union of what it touched.
    #[test]
    fn a_burst_reaches_the_union_of_its_files() {
        let bzl = invalidated(Path::new("/ws/lib/defs.bzl"));
        let text = invalidated(Path::new("/ws/lib/a.txt"));
        assert_eq!(bzl.with(text), Invalidates::GRAPH);
        assert_eq!(
            bzl.with(invalidated(Path::new("/ws/lib/BUILD.bazel"))),
            Invalidates::BOTH
        );
        assert!(!text.anything());
    }

    /// The convenience symlinks exist only at the root, so a package that
    /// merely starts with the same letters keeps its targets.
    #[test]
    fn the_convenience_symlinks_are_excluded_and_nothing_else_is() {
        let root = Path::new("/ws");
        assert!(excluded(root, Path::new("/ws/bazel-out/x/BUILD.bazel")));
        assert!(excluded(root, Path::new("/ws/bazel-ws/BUILD.bazel")));
        assert!(excluded(root, Path::new("/ws/.git/index")));
        assert!(excluded(root, Path::new("/ws/.jj/repo")));
        assert!(!excluded(
            root,
            Path::new("/ws/tools/bazel-helpers/BUILD.bazel")
        ));
        assert!(!excluded(root, Path::new("/ws/lib/BUILD.bazel")));
        // A path outside the workspace is not ours to care about.
        assert!(excluded(root, Path::new("/elsewhere/BUILD.bazel")));
    }
}
