//! De-ess: sibilance control (plan §3.1 / §3.2).
//!
//! Reduces harsh sibilance **level-independently** — the detector caps the
//! sibilance band to a fraction of the broadband level, so it ducks soft and
//! loud passages by the same amount rather than only the loud ones. Two modes:
//! `Split` (default voiceover) ducks only the high band and leaves the body
//! untouched; `Wide` (default singing) ducks the whole signal for a smoother,
//! less lisp-prone result. The high-frequency reduction it applied is published
//! via [`DeEss::hf_reduction_db`] so breath control (#13) won't cut the same
//! band twice. Bypassed → identity.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::dsp::Biquad;
use crate::processor::VocalProcessor;

/// How the reduction is applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeEssMode {
    /// Duck only the sibilance band; the body is untouched (default voiceover).
    Split,
    /// Duck the whole signal during sibilance (default singing).
    Wide,
}

/// Sibilance controller.
#[derive(Clone, Debug)]
pub struct DeEss {
    enabled: bool,
    mode: DeEssMode,
    /// Crossover between body and sibilance band, Hz.
    crossover_hz: f32,
    /// Allowed sibilance-band level as a fraction of broadband (lower = more
    /// de-essing). Level-independent because it's a ratio, not a threshold.
    ess_ratio: f32,
    /// Floor on the ducking gain (largest attenuation).
    min_gain: f32,
    sample_rate: f32,
    /// Published: high-frequency energy reduction actually applied, dB.
    hf_reduction_db: f32,
}

impl Default for DeEss {
    fn default() -> Self {
        DeEss {
            enabled: true,
            mode: DeEssMode::Split,
            crossover_hz: 5500.0,
            ess_ratio: 0.45,
            min_gain: 0.1,
            sample_rate: 48_000.0,
            hf_reduction_db: 0.0,
        }
    }
}

impl DeEss {
    /// New de-ess in the given mode.
    pub fn new(mode: DeEssMode) -> Self {
        DeEss {
            mode,
            ..DeEss::default()
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

    /// The active mode.
    pub fn mode(&self) -> DeEssMode {
        self.mode
    }

    /// Tune how hard de-essing works. `ess_ratio` is the allowed sibilance-band
    /// level as a fraction of broadband (higher = ducks less; reserve attenuation
    /// for genuinely harsh fricatives); `min_gain` floors the ducking gain
    /// (higher = less maximum attenuation). Singing uses a gentler pair so a
    /// soaring falsetto or breathy whisper isn't over-ducked (research §6).
    pub fn set_sensitivity(&mut self, ess_ratio: f32, min_gain: f32) {
        self.ess_ratio = ess_ratio.clamp(0.05, 1.0);
        self.min_gain = min_gain.clamp(0.0, 1.0);
    }

    /// High-frequency reduction applied on the last `process`, in dB. Breath
    /// control (#13) reads this to avoid double-cutting the shared HF band.
    pub fn hf_reduction_db(&self) -> f32 {
        self.hf_reduction_db
    }

    /// One-pole smoothing coefficient for `time_s` at the current sample rate.
    fn smooth_coeff(&self, time_s: f32) -> f32 {
        (-1.0 / (time_s * self.sample_rate)).exp()
    }
}

impl VocalProcessor for DeEss {
    fn prepare(&mut self, sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {
        self.sample_rate = sample_rate as f32;
        self.hf_reduction_db = 0.0;
    }

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled {
            self.hf_reduction_db = 0.0;
            return;
        }
        let sr = self.sample_rate;
        let atk = self.smooth_coeff(0.001); // 1 ms attack
        let rel = self.smooth_coeff(0.050); // 50 ms release

        let mut hf_before = 0.0f64;
        let mut hf_after = 0.0f64;

        for ch in 0..buffer.channel_count() {
            let samples = buffer.channel_mut(ch);
            // Body / sibilance split via a low-pass; high = x − low keeps the
            // recombination exact (low + high == x), so gain 1 is identity.
            let mut lp = Biquad::low_pass(sr, self.crossover_hz, 0.707);
            let mut broad_env = 0.0f32;
            let mut hi_env = 0.0f32;

            for s in samples.iter_mut() {
                let dry = *s;
                let low = lp.process(dry);
                let high = dry - low;

                // Envelopes (fast attack, slow release).
                let a_b = dry.abs();
                broad_env = env_step(broad_env, a_b, atk, rel);
                let a_h = high.abs();
                hi_env = env_step(hi_env, a_h, atk, rel);

                // Cap the sibilance band to `ess_ratio` of broadband.
                let target = self.ess_ratio * broad_env;
                let gain = if hi_env > target && hi_env > 1.0e-6 {
                    (target / hi_env).clamp(self.min_gain, 1.0)
                } else {
                    1.0
                };

                hf_before += (a_h as f64) * (a_h as f64);
                let new_high = gain * high;
                hf_after += (new_high as f64) * (new_high as f64);

                *s = match self.mode {
                    DeEssMode::Split => low + new_high,
                    DeEssMode::Wide => gain * dry,
                };
            }
        }

        self.hf_reduction_db = if hf_after > 0.0 && hf_before > 0.0 {
            (10.0 * (hf_before / hf_after).log10()) as f32
        } else {
            0.0
        };
    }

    fn reset(&mut self) {
        self.hf_reduction_db = 0.0;
    }
}

/// One-pole peak envelope: fast attack, slow release.
#[inline]
fn env_step(prev: f32, x: f32, atk: f32, rel: f32) -> f32 {
    let coeff = if x > prev { atk } else { rel };
    coeff * prev + (1.0 - coeff) * x
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

    fn run(mut m: DeEss, b: &AudioBuffer) -> (AudioBuffer, f32) {
        let mut out = b.clone();
        m.prepare(SR, 4096, b.layout());
        m.process(&mut out);
        (out, m.hf_reduction_db())
    }

    /// Voice tone + a 7 kHz sibilant, both scaled by `level`.
    fn voice_with_ess(level: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                level * (0.5 * (2.0 * PI * 200.0 * t).sin() + 0.5 * (2.0 * PI * 7000.0 * t).sin())
            })
            .collect()
    }

    fn band_db(x: &[f32], f: f32) -> f32 {
        10.0 * goertzel_power(x, SR as f32, f).max(1e-12).log10()
    }

    #[test]
    fn reduces_sibilance_consistently_across_levels() {
        let loud = voice_with_ess(1.0, 24_000);
        let soft = voice_with_ess(0.2, 24_000);
        let (loud_out, _) = run(DeEss::new(DeEssMode::Split), &buf(loud.clone()));
        let (soft_out, _) = run(DeEss::new(DeEssMode::Split), &buf(soft.clone()));

        let red_loud =
            band_db(&loud[2000..], 7000.0) - band_db(&loud_out.channel(0)[2000..], 7000.0);
        let red_soft =
            band_db(&soft[2000..], 7000.0) - band_db(&soft_out.channel(0)[2000..], 7000.0);

        assert!(red_loud > 4.0, "loud sibilance reduced: {red_loud} dB");
        assert!(red_soft > 4.0, "soft sibilance reduced: {red_soft} dB");
        // Level-independence: the two reductions are close.
        assert!(
            (red_loud - red_soft).abs() < 3.0,
            "reduction should be level-independent: {red_loud} vs {red_soft}"
        );
    }

    #[test]
    fn gentler_sensitivity_ducks_less() {
        let x = voice_with_ess(1.0, 24_000);
        let (_d, default_red) = run(DeEss::new(DeEssMode::Split), &buf(x.clone()));
        let mut gentle = DeEss::new(DeEssMode::Split);
        gentle.set_sensitivity(0.6, 0.25); // singing: duck less, floor higher
        let (_g, gentle_red) = run(gentle, &buf(x));
        assert!(
            gentle_red < default_red,
            "gentler de-ess should reduce HF less: gentle {gentle_red} vs default {default_red} dB"
        );
    }

    #[test]
    fn split_mode_preserves_the_body() {
        let x = voice_with_ess(1.0, 24_000);
        let (out, _) = run(DeEss::new(DeEssMode::Split), &buf(x.clone()));
        let body_before = band_db(&x[2000..], 200.0);
        let body_after = band_db(&out.channel(0)[2000..], 200.0);
        assert!(
            (body_before - body_after).abs() < 1.0,
            "split mode must keep the 200 Hz body: {body_before} -> {body_after}"
        );
        assert!(
            band_db(&out.channel(0)[2000..], 7000.0) < band_db(&x[2000..], 7000.0) - 4.0,
            "but still duck the sibilance"
        );
    }

    #[test]
    fn publishes_positive_hf_reduction() {
        let x = voice_with_ess(1.0, 24_000);
        let (_out, red) = run(DeEss::new(DeEssMode::Split), &buf(x));
        assert!(
            red > 1.0,
            "should publish a positive HF reduction: {red} dB"
        );
    }

    #[test]
    fn wide_mode_also_reduces_sibilance() {
        let x = voice_with_ess(1.0, 24_000);
        let (out, _) = run(DeEss::new(DeEssMode::Wide), &buf(x.clone()));
        assert!(
            band_db(&out.channel(0)[2000..], 7000.0) < band_db(&x[2000..], 7000.0) - 4.0,
            "wide mode reduces sibilance"
        );
    }

    #[test]
    fn bypass_is_exact_identity() {
        let x = voice_with_ess(1.0, 10_000);
        let mut m = DeEss::new(DeEssMode::Split);
        m.set_enabled(false);
        let (out, _) = run(m, &buf(x.clone()));
        for (a, b) in out.channel(0).iter().zip(&x) {
            assert!((a - b).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }
}
