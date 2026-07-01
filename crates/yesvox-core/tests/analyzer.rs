//! #2 acceptance gates that need the filesystem or wall-clock: the corrupt
//! header reject path and a CI smoke check for the ≤5 s / 30 s analysis budget.

mod common;

use std::time::Instant;

use yesvox_core::{analyze, analyze_file, AnalysisError, AudioBuffer, SampleEncoding};

fn analysis_budget_seconds() -> f32 {
    // The product budget is a release/perf-lane target on named hardware. This
    // debug-mode CI smoke test catches gross regressions but avoids false reds
    // from shared macOS runners, which can be much slower than the target SKU.
    if cfg!(target_os = "macos") && std::env::var_os("GITHUB_ACTIONS").is_some() {
        8.0
    } else {
        5.0
    }
}

#[test]
fn corrupt_header_is_rejected_with_a_decode_error() {
    let path = common::temp_wav("corrupt");
    // Not a valid RIFF/WAVE container — just noise with a .wav name.
    std::fs::write(&path, b"NOT-A-REAL-WAV-HEADER\x00\x01\x02\x03garbage").unwrap();

    match analyze_file(&path) {
        Err(AnalysisError::Decode(_)) => {}
        other => panic!("expected a Decode reject, got {other:?}"),
    }
    // The message must be actionable (mentions re-exporting).
    let msg = format!("{}", analyze_file(&path).unwrap_err());
    assert!(msg.to_lowercase().contains("wav") || msg.to_lowercase().contains("flac"));

    common::remove(&[&path]);
}

#[test]
fn analysis_meets_the_five_second_budget_on_a_thirty_second_clip() {
    // A 30 s stereo voiceover-ish clip: tone + a little hum + noise.
    let sr = 48_000u32;
    let n = sr as usize * 30;
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    let mut seed = 0x1234_5678u32;
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let tone = 0.3 * (2.0 * std::f32::consts::PI * 180.0 * t).sin();
        let hum = 0.02 * (2.0 * std::f32::consts::PI * 50.0 * t).sin();
        // Cheap LCG noise, deterministic.
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
        let s = tone + hum + 0.01 * noise;
        left.push(s);
        right.push(s * 0.95);
    }
    let buf = AudioBuffer::new(sr, 16, SampleEncoding::Int, vec![left, right]);

    let start = Instant::now();
    let report = analyze(&buf).expect("report");
    let elapsed = start.elapsed();

    let budget_s = analysis_budget_seconds();
    assert!(
        elapsed.as_secs_f32() < budget_s,
        "analysis took {elapsed:?}, over the {budget_s:.1} s budget"
    );
    // Sanity: the report is fully populated.
    assert!(report.integrated_lufs < 0.0);
    assert!(report.content.confidence >= 0.0 && report.content.confidence <= 1.0);
}
