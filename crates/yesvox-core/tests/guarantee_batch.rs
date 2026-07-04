//! BATCH guarantee coverage (#27, finding 4): the four whole-session guarantees
//! (non-destructive input, same length, loudness within tolerance, and
//! crash-recovery safety) must also hold when files are processed through the
//! [`RenderQueue`], not only through a hand-built [`yesvox_core::Chain`]. The
//! batch path is what users actually run over many files, so it needs its own
//! contracts.

use std::path::{Path, PathBuf};

use yesvox_core::loudness::integrated_lufs;
use yesvox_core::{
    read_wav, write_wav, AudioBuffer, JobStatus, Preset, RenderQueue, SampleEncoding, Settings,
};

const SR: u32 = 48_000;
/// Podcast preset integrated-loudness target.
const PODCAST_LUFS: f32 = -16.0;

/// ~3 s of a voice-like signal (fundamental + harmonics + gentle tremolo), long
/// enough for a gated integrated-LUFS measurement. `seed` de-correlates fixtures
/// so each queued file is distinct.
fn voice_like(seconds: f32, seed: f32) -> AudioBuffer {
    use std::f32::consts::PI;
    let n = (seconds * SR as f32) as usize;
    let x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let amp = 0.18 * (1.0 + 0.2 * (2.0 * PI * 4.0 * t).sin());
            amp * ((2.0 * PI * (150.0 + seed) * t).sin()
                + 0.5 * (2.0 * PI * (300.0 + seed) * t).sin()
                + 0.3 * (2.0 * PI * (450.0 + seed) * t).sin())
        })
        .collect();
    AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![x])
}

fn temp(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("yesvox-gbatch-{tag}-{}.wav", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn write_input(tag: &str, seed: f32) -> (PathBuf, AudioBuffer) {
    let path = temp(tag);
    let buf = voice_like(3.0, seed);
    write_wav(&path, &buf).expect("write input fixture");
    (path, buf)
}

fn done_path(status: &JobStatus) -> &Path {
    match status {
        JobStatus::Done(p) => p.as_path(),
        other => panic!("expected Done, got {other:?}"),
    }
}

#[test]
fn batch_run_is_nondestructive_length_and_loudness_preserving() {
    // Three distinct inputs through a fixed-target preset.
    let inputs: Vec<(PathBuf, AudioBuffer)> = (0..3)
        .map(|i| write_input(&format!("nd-in{i}"), i as f32 * 7.0))
        .collect();
    let outputs: Vec<PathBuf> = (0..3).map(|i| temp(&format!("nd-out{i}"))).collect();
    let before: Vec<Vec<u8>> = inputs
        .iter()
        .map(|(p, _)| std::fs::read(p).expect("read input bytes"))
        .collect();

    let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
    for ((inp, _), outp) in inputs.iter().zip(&outputs) {
        q.enqueue(inp, outp);
    }
    let summary = q.run();
    assert_eq!(summary.done, 3, "all three files rendered: {summary:?}");
    assert_eq!(summary.failed, 0);

    // (1) NON-DESTRUCTIVE: every input file's bytes are untouched.
    for ((inp, _), was) in inputs.iter().zip(&before) {
        let now = std::fs::read(inp).expect("re-read input bytes");
        assert_eq!(
            &now,
            was,
            "input {} changed during batch run",
            inp.display()
        );
    }

    for (item, (_, in_buf)) in q.items().iter().zip(&inputs) {
        let out = read_wav(done_path(&item.status)).expect("read output");
        // (2) SAME-LENGTH: output frames/rate/channels match the input.
        assert_eq!(out.frames(), in_buf.frames(), "frame count changed");
        assert_eq!(
            out.sample_rate(),
            in_buf.sample_rate(),
            "sample rate changed"
        );
        assert_eq!(
            out.channel_count(),
            in_buf.channel_count(),
            "channel count changed"
        );
        // (3) LOUDNESS: each output converges to the preset target.
        let lufs = integrated_lufs(&out).expect("integrated LUFS measurable");
        assert!(
            (lufs - PODCAST_LUFS).abs() <= 0.5,
            "output {lufs:.2} LUFS not within 0.5 LU of {PODCAST_LUFS}"
        );
    }

    for (p, _) in &inputs {
        let _ = std::fs::remove_file(p);
    }
    for item in q.items() {
        let _ = std::fs::remove_file(done_path(&item.status));
    }
}

#[test]
fn crash_recovery_double_run_stays_nondestructive() {
    // Two pending items, state persisted to JSON *before* processing — the
    // crash-recovery scenario: the app dies, restarts, reloads the queue, and
    // finishes the batch. Neither recovery nor completion may touch the inputs.
    let (in1, buf1) = write_input("cr-in1", 0.0);
    let (in2, buf2) = write_input("cr-in2", 11.0);
    let out1 = temp("cr-out1");
    let out2 = temp("cr-out2");
    let before1 = std::fs::read(&in1).expect("read in1");
    let before2 = std::fs::read(&in2).expect("read in2");

    let mut original = RenderQueue::new(Settings::default(), Preset::PODCAST);
    original.enqueue(&in1, &out1);
    original.enqueue(&in2, &out2);

    // Persist mid-batch state and recover a fresh queue from it (crash recovery).
    let json = original.to_json().expect("serialize queue");
    let mut recovered = RenderQueue::from_json(&json).expect("recover queue");
    // Recovery always restores *paused* (#19b) so a restart never auto-blasts a
    // batch; the user resumes deliberately.
    assert!(recovered.is_paused(), "recovered queue must restore paused");
    recovered.resume();
    let summary = recovered.run();
    assert_eq!(summary.done, 2, "recovered queue completes the batch");

    // Inputs survived recovery + completion byte-for-byte.
    assert_eq!(std::fs::read(&in1).unwrap(), before1, "in1 modified");
    assert_eq!(std::fs::read(&in2).unwrap(), before2, "in2 modified");

    // Re-running the recovered (now all-Done) queue is a no-op — resume-safe.
    let again = recovered.run();
    assert_eq!(again.done, 0, "completed queue re-run must do nothing");

    // A genuinely fresh double-process over the SAME inputs must never clobber
    // them or the first outputs: the writer auto-suffixes.
    let mut second = RenderQueue::new(Settings::default(), Preset::PODCAST);
    second.enqueue(&in1, &out1);
    second.enqueue(&in2, &out2);
    let s2 = second.run();
    assert_eq!(s2.done, 2);
    assert_eq!(
        std::fs::read(&in1).unwrap(),
        before1,
        "in1 modified on rerun"
    );
    assert_eq!(
        std::fs::read(&in2).unwrap(),
        before2,
        "in2 modified on rerun"
    );
    for item in second.items() {
        let p = done_path(&item.status);
        assert_ne!(p, in1, "second run wrote onto an input");
        assert_ne!(p, in2, "second run wrote onto an input");
    }

    // Length preserved on the recovered outputs too.
    for (item, in_buf) in recovered.items().iter().zip([&buf1, &buf2]) {
        let out = read_wav(done_path(&item.status)).expect("read recovered output");
        assert_eq!(out.frames(), in_buf.frames());
    }

    for item in recovered.items().iter().chain(second.items()) {
        let _ = std::fs::remove_file(done_path(&item.status));
    }
    let _ = std::fs::remove_file(&in1);
    let _ = std::fs::remove_file(&in2);
}
