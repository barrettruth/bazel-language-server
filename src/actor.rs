//! Serializes Bazel work behind one interruptible thread.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SendError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Result, bail};

use crate::bazel::{BazelClient, BazelConfig, Interrupt};
use crate::bazelrc::{CatalogHandle, FlagCatalog};
use crate::index::{IndexHandle, Tier};
use crate::repos::Repos;

pub struct Bazel {
    tx: Sender<Message>,
    running: Arc<Mutex<Running>>,
    thread: Option<JoinHandle<()>>,
}

impl Bazel {
    #[must_use]
    pub fn spawn(root: Option<PathBuf>, index: IndexHandle, catalog: CatalogHandle) -> Self {
        let (tx, rx) = channel();
        let running = Arc::new(Mutex::new(Running::default()));
        let thread = {
            let running = Arc::clone(&running);
            std::thread::Builder::new()
                .name("bazel".to_owned())
                .spawn(move || serve(root.as_deref(), &rx, &running, &index, &catalog))
                .expect("spawning the bazel thread")
        };
        Self {
            tx,
            running,
            thread: Some(thread),
        }
    }

    pub fn reconfigure(&self, config: BazelConfig) {
        drop(self.enqueue(Message::Configure(config)));
    }

    pub fn refresh(&self) {
        drop(self.enqueue(Message::Refresh));
    }

    pub fn run_target(&self, verb: &str, label: &str) -> Result<()> {
        if !matches!(verb, "build" | "run" | "test") {
            bail!("unsupported Bazel command: {verb}");
        }
        self.enqueue(Message::Run {
            verb: verb.to_owned(),
            label: label.to_owned(),
        })
        .map_err(|_| anyhow::anyhow!("the Bazel subsystem has stopped"))?;
        Ok(())
    }

    /// Interrupt the registered command and enqueue its successor as one
    /// operation, so this call cannot race ahead and interrupt its own work.
    fn enqueue(&self, message: Message) -> Result<(), SendError<Message>> {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if running
            .operation
            .is_some_and(|operation| message.supersedes(operation))
        {
            running.superseded = true;
            if let Some(interrupt) = running.interrupt.as_ref() {
                interrupt.send();
            }
        }
        self.tx.send(message)
    }
}

impl Drop for Bazel {
    fn drop(&mut self) {
        drop(self.enqueue(Message::Stop));
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

enum Message {
    Configure(BazelConfig),
    Refresh,
    Run { verb: String, label: String },
    Stop,
}

#[derive(Clone, Copy)]
enum Operation {
    Probe,
    Refresh,
}

#[derive(Default)]
struct Running {
    operation: Option<Operation>,
    interrupt: Option<Interrupt>,
    superseded: bool,
}

fn serve(
    root: Option<&Path>,
    rx: &Receiver<Message>,
    running: &Mutex<Running>,
    index: &IndexHandle,
    catalog: &CatalogHandle,
) {
    let mut pending = VecDeque::new();
    let mut config = None;
    let mut client = None;

    loop {
        let message = match pending.pop_front() {
            Some(message) => message,
            None => match rx.recv() {
                Ok(message) => message,
                Err(_) => return,
            },
        };
        match message {
            Message::Stop => return,
            Message::Configure(next)
                if config.as_ref() == Some(&next) && (client.is_some() || !next.enable) => {}
            Message::Configure(next) => {
                client = None;
                index.store_graph(Tier::default());
                index.store_repos(Repos::default());
                catalog.clear();
                let Some(root) = root else {
                    config = Some(next);
                    continue;
                };
                let candidate = BazelClient::new(next.clone(), root.to_path_buf());
                if !prepare(Operation::Probe, running, rx, &mut pending) {
                    continue;
                }
                let probe = candidate.probe_started(|child| set_running(running, child));
                if !finish(Operation::Probe, running, rx, &mut pending) {
                    continue;
                }
                config = Some(next);
                match probe {
                    Ok(probe) => {
                        let Some(flag_catalog) =
                            read_flag_catalog(&candidate, &probe, running, rx, &mut pending)
                        else {
                            continue;
                        };
                        tracing::info!(
                            version = %probe.version,
                            rule_schemas = probe.capabilities.rule_classes,
                            repo_mapping = probe.capabilities.repo_mapping,
                            "bazel"
                        );
                        match flag_catalog {
                            Ok(Some(flags)) => {
                                tracing::info!(flags = flags.flags().count(), "bazel flag catalog");
                                catalog.store(flags);
                            }
                            Err(err) => {
                                tracing::warn!("the Bazel flag catalog is unavailable: {err:#}");
                            }
                            Ok(None) => tracing::info!(
                                version = %probe.version,
                                "Bazelrc flag intelligence requires Bazel 8.7"
                            ),
                        }
                        client = Some(candidate);
                        pending.push_back(Message::Refresh);
                        coalesce_refreshes(&mut pending);
                    }
                    Err(err) => tracing::warn!("the Bazel tier is unavailable: {err:#}"),
                }
            }
            Message::Refresh => {
                index.store_graph(Tier::default());
                index.store_repos(Repos::default());
                let Some(client) = client.as_ref() else {
                    continue;
                };
                let Some(refreshed) = refresh(client, running, rx, &mut pending) else {
                    tracing::debug!("discarded a superseded Bazel refresh");
                    preserve_refresh(&mut pending);
                    continue;
                };
                if !publish_current(refreshed, running, rx, &mut pending, index) {
                    tracing::debug!("discarded a superseded Bazel refresh");
                    preserve_refresh(&mut pending);
                }
            }
            Message::Run { verb, label } => {
                let Some(client) = client.as_ref() else {
                    tracing::warn!(%verb, %label, "the Bazel tier is unavailable");
                    continue;
                };
                tracing::info!(%verb, %label, "bazel");
                if let Err(err) = client.spawn(&[&verb, &label]) {
                    tracing::warn!("running `bazel {verb} {label}`: {err:#}");
                }
            }
        }
    }
}

fn read_flag_catalog(
    client: &BazelClient,
    probe: &crate::bazel::Probe,
    running: &Mutex<Running>,
    rx: &Receiver<Message>,
    pending: &mut VecDeque<Message>,
) -> Option<Result<Option<FlagCatalog>>> {
    if probe.version.major != 8 || probe.version.minor != 7 {
        return Some(Ok(None));
    }
    if !prepare(Operation::Probe, running, rx, pending) {
        return None;
    }
    let catalog = FlagCatalog::read_started(client, probe.reported.clone(), |child| {
        set_running(running, child);
    })
    .map(Some);
    finish(Operation::Probe, running, rx, pending).then_some(catalog)
}

impl Message {
    fn supersedes(&self, operation: Operation) -> bool {
        match operation {
            Operation::Probe => matches!(self, Self::Configure(_) | Self::Stop),
            Operation::Refresh => matches!(
                self,
                Self::Configure(_) | Self::Refresh | Self::Run { .. } | Self::Stop
            ),
        }
    }
}

struct Refreshed {
    repos: Result<Repos>,
    graph: Result<crate::graph::Query>,
    elapsed: std::time::Duration,
}

fn refresh(
    client: &BazelClient,
    running: &Mutex<Running>,
    rx: &Receiver<Message>,
    pending: &mut VecDeque<Message>,
) -> Option<Refreshed> {
    if !prepare(Operation::Refresh, running, rx, pending) {
        return None;
    }
    let repos = Repos::read_started(client, |child| set_running(running, child));
    if !finish(Operation::Refresh, running, rx, pending)
        || !prepare(Operation::Refresh, running, rx, pending)
    {
        return None;
    }
    let started = Instant::now();
    let graph = crate::graph::query(client, |child| set_running(running, child));
    Some(Refreshed {
        repos,
        graph,
        elapsed: started.elapsed(),
    })
}

fn publish_current(
    refreshed: Refreshed,
    running: &Mutex<Running>,
    rx: &Receiver<Message>,
    pending: &mut VecDeque<Message>,
    index: &IndexHandle,
) -> bool {
    let mut state = running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    drain(rx, pending);
    let superseded = state.superseded
        || pending
            .iter()
            .any(|message| message.supersedes(Operation::Refresh));
    if !superseded {
        publish(refreshed, index);
    }
    *state = Running::default();
    !superseded
}

fn publish(refreshed: Refreshed, index: &IndexHandle) {
    match refreshed.repos {
        Ok(repos) => {
            tracing::info!(repositories = repos.len(), "repository mapping");
            index.store_repos(repos);
        }
        Err(err) => tracing::warn!("the repository mapping is unavailable: {err:#}"),
    }
    match refreshed.graph {
        Ok(query) if query.outcome.ok() => {
            tracing::info!(
                targets = query.tier.len(),
                ms = refreshed.elapsed.as_millis(),
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

fn prepare(
    operation: Operation,
    running: &Mutex<Running>,
    rx: &Receiver<Message>,
    pending: &mut VecDeque<Message>,
) -> bool {
    {
        let mut state = running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = Running {
            operation: Some(operation),
            interrupt: None,
            superseded: false,
        };
    }
    drain(rx, pending);
    let superseded = running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .superseded
        || pending.iter().any(|message| message.supersedes(operation));
    if superseded {
        clear_running(running);
    }
    !superseded
}

fn finish(
    operation: Operation,
    running: &Mutex<Running>,
    rx: &Receiver<Message>,
    pending: &mut VecDeque<Message>,
) -> bool {
    let mut state = running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    drain(rx, pending);
    let superseded =
        state.superseded || pending.iter().any(|message| message.supersedes(operation));
    *state = Running::default();
    !superseded
}

fn set_running(running: &Mutex<Running>, child: Interrupt) {
    let mut state = running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.superseded {
        child.send();
    }
    state.interrupt = Some(child);
}

fn clear_running(running: &Mutex<Running>) {
    *running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Running::default();
}

fn drain(rx: &Receiver<Message>, pending: &mut VecDeque<Message>) {
    while let Ok(message) = rx.try_recv() {
        pending.push_back(message);
    }
    if pending
        .iter()
        .any(|message| matches!(message, Message::Stop))
    {
        pending.clear();
        pending.push_back(Message::Stop);
        return;
    }
    coalesce_refreshes(pending);
}

fn preserve_refresh(pending: &mut VecDeque<Message>) {
    let another_refresh_will_follow = pending
        .iter()
        .any(|message| matches!(message, Message::Refresh | Message::Stop));
    if !another_refresh_will_follow {
        pending.push_back(Message::Refresh);
    }
}

/// Keep only the last refresh in a burst. Runs remain ordered around it.
fn coalesce_refreshes(pending: &mut VecDeque<Message>) {
    let mut refreshes = pending
        .iter()
        .filter(|message| matches!(message, Message::Refresh))
        .count();
    pending.retain(|message| {
        if matches!(message, Message::Refresh) {
            refreshes -= 1;
            refreshes == 0
        } else {
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_and_refresh_supersede_a_result() {
        for message in [
            Message::Stop,
            Message::Refresh,
            Message::Configure(BazelConfig::default()),
            Message::Run {
                verb: "build".to_owned(),
                label: "//:all".to_owned(),
            },
        ] {
            assert!(message.supersedes(Operation::Refresh));
        }
    }

    #[test]
    fn only_configuration_and_stop_supersede_a_probe() {
        assert!(Message::Stop.supersedes(Operation::Probe));
        assert!(Message::Configure(BazelConfig::default()).supersedes(Operation::Probe));
        assert!(!Message::Refresh.supersedes(Operation::Probe));
    }

    #[test]
    fn only_the_last_refresh_in_a_burst_survives() {
        let mut pending = VecDeque::from([
            Message::Refresh,
            Message::Run {
                verb: "build".to_owned(),
                label: "//:one".to_owned(),
            },
            Message::Refresh,
            Message::Run {
                verb: "test".to_owned(),
                label: "//:two".to_owned(),
            },
        ]);
        coalesce_refreshes(&mut pending);
        assert_eq!(pending.len(), 3);
        assert!(matches!(pending[0], Message::Run { .. }));
        assert!(matches!(pending[1], Message::Refresh));
        assert!(matches!(pending[2], Message::Run { .. }));
    }

    #[test]
    fn stop_discards_every_pending_operation() {
        let (tx, rx) = channel();
        tx.send(Message::Refresh).unwrap();
        tx.send(Message::Run {
            verb: "build".to_owned(),
            label: "//:one".to_owned(),
        })
        .unwrap();
        tx.send(Message::Stop).unwrap();
        let mut pending = VecDeque::new();
        drain(&rx, &mut pending);
        assert_eq!(pending.len(), 1);
        assert!(matches!(pending[0], Message::Stop));
    }

    #[test]
    fn supersession_preserves_one_refresh() {
        let mut pending = VecDeque::from([Message::Configure(BazelConfig::default())]);
        preserve_refresh(&mut pending);
        assert_eq!(pending.len(), 2);
        assert!(matches!(pending[0], Message::Configure(_)));
        assert!(matches!(pending[1], Message::Refresh));
    }
}
