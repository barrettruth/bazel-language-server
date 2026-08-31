//! One recursive workspace watch, with Bazel output trees excluded.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};

use crate::actor::Bazel;
use crate::bazelrc::ConfigurationHandle;
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
    /// The imported Bazelrc graph.
    configuration: bool,
}

impl Invalidates {
    const NOTHING: Self = Self {
        full_targets: false,
        target_files: Vec::new(),
        graph: false,
        configuration: false,
    };
    const GRAPH: Self = Self {
        full_targets: false,
        target_files: Vec::new(),
        graph: true,
        configuration: false,
    };
    const BOTH: Self = Self {
        full_targets: true,
        target_files: Vec::new(),
        graph: true,
        configuration: true,
    };
    const CONFIGURATION: Self = Self {
        full_targets: false,
        target_files: Vec::new(),
        graph: true,
        configuration: true,
    };
    const INITIAL: Self = Self {
        full_targets: true,
        target_files: Vec::new(),
        graph: false,
        configuration: true,
    };

    fn file(path: &Path) -> Self {
        Self {
            full_targets: false,
            target_files: vec![path.to_path_buf()],
            graph: true,
            configuration: false,
        }
    }

    fn with(mut self, other: Self) -> Self {
        self.full_targets |= other.full_targets;
        self.graph |= other.graph;
        self.configuration |= other.configuration;
        if self.full_targets {
            self.target_files.clear();
        } else {
            self.target_files.extend(other.target_files);
        }
        self
    }

    fn anything(&self) -> bool {
        self.full_targets || !self.target_files.is_empty() || self.graph || self.configuration
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
        | "WORKSPACE.bzlmod" | "REPO.bazel" => Invalidates::GRAPH,
        _ if name == ".bazelrc" || name.ends_with(".bazelrc") => Invalidates::CONFIGURATION,
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
    Fs,
    /// Somebody asked, through [`REINDEX_COMMAND`].
    Manual,
    Stop,
}

#[derive(Default)]
struct EventQueue {
    state: Mutex<EventState>,
}

#[derive(Default)]
struct EventState {
    queued: bool,
    pending: Invalidates,
}

impl EventQueue {
    const MAX_FILES: usize = 1_024;

    fn send(&self, root: &Path, tx: &Sender<Wake>, event: &notify::Result<notify::Event>) {
        let reaches = wanted(root, event);
        if !reaches.anything() {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending = std::mem::take(&mut state.pending).with(reaches);
        if state.pending.target_files.len() > Self::MAX_FILES {
            state.pending.full_targets = true;
            state.pending.target_files.clear();
        }
        if state.queued {
            return;
        }
        state.queued = true;
        if tx.send(Wake::Fs).is_err() {
            *state = EventState::default();
        }
    }

    fn take(&self) -> Invalidates {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.queued = false;
        std::mem::take(&mut state.pending)
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

struct Publishers {
    root: PathBuf,
    index: IndexHandle,
    configuration: ConfigurationHandle,
    bazel: Arc<Bazel>,
    semantic_wake: crossbeam_channel::Sender<()>,
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
    configuration: ConfigurationHandle,
    bazel: Arc<Bazel>,
    ready: crossbeam_channel::Sender<Ready>,
    semantic_wake: crossbeam_channel::Sender<()>,
) -> Watch {
    let (tx, rx) = channel();
    let publishers = Publishers {
        root: root.to_path_buf(),
        index,
        configuration,
        bazel,
        semantic_wake,
    };
    let watching = publishers.root.clone();
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
            settle(&publishers, &rx, &ready, &event_queue);
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
    let classified = root.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event| {
        queue.send(&classified, &tx, &event);
    })
    .context("creating the workspace watcher")?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", root.display()))?;
    Ok(watcher)
}

/// Build once, then collapse each event burst into one update.
fn settle(
    publishers: &Publishers,
    rx: &Receiver<Wake>,
    ready: &crossbeam_channel::Sender<Ready>,
    events: &EventQueue,
) {
    let started = std::time::Instant::now();
    rebuild(publishers, Invalidates::INITIAL, 0);
    let snapshot = publishers.index.load_disk();
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
        let mut reaches = reached(&first, events);
        if !reaches.anything() {
            continue;
        }
        let deadline = Instant::now() + SETTLE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(wake) => reaches = reaches.with(reached(&wake, events)),
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        nth += 1;
        rebuild(publishers, reaches, nth);
    }
}

/// Which tiers a wake reaches.
fn reached(wake: &Wake, events: &EventQueue) -> Invalidates {
    match wake {
        Wake::Manual => Invalidates::BOTH,
        Wake::Fs => events.take(),
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
        Ok(event) => {
            let ignored = crate::index::read_bazelignore(root);
            let relevant = |path: &Path| {
                !excluded(root, path) && !crate::index::is_ignored(root, path, &ignored)
            };
            if changes_package_tree(event) && event.paths.iter().any(|path| relevant(path)) {
                return Invalidates::BOTH;
            }
            event
                .paths
                .iter()
                .filter(|path| relevant(path))
                .fold(Invalidates::NOTHING, |reaches, path| {
                    reaches.with(invalidated(path))
                })
        }
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
fn rebuild(publishers: &Publishers, reaches: Invalidates, nth: u64) {
    if reaches.full_targets {
        let started = std::time::Instant::now();
        let built = crate::index::build_static(&publishers.root);
        let targets = built.len();
        publishers.index.store_disk(built);
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
        let built =
            crate::index::update_static(&publishers.root, &publishers.index.load_disk(), &changed);
        let targets = built.len();
        publishers.index.store_disk(built);
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
    if reaches.configuration {
        publishers
            .configuration
            .store(crate::bazelrc::ConfigurationSnapshot::build(
                &publishers.root,
            ));
        let _ = publishers.semantic_wake.try_send(());
    }
    if reaches.graph {
        publishers.bazel.refresh();
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
        ] {
            assert_eq!(
                invalidated(Path::new(path)),
                Invalidates::GRAPH,
                "{path} is the graph tier's business alone"
            );
        }
    }

    #[test]
    fn every_bazelrc_refreshes_the_configuration_and_graph() {
        for path in ["/ws/.bazelrc", "/ws/config/build.bazelrc"] {
            let reaches = invalidated(Path::new(path));
            assert!(reaches.graph, "{path}");
            assert!(reaches.configuration, "{path}");
            assert!(!reaches.full_targets, "{path}");
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
    fn filesystem_events_coalesce_before_the_watcher_wakes() {
        let (tx, rx) = channel();
        let events = EventQueue::default();
        let root = Path::new("/ws");
        for path in ["/ws/one/BUILD.bazel", "/ws/two/BUILD.bazel"] {
            events.send(
                root,
                &tx,
                &Ok(notify::Event::new(notify::EventKind::Any).add_path(PathBuf::from(path))),
            );
        }
        let wake = rx.recv().unwrap();
        let reaches = reached(&wake, &events);
        assert!(!reaches.full_targets);
        assert_eq!(reaches.target_files.len(), 2);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn ignored_build_files_do_not_reach_the_graph() {
        let root = std::env::temp_dir().join(format!("bls-watch-ignore-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::write(root.join(".bazelignore"), "ignored\n").unwrap();
        let event =
            notify::Event::new(notify::EventKind::Any).add_path(root.join("ignored/BUILD.bazel"));
        assert_eq!(wanted(&root, &Ok(event)), Invalidates::NOTHING);
        let directory =
            notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::Folder))
                .add_path(root.join("ignored/generated"));
        assert_eq!(wanted(&root, &Ok(directory)), Invalidates::NOTHING);
        std::fs::remove_dir_all(root).ok();
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
