//! Bounded execution for work that must not stall the protocol loop.

use std::thread::JoinHandle;

type Job<T> = Box<dyn FnOnce() -> T + Send>;

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
