//! Bounded execution for work that must not stall the protocol loop.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use shared_child::SharedChild;

type Job<T> = Box<dyn FnOnce() -> T + Send>;
type LatestJob<T> = Box<dyn FnOnce(&Cancellation) -> T + Send>;

#[derive(Clone, Default)]
pub struct Cancellation(Arc<CancellationState>);

#[derive(Default)]
struct CancellationState {
    cancelled: AtomicBool,
    process: Mutex<Option<Arc<SharedChild>>>,
}

impl Cancellation {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        if let Ok(process) = self.0.process.lock()
            && let Some(process) = process.as_ref()
        {
            drop(process.kill());
        }
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn track(&self, process: &Arc<SharedChild>) {
        if let Ok(mut running) = self.0.process.lock() {
            *running = Some(Arc::clone(process));
        }
        if self.is_cancelled() {
            drop(process.kill());
        }
    }

    pub fn clear(&self) {
        if let Ok(mut running) = self.0.process.lock() {
            running.take();
        }
    }
}

pub struct Pool<T> {
    jobs: Option<crossbeam_channel::Sender<Job<T>>>,
    threads: Vec<JoinHandle<()>>,
}

impl<T: Send + 'static> Pool<T> {
    #[must_use]
    pub fn new(size: usize, completed: &crossbeam_channel::Sender<T>) -> Self {
        let (jobs, pending) = crossbeam_channel::bounded::<Job<T>>(size * 4);
        let threads = (0..size)
            .map(|nth| {
                let pending = pending.clone();
                let completed = completed.clone();
                std::thread::Builder::new()
                    .name(format!("worker-{nth}"))
                    .spawn(move || {
                        while let Ok(job) = pending.recv() {
                            if completed.send(job()).is_err() {
                                return;
                            }
                        }
                    })
                    .expect("spawning a request worker")
            })
            .collect();
        Self {
            jobs: Some(jobs),
            threads,
        }
    }

    /// Admit work without ever waiting on the protocol thread.
    pub fn execute(&self, job: impl FnOnce() -> T + Send + 'static) -> bool {
        if let Some(jobs) = &self.jobs {
            jobs.try_send(Box::new(job)).is_ok()
        } else {
            false
        }
    }
}

impl<T> Drop for Pool<T> {
    fn drop(&mut self) {
        self.jobs.take();
        for thread in self.threads.drain(..) {
            drop(thread.join());
        }
    }
}

struct LatestState<K, T> {
    pending: VecDeque<(K, LatestJob<T>)>,
    running: Option<(K, Cancellation)>,
    stopping: bool,
}

/// One worker that replaces queued work carrying the same key.
pub struct Latest<K, T> {
    state: Arc<(Mutex<LatestState<K, T>>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl<K, T> Latest<K, T>
where
    K: Eq + Send + 'static,
    T: Send + 'static,
{
    #[must_use]
    pub fn new(completed: &crossbeam_channel::Sender<T>) -> Self {
        let state = Arc::new((
            Mutex::new(LatestState {
                pending: VecDeque::new(),
                running: None,
                stopping: false,
            }),
            Condvar::new(),
        ));
        let shared = Arc::clone(&state);
        let completed = completed.clone();
        let thread = std::thread::Builder::new()
            .name("latest-worker".to_owned())
            .spawn(move || serve_latest(&shared, &completed))
            .expect("spawning the latest-work worker");
        Self {
            state,
            thread: Some(thread),
        }
    }

    pub fn execute(&self, key: K, job: impl FnOnce(&Cancellation) -> T + Send + 'static) {
        let (state, wake) = &*self.state;
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending.retain(|(queued, _)| queued != &key);
        if let Some((running, cancellation)) = &state.running
            && running == &key
        {
            cancellation.cancel();
        }
        state.pending.push_back((key, Box::new(job)));
        wake.notify_one();
    }

    pub fn cancel(&self, key: &K) {
        let (state, _) = &*self.state;
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending.retain(|(queued, _)| queued != key);
        if let Some((running, cancellation)) = &state.running
            && running == key
        {
            cancellation.cancel();
        }
    }
}

impl<K, T> Drop for Latest<K, T> {
    fn drop(&mut self) {
        let (state, wake) = &*self.state;
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.stopping = true;
        state.pending.clear();
        if let Some((_, cancellation)) = &state.running {
            cancellation.cancel();
        }
        wake.notify_one();
        drop(state);
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

fn serve_latest<K, T>(
    shared: &Arc<(Mutex<LatestState<K, T>>, Condvar)>,
    completed: &crossbeam_channel::Sender<T>,
) where
    K: Send + 'static,
    T: Send + 'static,
{
    loop {
        let (job, cancellation) = {
            let (state, wake) = &**shared;
            let mut state = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while state.pending.is_empty() && !state.stopping {
                state = wake
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if state.stopping {
                return;
            }
            let (key, job) = state.pending.pop_front().expect("pending latest work");
            let cancellation = Cancellation::default();
            state.running = Some((key, cancellation.clone()));
            (job, cancellation)
        };
        let result = job(&cancellation);
        {
            let (state, _) = &**shared;
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .running = None;
        }
        if completed.send(result).is_err() {
            return;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn cancellation_terminates_a_tracked_process() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "exec sleep 10"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = Arc::new(SharedChild::spawn(&mut command).unwrap());
        let cancellation = Cancellation::default();
        cancellation.track(&child);
        cancellation.cancel();

        let status = child.wait().unwrap();
        assert!(!status.success());
    }

    #[test]
    fn latest_work_replaces_a_queued_job_with_the_same_key() {
        let (completed_tx, completed_rx) = crossbeam_channel::unbounded();
        let latest = Latest::new(&completed_tx);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        latest.execute("document", move |cancellation| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            cancellation.is_cancelled().then_some(1)
        });
        started_rx.recv().unwrap();
        latest.execute("document", |_| Some(2));
        latest.execute("document", |_| Some(3));
        release_tx.send(()).unwrap();

        assert_eq!(completed_rx.recv().unwrap(), Some(1));
        assert_eq!(completed_rx.recv().unwrap(), Some(3));
        assert!(completed_rx.try_recv().is_err());
    }
}
