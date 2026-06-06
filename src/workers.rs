//! `BoundedWorkers` — the reusable connection-concurrency primitive.
//!
//! A bounded thread-per-task dispatcher: each `dispatch` acquires one of
//! `capacity` permits (blocking with backpressure when all are held), runs the
//! task on its own thread, and releases the permit when the task finishes.
//!
//! A `MultiListenerRuntime` whose `handle_stream` offloads each request onto a
//! `BoundedWorkers` serves many connections concurrently — a long operation on
//! one connection never blocks the accept loop or other connections — while the
//! permit cap keeps a flood from spawning an unbounded thread count. This lives
//! here so every thread-per-connection daemon reuses one concurrency primitive
//! rather than reimplementing it (intent k6w1). Daemons with their own
//! concurrency model (e.g. an actor runtime) simply do not use it.

use std::sync::{Arc, Condvar, Mutex};
use std::thread;

/// A bounded set of worker threads: `dispatch` runs a task on its own thread,
/// capped at `capacity` concurrent tasks with backpressure on the caller.
#[derive(Clone)]
pub struct BoundedWorkers {
    permits: Arc<WorkerPermits>,
}

impl BoundedWorkers {
    /// A dispatcher capped at `capacity` concurrent tasks. A capacity of zero
    /// would deadlock the first `dispatch`, so it is clamped to one.
    pub fn new(capacity: usize) -> Self {
        Self {
            permits: Arc::new(WorkerPermits::new(capacity.max(1))),
        }
    }

    /// The configured concurrency cap.
    pub fn capacity(&self) -> usize {
        self.permits.capacity
    }

    /// Acquire a permit (blocking with backpressure when at capacity), then run
    /// `task` on its own thread; the permit releases when the task finishes.
    pub fn dispatch<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let permit = WorkerPermit::acquire(self.permits.clone());
        thread::spawn(move || {
            let _permit = permit;
            task();
        });
    }
}

/// A counting semaphore (`Mutex` + `Condvar`) over the available permit count.
struct WorkerPermits {
    capacity: usize,
    available: Mutex<usize>,
    released: Condvar,
}

impl WorkerPermits {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            available: Mutex::new(capacity),
            released: Condvar::new(),
        }
    }
}

/// One held permit. Releases its slot — and wakes a waiting `dispatch` — on drop.
struct WorkerPermit {
    permits: Arc<WorkerPermits>,
}

impl WorkerPermit {
    fn acquire(permits: Arc<WorkerPermits>) -> Self {
        let mut available = permits
            .available
            .lock()
            .expect("worker-permit mutex poisoned");
        while *available == 0 {
            available = permits
                .released
                .wait(available)
                .expect("worker-permit condvar poisoned");
        }
        *available -= 1;
        drop(available);
        Self { permits }
    }
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        let mut available = self
            .permits
            .available
            .lock()
            .expect("worker-permit mutex poisoned");
        *available += 1;
        self.permits.released.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn dispatch_runs_every_task() {
        let workers = BoundedWorkers::new(4);
        let (sender, receiver) = mpsc::channel();
        for index in 0..50 {
            let sender = sender.clone();
            workers.dispatch(move || sender.send(index).expect("send"));
        }
        drop(sender);
        let mut received: Vec<usize> = receiver.iter().collect();
        received.sort_unstable();
        assert_eq!(received, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn dispatch_never_exceeds_the_capacity() {
        let capacity = 3;
        let workers = BoundedWorkers::new(capacity);
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (done, finished) = mpsc::channel();
        for _ in 0..24 {
            let live = live.clone();
            let peak = peak.clone();
            let done = done.clone();
            workers.dispatch(move || {
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(20));
                live.fetch_sub(1, Ordering::SeqCst);
                done.send(()).expect("send");
            });
        }
        drop(done);
        for _ in 0..24 {
            finished.recv().expect("recv");
        }
        assert!(
            peak.load(Ordering::SeqCst) <= capacity,
            "peak concurrency {} exceeded the cap {capacity}",
            peak.load(Ordering::SeqCst),
        );
    }
}
