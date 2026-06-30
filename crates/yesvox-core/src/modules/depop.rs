//! De-pop: plosive control (plan §3.1).
//!
//! Tames p/b plosive pops with a transient-triggered low-cut applied **only**
//! over the plosive region — never the whole track — so the body of the vocal
//! keeps its low end. It is armed by the **shared** [`TransientDetector`] (no
//! new detection pass) and owns only [`TransientKind::Plosive`] events; clicks
//! and mouth-clicks belong to de-click (#5), so nothing is double-repaired.
//! Bypassed → identity.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::dsp::Biquad;
use crate::modules::transient::{TransientDetector, TransientKind};
use crate::processor::VocalProcessor;

/// Plosive-pop remover.
#[derive(Clone, Copy, Debug)]
pub struct DePop {
    enabled: bool,
    detector: TransientDetector,
    /// Low-cut corner applied over the plosive region.
    cutoff_hz: f32,
    /// How much of the low-cut to blend in `[0, 1]`.
    strength: f32,
    sample_rate: f32,
}

impl Default for DePop {
    fn default() -> Self {
        DePop {
            enabled: true,
            detector: TransientDetector::default(),
            cutoff_hz: 120.0,
            strength: 1.0,
            sample_rate: 48_000.0,
        }
    }
}

impl DePop {
    /// New de-pop with the shared detector and default low-cut.
    pub fn new() -> Self {
        DePop::default()
    }

    /// Enable or bypass.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Whether the module is active.
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

impl VocalProcessor for DePop {
    fn prepare(&mut self, sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {
        self.sample_rate = sample_rate as f32;
    }

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled || self.strength == 0.0 {
            return;
        }
        let sr = self.sample_rate;
        let region = (0.030 * sr) as usize; // plosive low-cut spans ~30 ms
        let ramp = ((0.005 * sr) as usize).max(1); // 5 ms raised-cosine edges

        for ch in 0..buffer.channel_count() {
            let samples = buffer.channel_mut(ch);
            let n = samples.len();
            let events = self.detector.detect(samples, sr);

            // Build a wet envelope that is 0 everywhere except a windowed,
            // ramped 1.0 over each plosive — so non-plosive audio is untouched.
            let mut env = vec![0.0f32; n];
            for ev in events.iter().filter(|e| e.kind == TransientKind::Plosive) {
                let start = ev.start;
                let end = (ev.start + region).min(n);
                for (i, e) in env.iter_mut().enumerate().take(end).skip(start) {
                    let into = i - start;
                    let outof = end - 1 - i;
                    let edge = into.min(outof);
                    let g = if edge < ramp {
                        0.5 * (1.0 - (std::f32::consts::PI * edge as f32 / ramp as f32).cos())
                    } else {
                        1.0
                    };
                    *e = e.max(g);
                }
            }

            // No plosives → leave the channel exactly as-is (identity).
            if env.iter().all(|&g| g == 0.0) {
                continue;
            }

            // High-pass the whole channel (continuous state), blend by envelope.
            let mut hp = Biquad::high_pass(sr, self.cutoff_hz, 0.707);
            for (s, &g) in samples.iter_mut().zip(env.iter()) {
                let dry = *s;
                let wet = hp.process(dry);
                *s = dry + self.strength * g * (wet - dry);
            }
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use crate::dsp::goertzel_power;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    fn buf(ch: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![ch])
    }

    fn run(mut m: DePop, mut b: AudioBuffer) -> AudioBuffer {
        m.prepare(SR, 4096, b.layout());
        m.process(&mut b);
        b
    }

    /// A voiced tone with an 80 Hz plosive thump (sharp onset, decaying) at `at`.
    /// The thump uses `cos` so it starts at full amplitude — a real plosive's
    /// sharp pressure onset, which the detector keys on.
    fn with_plosive(at: usize, n: usize) -> Vec<f32> {
        let mut x: Vec<f32> = (0..n)
            .map(|i| 0.2 * (2.0 * PI * 200.0 * i as f32 / SR as f32).sin())
            .collect();
        for t in 0..(0.030 * SR as f32) as usize {
            if at + t < n {
                let env = (-(t as f32) / (0.012 * SR as f32)).exp();
                x[at + t] += 0.8 * env * (2.0 * PI * 80.0 * t as f32 / SR as f32).cos();
            }
        }
        x
    }

    #[test]
    fn plosive_is_classified_as_plosive() {
        let x = with_plosive(10_000, 48_000);
        let events = TransientDetector::default().detect(&x, SR as f32);
        assert!(
            events.iter().any(|e| e.kind == TransientKind::Plosive),
            "the 80 Hz thump should be owned by de-pop"
        );
    }

    #[test]
    fn cuts_low_end_in_the_plosive_region_only() {
        let at = 10_000;
        let x = with_plosive(at, 48_000);
        let out = run(DePop::new(), buf(x.clone()));

        // 80 Hz energy in the plosive region drops.
        let region_before = &x[at..at + 1440];
        let region_after = &out.channel(0)[at..at + 1440];
        let lo_before = goertzel_power(region_before, SR as f32, 80.0);
        let lo_after = goertzel_power(region_after, SR as f32, 80.0);
        assert!(
            lo_after < lo_before * 0.5,
            "plosive low end should drop ≥6 dB: {lo_before} -> {lo_after}"
        );

        // Far from the plosive the track is untouched (no global thinning).
        for (a, b) in out.channel(0)[30_000..40_000]
            .iter()
            .zip(&x[30_000..40_000])
        {
            assert!(
                (a - b).abs() <= f32::EPSILON,
                "non-plosive audio must be identical (no thinning)"
            );
        }
    }

    #[test]
    fn bypass_is_exact_identity() {
        let x = with_plosive(5_000, 20_000);
        let mut m = DePop::new();
        m.set_enabled(false);
        let out = run(m, buf(x.clone()));
        for (a, b) in out.channel(0).iter().zip(&x) {
            assert!((a - b).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }
}
