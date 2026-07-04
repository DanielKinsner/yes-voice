//! De-hum: adaptive notch/comb cascade (plan §3.1).
//!
//! Detects the dominant mains-hum fundamental (50 vs 60 Hz) and removes it plus
//! its harmonics with a cascade of high-Q notches. A slow tracker re-estimates
//! the fundamental per sub-block and retunes the notches (keeping their state)
//! so the cascade follows drift without dulling the voice. Bypassed, it is the
//! exact identity.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::dsp::{goertzel_power, Biquad};
use crate::processor::VocalProcessor;

/// Adaptive mains-hum remover.
#[derive(Clone, Debug)]
pub struct DeHum {
    enabled: bool,
    /// Nominal fundamental; `None` auto-detects 50 vs 60 Hz on the first block.
    nominal: Option<f32>,
    /// How many harmonics (including the fundamental) to notch.
    harmonics: u32,
    /// Notch quality (higher = narrower, more voice-preserving).
    q: f32,
    /// Whether the tracker follows drift.
    track: bool,

    sample_rate: f32,
    f0: f32,
    // One notch cascade per channel so dual-mono lanes stay independent.
    cascades: Vec<Vec<Biquad>>,
}

impl Default for DeHum {
    fn default() -> Self {
        DeHum {
            enabled: true,
            nominal: None,
            harmonics: 6,
            q: 30.0,
            track: true,
            sample_rate: 48_000.0,
            f0: 0.0,
            cascades: Vec::new(),
        }
    }
}

impl DeHum {
    fn normalize_nominal(nominal: Option<f32>) -> Option<f32> {
        nominal.map(|f| if f < 55.0 { 50.0 } else { 60.0 })
    }

    /// A de-hum with an explicit nominal fundamental (e.g. from the analysis
    /// report). Pass `None` to auto-detect.
    pub fn new(nominal: Option<f32>) -> Self {
        DeHum {
            nominal,
            ..DeHum::default()
        }
    }

    /// A de-hum with the Advanced controls applied.
    pub fn with_settings(nominal: Option<f32>, harmonics: u32, q: f32, track: bool) -> Self {
        let mut dehum = DeHum::new(nominal);
        dehum.set_settings(nominal, harmonics, q, track);
        dehum
    }

    /// Enable or bypass. Bypassed → identity.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Whether the module is active.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The currently tracked fundamental in Hz (`0.0` before the first block).
    pub fn tracked_frequency(&self) -> f32 {
        self.f0
    }

    /// Advanced settings: nominal fundamental, harmonic count, notch Q, tracking.
    pub fn settings(&self) -> (Option<f32>, u32, f32, bool) {
        (self.nominal, self.harmonics, self.q, self.track)
    }

    /// Apply Advanced controls. Harmonics allow `0` for a true no-notch dry
    /// comparison; upper bounds keep this module a hum remover, not a tone tool.
    pub fn set_settings(&mut self, nominal: Option<f32>, harmonics: u32, q: f32, track: bool) {
        let nominal = Self::normalize_nominal(nominal);
        let harmonics = harmonics.min(12);
        let q = q.clamp(5.0, 80.0);
        if self.nominal != nominal || self.harmonics != harmonics || self.track != track {
            self.f0 = 0.0;
        }
        if self.harmonics != harmonics {
            self.cascades.clear();
        }
        self.nominal = nominal;
        self.harmonics = harmonics;
        self.q = q;
        self.track = track;
    }

    /// Estimate the dominant fundamental (50 vs 60 Hz) over `samples`.
    fn detect_fundamental(&self, samples: &[f32]) -> f32 {
        let p50 = goertzel_power(samples, self.sample_rate, 50.0);
        let p60 = goertzel_power(samples, self.sample_rate, 60.0);
        if p50 >= p60 {
            50.0
        } else {
            60.0
        }
    }

    /// Refine `f0` by scanning ±1.5 Hz around the current estimate.
    fn refine(&self, samples: &[f32], around: f32) -> f32 {
        let mut best = (around, goertzel_power(samples, self.sample_rate, around));
        let mut step = -1.5;
        while step <= 1.5 {
            let f = around + step;
            let p = goertzel_power(samples, self.sample_rate, f);
            if p > best.1 {
                best = (f, p);
            }
            step += 0.25;
        }
        best.0
    }

    /// Rebuild notch coefficients for the cascade at `f0`, preserving each
    /// biquad's delay-line state (retuning is click-free).
    fn retune(&mut self, f0: f32) {
        self.f0 = f0;
        let nyq = self.sample_rate / 2.0;
        for cascade in &mut self.cascades {
            for (h, notch) in cascade.iter_mut().enumerate() {
                let f = f0 * (h as f32 + 1.0);
                if f < nyq {
                    let fresh = Biquad::notch(self.sample_rate, f, self.q);
                    notch.copy_coeffs_from(&fresh);
                } else {
                    notch.copy_coeffs_from(&Biquad::identity());
                }
            }
        }
    }
}

impl VocalProcessor for DeHum {
    fn prepare(&mut self, sample_rate: u32, _max_block: usize, layout: ChannelLayout) {
        self.sample_rate = sample_rate as f32;
        self.f0 = 0.0;
        self.cascades = (0..layout.channels())
            .map(|_| vec![Biquad::identity(); self.harmonics as usize])
            .collect();
    }

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled || self.harmonics == 0 {
            return;
        }
        // Lazily size the cascades if prepare wasn't called with this layout.
        if self.cascades.len() != buffer.channel_count() {
            self.cascades = (0..buffer.channel_count())
                .map(|_| vec![Biquad::identity(); self.harmonics as usize])
                .collect();
            self.f0 = 0.0;
        }

        let frames = buffer.frames();
        if frames == 0 {
            return;
        }

        // Initial fundamental from the first channel.
        if self.f0 == 0.0 {
            let probe = &buffer.channel(0)[..frames.min((0.2 * self.sample_rate) as usize).max(1)];
            let base = self
                .nominal
                .unwrap_or_else(|| self.detect_fundamental(probe));
            self.retune(base);
        }

        let track_win = ((0.100 * self.sample_rate) as usize).max(256);
        let mut start = 0;
        while start < frames {
            let end = (start + track_win).min(frames);

            if self.track && self.nominal.is_none() {
                // Re-estimate from channel 0 in this sub-block and retune.
                let seg: Vec<f32> = buffer.channel(0)[start..end].to_vec();
                let refined = self.refine(&seg, self.f0);
                // Nudge toward the refined estimate to avoid jitter.
                let nudged = self.f0 + 0.5 * (refined - self.f0);
                self.retune(nudged);
            }

            for ch in 0..buffer.channel_count() {
                let samples = buffer.channel_mut(ch);
                // SAFETY-free split: clone the cascade out to satisfy the borrow
                // checker, process, write back.
                let mut cascade = std::mem::take(&mut self.cascades[ch]);
                for s in &mut samples[start..end] {
                    let mut y = *s;
                    for notch in &mut cascade {
                        y = notch.process(y);
                    }
                    *s = y;
                }
                self.cascades[ch] = cascade;
            }
            start = end;
        }
    }

    fn reset(&mut self) {
        self.f0 = 0.0;
        for cascade in &mut self.cascades {
            for notch in cascade {
                notch.reset();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use crate::dsp::goertzel_power;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;

    fn buf(ch: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(SR as u32, 16, SampleEncoding::Int, vec![ch])
    }

    fn band_db(samples: &[f32], freq: f32) -> f32 {
        10.0 * goertzel_power(samples, SR, freq).max(1e-12).log10()
    }

    fn run(mut m: DeHum, mut b: AudioBuffer) -> AudioBuffer {
        m.prepare(SR as u32, 4096, b.layout());
        m.process(&mut b);
        b
    }

    #[test]
    fn removes_sixty_hz_and_harmonics_keeps_voice() {
        let n = 48_000;
        let voice = 220.0;
        let mut ch = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / SR;
            let s = 0.4 * (2.0 * PI * voice * t).sin()
                + 0.25 * (2.0 * PI * 60.0 * t).sin()
                + 0.12 * (2.0 * PI * 120.0 * t).sin()
                + 0.06 * (2.0 * PI * 180.0 * t).sin();
            ch.push(s);
        }
        let before = buf(ch.clone());
        let after = run(DeHum::new(Some(60.0)), buf(ch));

        let b0 = before.channel(0);
        let a0 = after.channel(0);
        // Skip the filter transient.
        let b = &b0[8000..];
        let a = &a0[8000..];

        assert!(
            band_db(a, 60.0) < band_db(b, 60.0) - 20.0,
            "60 Hz should drop ≥20 dB"
        );
        assert!(
            band_db(a, 120.0) < band_db(b, 120.0) - 15.0,
            "120 Hz harmonic should drop"
        );
        // Voice tone preserved within ~1 dB.
        assert!(
            (band_db(a, voice) - band_db(b, voice)).abs() < 1.0,
            "voice band must survive"
        );
    }

    #[test]
    fn bypass_is_exact_identity() {
        let n = 10_000;
        let ch: Vec<f32> = (0..n)
            .map(|i| 0.3 * (2.0 * PI * 60.0 * i as f32 / SR).sin())
            .collect();
        let mut m = DeHum::new(Some(60.0));
        m.set_enabled(false);
        let original = buf(ch.clone());
        let out = run(m, buf(ch));
        for (a, b) in out.channel(0).iter().zip(original.channel(0)) {
            assert!((a - b).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }

    #[test]
    fn tracks_a_drifting_fundamental() {
        // Hum sweeps 59 -> 61 Hz over the file; a static notch at 60 would leak.
        let n = 96_000; // 2 s
        let mut ch = Vec::with_capacity(n);
        let mut phase = 0.0f32;
        for i in 0..n {
            let frac = i as f32 / n as f32;
            let f = 59.0 + 2.0 * frac;
            phase += 2.0 * PI * f / SR;
            ch.push(0.3 * phase.sin());
        }
        let before = buf(ch.clone());
        let tracked = run(DeHum::new(None), buf(ch.clone())); // auto + tracking

        let mut stat = DeHum::new(Some(60.0));
        stat.track = false;
        let static_notch = run(stat, buf(ch));

        let energy = |b: &AudioBuffer| {
            let s = &b.channel(0)[16_000..];
            s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32
        };
        let e_before = energy(&before);
        let e_tracked = energy(&tracked);
        let e_static = energy(&static_notch);
        assert!(
            e_tracked < e_before * 0.1,
            "tracker should cut drifting hum ≥10 dB"
        );
        assert!(
            e_tracked < e_static,
            "tracking should beat a static notch on drift: {e_tracked} vs {e_static}"
        );
    }
}
