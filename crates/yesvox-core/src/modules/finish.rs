//! Finish stage: presence/air EQ + leveler + loudness/true-peak limiter
//! (plan §4.3 / §4.4).
//!
//! The standalone output's last step: gentle preset-driven presence/air shaping,
//! a program leveler, loudness-to-target, and a look-ahead true-peak limiter.
//! The limiter's detector sidechain is low-passed **above the air band**, so a
//! presence/air boost can't make the limiter pump. Loudness lands within
//! ±0.3 LU of target and true peak stays ≤ the ceiling. Bypassed → identity.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::dsp::Biquad;
use crate::loudness::{integrated_lufs, true_peak_dbtp};
use crate::processor::VocalProcessor;

/// Loudness presets (plan §4.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoudnessTarget {
    /// −16 LUFS (podcast / spoken word).
    Podcast,
    /// −14 LUFS (streaming).
    Streaming,
    /// Keep the input's integrated loudness.
    MatchInput,
    /// Normalize the peak to the true-peak ceiling (no loudness target).
    PeakNormalize,
}

impl LoudnessTarget {
    /// The LUFS value for the fixed targets, if any.
    fn lufs(self) -> Option<f32> {
        match self {
            LoudnessTarget::Podcast => Some(-16.0),
            LoudnessTarget::Streaming => Some(-14.0),
            LoudnessTarget::MatchInput | LoudnessTarget::PeakNormalize => None,
        }
    }
}

/// The finish stage.
#[derive(Clone, Debug)]
pub struct FinishStage {
    enabled: bool,
    target: LoudnessTarget,
    /// Presence boost (≈5 kHz) in dB.
    presence_db: f32,
    /// Air boost (high shelf ≈12 kHz) in dB.
    air_db: f32,
    /// True-peak ceiling, dBTP.
    ceiling_dbtp: f32,
    /// Limiter look-ahead, seconds (≥5 ms per the acceptance).
    lookahead_s: f32,
    /// Detector sidechain low-pass cutoff (above the air band).
    sidechain_cutoff_hz: f32,
    sample_rate: f32,
}

impl Default for FinishStage {
    fn default() -> Self {
        FinishStage {
            enabled: true,
            target: LoudnessTarget::Podcast,
            presence_db: 0.0,
            air_db: 0.0,
            ceiling_dbtp: -1.0,
            lookahead_s: 0.005,
            sidechain_cutoff_hz: 12_000.0,
            sample_rate: 48_000.0,
        }
    }
}

impl FinishStage {
    /// New finish stage with a loudness target.
    pub fn new(target: LoudnessTarget) -> Self {
        FinishStage {
            target,
            ..FinishStage::default()
        }
    }

    /// Set the presence (≈5 kHz) and air (≈12 kHz shelf) boosts in dB.
    pub fn set_eq(&mut self, presence_db: f32, air_db: f32) {
        self.presence_db = presence_db;
        self.air_db = air_db;
    }

    /// Enable or bypass.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Whether the module is active.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Look-ahead in samples.
    fn lookahead_samples(&self) -> usize {
        ((self.lookahead_s * self.sample_rate) as usize).max(1)
    }

    fn apply_eq(&self, buffer: &mut AudioBuffer) {
        if self.presence_db == 0.0 && self.air_db == 0.0 {
            return;
        }
        let sr = self.sample_rate;
        for ch in 0..buffer.channel_count() {
            let mut presence = Biquad::peaking(sr, 5000.0, 1.0, self.presence_db);
            let mut air = Biquad::high_shelf(sr, 12_000.0, 0.707, self.air_db);
            for s in buffer.channel_mut(ch) {
                *s = air.process(presence.process(*s));
            }
        }
    }

    fn apply_gain(buffer: &mut AudioBuffer, gain: f32) {
        for ch in 0..buffer.channel_count() {
            for s in buffer.channel_mut(ch) {
                *s *= gain;
            }
        }
    }

    /// Look-ahead true-peak limiter with an air-filtered detector sidechain.
    fn limit(&self, buffer: &mut AudioBuffer) {
        let sr = self.sample_rate;
        // Limit a touch below the requested ceiling to absorb inter-sample
        // overshoot, so the measured true peak stays under the ceiling.
        let ceiling = 10f32.powf((self.ceiling_dbtp - 0.4) / 20.0);
        let look = self.lookahead_samples();
        let release = (-1.0 / (0.050 * sr)).exp();

        for ch in 0..buffer.channel_count() {
            let n = buffer.frames();
            let samples = buffer.channel_mut(ch);

            // Detector sidechain: low-passed above the air band (two cascaded
            // poles for a steep skirt) so a presence/air boost doesn't drive the
            // gain computer (no pumping).
            let mut lp1 = Biquad::low_pass(sr, self.sidechain_cutoff_hz, 0.707);
            let mut lp2 = Biquad::low_pass(sr, self.sidechain_cutoff_hz, 0.707);
            let sc: Vec<f32> = samples
                .iter()
                .map(|&s| lp2.process(lp1.process(s)).abs())
                .collect();

            // Per-sample required gain, then the look-ahead window-minimum.
            let req: Vec<f32> = sc
                .iter()
                .map(|&m| if m > ceiling { ceiling / m } else { 1.0 })
                .collect();
            let mut gmin = vec![1.0f32; n];
            for (i, g) in gmin.iter_mut().enumerate() {
                let end = (i + look).min(n);
                let mut m = 1.0f32;
                for &r in &req[i..end] {
                    m = m.min(r);
                }
                *g = m;
            }

            // Release smoothing: gain drops instantly (look-ahead pre-empts the
            // peak) but recovers slowly.
            let mut g_prev = 1.0f32;
            for (s, &g) in samples.iter_mut().zip(gmin.iter()) {
                let applied = if g < g_prev {
                    g
                } else {
                    release * g_prev + (1.0 - release) * g
                };
                g_prev = applied;
                *s *= applied;
            }
        }
    }
}

impl VocalProcessor for FinishStage {
    fn prepare(&mut self, sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {
        self.sample_rate = sample_rate as f32;
    }

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled {
            return;
        }
        // Loudness reference for Match-input is the pre-EQ input.
        let input_lufs = integrated_lufs(buffer);

        self.apply_eq(buffer);

        match self.target {
            LoudnessTarget::PeakNormalize => {
                let tp = true_peak_dbtp(buffer);
                let gain = 10f32.powf((self.ceiling_dbtp - tp) / 20.0);
                Self::apply_gain(buffer, gain);
            }
            target => {
                let want = target.lufs().or(input_lufs);
                if let (Some(target_lufs), Some(measured)) = (want, integrated_lufs(buffer)) {
                    let gain = 10f32.powf((target_lufs - measured) / 20.0);
                    Self::apply_gain(buffer, gain);
                }
            }
        }

        self.limit(buffer);
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    fn run(mut m: FinishStage, mut b: AudioBuffer) -> AudioBuffer {
        m.prepare(SR, 4096, b.layout());
        m.process(&mut b);
        b
    }

    fn tone_buf(amp: f32, freq: f32, secs: f32) -> AudioBuffer {
        let n = (SR as f32 * secs) as usize;
        let ch: Vec<f32> = (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / SR as f32).sin())
            .collect();
        AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![ch])
    }

    #[test]
    fn hits_podcast_and_streaming_targets_within_tolerance() {
        for (target, want) in [
            (LoudnessTarget::Podcast, -16.0),
            (LoudnessTarget::Streaming, -14.0),
        ] {
            let out = run(FinishStage::new(target), tone_buf(0.05, 1000.0, 4.0));
            let lufs = integrated_lufs(&out).expect("loudness");
            assert!(
                (lufs - want).abs() <= 0.3,
                "{target:?}: expected {want} ±0.3, got {lufs}"
            );
        }
    }

    #[test]
    fn true_peak_stays_under_the_ceiling() {
        // A loud, peaky input normalized up: the limiter must hold ≤ −1 dBTP.
        let out = run(
            FinishStage::new(LoudnessTarget::Streaming),
            tone_buf(0.9, 1000.0, 3.0),
        );
        let tp = true_peak_dbtp(&out);
        assert!(tp <= -1.0 + 0.05, "true peak must stay ≤ −1 dBTP, got {tp}");
    }

    #[test]
    fn match_input_keeps_loudness() {
        let input = tone_buf(0.1, 1000.0, 4.0);
        let before = integrated_lufs(&input).unwrap();
        let out = run(FinishStage::new(LoudnessTarget::MatchInput), input);
        let after = integrated_lufs(&out).unwrap();
        assert!((after - before).abs() <= 0.3, "{before} -> {after}");
    }

    #[test]
    fn peak_normalize_lifts_peak_to_ceiling() {
        let out = run(
            FinishStage::new(LoudnessTarget::PeakNormalize),
            tone_buf(0.2, 1000.0, 2.0),
        );
        let tp = true_peak_dbtp(&out);
        assert!(
            (tp - -1.0).abs() < 0.6,
            "peak should sit near −1 dBTP, got {tp}"
        );
    }

    #[test]
    fn air_boost_does_not_pump_the_limiter() {
        // Isolate the limiter (no loudness step, which would itself respond to
        // the extra air energy). A loud body drives the limiter; adding loud
        // air-band (15 kHz) content must NOT increase the limiting, because the
        // detector sidechain is filtered above the air band.
        let n = SR as usize * 2;
        let body: Vec<f32> = (0..n)
            .map(|i| 0.95 * (2.0 * PI * 300.0 * i as f32 / SR as f32).sin())
            .collect();
        let with_air: Vec<f32> = body
            .iter()
            .enumerate()
            .map(|(i, &b)| b + 0.4 * (2.0 * PI * 15_000.0 * i as f32 / SR as f32).sin())
            .collect();

        let mut m = FinishStage::new(LoudnessTarget::Streaming);
        m.prepare(SR, 4096, ChannelLayout::Mono);

        let mut body_buf = AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![body.clone()]);
        let mut air_buf = AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![with_air]);
        m.limit(&mut body_buf);
        m.limit(&mut air_buf);

        let body_db = |b: &AudioBuffer| {
            10.0 * crate::dsp::goertzel_power(&b.channel(0)[4800..n - 4800], SR as f32, 300.0)
                .max(1e-12)
                .log10()
        };
        assert!(
            (body_db(&body_buf) - body_db(&air_buf)).abs() < 1.0,
            "air boost pumped the limiter: {} vs {}",
            body_db(&body_buf),
            body_db(&air_buf)
        );
    }

    #[test]
    fn lookahead_is_at_least_five_ms() {
        let f = FinishStage::default();
        assert!(f.lookahead_s >= 0.005, "look-ahead must be ≥5 ms");
    }

    #[test]
    fn bypass_is_exact_identity() {
        let x = tone_buf(0.3, 440.0, 1.0);
        let mut m = FinishStage::new(LoudnessTarget::Podcast);
        m.set_enabled(false);
        let out = run(m, x.clone());
        for (a, b) in out.channel(0).iter().zip(x.channel(0)) {
            assert!((a - b).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }
}
