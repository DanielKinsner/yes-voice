//! Declip: clipping-distortion repair (plan §3.1 / §3.2).
//!
//! Detects flat-topped clipped runs and reconstructs the missing peaks by
//! cubic interpolation through the surviving samples on each side. The
//! reconstruction is **constrained**: it can only push a clipped sample *past*
//! the clip level (the true signal was at least that loud there), never below
//! it, and never beyond a bounded overshoot — so it cannot invent energy that
//! contradicts the un-clipped neighbours. v1 ships this neighbourhood estimator;
//! slice #8's denoiser sidechain upgrades it later. Bypassed → identity.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::processor::VocalProcessor;

/// Clipping-distortion repair.
#[derive(Clone, Debug)]
pub struct DeClip {
    enabled: bool,
    /// Detection level: samples at/above this magnitude are clip candidates.
    threshold: f32,
    /// Repair strength `[0, 1]`: 0 leaves the flat top, 1 applies the full
    /// reconstructed peak.
    strength: f32,
    /// Largest overshoot above the clip level the estimator may synthesize.
    ceiling_ratio: f32,
}

impl Default for DeClip {
    fn default() -> Self {
        DeClip {
            enabled: true,
            threshold: 0.98,
            strength: 1.0,
            ceiling_ratio: 1.6,
        }
    }
}

impl DeClip {
    /// New declipper with an explicit detection threshold and repair strength.
    pub fn new(threshold: f32, strength: f32) -> Self {
        DeClip {
            threshold,
            strength: strength.clamp(0.0, 1.0),
            ..DeClip::default()
        }
    }

    /// Enable or bypass.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Whether the module is active.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Detection threshold.
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Repair strength.
    pub fn strength(&self) -> f32 {
        self.strength
    }

    fn repair_channel(&self, x: &mut [f32]) {
        let n = x.len();
        let mut i = 0;
        while i < n {
            if x[i].abs() < self.threshold {
                i += 1;
                continue;
            }
            // Extend the run while samples stay near the same rail (flat top).
            let sign = x[i].signum();
            let mut j = i;
            while j < n && x[j].abs() >= self.threshold && x[j].signum() == sign {
                j += 1;
            }
            let run_len = j - i;
            // Single isolated over-threshold sample is a legit peak, not a clip.
            if run_len >= 2 {
                self.reconstruct(x, i, j - 1, sign);
            }
            i = j;
        }
    }

    /// Reconstruct clipped indices `[a, b]` with a cubic Hermite spline anchored
    /// to the un-clipped samples (and their slopes) on each side. A rising slope
    /// at the entry and a falling slope at the exit make the spline bow upward
    /// into the missing peak — what a smooth interpolation through equal
    /// endpoints cannot do. Constrained to `[level, ceiling]` on the clip's rail.
    fn reconstruct(&self, x: &mut [f32], a: usize, b: usize, sign: f32) {
        // Need two samples each side to estimate value + slope; else hold rail.
        if a < 2 || b + 2 >= x.len() {
            return;
        }
        let left = a - 1; // last good sample before the run
        let right = b + 1; // first good sample after the run
        let y_l = x[left];
        let y_r = x[right];
        let m_l = x[left] - x[left - 1]; // slope per sample entering
        let m_r = x[right + 1] - x[right]; // slope per sample exiting
        let span = (right - left) as f32; // u in [0, span]

        let level = self.threshold;
        let ceiling = level * self.ceiling_ratio;
        for (k, sample) in x.iter_mut().enumerate().take(b + 1).skip(a) {
            let u = (k - left) as f32;
            let s = u / span;
            let interp = hermite(y_l, m_l, y_r, m_r, span, s);
            // Constrain to the clip's rail within [level, ceiling].
            let mag = interp.abs().clamp(level, ceiling);
            let target = sign * mag;
            *sample += self.strength * (target - *sample);
        }
    }
}

/// Cubic Hermite at normalized `s` ∈ [0, 1] over a span of `span` samples, with
/// endpoint values `p0`/`p1` and per-sample slopes `m0`/`m1`.
fn hermite(p0: f32, m0: f32, p1: f32, m1: f32, span: f32, s: f32) -> f32 {
    let s2 = s * s;
    let s3 = s2 * s;
    let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
    let h10 = s3 - 2.0 * s2 + s;
    let h01 = -2.0 * s3 + 3.0 * s2;
    let h11 = s3 - s2;
    h00 * p0 + h10 * (m0 * span) + h01 * p1 + h11 * (m1 * span)
}

impl VocalProcessor for DeClip {
    fn prepare(&mut self, _sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {}

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled || self.strength == 0.0 {
            return;
        }
        for ch in 0..buffer.channel_count() {
            self.repair_channel(buffer.channel_mut(ch));
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    fn buf(ch: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![ch])
    }

    fn run(mut m: DeClip, mut b: AudioBuffer) -> AudioBuffer {
        m.prepare(SR, 4096, b.layout());
        m.process(&mut b);
        b
    }

    /// True (unclipped) sine and its hard-clipped version.
    fn clipped_pair(amp: f32, clip: f32, freq: f32, n: usize) -> (Vec<f32>, Vec<f32>) {
        let truth: Vec<f32> = (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / SR as f32).sin())
            .collect();
        let clipped: Vec<f32> = truth.iter().map(|&s| s.clamp(-clip, clip)).collect();
        (truth, clipped)
    }

    /// Samples sitting *exactly* on the clip plateau (the flat top to remove).
    fn count_plateau(x: &[f32], clip: f32) -> usize {
        x.iter().filter(|v| (v.abs() - clip).abs() < 1e-3).count()
    }

    #[test]
    fn restores_peaks_and_moves_toward_truth() {
        let (truth, clipped) = clipped_pair(1.0, 0.85, 150.0, 48_000);
        let flat_before = count_plateau(&clipped, 0.85);
        let out = run(DeClip::new(0.84, 1.0), buf(clipped.clone()));
        let restored = out.channel(0);

        // The flat plateau is largely gone — peaks reshaped into curves.
        let flat_after = count_plateau(restored, 0.85);
        assert!(
            flat_after < flat_before / 4,
            "flat-top plateau should collapse: {flat_before} -> {flat_after}"
        );
        assert!(
            restored.iter().fold(0.0f32, |m, &v| m.max(v.abs())) > 0.9,
            "peak should be restored above the clip level"
        );

        // Reconstruction is closer to the true sine than the clipped signal.
        let err = |a: &[f32]| -> f32 {
            a.iter()
                .zip(&truth)
                .map(|(x, t)| (x - t).powi(2))
                .sum::<f32>()
        };
        assert!(
            err(restored) < err(&clipped) * 0.6,
            "declip should reduce error vs truth"
        );
    }

    #[test]
    fn clean_audio_is_left_essentially_untouched() {
        // Peaks at 0.5, well below threshold → nothing to repair.
        let (_t, clean) = clipped_pair(0.5, 1.0, 300.0, 20_000);
        let out = run(DeClip::default(), buf(clean.clone()));
        for (a, b) in out.channel(0).iter().zip(&clean) {
            assert!(
                (a - b).abs() <= f32::EPSILON,
                "clean audio must be untouched"
            );
        }
    }

    #[test]
    fn bypass_is_exact_identity() {
        let (_t, clipped) = clipped_pair(1.0, 0.8, 200.0, 10_000);
        let mut m = DeClip::new(0.79, 1.0);
        m.set_enabled(false);
        let out = run(m, buf(clipped.clone()));
        for (a, b) in out.channel(0).iter().zip(&clipped) {
            assert!((a - b).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }

    #[test]
    fn strength_scales_the_repair() {
        let (_t, clipped) = clipped_pair(1.0, 0.85, 150.0, 24_000);
        let peak = |b: &AudioBuffer| b.channel(0).iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        let full = peak(&run(DeClip::new(0.84, 1.0), buf(clipped.clone())));
        let half = peak(&run(DeClip::new(0.84, 0.5), buf(clipped.clone())));
        assert!(
            full > half,
            "more strength lifts peaks further: {half} < {full}"
        );
        assert!(
            half > 0.85,
            "even half strength should exceed the clip level"
        );
    }
}
