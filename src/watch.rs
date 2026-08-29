//! One recursive workspace watch, with Bazel output trees excluded.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};

use crate::actor::Bazel;
use crate::index::IndexHandle;

/// Debounce window for filesystem bursts.
const SETTLE: Duration = Duration::from_millis(250);

/// Which tiers a changed file can possibly affect.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
struct Invalidates {
    full_targets: bool,
    target_files: Vec<PathBuf>,
    /// What Bazel would say, which every file Bazel loads can change.
    graph: bool,
}

impl Invalidates {
    const NOTHING: Self = Self {
        full_targets: false,
        target_files: Vec::new(),
        graph: false,
    };
    const GRAPH: Self = Self {
        full_targets: false,
        target_files: Vec::new(),
        graph: true,
    };
    const TARGETS: Self = Self {
        full_targets: true,
        target_files: Vec::new(),
        graph: false,
    };
    const BOTH: Self = Self {
        full_targets: true,
        target_files: Vec::new(),
        graph: true,
    };

    fn file(path: &Path) -> Self {
        Self {
            full_targets: false,
            target_files: vec![path.to_path_buf()],
            graph: true,
        }
    }

    fn with(mut self, other: Self) -> Self {
        self.full_targets |= other.full_targets;
        self.graph |= other.graph;
        if self.full_targets {
            self.target_files.clear();
        } else {
            self.target_files.extend(other.target_files);
        }
        self
    }

    fn anything(&self) -> bool {
        self.full_targets || !self.target_files.is_empty() || self.graph
    }
}

/// Classify a change by the index tiers it can affect.
fn invalidated(path: &Path) -> Invalidates {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Invalidates::NOTHING;
    };
    match name {
        "BUILD" | "BUILD.bazel" => Invalidates::file(path),
        ".bazelignore" => Invalidates::BOTH,
        "MODULE.bazel" | "MODULE.bazel.lock" | "WORKSPACE" | "WORKSPACE.bazel"
        | "WORKSPACE.bzlmod" | "REPO.bazel" | ".bazelrc" => Invalidates::GRAPH,
        _ if path.extension().is_some_and(|kind| kind == "bzl") => Invalidates::GRAPH,
        _ => Invalidates::NOTHING,
    }
}

/// Whether a path lies in metadata or a root-level Bazel output tree.
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

#[derive(Default)]
struct EventQueue {
    state: AtomicU8,
}

impl EventQueue {
    const IDLE: u8 = 0;
    const QUEUED: u8 = 1;
    const OVERFLOWED: u8 = 2;

    fn send(&self, tx: &Sender<Wake>, event: notify::Result<notify::Event>) {
        loop {
            let state = self.state.load(Ordering::Acquire);
            let next = if state == Self::IDLE {
                Self::QUEUED
            } else {
                Self::OVERFLOWED
            };
            if self
                .state
                .compare_exchange(state, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if state != Self::IDLE {
                    return;
                }
                break;
            }
        }
        if tx.send(Wake::Fs(event)).is_err() {
            self.state.store(Self::IDLE, Ordering::Release);
        }
    }

    fn take(&self, root: &Path, event: &notify::Result<notify::Event>) -> Invalidates {
        if self.state.swap(Self::IDLE, Ordering::AcqRel) == Self::OVERFLOWED {
            tracing::debug!("filesystem events were coalesced; rebuilding everything");
            Invalidates::BOTH
        } else {
            wanted(root, event)
        }
    }
}

/// The rebuild thread. Dropping it stops the watch and joins the thread.
pub struct Watch {
    tx: Sender<Wake>,
    thread: Option<JoinHandle<()>>,
}

pub struct Ready {
    pub files: usize,
    pub targets: usize,
    pub elapsed: Duration,
}

impl Watch {
    /// Queue a full rebuild.
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

/// Start background indexing and filesystem refresh for `root`.
///
/// Manual reindexing remains available if watch registration fails.
#[must_use]
pub fn spawn(
    root: &Path,
    index: IndexHandle,
    bazel: Arc<Bazel>,
    ready: crossbeam_channel::Sender<Ready>,
) -> Watch {
    let (tx, rx) = channel();
    let watching = root.to_path_buf();
    let events = tx.clone();
    let event_queue = Arc::new(EventQueue::default());
    let callback_queue = Arc::clone(&event_queue);
    let thread = std::thread::Builder::new()
        .name("watch".to_owned())
        .spawn(move || {
            let _watcher = match establish(&watching, events, callback_queue) {
                Ok(watcher) => Some(watcher),
                Err(err) => {
                    tracing::warn!(
                        "the workspace is not being watched, so the index updates only on \
                         `{REINDEX_COMMAND}`: {err:#}"
                    );
                    None
                }
            };
            settle(&watching, &index, &bazel, &rx, &ready, &event_queue);
        })
        .expect("spawning the watch thread");
    Watch {
        tx,
        thread: Some(thread),
    }
}

/// One recursive watch on `root`, reporting into `tx`.
fn establish(
    root: &Path,
    tx: Sender<Wake>,
    queue: Arc<EventQueue>,
) -> Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event| {
        queue.send(&tx, event);
    })
    .context("creating the workspace watcher")?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", root.display()))?;
    Ok(watcher)
}

/// Build once, then collapse each event burst into one update.
fn settle(
    root: &Path,
    index: &IndexHandle,
    bazel: &Bazel,
    rx: &Receiver<Wake>,
    ready: &crossbeam_channel::Sender<Ready>,
    events: &EventQueue,
) {
    let started = std::time::Instant::now();
    rebuild(root, index, bazel, Invalidates::TARGETS, 0);
    let snapshot = index.load_disk();
    drop(ready.send(Ready {
        files: snapshot.files.len(),
        targets: snapshot.len(),
        elapsed: started.elapsed(),
    }));
    let mut nth = 0_u64;
    while let Ok(first) = rx.recv() {
        if matches!(first, Wake::Stop) {
            return;
        }
        let mut reaches = reached(root, &first, events);
        if !reaches.anything() {
            continue;
        }
        loop {
            match rx.recv_timeout(SETTLE) {
                Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(wake) => reaches = reaches.with(reached(root, &wake, events)),
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        nth += 1;
        rebuild(root, index, bazel, reaches, nth);
    }
}

/// Which tiers a wake reaches.
fn reached(root: &Path, wake: &Wake, events: &EventQueue) -> Invalidates {
    match wake {
        Wake::Manual => Invalidates::BOTH,
        Wake::Fs(event) => events.take(root, event),
        Wake::Stop => Invalidates::NOTHING,
    }
}

/// Map backend events to index invalidations. Dropped events require a full
/// rebuild because their paths are unknown.
fn wanted(root: &Path, event: &notify::Result<notify::Event>) -> Invalidates {
    match event {
        Ok(event) if event.need_rescan() => {
            tracing::debug!("the watcher dropped events; rebuilding everything");
            Invalidates::BOTH
        }
        Ok(event) if changes_package_tree(event) => Invalidates::BOTH,
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

fn changes_package_tree(event: &notify::Event) -> bool {
    use notify::EventKind::{Create, Modify, Remove};
    use notify::event::{CreateKind, ModifyKind, RemoveKind};

    matches!(
        event.kind,
        Create(CreateKind::Folder) | Remove(RemoveKind::Folder)
    ) || matches!(event.kind, Modify(ModifyKind::Name(_)))
        && event.paths.iter().any(|path| path.is_dir())
}

/// Refresh the affected tiers.
fn rebuild(root: &Path, index: &IndexHandle, bazel: &Bazel, reaches: Invalidates, nth: u64) {
    if reaches.full_targets {
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
    } else if !reaches.target_files.is_empty() {
        let started = std::time::Instant::now();
        let mut changed = reaches.target_files;
        changed.sort_unstable();
        changed.dedup();
        let built = crate::index::update_static(root, &index.load_disk(), &changed);
        let targets = built.len();
        index.store_disk(built);
        tracing::info!(
            nth,
            files = changed.len(),
            targets,
            ms = started.elapsed().as_millis(),
            "updated static index"
        );
    } else {
        tracing::info!(nth, "the target table is untouched by this change");
    }
    if reaches.graph {
        index.store_graph(crate::index::Tier::default());
        bazel.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_build_file_reaches_the_target_table() {
        for path in ["/ws/lib/BUILD", "/ws/lib/BUILD.bazel"] {
            let reaches = invalidated(Path::new(path));
            assert!(reaches.graph);
            assert_eq!(reaches.target_files, vec![PathBuf::from(path)]);
        }
        assert_eq!(
            invalidated(Path::new("/ws/.bazelignore")),
            Invalidates::BOTH
        );
    }

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

    #[test]
    fn a_burst_reaches_the_union_of_its_files() {
        let bzl = invalidated(Path::new("/ws/lib/defs.bzl"));
        let text = invalidated(Path::new("/ws/lib/a.txt"));
        assert_eq!(bzl.clone().with(text.clone()), Invalidates::GRAPH);
        let both = bzl
            .clone()
            .with(invalidated(Path::new("/ws/lib/BUILD.bazel")));
        assert!(both.graph);
        assert_eq!(
            both.target_files,
            vec![PathBuf::from("/ws/lib/BUILD.bazel")]
        );
        assert!(!text.anything());
    }

    #[test]
    fn a_full_event_slot_becomes_one_rescan() {
        let (tx, rx) = channel();
        let events = EventQueue::default();
        events.send(&tx, Ok(notify::Event::new(notify::EventKind::Any)));
        events.send(&tx, Ok(notify::Event::new(notify::EventKind::Any)));
        let wake = rx.recv().unwrap();
        assert_eq!(reached(Path::new("/ws"), &wake, &events), Invalidates::BOTH);
        assert!(rx.try_recv().is_err());
    }

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
