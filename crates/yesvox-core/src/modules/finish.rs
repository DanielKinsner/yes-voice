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
    /// An arbitrary integrated-loudness target in LUFS (the editable OUTPUT-panel
    /// box). Carried as `f32`, so [`LoudnessTarget`] is `PartialEq` but not `Eq`.
    Custom(f32),
}

impl LoudnessTarget {
    /// The integrated-loudness target in LUFS, if this target has a fixed value
    /// ([`LoudnessTarget::MatchInput`]/[`LoudnessTarget::PeakNormalize`] do not).
    pub fn lufs(self) -> Option<f32> {
        match self {
            LoudnessTarget::Podcast => Some(-16.0),
            LoudnessTarget::Streaming => Some(-14.0),
            LoudnessTarget::Custom(lufs) => Some(lufs),
            LoudnessTarget::MatchInput | LoudnessTarget::PeakNormalize => None,
        }
    }
}

/// The finish stage.
#[derive(Clone, Debug)]
pub struct FinishStage {
    enabled: bool,
    presence_air_enabled: bool,
    leveling_enabled: bool,
    loudness_enabled: bool,
    target: LoudnessTarget,
    /// Presence boost (≈5 kHz) in dB.
    presence_db: f32,
    /// Air boost (high shelf ≈12 kHz) in dB.
    air_db: f32,
    /// User-facing Presence/Air amount, `[0, 1]`, scaling the preset EQ patch.
    presence_air_amount: f32,
    /// User-facing Leveling amount, `[0, 1]`, scaling the program gain rider.
    leveling_amount: f32,
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
            presence_air_enabled: true,
            leveling_enabled: true,
            loudness_enabled: true,
            target: LoudnessTarget::Podcast,
            presence_db: 0.0,
            air_db: 0.0,
            presence_air_amount: 1.0,
            leveling_amount: 1.0,
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

    /// Set the Presence/Air amount, scaling the preset EQ patch.
    pub fn set_presence_air_amount(&mut self, amount: f32) {
        self.presence_air_amount = amount.clamp(0.0, 1.0);
    }

    /// The current Presence/Air amount.
    pub fn presence_air_amount(&self) -> f32 {
        self.presence_air_amount
    }

    /// Set the Leveling amount, scaling the program-level gain rider.
    pub fn set_leveling_amount(&mut self, amount: f32) {
        self.leveling_amount = amount.clamp(0.0, 1.0);
    }

    /// The current Leveling amount.
    pub fn leveling_amount(&self) -> f32 {
        self.leveling_amount
    }

    /// Override the loudness target (the Session-level OUTPUT/Loudness control).
    pub fn set_target(&mut self, target: LoudnessTarget) {
        self.target = target;
    }

    /// Enable or bypass.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Whether the module is active.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Enable or bypass the presence/air EQ sub-control.
    pub fn set_presence_air_enabled(&mut self, on: bool) {
        self.presence_air_enabled = on;
    }

    /// Whether the presence/air EQ sub-control is active.
    pub fn presence_air_enabled(&self) -> bool {
        self.presence_air_enabled
    }

    /// Enable or bypass the program-leveling sub-control.
    pub fn set_leveling_enabled(&mut self, on: bool) {
        self.leveling_enabled = on;
    }

    /// Whether the program-leveling sub-control is active.
    pub fn leveling_enabled(&self) -> bool {
        self.leveling_enabled
    }

    /// Enable or bypass loudness normalization plus the true-peak limiter.
    pub fn set_loudness_enabled(&mut self, on: bool) {
        self.loudness_enabled = on;
    }

    /// Whether loudness normalization plus the true-peak limiter are active.
    pub fn loudness_enabled(&self) -> bool {
        self.loudness_enabled
    }

    /// Look-ahead in samples.
    fn lookahead_samples(&self) -> usize {
        ((self.lookahead_s * self.sample_rate) as usize).max(1)
    }

    fn apply_eq(&self, buffer: &mut AudioBuffer) {
        let presence_db = self.presence_db * self.presence_air_amount;
        let air_db = self.air_db * self.presence_air_amount;
        if presence_db == 0.0 && air_db == 0.0 {
            return;
        }
        let sr = self.sample_rate;
        for ch in 0..buffer.channel_count() {
            let mut presence = Biquad::peaking(sr, 5000.0, 1.0, presence_db);
            let mut air = Biquad::high_shelf(sr, 12_000.0, 0.707, air_db);
            for s in buffer.channel_mut(ch) {
                *s = air.process(presence.process(*s));
            }
        }
    }

    fn apply_leveling(&self, buffer: &mut AudioBuffer) {
        if self.leveling_amount <= 0.0 || buffer.frames() == 0 {
            return;
        }

        let sr = self.sample_rate.max(1.0);
        let block = ((0.020 * sr) as usize).max(1);
        let max_gain_db = 6.0 * self.leveling_amount;
        let strength = 0.65 * self.leveling_amount;

        for ch in 0..buffer.channel_count() {
            let samples = buffer.channel_mut(ch);
            let global_rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32)
                .sqrt()
                .max(1.0e-6);
            let global_db = 20.0 * global_rms.log10();
            let mut smoothed_gain = 1.0f32;

            for chunk in samples.chunks_mut(block) {
                let block_rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32)
                    .sqrt()
                    .max(1.0e-6);
                let block_db = 20.0 * block_rms.log10();
                let gain_db = ((global_db - block_db) * strength).clamp(-max_gain_db, max_gain_db);
                let target_gain = 10f32.powf(gain_db / 20.0);

                for s in chunk {
                    smoothed_gain += 0.0025 * (target_gain - smoothed_gain);
                    *s *= smoothed_gain;
                }
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
        // Loudness reference for Match-input is the pre-finish input.
        let input_lufs = integrated_lufs(buffer);

        if self.presence_air_enabled {
            self.apply_eq(buffer);
        }
        if self.leveling_enabled {
            self.apply_leveling(buffer);
        }
        if self.loudness_enabled {
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

    fn stepped_tone_buf() -> AudioBuffer {
        let n = SR as usize * 4;
        let mut ch = Vec::with_capacity(n);
        for i in 0..n {
            let amp = if i < n / 4 {
                0.04
            } else if i < n / 2 {
                0.22
            } else if i < 3 * n / 4 {
                0.07
            } else {
                0.35
            };
            ch.push(amp * (2.0 * PI * 220.0 * i as f32 / SR as f32).sin());
        }
        AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![ch])
    }

    fn block_rms_spread_db(buffer: &AudioBuffer, block: usize) -> f32 {
        let mut levels: Vec<f32> = buffer
            .channel(0)
            .chunks(block)
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| {
                let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt();
                20.0 * rms.max(1.0e-6).log10()
            })
            .collect();
        levels.sort_by(|a, b| a.partial_cmp(b).unwrap());
        levels[levels.len() - 1] - levels[0]
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
    fn presence_air_bypass_skips_eq_but_loudness_still_runs() {
        let input = tone_buf(0.02, 5000.0, 4.0);

        let baseline = run(FinishStage::new(LoudnessTarget::Podcast), input.clone());

        let mut no_eq = FinishStage::new(LoudnessTarget::Podcast);
        no_eq.set_eq(9.0, 9.0);
        no_eq.set_presence_air_enabled(false);
        let out = run(no_eq, input);

        for (a, b) in out.channel(0).iter().zip(baseline.channel(0)) {
            assert!(
                (a - b).abs() <= 1e-6,
                "presence/air bypass must skip EQ exactly"
            );
        }
        let lufs = integrated_lufs(&out).expect("loudness");
        assert!(
            (lufs - (-16.0)).abs() <= 0.3,
            "loudness must still run with presence/air bypassed: {lufs}"
        );
    }

    #[test]
    fn presence_air_amount_zero_bypasses_eq_but_loudness_still_runs() {
        let input = tone_buf(0.02, 5000.0, 4.0);

        let baseline = run(FinishStage::new(LoudnessTarget::Podcast), input.clone());

        let mut dry_eq = FinishStage::new(LoudnessTarget::Podcast);
        dry_eq.set_eq(9.0, 9.0);
        dry_eq.set_presence_air_amount(0.0);
        let out = run(dry_eq, input);

        for (a, b) in out.channel(0).iter().zip(baseline.channel(0)) {
            assert!(
                (a - b).abs() <= 1e-6,
                "Presence/Air amount 0 must bypass EQ exactly"
            );
        }
        let lufs = integrated_lufs(&out).expect("loudness");
        assert!(
            (lufs - (-16.0)).abs() <= 0.3,
            "loudness must still run with Presence/Air amount 0: {lufs}"
        );
    }

    #[test]
    fn presence_air_amount_scales_the_eq_patch() {
        let input = tone_buf(0.1, 5000.0, 2.0);
        let input_lufs = integrated_lufs(&input).expect("input loudness");

        let run_eq = |amount: f32| {
            let mut m = FinishStage::new(LoudnessTarget::MatchInput);
            m.set_loudness_enabled(false);
            m.set_eq(6.0, 0.0);
            m.set_presence_air_amount(amount);
            run(m, input.clone())
        };
        let dry_lufs = integrated_lufs(&run_eq(0.0)).expect("dry eq loudness");
        let half_lufs = integrated_lufs(&run_eq(0.5)).expect("half eq loudness");
        let full_lufs = integrated_lufs(&run_eq(1.0)).expect("full eq loudness");

        assert!(
            (dry_lufs - input_lufs).abs() <= 0.1,
            "amount 0 must leave the EQ patch dry"
        );
        assert!(
            dry_lufs < half_lufs && half_lufs < full_lufs,
            "amount must scale the EQ monotonically: {dry_lufs}, {half_lufs}, {full_lufs}"
        );
    }

    #[test]
    fn loudness_bypass_skips_normalization_and_limiter_but_eq_still_runs() {
        let input = tone_buf(0.7, 5000.0, 4.0);
        let before_lufs = integrated_lufs(&input).expect("input loudness");

        let mut m = FinishStage::new(LoudnessTarget::Podcast);
        m.set_eq(6.0, 0.0);
        m.set_loudness_enabled(false);
        let out = run(m, input);

        let after_lufs = integrated_lufs(&out).expect("output loudness");
        assert!(
            (after_lufs - (-16.0)).abs() > 3.0,
            "loudness bypass must not normalize to the target: {after_lufs}"
        );
        assert!(
            after_lufs > before_lufs + 3.0,
            "presence EQ must still run with loudness bypassed: {before_lufs} -> {after_lufs}"
        );
        let tp = true_peak_dbtp(&out);
        assert!(
            tp > -0.2,
            "loudness bypass must also bypass the limiter ceiling, got {tp} dBTP"
        );
    }

    #[test]
    fn leveling_amount_zero_is_audio_neutral() {
        let input = stepped_tone_buf();
        let mut on = FinishStage::new(LoudnessTarget::MatchInput);
        on.set_loudness_enabled(false);
        on.set_leveling_enabled(true);
        on.set_leveling_amount(0.0);
        let out_on = run(on, input.clone());

        let mut off = FinishStage::new(LoudnessTarget::MatchInput);
        off.set_loudness_enabled(false);
        off.set_leveling_enabled(false);
        let out_off = run(off, input);

        for (a, b) in out_on.channel(0).iter().zip(out_off.channel(0)) {
            assert!(
                (a - b).abs() <= f32::EPSILON,
                "leveling amount 0 must render like Leveling disabled"
            );
        }
    }

    #[test]
    fn leveling_reduces_program_level_spread_without_loudness() {
        let input = stepped_tone_buf();
        let before_spread = block_rms_spread_db(&input, SR as usize / 2);

        let mut m = FinishStage::new(LoudnessTarget::MatchInput);
        m.set_loudness_enabled(false);
        m.set_leveling_amount(1.0);
        let out = run(m, input.clone());
        let after_spread = block_rms_spread_db(&out, SR as usize / 2);

        assert_eq!(out.frames(), input.frames(), "leveler must preserve length");
        assert!(
            after_spread < before_spread - 1.5,
            "leveling should reduce program spread: {before_spread} -> {after_spread} dB"
        );
        assert!(
            true_peak_dbtp(&out) < 0.0,
            "bounded leveler must not create full-scale overs"
        );
    }

    #[test]
    fn leveling_amount_scales_the_program_gain_rider() {
        let input = stepped_tone_buf();
        let before_spread = block_rms_spread_db(&input, SR as usize / 2);
        let run_leveler = |amount: f32| {
            let mut m = FinishStage::new(LoudnessTarget::MatchInput);
            m.set_loudness_enabled(false);
            m.set_leveling_amount(amount);
            run(m, input.clone())
        };
        let half = run_leveler(0.5);
        let full = run_leveler(1.0);
        let half_spread = block_rms_spread_db(&half, SR as usize / 2);
        let full_spread = block_rms_spread_db(&full, SR as usize / 2);

        assert!(
            before_spread > half_spread && half_spread > full_spread,
            "amount must monotonically scale leveling: {before_spread}, {half_spread}, {full_spread}"
        );
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
