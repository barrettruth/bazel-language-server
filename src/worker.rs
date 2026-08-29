//! Bounded execution for work that must not stall the protocol loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use shared_child::SharedChild;

type Job<T> = Box<dyn FnOnce() -> T + Send>;

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
        let (jobs, pending) = crossbeam_channel::unbounded::<Job<T>>();
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

    pub fn execute(&self, job: impl FnOnce() -> T + Send + 'static) {
        if let Some(jobs) = &self.jobs {
            drop(jobs.send(Box::new(job)));
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

#[cfg(all(test, unix))]
mod tests {
    use std::process::{Command, Stdio};

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
}
