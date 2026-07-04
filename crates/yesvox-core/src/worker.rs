//! Off-thread job worker (#22).
//!
//! The GUI must never call `Session::render` / `export` / `RenderQueue::run`
//! synchronously on the UI thread — a non-trivial vocal freezes the window.
//! This module runs any `FnOnce(&JobContext<T>) -> Result<T, String>` on a
//! worker thread and delivers **monotonic progress** followed by a single
//! **terminal** event (`Done`/`Failed`) over one channel the caller polls once
//! per frame. A job-body panic is caught ([`std::panic::catch_unwind`]) and
//! surfaced as a friendly `Failed` event, never a process abort. Cancellation
//! is cooperative: the job polls [`JobContext::is_canceled`] — today the batch
//! job honors it between files; render/export run to completion once started
//! (`Session::render` has no stage-boundary cancel polling yet).
//!
//! The worker is payload-agnostic — render/export/batch specifics live in the
//! job closure — so it is testable in `yesvox-core` without a GUI.

use std::any::Any;
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A progress or terminal event from a running job.
#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent<T> {
    /// Fractional progress in `[0, 1]`, guaranteed monotonic non-decreasing.
    Progress(f32),
    /// The job completed and produced `T`.
    Done(T),
    /// The job returned an error, or its body panicked (message preserved).
    Failed(String),
}

impl<T> JobEvent<T> {
    /// Whether this is a terminal event (`Done`/`Failed`).
    pub fn is_terminal(&self) -> bool {
        matches!(self, JobEvent::Done(_) | JobEvent::Failed(_))
    }
}

/// Handed to the job body (on the worker thread): report progress and check for
/// a cancellation request. Not `Sync`; it lives only on the worker thread.
pub struct JobContext<T> {
    tx: Sender<JobEvent<T>>,
    cancel: Arc<AtomicBool>,
    last: Cell<f32>,
}

impl<T> JobContext<T> {
    /// Report fractional progress. Clamped to `[0, 1]` and forced monotonic
    /// non-decreasing (a regression is pinned to the last value), so consumers
    /// can drive a progress bar without sanity-checking. Sub-0.1% increments are
    /// coalesced: a job reporting per-sample progress would otherwise queue
    /// millions of events between UI polls (the channel is unbounded).
    pub fn progress(&self, fraction: f32) {
        let clamped = fraction.clamp(0.0, 1.0).max(self.last.get());
        if clamped - self.last.get() >= 1e-3 || (clamped >= 1.0 && self.last.get() < 1.0) {
            self.last.set(clamped);
            let _ = self.tx.send(JobEvent::Progress(clamped));
        }
    }

    /// Whether the caller has requested cancellation. Long-running jobs should
    /// poll this and return early (cooperatively).
    pub fn is_canceled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A handle to a running job: poll it for events, or request cancellation.
pub struct JobHandle<T> {
    events: Receiver<JobEvent<T>>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    finished: bool,
}

/// Spawn `job` on a worker thread. The returned [`JobHandle`] delivers progress
/// and exactly one terminal event, in order, on a single channel.
pub fn spawn<T, F>(job: F) -> JobHandle<T>
where
    T: Send + 'static,
    F: FnOnce(&JobContext<T>) -> Result<T, String> + Send + 'static,
{
    let (tx, rx) = channel::<JobEvent<T>>();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);

    let handle = std::thread::spawn(move || {
        let ctx = JobContext {
            tx: tx.clone(),
            cancel: worker_cancel,
            last: Cell::new(0.0),
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| job(&ctx)));
        let terminal = match outcome {
            Ok(Ok(value)) => JobEvent::Done(value),
            Ok(Err(message)) => JobEvent::Failed(message),
            Err(panic) => JobEvent::Failed(panic_message(panic)),
        };
        // Sent strictly after every progress event on the same channel, so the
        // consumer always sees progress before the terminal event.
        let _ = tx.send(terminal);
    });

    JobHandle {
        events: rx,
        cancel,
        handle: Some(handle),
        finished: false,
    }
}

fn panic_message(panic: Box<dyn Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        format!("worker panicked: {s}")
    } else if let Some(s) = panic.downcast_ref::<String>() {
        format!("worker panicked: {s}")
    } else {
        "worker panicked".to_string()
    }
}

impl<T> JobHandle<T> {
    /// Drain all events available right now (call once per frame). A terminal
    /// event marks the job finished; further polls return nothing.
    pub fn poll(&mut self) -> Vec<JobEvent<T>> {
        let mut out = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    if event.is_terminal() {
                        self.finished = true;
                    }
                    out.push(event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        out
    }

    /// Request cooperative cancellation. The job observes it via
    /// [`JobContext::is_canceled`]; it may still deliver a `Done` with a partial
    /// result it chose (e.g. a batch summary marked canceled).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Whether a terminal event has been observed via [`poll`](Self::poll).
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Block until the terminal event, collecting every progress fraction first.
    /// Test/shutdown convenience; the GUI uses [`poll`](Self::poll).
    pub fn wait(mut self) -> (Vec<f32>, JobEvent<T>) {
        let mut progress = Vec::new();
        loop {
            match self.events.recv() {
                Ok(JobEvent::Progress(p)) => progress.push(p),
                Ok(terminal) => {
                    self.finished = true;
                    if let Some(h) = self.handle.take() {
                        let _ = h.join();
                    }
                    return (progress, terminal);
                }
                Err(_) => return (progress, JobEvent::Failed("worker vanished".to_string())),
            }
        }
    }
}

impl<T> Drop for JobHandle<T> {
    fn drop(&mut self) {
        // Request cancel, then give the worker a short grace period to finish.
        // Joining unconditionally would freeze the dropping thread (the UI, on
        // app close) for the full duration of a job that can't observe
        // cancellation mid-DSP-call — the exact freeze #22 exists to eliminate.
        // Past the grace period the worker is detached instead: that is safe
        // because file writes are atomic (temp + rename, #31), so an orphaned
        // worker can never leave a partial file at an export target, and
        // process exit reaps the thread.
        self.cancel.store(true, Ordering::Relaxed);
        let Some(h) = self.handle.take() else { return };
        if self.finished {
            let _ = h.join(); // terminal event already seen; the join is immediate
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(200);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(ev) if ev.is_terminal() => {
                    let _ = h.join();
                    return;
                }
                Ok(_) => continue, // progress; keep draining within the grace period
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = h.join(); // worker is done sending; it exits imminently
                    return;
                }
                Err(RecvTimeoutError::Timeout) => return, // detach (see above)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_monotonic_progress_then_done_value() {
        let handle = spawn(|ctx| {
            ctx.progress(0.2);
            ctx.progress(0.5);
            ctx.progress(1.0);
            Ok(42u32)
        });
        let (progress, terminal) = handle.wait();
        assert!(
            progress.windows(2).all(|w| w[1] >= w[0]),
            "progress not monotonic: {progress:?}"
        );
        assert_eq!(terminal, JobEvent::Done(42));
    }

    #[test]
    fn regressing_progress_is_clamped_monotonic() {
        let handle = spawn(|ctx| {
            ctx.progress(0.6);
            ctx.progress(0.2); // regression — must be pinned to 0.6
            ctx.progress(0.9);
            Ok(())
        });
        let (progress, _) = handle.wait();
        assert!(
            progress.windows(2).all(|w| w[1] >= w[0]),
            "regression leaked through: {progress:?}"
        );
        assert!(progress.contains(&0.6));
    }

    #[test]
    fn out_of_range_progress_is_clamped_to_unit_interval() {
        let handle = spawn(|ctx| {
            ctx.progress(-1.0);
            ctx.progress(5.0);
            Ok(())
        });
        let (progress, _) = handle.wait();
        assert!(progress.iter().all(|&p| (0.0..=1.0).contains(&p)));
    }

    #[test]
    fn progress_floods_are_coalesced() {
        let handle = spawn(|ctx| {
            for i in 0..100_000u32 {
                ctx.progress(i as f32 / 100_000.0);
            }
            Ok(())
        });
        let (progress, _) = handle.wait();
        assert!(
            progress.len() <= 1_001,
            "a per-iteration progress flood must coalesce, got {} events",
            progress.len()
        );
    }

    #[test]
    fn dropping_a_live_uncancelable_job_does_not_block() {
        // A job that never observes cancellation (like a render mid-DSP-call)
        // must not stall the dropping thread — the UI drops handles on app
        // close. Grace period is 200 ms; 2 s is 10x headroom for slow runners.
        let start = std::time::Instant::now();
        let handle = spawn(|_ctx| -> Result<(), String> {
            std::thread::sleep(std::time::Duration::from_secs(5));
            Ok(())
        });
        drop(handle);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "Drop blocked on a job that ignores cancellation"
        );
    }

    #[test]
    fn a_panicking_job_becomes_a_failed_event_not_an_abort() {
        // If this aborted the process the whole test binary would die; reaching
        // the assertion proves the panic was contained.
        let handle = spawn(|_ctx| -> Result<(), String> {
            panic!("boom in the worker");
        });
        let (_, terminal) = handle.wait();
        match terminal {
            JobEvent::Failed(msg) => assert!(msg.contains("boom"), "message lost: {msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn an_error_returning_job_becomes_a_failed_event() {
        let handle = spawn(|_ctx| -> Result<u8, String> { Err("nope".to_string()) });
        let (_, terminal) = handle.wait();
        assert_eq!(terminal, JobEvent::Failed("nope".to_string()));
    }

    #[test]
    fn cancellation_is_observed_by_a_cooperative_job() {
        let handle = spawn(|ctx| {
            for i in 0..100_000_000u64 {
                if ctx.is_canceled() {
                    return Ok("canceled");
                }
                if i % 4096 == 0 {
                    ctx.progress((i as f32 / 100_000_000.0).min(0.99));
                }
                std::hint::spin_loop();
            }
            Ok("ran-to-completion")
        });
        handle.cancel();
        let (_, terminal) = handle.wait();
        assert_eq!(
            terminal,
            JobEvent::Done("canceled"),
            "job ignored the cancel request"
        );
    }

    #[test]
    fn poll_drains_events_and_marks_finished() {
        use std::time::{Duration, Instant};
        let mut handle = spawn(|ctx| {
            ctx.progress(1.0);
            Ok(7u8)
        });
        // Sleep-poll (not yield_now): under the full parallel test suite the
        // worker thread can be starved, and yield_now is only a hint — sleeping
        // reliably lets it run. Generous deadline so this can never flake.
        let mut done = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            for ev in handle.poll() {
                if let JobEvent::Done(v) = ev {
                    done = Some(v);
                }
            }
            if handle.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(done, Some(7));
        assert!(handle.is_finished());
        assert!(handle.poll().is_empty(), "no events after terminal");
    }
}
