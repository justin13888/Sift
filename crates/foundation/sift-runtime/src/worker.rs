//! D-19's blocking pool, at the size the product needs today: one thread, a coalescing queue.
//!
//! # Why this exists before the runtime does
//!
//! D-19 names two things — a work-stealing runtime for concurrent work and a blocking pool for
//! work that will not yield — and neither was built. Sync and the queue flush are blocking
//! calls from top to bottom (the transport is synchronous, the store is SQLite), so under
//! D-92's rule they belong on the **blocking pool**, and they were running on the shell's main
//! loop instead because there was nowhere else for them to go. This is the pool, with nothing
//! in it the product does not use yet.
//!
//! # One thread, and why that is enough
//!
//! Every job this pool runs takes the one session lock, so a second thread would only queue
//! behind the first on that lock rather than run beside it. One thread is also the idle cost
//! that matters: a thread parked on a condition variable takes no wakeups, which is NFR-11's
//! whole concern, and a pool sized for parallelism nobody can use would be memory spent on
//! stacks that do nothing. Widening it is a change to [`Worker::spawn`] and nothing else.
//!
//! # Coalescing is the bound
//!
//! A job equal to one already queued is not queued again. That is what bounds the queue
//! without a number: the job vocabulary is closed (a wheel fire, a sync of a named account),
//! so the queue can hold at most one of each, and a person clicking "fetch" five times while
//! a slow provider answers the first costs one more sync rather than five.
//!
//! # Teardown waits for nothing
//!
//! D-70's quit is bounded and awaits no provider call, so [`Worker::close`] discards what is
//! queued and returns. A job already running is **abandoned rather than joined**: it finishes
//! or does not on its own time, and whatever it holds keeps what it needs alive until then.
//! The thread exits the next time it looks at the queue. Nothing here ever joins it.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A handle to the pool's one thread. Dropping it closes the pool.
pub struct Worker<J> {
    shared: Arc<Shared<J>>,
}

impl<J> std::fmt::Debug for Worker<J> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A job can name an account, and an account label is the user's text. The queue's
        // length is what a reader of a debug dump needs.
        let state = self.shared.lock();
        f.debug_struct("Worker")
            .field("queued", &state.queue.len())
            .field("busy", &state.busy)
            .field("closed", &state.closed)
            .finish()
    }
}

struct Shared<J> {
    state: Mutex<State<J>>,
    /// Signalled when a job arrives, when the pool closes, and when the thread goes idle.
    changed: Condvar,
}

struct State<J> {
    queue: VecDeque<J>,
    /// A job is running. Separate from the queue, because "nothing queued" is not "nothing
    /// happening" and a caller waiting for the second must not be satisfied by the first.
    busy: bool,
    closed: bool,
}

impl<J> Shared<J> {
    /// The state, whether or not a job panicked while holding it.
    ///
    /// **The lock is never held across a job**, so a poisoned one can only mean a panic in the
    /// few lines of bookkeeping below, and the state they leave is still coherent. Refusing
    /// every later submission because of it would stop sync for the life of the process.
    fn lock(&self) -> std::sync::MutexGuard<'_, State<J>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What [`Worker::submit`] did with a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submitted {
    /// It is queued and will run.
    Queued,
    /// An equal job was already queued, and that one will do this one's work.
    Coalesced,
    /// The pool is closed. Nothing will run it.
    Closed,
}

impl<J: PartialEq + Send + 'static> Worker<J> {
    /// Start the pool's thread. `run` is called on it, once per job, in submission order.
    ///
    /// A panic inside `run` is caught at the job boundary — D-47's rule, applied where a job
    /// is the unit of discardable state — so one bad job costs that job, not the thread and
    /// with it every sync for the rest of the process.
    ///
    /// # Errors
    /// The platform would not start a thread.
    pub fn spawn(name: &str, mut run: impl FnMut(J) + Send + 'static) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                busy: false,
                closed: false,
            }),
            changed: Condvar::new(),
        });
        let theirs = Arc::clone(&shared);
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                loop {
                    let job = {
                        let mut state = theirs.lock();
                        loop {
                            if state.closed {
                                return;
                            }
                            if let Some(job) = state.queue.pop_front() {
                                state.busy = true;
                                break job;
                            }
                            state = theirs
                                .changed
                                .wait(state)
                                .unwrap_or_else(PoisonError::into_inner);
                        }
                    };
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job)));
                    theirs.lock().busy = false;
                    theirs.changed.notify_all();
                }
            })?;
        Ok(Self { shared })
    }

    /// Queue a job, and return immediately.
    pub fn submit(&self, job: J) -> Submitted {
        let mut state = self.shared.lock();
        if state.closed {
            return Submitted::Closed;
        }
        if state.queue.contains(&job) {
            return Submitted::Coalesced;
        }
        state.queue.push_back(job);
        drop(state);
        self.shared.changed.notify_all();
        Submitted::Queued
    }
}

impl<J> Worker<J> {
    /// Stop taking jobs, discard what is queued, and return **without waiting** for the one
    /// that may be running. D-70.
    pub fn close(&self) {
        let mut state = self.shared.lock();
        state.closed = true;
        state.queue.clear();
        drop(state);
        self.shared.changed.notify_all();
    }

    /// Wait until nothing is queued and nothing is running, for at most `bound`.
    ///
    /// Returns whether it got there. **Not for a shell and not for quit** — D-70 waits for
    /// nothing. It exists so a test driving the boundary can observe a job's effect, which
    /// otherwise lands at a moment the test cannot name.
    #[must_use]
    pub fn wait_idle(&self, bound: Duration) -> bool {
        let deadline = Instant::now() + bound;
        let mut state = self.shared.lock();
        while state.busy || (!state.queue.is_empty() && !state.closed) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl<J> Drop for Worker<J> {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const BOUND: Duration = Duration::from_secs(10);

    #[test]
    fn a_job_runs_off_the_submitting_thread() {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn("sift-test-worker", move |job: u32| {
            let _ = tx.send((job, std::thread::current().name().map(str::to_owned)));
        })
        .expect("spawn");
        assert_eq!(worker.submit(7), Submitted::Queued);
        let (job, thread) = rx.recv_timeout(BOUND).expect("the job never ran");
        assert_eq!(job, 7);
        assert_eq!(
            thread.as_deref(),
            Some("sift-test-worker"),
            "the job ran on the thread that submitted it, which is the stall this exists to end"
        );
    }

    #[test]
    fn an_equal_job_already_queued_is_not_queued_twice() {
        // Hold the thread inside a first job so the next two sit in the queue together.
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (ran_tx, ran_rx) = mpsc::channel();
        let worker = Worker::spawn("sift-test-coalesce", move |job: u32| {
            if job == 0 {
                let _ = release_rx.recv_timeout(BOUND);
            }
            let _ = ran_tx.send(job);
        })
        .expect("spawn");
        assert_eq!(worker.submit(0), Submitted::Queued);
        // The thread has taken job 0 once it reports busy; until then 0 is still queued and a
        // second 0 would coalesce with it, which is correct but not what this test asks.
        let started = Instant::now();
        while !worker.shared.lock().busy {
            assert!(started.elapsed() < BOUND, "the first job never started");
            std::thread::yield_now();
        }
        assert_eq!(worker.submit(1), Submitted::Queued);
        assert_eq!(
            worker.submit(1),
            Submitted::Coalesced,
            "the queue is bounded by its vocabulary only if an equal job coalesces"
        );
        release_tx.send(()).expect("release");
        assert!(worker.wait_idle(BOUND));
        let ran: Vec<u32> = ran_rx.try_iter().collect();
        assert_eq!(ran, vec![0, 1], "the coalesced job ran once");
    }

    #[test]
    fn a_panicking_job_does_not_take_the_thread_with_it() {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn("sift-test-panic", move |job: u32| {
            assert!(job != 0, "a job that panics");
            let _ = tx.send(job);
        })
        .expect("spawn");
        assert_eq!(worker.submit(0), Submitted::Queued);
        assert_eq!(worker.submit(1), Submitted::Queued);
        assert_eq!(
            rx.recv_timeout(BOUND)
                .expect("the job after the panic never ran"),
            1
        );
    }

    #[test]
    fn close_discards_the_queue_and_waits_for_nothing() {
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (ran_tx, ran_rx) = mpsc::channel();
        let worker = Worker::spawn("sift-test-close", move |job: u32| {
            if job == 0 {
                let _ = release_rx.recv_timeout(BOUND);
            }
            let _ = ran_tx.send(job);
        })
        .expect("spawn");
        assert_eq!(worker.submit(0), Submitted::Queued);
        let started = Instant::now();
        while !worker.shared.lock().busy {
            assert!(started.elapsed() < BOUND, "the first job never started");
            std::thread::yield_now();
        }
        assert_eq!(worker.submit(1), Submitted::Queued);

        // The first job is still running and blocked; close must return regardless.
        let before = Instant::now();
        worker.close();
        assert!(
            before.elapsed() < Duration::from_secs(1),
            "close waited on a running job, which D-70 forbids"
        );
        assert_eq!(worker.submit(2), Submitted::Closed);

        release_tx.send(()).expect("release");
        assert!(worker.wait_idle(BOUND));
        let ran: Vec<u32> = ran_rx.try_iter().collect();
        assert_eq!(ran, vec![0], "a job queued behind a close ran anyway");
    }
}
