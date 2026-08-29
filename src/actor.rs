//! Serializes Bazel work behind one interruptible thread.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Result, bail};

use crate::bazel::{BazelClient, BazelConfig, Interrupt};
use crate::index::{IndexHandle, Tier};
use crate::repos::Repos;

pub struct Bazel {
    tx: Sender<Message>,
    running: Arc<Mutex<Option<Interrupt>>>,
    thread: Option<JoinHandle<()>>,
}

impl Bazel {
    #[must_use]
    pub fn spawn(root: Option<PathBuf>, index: IndexHandle) -> Self {
        let (tx, rx) = channel();
        let running = Arc::new(Mutex::new(None));
        let thread = {
            let running = Arc::clone(&running);
            std::thread::Builder::new()
                .name("bazel".to_owned())
                .spawn(move || serve(root.as_deref(), &rx, &running, &index))
                .expect("spawning the bazel thread")
        };
        Self {
            tx,
            running,
            thread: Some(thread),
        }
    }

    pub fn reconfigure(&self, config: BazelConfig) {
        drop(self.tx.send(Message::Configure(config)));
        self.interrupt();
    }

    pub fn refresh(&self) {
        drop(self.tx.send(Message::Refresh));
        self.interrupt();
    }

    pub fn run_target(&self, verb: &str, label: &str) -> Result<()> {
        if !matches!(verb, "build" | "run" | "test") {
            bail!("unsupported Bazel command: {verb}");
        }
        self.tx
            .send(Message::Run {
                verb: verb.to_owned(),
                label: label.to_owned(),
            })
            .map_err(|_| anyhow::anyhow!("the Bazel subsystem has stopped"))?;
        self.interrupt();
        Ok(())
    }

    fn interrupt(&self) {
        if let Ok(running) = self.running.lock()
            && let Some(interrupt) = running.as_ref()
        {
            interrupt.send();
        }
    }
}

impl Drop for Bazel {
    fn drop(&mut self) {
        drop(self.tx.send(Message::Stop));
        self.interrupt();
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

fn serve(
    root: Option<&Path>,
    rx: &Receiver<Message>,
    running: &Mutex<Option<Interrupt>>,
    index: &IndexHandle,
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
            Message::Configure(next) if config.as_ref() == Some(&next) => {}
            Message::Configure(next) => {
                config = Some(next.clone());
                client = None;
                index.store_graph(Tier::default());
                index.store_repos(Repos::default());
                let Some(root) = root else { continue };
                let candidate = BazelClient::new(next, root.to_path_buf());
                let probe = candidate.probe_started(|child| set_running(running, child));
                clear_running(running);
                drain(rx, &mut pending);
                if pending.iter().any(Message::reconfigures_or_stops) {
                    continue;
                }
                match probe {
                    Ok(probe) => {
                        tracing::info!(
                            version = %probe.version,
                            rule_schemas = probe.capabilities.rule_classes,
                            repo_mapping = probe.capabilities.repo_mapping,
                            "bazel"
                        );
                        client = Some(candidate);
                        pending.push_back(Message::Refresh);
                    }
                    Err(err) => tracing::warn!("the Bazel tier is unavailable: {err:#}"),
                }
            }
            Message::Refresh => {
                let Some(client) = client.as_ref() else {
                    continue;
                };
                let refreshed = refresh(client, running);
                clear_running(running);
                drain(rx, &mut pending);
                if pending.iter().any(Message::supersedes_refresh) {
                    tracing::debug!("discarded a superseded Bazel refresh");
                    continue;
                }
                publish(refreshed, index);
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

impl Message {
    fn reconfigures_or_stops(&self) -> bool {
        matches!(self, Self::Configure(_) | Self::Stop)
    }

    fn supersedes_refresh(&self) -> bool {
        matches!(self, Self::Configure(_) | Self::Refresh | Self::Stop)
    }
}

struct Refreshed {
    repos: Result<Repos>,
    graph: Result<crate::graph::Query>,
    elapsed: std::time::Duration,
}

fn refresh(client: &BazelClient, running: &Mutex<Option<Interrupt>>) -> Refreshed {
    let repos = Repos::read_started(client, |child| set_running(running, child));
    let started = Instant::now();
    let graph = crate::graph::query(client, |child| set_running(running, child));
    Refreshed {
        repos,
        graph,
        elapsed: started.elapsed(),
    }
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

fn set_running(running: &Mutex<Option<Interrupt>>, child: Interrupt) {
    if let Ok(mut slot) = running.lock() {
        *slot = Some(child);
    }
}

fn clear_running(running: &Mutex<Option<Interrupt>>) {
    if let Ok(mut slot) = running.lock() {
        *slot = None;
    }
}

fn drain(rx: &Receiver<Message>, pending: &mut VecDeque<Message>) {
    loop {
        match rx.try_recv() {
            Ok(message) => pending.push_back(message),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_and_refresh_supersede_a_result() {
        assert!(Message::Stop.supersedes_refresh());
        assert!(Message::Refresh.supersedes_refresh());
        assert!(Message::Configure(BazelConfig::default()).supersedes_refresh());
        assert!(
            !Message::Run {
                verb: "build".to_owned(),
                label: "//:all".to_owned(),
            }
            .supersedes_refresh()
        );
    }

    #[test]
    fn only_configuration_and_stop_supersede_a_probe() {
        assert!(Message::Stop.reconfigures_or_stops());
        assert!(Message::Configure(BazelConfig::default()).reconfigures_or_stops());
        assert!(!Message::Refresh.reconfigures_or_stops());
    }
}
