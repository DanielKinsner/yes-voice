//! LOUDNESS guarantee (plan §7.6 adjacent assertions): the **assembled chain**
//! output converges to the preset's loudness target and stays under the
//! true-peak ceiling — not only the FinishStage in isolation, but through the
//! whole rack (where every upstream gain change and the presence/air EQ feed the
//! limiter). Fixed-target presets must hit their integrated LUFS target; every
//! preset must keep true peak ≤ −1.0 dBTP.

use std::f32::consts::PI;
use std::sync::Arc;

use yesvox_core::{
    loudness::{integrated_lufs, true_peak_dbtp},
    Denoiser, Preset, SpectralDenoiser, VocalProcessor,
};
use yesvox_core::{AudioBuffer, SampleEncoding};

const SR: u32 = 48_000;

fn light() -> Arc<dyn Denoiser> {
    Arc::new(SpectralDenoiser::default())
}

/// ~3 s of a voice-like signal (fundamental + harmonics, gentle 4 Hz tremolo) at
/// a moderate level — long enough for a gated integrated-LUFS measurement.
fn voice_like(seconds: f32) -> Vec<f32> {
    let n = (seconds * SR as f32) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let amp = 0.18 * (1.0 + 0.2 * (2.0 * PI * 4.0 * t).sin());
            amp * ((2.0 * PI * 150.0 * t).sin()
                + 0.5 * (2.0 * PI * 300.0 * t).sin()
                + 0.3 * (2.0 * PI * 450.0 * t).sin())
        })
        .collect()
}

fn process_through(preset: &Preset, x: Vec<f32>) -> AudioBuffer {
    let mut buf = AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![x]);
    let mut chain = preset.build_chain(light(), 2);
    chain.prepare(SR, 4096, buf.layout());
    chain.reset();
    chain.process(&mut buf);
    buf
}

#[test]
fn fixed_target_presets_converge_and_stay_under_the_ceiling() {
    // (preset, expected integrated LUFS target)
    let cases = [
        (Preset::PODCAST, -16.0_f32),
        (Preset::NARRATION, -16.0),
        (Preset::LEAD_VOCAL, -14.0),
    ];
    for (preset, target) in cases {
        let out = process_through(&preset, voice_like(3.0));
        let lufs = integrated_lufs(&out).expect("integrated LUFS measurable");
        let tp = true_peak_dbtp(&out);
        eprintln!(
            "{}: integrated {lufs:.2} LUFS (target {target}), true peak {tp:.2} dBTP",
            preset.name
        );
        assert!(
            (lufs - target).abs() <= 0.5,
            "{} integrated {lufs:.2} not within 0.5 LU of {target}",
            preset.name
        );
        assert!(
            tp <= -1.0 + 0.05,
            "{} true peak {tp:.2} dBTP exceeds the -1.0 ceiling",
            preset.name
        );
    }
}

#[test]
fn every_preset_keeps_true_peak_under_the_ceiling() {
    // Regression for the de-reverb OLA edge blow-up: at certain lengths (e.g.
    // exactly 2.0 s) de-reverb amplified the trailing edge by ~20 dB and the
    // limiter couldn't claw it back under the ceiling. Sweep a few lengths,
    // including the integer-hop 2.0 s case, across every preset.
    for preset in Preset::ALL {
        for secs in [2.0f32, 2.5, 3.0] {
            let out = process_through(&preset, voice_like(secs));
            let tp = true_peak_dbtp(&out);
            assert!(
                tp <= -1.0 + 0.05,
                "{} at {secs}s: true peak {tp:.2} dBTP exceeds the -1.0 ceiling",
                preset.name
            );
        }
    }
}
