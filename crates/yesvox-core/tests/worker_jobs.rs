//! Worker-payload guarantees (#22): rendering and batch processing carried by
//! the off-thread `worker` produce the *same* result as the synchronous path,
//! stream monotonic progress, and honor a mid-batch cancel. These are the
//! mechanical acceptance criteria for moving processing off the UI thread —
//! verified without a GUI.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use yesvox_core::worker::{spawn, JobEvent};
use yesvox_core::{
    write_wav, AudioBuffer, BatchControl, JobStatus, Preset, RenderQueue, SampleEncoding, Session,
    Settings,
};

const SR: u32 = 48_000;

fn voice_like(seconds: f32, seed: f32) -> AudioBuffer {
    use std::f32::consts::PI;
    let n = (seconds * SR as f32) as usize;
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let amp = 0.18 * (1.0 + 0.2 * (2.0 * PI * 4.0 * t).sin());
            amp * ((2.0 * PI * (150.0 + seed) * t).sin()
                + 0.4 * (2.0 * PI * (300.0 + seed) * t).sin())
        })
        .collect();
    AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![x])
}

fn temp(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("yesvox-worker-{tag}-{}.wav", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn write_input(tag: &str, seed: f32) -> PathBuf {
    let path = temp(tag);
    write_wav(&path, &voice_like(3.0, seed)).expect("write input");
    path
}

fn rendered_output(path: &Path) -> AudioBuffer {
    let mut s = Session::new();
    s.open(path).expect("open");
    s.render().expect("render");
    s.output().expect("output").clone()
}

#[test]
fn render_via_worker_is_byte_identical_to_a_synchronous_render() {
    let input = write_input("render-identical", 0.0);

    // Synchronous reference.
    let sync = rendered_output(&input);

    // Same render, moved onto a worker thread.
    let mut off_thread = Session::new();
    off_thread.open(&input).expect("open");
    let handle = spawn(move |ctx| {
        ctx.progress(0.05);
        let mut s = off_thread;
        s.render().map_err(|e| e.to_string())?;
        ctx.progress(1.0);
        Ok(s.output().expect("output").clone())
    });
    let (progress, terminal) = handle.wait();

    assert!(!progress.is_empty(), "no progress reported");
    assert!(
        progress.last() == Some(&1.0),
        "progress did not reach 1.0: {progress:?}"
    );
    let worker_out = match terminal {
        JobEvent::Done(buf) => buf,
        other => panic!("render job failed: {other:?}"),
    };

    assert_eq!(worker_out.frames(), sync.frames(), "frame count differs");
    assert_eq!(worker_out.channel_count(), sync.channel_count());
    for ch in 0..sync.channel_count() {
        assert_eq!(
            worker_out.channel(ch),
            sync.channel(ch),
            "channel {ch} differs between worker and sync render"
        );
    }

    let _ = std::fs::remove_file(&input);
}

fn build_queue(inputs: &[PathBuf], outputs: &[PathBuf]) -> RenderQueue {
    let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
    for (i, o) in inputs.iter().zip(outputs) {
        q.enqueue(i, o);
    }
    q
}

#[test]
fn batch_via_worker_matches_sync_and_streams_monotonic_progress() {
    let inputs: Vec<PathBuf> = (0..3)
        .map(|i| write_input(&format!("batch-{i}"), i as f32 * 5.0))
        .collect();
    let out_sync: Vec<PathBuf> = (0..3)
        .map(|i| temp(&format!("batch-sync-out{i}")))
        .collect();
    let out_wrk: Vec<PathBuf> = (0..3).map(|i| temp(&format!("batch-wrk-out{i}"))).collect();

    // Synchronous reference run.
    let mut sync_q = build_queue(&inputs, &out_sync);
    let sync_summary = sync_q.run();

    // Same work off-thread, forwarding batch progress to the worker channel.
    let worker_q = build_queue(&inputs, &out_wrk);
    let handle = spawn(move |ctx| {
        let mut q = worker_q;
        let summary = q.run_with_progress(|p| {
            ctx.progress(p.completed as f32 / p.total.max(1) as f32);
            if ctx.is_canceled() {
                BatchControl::Cancel
            } else {
                BatchControl::Continue
            }
        });
        Ok((summary, q))
    });
    let (progress, terminal) = handle.wait();

    assert!(
        progress.windows(2).all(|w| w[1] >= w[0]),
        "batch progress not monotonic: {progress:?}"
    );
    let (worker_summary, _q) = match terminal {
        JobEvent::Done(v) => v,
        JobEvent::Failed(msg) => panic!("batch job failed: {msg}"),
        JobEvent::Progress(_) => unreachable!("wait returns a terminal event"),
    };
    assert_eq!(worker_summary.done, sync_summary.done);
    assert_eq!(worker_summary.failed, sync_summary.failed);
    assert!(!worker_summary.canceled);
    assert_eq!(worker_summary.done, 3);

    for p in inputs.iter().chain(&out_sync).chain(&out_wrk) {
        let _ = std::fs::remove_file(p);
    }
}

#[test]
fn cancel_mid_batch_leaves_unprocessed_rows_pending() {
    let inputs: Vec<PathBuf> = (0..4)
        .map(|i| write_input(&format!("cancel-{i}"), i as f32 * 3.0))
        .collect();
    let outputs: Vec<PathBuf> = (0..4).map(|i| temp(&format!("cancel-out{i}"))).collect();
    let worker_q = build_queue(&inputs, &outputs);

    // A latch the observer flips once, on the first pre-file callback, so the
    // test can cancel *before any file is processed* deterministically — no
    // reliance on thread timing.
    let waiting = Arc::new(AtomicUsize::new(0));
    let waiting_job = Arc::clone(&waiting);

    let handle = spawn(move |ctx| {
        let mut q = worker_q;
        let summary = q.run_with_progress(|p| {
            // First pre-file callback (index set, nothing done yet): block until
            // the caller has requested cancellation.
            if p.current_index.is_some() && p.done == 0 {
                waiting_job.store(1, Ordering::SeqCst);
                while !ctx.is_canceled() {
                    std::thread::yield_now(); // parked, not pinning a core
                }
            }
            if ctx.is_canceled() {
                BatchControl::Cancel
            } else {
                BatchControl::Continue
            }
        });
        Ok((summary, q))
    });

    // Wait until the observer is parked on the first file, then cancel. The
    // deadline keeps a worker that dies before its first callback from hanging
    // the whole test binary.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while waiting.load(Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "worker never reached the first pre-file callback"
        );
        std::thread::yield_now();
    }
    handle.cancel();
    let (_, terminal) = handle.wait();

    let (summary, q) = match terminal {
        JobEvent::Done(v) => v,
        JobEvent::Failed(msg) => panic!("batch job failed: {msg}"),
        JobEvent::Progress(_) => unreachable!("wait returns a terminal event"),
    };
    assert!(summary.canceled, "summary should report cancellation");
    assert_eq!(
        summary.done, 0,
        "cancel before file 0 should process nothing"
    );
    assert!(
        q.items()
            .iter()
            .all(|i| matches!(i.status, JobStatus::Pending)),
        "canceled rows must stay pending"
    );

    for p in inputs.iter().chain(&outputs) {
        let _ = std::fs::remove_file(p);
    }
}
