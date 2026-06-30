//! End-to-end CLI surfacing (#11): the binary must run the **real** restoration
//! chain, not a no-op. A degraded input is genuinely processed (output differs
//! from input), while the same-length and non-destructive guarantees still hold
//! through the actual `yesvox` executable.

use std::f32::consts::PI;
use std::path::Path;
use std::process::Command;

use yesvox_core::{read_wav, write_wav, AudioBuffer, SampleEncoding};

/// One second of tone + sibilance with hard-clipped peaks and injected clicks —
/// material the chain has clear, audible work to do.
fn degraded_wav(path: &Path) -> AudioBuffer {
    let sr = 48_000u32;
    let n = 48_000usize;
    let mut x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / sr as f32;
            (0.4 * (2.0 * PI * 200.0 * t).sin() + 0.3 * (2.0 * PI * 7000.0 * t).sin())
                .clamp(-0.7, 0.7)
        })
        .collect();
    for p in [5_000usize, 15_000, 25_000, 35_000] {
        x[p] += 0.9;
    }
    let buf = AudioBuffer::new(sr, 16, SampleEncoding::Int, vec![x]);
    write_wav(path, &buf).expect("write input fixture");
    buf
}

#[test]
fn cli_runs_the_chain_preserves_length_and_keeps_input() {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let in_path = dir.join(format!("yesvox-cli-in-{pid}.wav"));
    let out_path = dir.join(format!("yesvox-cli-out-{pid}.wav"));
    let _ = std::fs::remove_file(&in_path);
    let _ = std::fs::remove_file(&out_path);

    let input = degraded_wav(&in_path);
    let before = std::fs::read(&in_path).expect("read input bytes");

    let status = Command::new(env!("CARGO_BIN_EXE_yesvox"))
        .arg(&in_path)
        .arg(&out_path)
        .status()
        .expect("run yesvox");
    assert!(status.success(), "yesvox exited with failure");

    let output = read_wav(&out_path).expect("read output");
    assert_eq!(output.frames(), input.frames(), "length changed");
    assert_eq!(
        output.sample_rate(),
        input.sample_rate(),
        "sample rate changed"
    );
    assert_eq!(
        output.channel_count(),
        input.channel_count(),
        "channel count changed"
    );

    // The processed output must differ from the input — proves the real chain
    // ran, not the all-bypass identity.
    let changed = output
        .channel(0)
        .iter()
        .zip(input.channel(0))
        .any(|(a, b)| (a - b).abs() > 1e-3);
    assert!(
        changed,
        "output is identical to input — the chain did not run"
    );

    // Non-destructive: the input file's bytes are untouched.
    let after = std::fs::read(&in_path).expect("read input bytes again");
    assert_eq!(before, after, "input file bytes changed");

    let _ = std::fs::remove_file(&in_path);
    let _ = std::fs::remove_file(&out_path);
}
