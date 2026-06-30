//! Off-thread worker pool (plan §1.5).
//!
//! A static pool of worker threads built once; chunk-denoise jobs are dispatched
//! to it so heavy inference never runs on the audio path. v1's offline render
//! has no audio callback, but keeping inference off the calling thread is the
//! same discipline the later preview path needs. Communication is via channels;
//! the lock-free `rtrb`/`Parker` rings the plan describes are a preview-path
//! optimization deferred with the rest of the realtime preview.

use super::engine::Denoiser;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce() + Send + 'static>;

enum Message {
    Run(Job),
    Stop,
}

/// A fixed pool of worker threads.
pub struct WorkerPool {
    sender: Sender<Message>,
    workers: Vec<JoinHandle<()>>,
    size: usize,
}

impl WorkerPool {
    /// Build a pool with `workers` threads (at least one).
    pub fn new(workers: usize) -> Self {
        let size = workers.max(1);
        let (sender, receiver) = channel::<Message>();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut handles = Vec::with_capacity(size);
        for _ in 0..size {
            let rx = Arc::clone(&receiver);
            handles.push(std::thread::spawn(move || loop {
                let msg = {
                    let lock = rx.lock().unwrap();
                    lock.recv()
                };
                match msg {
                    Ok(Message::Run(job)) => job(),
                    Ok(Message::Stop) | Err(_) => break,
                }
            }));
        }
        WorkerPool {
            sender,
            workers: handles,
            size,
        }
    }

    /// Number of worker threads.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Denoise `chunks` on the pool, returning outputs in the same order. The
    /// engine runs on worker threads, never the caller's thread.
    pub fn denoise_chunks(
        &self,
        chunks: Vec<Vec<f32>>,
        sample_rate: f32,
        denoiser: &Arc<dyn Denoiser>,
    ) -> Vec<Vec<f32>> {
        let n = chunks.len();
        let (tx, rx) = channel::<(usize, Vec<f32>)>();
        for (i, chunk) in chunks.into_iter().enumerate() {
            let den = Arc::clone(denoiser);
            let tx = tx.clone();
            self.sender
                .send(Message::Run(Box::new(move || {
                    let out = den.denoise(&chunk, sample_rate);
                    let _ = tx.send((i, out));
                })))
                .expect("worker pool send");
        }
        drop(tx);
        let mut results: Vec<Option<Vec<f32>>> = (0..n).map(|_| None).collect();
        for _ in 0..n {
            let (i, out) = rx.recv().expect("worker pool recv");
            results[i] = Some(out);
        }
        results.into_iter().map(|r| r.unwrap()).collect()
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        for _ in 0..self.size {
            let _ = self.sender.send(Message::Stop);
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::thread::ThreadId;

    /// A denoiser that records which thread ran it (and doubles its input so we
    /// can confirm it actually executed).
    struct SpyDenoiser {
        threads: Mutex<HashSet<ThreadId>>,
    }
    impl Denoiser for SpyDenoiser {
        fn denoise(&self, input: &[f32], _sr: f32) -> Vec<f32> {
            self.threads
                .lock()
                .unwrap()
                .insert(std::thread::current().id());
            input.iter().map(|x| x * 2.0).collect()
        }
    }

    #[test]
    fn runs_jobs_on_worker_threads_not_the_caller() {
        // Keep the concrete Arc to read the spy after; hand a trait-object
        // clone to the pool. Both share the same recorded-threads set.
        let spy = Arc::new(SpyDenoiser {
            threads: Mutex::new(HashSet::new()),
        });
        let dyn_spy: Arc<dyn Denoiser> = spy.clone();
        let pool = WorkerPool::new(3);
        let chunks: Vec<Vec<f32>> = (0..6).map(|k| vec![k as f32; 4]).collect();
        let out = pool.denoise_chunks(chunks, 48_000.0, &dyn_spy);

        // Results are in order and actually processed (doubled).
        assert_eq!(out[0], vec![0.0; 4]);
        assert_eq!(out[5], vec![10.0; 4]);

        // The engine ran, and never on the calling (main) thread.
        let threads = spy.threads.lock().unwrap();
        assert!(!threads.is_empty(), "engine never ran");
        assert!(
            !threads.contains(&std::thread::current().id()),
            "inference ran on the caller thread"
        );
    }
}
