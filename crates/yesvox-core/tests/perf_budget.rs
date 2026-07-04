//! #21 performance-budget smoke tests.
//!
//! These are CI-safe tripwires, not precise benchmark numbers. Criterion covers
//! local precision tracking; this lane catches order-of-magnitude regressions in
//! the full render path on a fixed synthetic fixture.

mod common;

use std::time::Instant;

use yesvox_core::{write_wav, AudioBuffer, Preset, QualityMode, SampleEncoding, Session};

const SR: u32 = 48_000;
const FIXTURE_SECONDS: usize = 180;

#[test]
fn full_chain_render_stays_inside_ci_perf_tripwire() {
    let path = common::temp_wav("perf-budget-3min");
    let input = three_minute_voice_like_fixture();
    write_wav(&path, &input).expect("write perf fixture");

    let mut session = Session::new();
    session.open(&path).expect("open perf fixture");
    session.select_preset(Preset::PODCAST);
    session.set_quality_mode(QualityMode::Natural);
    session.set_debleed_thresholds(0.99, 1.0);

    let started = Instant::now();
    session.render().expect("render perf fixture");
    let elapsed = started.elapsed();

    let output = session.output().expect("rendered output");
    assert_eq!(output.frames(), input.frames());
    assert_eq!(output.sample_rate(), SR);

    let budget_s = full_chain_budget_seconds();
    let too_slow = elapsed.as_secs_f32() >= budget_s;
    common::remove(&[&path]);
    eprintln!(
        "3-minute full-chain render took {:.2}s (budget {:.1}s)",
        elapsed.as_secs_f32(),
        budget_s
    );
    assert!(
        !too_slow,
        "3-minute full-chain render took {elapsed:?}, over the {budget_s:.1} s CI tripwire"
    );
}

fn full_chain_budget_seconds() -> f32 {
    if let Ok(raw) = std::env::var("YESVOX_PERF_RENDER_BUDGET_SECONDS") {
        if let Ok(value) = raw.parse::<f32>() {
            if value.is_finite() && value > 0.0 {
                return value;
            }
        }
    }
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        240.0
    } else {
        180.0
    }
}

fn three_minute_voice_like_fixture() -> AudioBuffer {
    let frames = SR as usize * FIXTURE_SECONDS;
    let mut samples = Vec::with_capacity(frames);
    let mut seed = 0x4d59_5658u32; // "MYVX"

    for i in 0..frames {
        let t = i as f32 / SR as f32;
        let envelope = 0.55 + 0.35 * (2.0 * std::f32::consts::PI * 0.9 * t).sin().abs();
        let vibrato_hz = 5.4;
        let vibrato = 1.0 + 0.012 * (2.0 * std::f32::consts::PI * vibrato_hz * t).sin();
        let f0 = 185.0 * vibrato;
        let voice = envelope
            * (0.24 * (2.0 * std::f32::consts::PI * f0 * t).sin()
                + 0.08 * (2.0 * std::f32::consts::PI * f0 * 2.0 * t).sin()
                + 0.035 * (2.0 * std::f32::consts::PI * f0 * 3.0 * t).sin());
        let hum = 0.006 * (2.0 * std::f32::consts::PI * 60.0 * t).sin();
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.002;
        let click = if i % 393_217 == 0 { 0.18 } else { 0.0 };
        samples.push((voice + hum + noise + click).clamp(-0.98, 0.98));
    }

    AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![samples])
}
