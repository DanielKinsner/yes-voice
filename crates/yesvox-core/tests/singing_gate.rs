//! The mechanical singing-quality gate (plan §6.2) — the objective replacement
//! for the former #16 blind-listening release plan.
//!
//! Generate synthetic sung fixtures with *known* ground truth (a vibrato note
//! that decays to a tail), run them through the real singing presets, and assert
//! the chain preserved sung character within the §6.2 tolerances: F0 within a
//! few cents, vibrato rate + depth intact, HNR not degraded, timbre (spectral
//! envelope) preserved, and the sustain/tail not truncated. No human ears, no
//! curated clips — every input is generated and every check is mechanical, so
//! this runs in CI like any other gate.

use std::f32::consts::PI;
use std::sync::Arc;

use yesvox_core::singing::{
    f0_cents_deviation, hnr_db, spectral_envelope_cosine, tail_survival_db, vibrato,
};
use yesvox_core::{
    AudioBuffer, Denoiser, Preset, SampleEncoding, SpectralDenoiser, VocalProcessor,
};

const SR: f32 = 48_000.0;
const SR_U: u32 = 48_000;

/// A sung vowel: vibrato fundamental + harmonics, decaying to a tail at the end.
fn sung_note(f0_center: f32, rate_hz: f32, depth_cents: f32, n: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    let mut phase = 0.0f32;
    let decay_start = n * 5 / 6;
    for i in 0..n {
        let t = i as f32 / SR;
        let f0 = f0_center * 2.0_f32.powf((depth_cents / 1200.0) * (2.0 * PI * rate_hz * t).sin());
        phase += 2.0 * PI * f0 / SR;
        let mut s = 0.5 * phase.sin()
            + 0.3 * (2.0 * phase).sin()
            + 0.18 * (3.0 * phase).sin()
            + 0.08 * (4.0 * phase).sin();
        if i >= decay_start {
            s *= (-((i - decay_start) as f32 / SR) / 0.08).exp();
        }
        out.push(0.5 * s);
    }
    out
}

/// Scale `processed` so its RMS matches `reference` — the finish stage normalizes
/// loudness, and every preservation metric here is meant to be level-invariant.
fn level_match(reference: &[f32], processed: &[f32]) -> Vec<f32> {
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
    let r = rms(reference);
    let p = rms(processed);
    let g = if p > 1.0e-9 { r / p } else { 1.0 };
    processed.iter().map(|&v| v * g).collect()
}

fn run_singing(preset: &Preset, input: &[f32]) -> Vec<f32> {
    let denoiser: Arc<dyn Denoiser> = Arc::new(SpectralDenoiser::default());
    let mut chain = preset.build_chain(denoiser, 2);
    let mut buf = AudioBuffer::new(SR_U, 24, SampleEncoding::Int, vec![input.to_vec()]);
    chain.prepare(SR_U, 4096, buf.layout());
    chain.reset();
    chain.process(&mut buf);
    buf.channel(0).to_vec()
}

/// Assert the §6.2 character-preservation tolerances for one preset on one note.
fn assert_preserves(preset: &Preset, reference: &[f32]) {
    let processed = level_match(reference, &run_singing(preset, reference));

    // F0 / vibrato / HNR / timbre are §6.2 "sustained voiced frame" metrics —
    // measure them on the sustained body (the dying decay tail is unreliable for
    // pitch/HNR and has its own survival metric below).
    let sustain = reference.len() * 5 / 6;
    let ref_sus = &reference[..sustain];
    let proc_sus = &processed[..sustain];

    let cents = f0_cents_deviation(ref_sus, proc_sus, SR);
    let vin = vibrato(ref_sus, SR).expect("input vibrato");
    let vout = vibrato(proc_sus, SR).expect("output vibrato");
    let depth_ratio = vout.depth_cents / vin.depth_cents.max(1.0e-3);
    let rate_err = (vout.rate_hz - vin.rate_hz).abs() / vin.rate_hz.max(1.0e-3);
    let hnr_change = hnr_db(proc_sus, SR).unwrap() - hnr_db(ref_sus, SR).unwrap();
    let cosine = spectral_envelope_cosine(ref_sus, proc_sus, SR);
    // Tail survival uses the full signal (its region is the decay itself).
    let tail = tail_survival_db(reference, &processed, SR);

    eprintln!(
        "[{}] cents med {:.2}/p95 {:.2} | vibrato depth x{:.2} rate err {:.1}% | dHNR {:.2} dB | cos {:.4} | tail {:.2} dB",
        preset.name,
        cents.median,
        cents.p95,
        depth_ratio,
        rate_err * 100.0,
        hnr_change,
        cosine,
        tail
    );

    assert!(
        cents.median <= 3.0,
        "{}: F0 median {} cents > 3 (§6.2)",
        preset.name,
        cents.median
    );
    assert!(
        cents.p95 <= 5.0,
        "{}: F0 p95 {} cents > 5 (§6.2)",
        preset.name,
        cents.p95
    );
    assert!(
        depth_ratio >= 0.90,
        "{}: vibrato depth fell to {:.0}% of input (flattening)",
        preset.name,
        depth_ratio * 100.0
    );
    assert!(
        rate_err <= 0.10,
        "{}: vibrato rate drifted {:.0}%",
        preset.name,
        rate_err * 100.0
    );
    assert!(
        hnr_change >= -2.0,
        "{}: HNR dropped {:.2} dB > 2 (§6.2)",
        preset.name,
        -hnr_change
    );
    assert!(
        cosine >= 0.95,
        "{}: spectral-envelope cosine {:.4} < 0.95 (§6.4)",
        preset.name,
        cosine
    );
    assert!(
        tail >= -3.0,
        "{}: tail survival {:.2} dB — sustain truncated",
        preset.name,
        tail
    );
}

#[test]
fn lead_vocal_preserves_sung_character() {
    // ~2 s lead vocal: a soprano-ish G3 with 5.5 Hz / 70-cent vibrato and a tail.
    let note = sung_note(196.0, 5.5, 70.0, 96_000);
    assert_preserves(&Preset::LEAD_VOCAL, &note);
}

#[test]
fn natural_vocal_preserves_sung_character() {
    // A lower, gentler-vibrato sustained note (realistic "natural vocal") through
    // the most conservative preset.
    let note = sung_note(146.83, 5.0, 45.0, 96_000);
    assert_preserves(&Preset::NATURAL_VOCAL, &note);
}
