//! Breath control (plan §4.2).
//!
//! Detects breaths and **attenuates** the harsh ones — never gates them to
//! silence — so take length and natural delivery are preserved (this module
//! only applies gain; it never removes time). Breaths are low-to-moderate
//! energy, aperiodic (high zero-crossing rate), noisy frames between phrases.
//! It reads the de-ess high-frequency reduction (`DeEss::hf_reduction_db`) and
//! backs its own attenuation off by that amount, so the shared HF band is never
//! cut twice. Default **on** for voiceover, **off** for singing. Bypassed →
//! identity.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::processor::VocalProcessor;
use crate::settings::ContentMode;

/// Breath attenuator.
#[derive(Clone, Debug)]
pub struct BreathControl {
    enabled: bool,
    /// Requested breath attenuation, dB.
    breath_db: f32,
    /// HF reduction already applied by de-ess, dB (shared-band guard).
    shared_hf_db: f32,
    /// Gain floor — breaths are reduced, never silenced.
    min_gain: f32,
    sample_rate: f32,
}

impl Default for BreathControl {
    fn default() -> Self {
        BreathControl {
            enabled: true,
            breath_db: 9.0,
            shared_hf_db: 0.0,
            min_gain: 0.25, // never below −12 dB → never silence
            sample_rate: 48_000.0,
        }
    }
}

impl BreathControl {
    /// Breath control with content-appropriate default: on for voiceover, off
    /// for singing (plan §4.2).
    pub fn for_content(mode: ContentMode) -> Self {
        BreathControl {
            enabled: mode == ContentMode::Voiceover,
            ..BreathControl::default()
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

    /// Requested breath attenuation in dB.
    pub fn strength_db(&self) -> f32 {
        self.breath_db
    }

    /// Set the requested breath attenuation in dB.
    pub fn set_strength_db(&mut self, db: f32) {
        self.breath_db = db.max(0.0);
    }

    /// Tell breath control how much HF de-ess already removed (shared-band
    /// guard) so it won't double-cut. Read from [`crate::DeEss::hf_reduction_db`].
    pub fn set_shared_hf_reduction_db(&mut self, db: f32) {
        self.shared_hf_db = db.max(0.0);
    }

    /// The attenuation actually applied to breaths after backing off whatever
    /// de-ess already removed from the shared band.
    fn effective_atten_db(&self) -> f32 {
        (self.breath_db - self.shared_hf_db).max(0.0)
    }
}

/// Per-frame breath flag from energy + zero-crossing rate.
fn breath_frames(samples: &[f32], frame: usize, hop: usize) -> Vec<bool> {
    let mut energies = Vec::new();
    let mut zcrs = Vec::new();
    let mut starts = Vec::new();
    let mut pos = 0;
    while pos + frame <= samples.len() {
        let seg = &samples[pos..pos + frame];
        let e = seg.iter().map(|x| x * x).sum::<f32>() / frame as f32;
        let mut z = 0u32;
        for w in seg.windows(2) {
            if (w[0] >= 0.0) != (w[1] >= 0.0) {
                z += 1;
            }
        }
        energies.push(e);
        zcrs.push(z as f32 / frame as f32);
        starts.push(pos);
        pos += hop;
    }
    if energies.is_empty() {
        return Vec::new();
    }
    let peak_e = energies.iter().cloned().fold(0.0f32, f32::max).max(1e-12);

    energies
        .iter()
        .zip(&zcrs)
        .map(|(&e, &z)| {
            // Low-to-moderate energy (not silence, not full voice) + high ZCR
            // (aperiodic/noisy) → a breath.
            let energetic_enough = e > peak_e * 1.0e-4;
            let not_full_voice = e < peak_e * 0.25;
            let noisy = z > 0.15; // ≥ ~0.15 crossings/sample is broadband noise
            energetic_enough && not_full_voice && noisy
        })
        .collect()
}

impl VocalProcessor for BreathControl {
    fn prepare(&mut self, sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {
        self.sample_rate = sample_rate as f32;
    }

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled {
            return;
        }
        let atten = self.effective_atten_db();
        if atten <= 0.0 {
            return;
        }
        let gain = 10f32.powf(-atten / 20.0).max(self.min_gain);
        let sr = self.sample_rate;
        let frame = ((0.020 * sr) as usize).max(64);
        let hop = (frame / 2).max(1);
        let ramp = hop;

        for ch in 0..buffer.channel_count() {
            let n = buffer.frames();
            let samples = buffer.channel_mut(ch);
            let flags = breath_frames(samples, frame, hop);

            // Build a per-sample gain envelope (1.0 normally, `gain` over
            // breaths) with raised-cosine ramps so it never clicks.
            let mut env = vec![1.0f32; n];
            for (fi, &is_breath) in flags.iter().enumerate() {
                if !is_breath {
                    continue;
                }
                let start = fi * hop;
                let end = (start + frame).min(n);
                for (i, e) in env.iter_mut().enumerate().take(end).skip(start) {
                    let into = i - start;
                    let outof = end - 1 - i;
                    let edge = into.min(outof);
                    let t = (edge.min(ramp) as f32 + 0.5) / ramp as f32;
                    let ramp_g =
                        1.0 - (0.5 - 0.5 * (std::f32::consts::PI * t).cos()) * (1.0 - gain);
                    *e = e.min(ramp_g);
                }
            }
            for (s, &g) in samples.iter_mut().zip(env.iter()) {
                *s *= g;
            }
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

    fn run(mut m: BreathControl, mut b: AudioBuffer) -> AudioBuffer {
        m.prepare(SR, 4096, b.layout());
        m.process(&mut b);
        b
    }

    /// Voiced tone, then a noisy breath, then voiced tone again.
    fn speech_with_breath() -> (Vec<f32>, usize, usize) {
        let seg = 12_000usize; // 0.25 s each
        let mut x = vec![0.0f32; seg * 3];
        // Voiced segments (low ZCR).
        for i in 0..seg {
            let t = i as f32 / SR as f32;
            x[i] = 0.4 * (2.0 * PI * 200.0 * t).sin();
            x[seg * 2 + i] = 0.4 * (2.0 * PI * 200.0 * (i as f32 / SR as f32)).sin();
            let _ = t;
        }
        // Breath segment: low-level high-ZCR noise.
        let mut s = 0x1234u32;
        for i in 0..seg {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (s >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
            x[seg + i] = 0.12 * noise;
        }
        (x, seg, seg * 2) // breath spans [seg, seg*2)
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn attenuates_breath_but_never_silences_it() {
        let (x, b0, b1) = speech_with_breath();
        let out = run(BreathControl::default(), buf(x.clone()));

        let breath_before = rms(&x[b0..b1]);
        let breath_after = rms(&out.channel(0)[b0..b1]);
        assert!(
            breath_after < breath_before * 0.7,
            "breath should be attenuated: {breath_before} -> {breath_after}"
        );
        assert!(breath_after > 1.0e-4, "breath must not be silenced");
    }

    #[test]
    fn preserves_take_length_and_voiced_segments() {
        let (x, _b0, _b1) = speech_with_breath();
        let n = x.len();
        let out = run(BreathControl::default(), buf(x.clone()));
        assert_eq!(out.frames(), n, "length must be unchanged");
        // First voiced segment essentially untouched.
        let v_before = rms(&x[2000..10_000]);
        let v_after = rms(&out.channel(0)[2000..10_000]);
        assert!(
            (v_before - v_after).abs() < v_before * 0.05,
            "voiced delivery must be preserved"
        );
    }

    #[test]
    fn backs_off_when_deess_already_cut_the_shared_band() {
        let (x, b0, b1) = speech_with_breath();

        let mut alone = BreathControl::default();
        alone.set_strength_db(9.0);
        let out_alone = run(alone, buf(x.clone()));

        let mut with_deess = BreathControl::default();
        with_deess.set_strength_db(9.0);
        with_deess.set_shared_hf_reduction_db(9.0); // de-ess already did the cut
        let out_shared = run(with_deess, buf(x.clone()));

        let breath_alone = rms(&out_alone.channel(0)[b0..b1]);
        let breath_shared = rms(&out_shared.channel(0)[b0..b1]);
        // With de-ess having cut the shared band, breath control backs off →
        // it attenuates less (no double-cut), so the breath is louder here.
        assert!(
            breath_shared > breath_alone * 1.2,
            "shared HF guard should reduce double-cutting: {breath_alone} vs {breath_shared}"
        );
    }

    #[test]
    fn singing_default_is_off_voiceover_default_is_on() {
        assert!(BreathControl::for_content(ContentMode::Voiceover).enabled());
        assert!(!BreathControl::for_content(ContentMode::Singing).enabled());
    }

    #[test]
    fn bypass_is_exact_identity() {
        let (x, _b0, _b1) = speech_with_breath();
        let mut m = BreathControl::default();
        m.set_enabled(false);
        let out = run(m, buf(x.clone()));
        for (a, b) in out.channel(0).iter().zip(&x) {
            assert!((a - b).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }
}
