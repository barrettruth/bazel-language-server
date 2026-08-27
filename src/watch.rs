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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};

use crate::actor::Actor;
use crate::index::IndexHandle;

/// How long a burst may keep arriving before the rebuild starts.
///
/// A branch switch rewrites thousands of files and a save rewrites one; both
/// should cost a single rebuild. Long enough to swallow the first, short enough
/// that the second feels immediate.
const SETTLE: Duration = Duration::from_millis(250);

/// The files whose contents change what the index holds.
///
/// `BUILD` and `.bazelignore` change the static tier; `.bzl`, `MODULE.bazel`
/// and `.bazelrc` change only what Bazel would say. Both rebuild everything
/// today, and telling them apart is what buys a cheaper refresh later.
fn indexed(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        "BUILD"
            | "BUILD.bazel"
            | "MODULE.bazel"
            | "MODULE.bazel.lock"
            | "WORKSPACE"
            | "WORKSPACE.bazel"
            | "WORKSPACE.bzlmod"
            | "REPO.bazel"
            | ".bazelrc"
            | ".bazelignore"
    ) || path.extension().is_some_and(|kind| kind == "bzl")
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

/// The live watch. Dropping it stops watching.
pub struct Watch {
    _watcher: notify::RecommendedWatcher,
}

/// Watch `root`, rebuilding the index whenever something it reads changes.
///
/// # Errors
///
/// If the watch cannot be established on `root`.
pub fn spawn(root: PathBuf, index: IndexHandle, actor: Option<Arc<Actor>>) -> Result<Watch> {
    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(tx).context("creating the workspace watcher")?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", root.display()))?;

    std::thread::Builder::new()
        .name("watch".to_owned())
        .spawn(move || settle(&root, &index, actor.as_deref(), &rx))
        .context("spawning the watch thread")?;

    Ok(Watch { _watcher: watcher })
}

/// Collapse a burst of events into one rebuild.
fn settle(
    root: &Path,
    index: &IndexHandle,
    actor: Option<&Actor>,
    rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
) {
    while let Ok(first) = rx.recv() {
        if !wanted(root, &first) {
            continue;
        }
        // Everything still arriving is part of the same edit, checkout or
        // build. Wait for quiet rather than rebuilding once per file.
        loop {
            match rx.recv_timeout(SETTLE) {
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        rebuild(root, index, actor);
    }
}

/// Whether an event should cost a rebuild.
///
/// `need_rescan` is the backend saying it dropped events — `FSEvents`'
/// `MustScanSubDirs` and its two dropped flags, inotify's `IN_Q_OVERFLOW`,
/// the Windows overflow. What was lost is unknowable, so the answer is to
/// rebuild everything, and not asking is a silent staleness bug.
fn wanted(root: &Path, event: &notify::Result<notify::Event>) -> bool {
    match event {
        Ok(event) if event.need_rescan() => {
            tracing::debug!("the watcher dropped events; rebuilding everything");
            true
        }
        Ok(event) => event
            .paths
            .iter()
            .any(|path| indexed(path) && !excluded(root, path)),
        Err(err) => {
            tracing::warn!("watching the workspace: {err}");
            false
        }
    }
}

/// Rebuild the static tier and ask for the graph tier.
fn rebuild(root: &Path, index: &IndexHandle, actor: Option<&Actor>) {
    let started = std::time::Instant::now();
    let built = crate::index::build_static(root);
    let targets = built.len();
    index.store(built);
    tracing::info!(targets, ms = started.elapsed().as_millis(), "reindexed");
    if let Some(actor) = actor {
        actor.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_files_the_index_reads_are_watched() {
        assert!(indexed(Path::new("/ws/lib/BUILD.bazel")));
        assert!(indexed(Path::new("/ws/lib/defs.bzl")));
        assert!(indexed(Path::new("/ws/MODULE.bazel")));
        assert!(indexed(Path::new("/ws/.bazelrc")));
        assert!(!indexed(Path::new("/ws/lib/main.cc")));
        assert!(!indexed(Path::new("/ws/README.md")));
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
