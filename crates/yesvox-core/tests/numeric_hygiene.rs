//! Numeric-hygiene gates (#28): hostile float samples (NaN, ±Inf, wildly
//! out-of-range, denormals) must never panic the analyze → render → export
//! path, and the output must be entirely finite. Sanitization happens at decode
//! (`io::deinterleave`); these tests exercise both the decoded-file path and the
//! sanitize-then-chain contract on synthetic buffers.

use std::path::PathBuf;
use std::sync::Arc;

use proptest::prelude::*;
use yesvox_core::analysis::analyze;
use yesvox_core::{
    read_wav, write_wav, AudioBuffer, Denoiser, Preset, SampleEncoding, SpectralDenoiser,
    VocalProcessor,
};

const SR: u32 = 48_000;

fn temp(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("yesvox-hygiene-{tag}-{}.wav", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn render_full_chain(buf: &mut AudioBuffer) {
    let denoiser: Arc<dyn Denoiser> = Arc::new(SpectralDenoiser::default());
    let mut chain = Preset::PODCAST.build_chain(denoiser, 2);
    chain.prepare(SR, buf.frames().max(1), buf.layout());
    chain.reset();
    chain.process(buf);
}

fn all_finite(buf: &AudioBuffer) -> bool {
    (0..buf.channel_count()).all(|ch| buf.channel(ch).iter().all(|s| s.is_finite()))
}

/// A sample generator biased toward hostile values.
fn hostile_sample() -> impl Strategy<Value = f32> {
    prop_oneof![
        Just(f32::NAN),
        Just(f32::INFINITY),
        Just(f32::NEG_INFINITY),
        Just(1.0e30_f32),
        Just(-1.0e30_f32),
        Just(1.0e-40_f32), // denormal
        Just(-1.0e-42_f32),
        (-1.5_f32..1.5_f32),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn hostile_buffers_through_analyze_render_are_panic_free_and_finite(
        raw in prop::collection::vec(hostile_sample(), 2049..3072)
    ) {
        // Decode-time contract: sanitize first (what io::deinterleave does).
        let mut buf = AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![raw]);
        buf.sanitize();
        prop_assert!(all_finite(&buf), "sanitize left a non-finite sample");

        // The analyzer's percentile sorts must not choke on the (now-finite) data.
        let _report = analyze(&buf).expect("analyze hostile buffer");

        // The full rack must keep the output finite.
        render_full_chain(&mut buf);
        prop_assert!(all_finite(&buf), "the chain produced a non-finite output");
    }
}

#[test]
fn a_wav_with_nan_floats_round_trips_with_a_friendly_report() {
    // A real float WAV laced with NaN/Inf/out-of-range samples.
    let path = temp("nan-floats");
    let mut ch = vec![0.1_f32; 4800];
    ch[100] = f32::NAN;
    ch[200] = f32::INFINITY;
    ch[300] = f32::NEG_INFINITY;
    ch[400] = 1.0e30; // clamped, not counted as non-finite
    let source = AudioBuffer::new(SR, 32, SampleEncoding::Float, vec![ch]);
    write_wav(&path, &source).expect("write nan wav");

    // Decode sanitizes (io::deinterleave) — no crash, all-finite audio.
    let decoded = read_wav(&path).expect("decode nan wav");
    assert!(all_finite(&decoded), "decode did not sanitize the samples");
    assert_eq!(
        decoded.non_finite_samples(),
        3,
        "NaN + ±Inf counted as non-finite"
    );

    // The report surfaces the defect rather than crashing on it.
    let report = analyze(&decoded).expect("analyze decoded");
    assert_eq!(report.non_finite_samples, 3, "report surfaces the defect");

    // Rendering the recovered audio stays finite.
    let mut rendered = decoded.clone();
    render_full_chain(&mut rendered);
    assert!(all_finite(&rendered), "render stayed finite");

    let _ = std::fs::remove_file(&path);
}
