//! The one thread that runs Bazel.
//!
//! Invariant 1 in mechanical form. Every Bazel invocation the server makes on
//! its own behalf happens here, and a request handler reaches it only by asking
//! for a refresh it does not wait on.
//!
//! One thread rather than a pool, because Bazel serialises on the output base
//! anyway: a second invocation would queue on a lock instead of a channel, and
//! a lock is the harder of the two to reason about.

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use arc_swap::ArcSwap;

use crate::bazel::{BazelClient, BazelConfig, Interrupt};
use crate::index::IndexHandle;

/// The Bazel subsystem as it currently stands.
///
/// The configuration and the thread built from it live in one slot, because
/// two slots is how they come to disagree: a `bazel.path` that has changed and
/// an actor still running the old binary is a subsystem that reports one thing
/// and does another.
#[derive(Default)]
pub struct Bazel(ArcSwap<Running>);

#[derive(Default)]
struct Running {
    config: BazelConfig,
    /// `None` where this configuration has no Bazel worth driving, which is the
    /// ordinary case rather than a failure — invariant 2.
    actor: Option<Actor>,
}

impl Bazel {
    /// Adopt `config`, replacing whatever was in force.
    ///
    /// The old actor is dropped, which stops its thread and takes any
    /// invocation with it. A configuration that names no usable Bazel leaves
    /// the static tier as the whole server, and says why.
    pub fn reconfigure(&self, config: BazelConfig, root: Option<&Path>, index: &IndexHandle) {
        let actor = root.and_then(|root| {
            let client = BazelClient::new(config.clone(), root.to_path_buf());
            match client.probe() {
                Ok(probe) => {
                    tracing::info!(
                        version = %probe.version,
                        rule_schemas = probe.capabilities.rule_classes,
                        repo_mapping = probe.capabilities.repo_mapping,
                        "bazel"
                    );
                    Some(Actor::spawn(client, index.clone()))
                }
                Err(err) => {
                    tracing::warn!("the Bazel tier is unavailable: {err:#}");
                    None
                }
            }
        });
        self.0.store(Arc::new(Running { config, actor }));
        // Warm the server now rather than on the first request that wants it.
        // The cost is the same either way and this way it overlaps with the
        // user reading code instead of with their first question.
        self.refresh();
    }

    /// Ask for the graph and the repository mapping to be brought up to date.
    pub fn refresh(&self) {
        if let Some(actor) = self.0.load().actor.as_ref() {
            actor.refresh();
        }
    }

    /// The configuration in force. Cloned: it is four small fields, and a
    /// borrow would outlive the snapshot it came from.
    #[must_use]
    pub fn config(&self) -> BazelConfig {
        self.0.load().config.clone()
    }
}

enum Message {
    Refresh,
    Stop,
}

/// The Bazel thread, and the only way to reach it.
///
/// Dropping this stops the thread, interrupting whatever it is running.
pub struct Actor {
    tx: Sender<Message>,
    /// The invocation in flight, for a superseding refresh to interrupt.
    ///
    /// Held rather than derived because the thread is blocked inside the
    /// invocation while it runs and cannot answer for itself.
    running: Arc<Mutex<Option<Interrupt>>>,
    thread: Option<JoinHandle<()>>,
}

impl Actor {
    /// Start the thread. It idles until asked for a refresh.
    #[must_use]
    pub fn spawn(client: BazelClient, index: IndexHandle) -> Self {
        let (tx, rx) = channel();
        let running = Arc::new(Mutex::new(None));
        let thread = {
            let running = Arc::clone(&running);
            std::thread::Builder::new()
                .name("bazel".to_owned())
                .spawn(move || serve(&client, &rx, &running, &index))
                .expect("spawning the bazel thread")
        };
        Self {
            tx,
            running,
            thread: Some(thread),
        }
    }

    /// Ask for the graph tier to be brought up to date.
    ///
    /// Never blocks. A refresh already in flight is interrupted and its result
    /// discarded, because the answer it is computing is about a tree that has
    /// already moved on.
    pub fn refresh(&self) {
        drop(self.tx.send(Message::Refresh));
        self.interrupt();
    }

    fn interrupt(&self) {
        if let Ok(running) = self.running.lock()
            && let Some(interrupt) = running.as_ref()
        {
            interrupt.send();
        }
    }
}

impl Drop for Actor {
    fn drop(&mut self) {
        drop(self.tx.send(Message::Stop));
        self.interrupt();
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

/// The thread body: one invocation at a time, newest request wins.
fn serve(
    client: &BazelClient,
    rx: &Receiver<Message>,
    running: &Mutex<Option<Interrupt>>,
    index: &IndexHandle,
) {
    while let Ok(message) = rx.recv() {
        if matches!(message, Message::Stop) {
            return;
        }
        // Everything queued behind this one asks for the same thing, so the
        // queue collapses to a single bit: refresh once more, or stop.
        let mut again = true;
        while again {
            if drained_stop(rx) {
                return;
            }
            refresh_once(client, running, index);
            match drained(rx) {
                Pending::Refresh => again = true,
                Pending::Idle => again = false,
                Pending::Stop => return,
            }
        }
    }
}

/// Run one query, and say what it cost.
///
/// There is no wall-clock timeout. A cold `query //...` is 16.76 s at 60k
/// targets and minutes above that, so a deadline short enough to catch a hang
/// is short enough to kill legitimate work and never converge. A superseded
/// refresh is stopped by [`Actor::refresh`] instead, and Bazel's output
/// serialisation runs to completion regardless — so the result is *discarded*
/// rather than relied upon to stop.
fn refresh_once(client: &BazelClient, running: &Mutex<Option<Interrupt>>, index: &IndexHandle) {
    // The mapping first, and cheap: one bounded command, where the query below
    // can run for minutes on a cold repository, and every external label in the
    // workspace is unresolvable until it lands.
    match crate::repos::Repos::read(client) {
        Ok(repos) => {
            tracing::info!(repositories = repos.len(), "repository mapping");
            index.store_repos(repos);
        }
        Err(err) => tracing::warn!("the repository mapping is unavailable: {err:#}"),
    }

    let started = Instant::now();
    let query = crate::graph::query(client, |interrupt| {
        if let Ok(mut slot) = running.lock() {
            *slot = Some(interrupt);
        }
    });
    if let Ok(mut slot) = running.lock() {
        *slot = None;
    }

    match query {
        Ok(query) if query.outcome.ok() => {
            tracing::info!(
                targets = query.tier.len(),
                ms = started.elapsed().as_millis(),
                "graph refresh"
            );
            index.store_graph(query.tier);
        }
        Ok(query) => tracing::warn!(
            status = query.outcome.status,
            "bazel query declined: {}",
            query.outcome.stderr.lines().next_back().unwrap_or_default()
        ),
        Err(err) => tracing::warn!("bazel query could not run: {err:#}"),
    }
}

/// Whether a stop is waiting, consuming everything up to it.
fn drained_stop(rx: &Receiver<Message>) -> bool {
    loop {
        match rx.try_recv() {
            Ok(Message::Stop) | Err(TryRecvError::Disconnected) => return true,
            Ok(Message::Refresh) => {}
            Err(TryRecvError::Empty) => return false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Pending {
    Idle,
    Refresh,
    Stop,
}

/// What arrived while the last refresh ran.
fn drained(rx: &Receiver<Message>) -> Pending {
    let mut wanted = false;
    loop {
        match rx.try_recv() {
            Ok(Message::Refresh) => wanted = true,
            Ok(Message::Stop) | Err(TryRecvError::Disconnected) => return Pending::Stop,
            Err(TryRecvError::Empty) if wanted => return Pending::Refresh,
            Err(TryRecvError::Empty) => return Pending::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_survives_queue_draining() {
        let (tx, rx) = channel();
        tx.send(Message::Refresh).unwrap();
        tx.send(Message::Stop).unwrap();
        assert_eq!(drained(&rx), Pending::Stop);
    }

    #[test]
    fn refreshes_collapse() {
        let (tx, rx) = channel();
        tx.send(Message::Refresh).unwrap();
        tx.send(Message::Refresh).unwrap();
        assert_eq!(drained(&rx), Pending::Refresh);
        assert_eq!(drained(&rx), Pending::Idle);
    }
}
