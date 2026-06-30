//! De-bleed + mix-refusal guard (plan §4.5).
//!
//! Cleans residual instrumental leakage and musical-noise on imported separated
//! stems — **artifact cleanup, never separation**. A hard promise boundary keeps
//! YesVox from pretending to be a separator: it measures how non-vocal / mix-like
//! the input is (spectral flatness — a solo vocal is tonal/peaky, a full mix is
//! broadband) and **blocks** files that are really full mixes ("looks like a mix
//! — run YesStems first"), **warns** on borderline ones, and silently **cleans**
//! genuine stems. On by default for imported stems, off for native single takes.
//! Block/warn thresholds are tunable. Bypassed → identity.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::denoise::engine::{Denoiser, SpectralDenoiser};
use crate::processor::VocalProcessor;
use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;

/// The guard's verdict on an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleedVerdict {
    /// A genuine stem — clean it silently.
    Clean,
    /// Borderline — clean, but warn the user.
    Warn,
    /// Looks like a full mix — refuse and point at YesStems.
    Block,
}

impl BleedVerdict {
    /// A user-facing message for warn/block, or `None` when clean.
    pub fn message(self) -> Option<&'static str> {
        match self {
            BleedVerdict::Clean => None,
            BleedVerdict::Warn => {
                Some("this stem has noticeable non-vocal bleed — cleaning, but check the result.")
            }
            BleedVerdict::Block => Some(
                "looks like a mix — run YesStems first, then bring the isolated vocal back here.",
            ),
        }
    }
}

/// De-bleed cleanup + mix-refusal guard.
#[derive(Clone)]
pub struct DeBleed {
    enabled: bool,
    /// Mix-likeness above this warns.
    warn_threshold: f32,
    /// Mix-likeness above this blocks.
    block_threshold: f32,
    cleaner: SpectralDenoiser,
}

impl Default for DeBleed {
    fn default() -> Self {
        DeBleed {
            enabled: true,
            warn_threshold: 0.02,
            block_threshold: 0.15,
            cleaner: SpectralDenoiser::default(),
        }
    }
}

impl DeBleed {
    /// De-bleed configured for the source: on for imported (separated) stems,
    /// off for native single takes (plan §4.5).
    pub fn for_source(imported: bool) -> Self {
        DeBleed {
            enabled: imported,
            ..DeBleed::default()
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

    /// Set the (tunable) warn and block thresholds on the mix-likeness score.
    pub fn set_thresholds(&mut self, warn: f32, block: f32) {
        self.warn_threshold = warn;
        self.block_threshold = block.max(warn);
    }

    /// Mix-likeness in `[0, 1]`: the average spectral flatness. A solo vocal is
    /// tonal (low flatness); a full mix is broadband (high flatness).
    pub fn mix_likeness(&self, buffer: &AudioBuffer) -> f32 {
        let frame = 1024;
        let hop = 512;
        let nbins = frame / 2 + 1;
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(frame);
        let window: Vec<f32> = (0..frame)
            .map(|n| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * n as f32 / frame as f32).cos())
            .collect();

        let mut acc = vec![0.0f64; nbins];
        let mut frames = 0u32;
        for ch in buffer.channels() {
            let mut pos = 0;
            while pos + frame <= ch.len() {
                let mut buf: Vec<Complex32> = (0..frame)
                    .map(|i| Complex32::new(ch[pos + i] * window[i], 0.0))
                    .collect();
                fft.process(&mut buf);
                let e: f32 = buf[..nbins].iter().map(|c| c.norm_sqr()).sum();
                if e > 1.0e-9 {
                    for (a, c) in acc.iter_mut().zip(buf[..nbins].iter()) {
                        *a += c.norm_sqr() as f64;
                    }
                    frames += 1;
                }
                pos += hop;
            }
        }
        if frames == 0 {
            return 0.0;
        }
        // Spectral flatness = geometric mean / arithmetic mean of the average
        // power spectrum.
        let mut log_sum = 0.0f64;
        let mut lin_sum = 0.0f64;
        let mut count = 0u32;
        for &p in &acc {
            let p = p / frames as f64;
            if p > 1.0e-12 {
                log_sum += p.ln();
                lin_sum += p;
                count += 1;
            }
        }
        if count == 0 || lin_sum <= 0.0 {
            return 0.0;
        }
        let geo = (log_sum / count as f64).exp();
        let arith = lin_sum / count as f64;
        (geo / arith).clamp(0.0, 1.0) as f32
    }

    /// Assess an input: the verdict plus its message (if any).
    pub fn assess(&self, buffer: &AudioBuffer) -> BleedVerdict {
        let score = self.mix_likeness(buffer);
        if score >= self.block_threshold {
            BleedVerdict::Block
        } else if score >= self.warn_threshold {
            BleedVerdict::Warn
        } else {
            BleedVerdict::Clean
        }
    }
}

impl VocalProcessor for DeBleed {
    fn prepare(&mut self, _sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {}

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled {
            return;
        }
        // A mix is refused, not processed — the host surfaces the block message.
        if self.assess(buffer) == BleedVerdict::Block {
            return;
        }
        // Clean residual leakage / musical noise with a gentle spectral pass.
        let sr = buffer.sample_rate() as f32;
        for ch in 0..buffer.channel_count() {
            let input = buffer.channel(ch).to_vec();
            let cleaned = self.cleaner.denoise(&input, sr);
            buffer.channel_mut(ch).copy_from_slice(&cleaned);
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

    fn noise(amp: f32, n: usize, seed0: u32) -> Vec<f32> {
        let mut s = seed0;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (((s >> 9) as f32 / (1u32 << 23) as f32) - 0.5) * 2.0 * amp
            })
            .collect()
    }

    /// A clean harmonic vocal stem (tonal, low flatness).
    fn clean_vocal(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                0.5 * (2.0 * PI * 180.0 * t).sin()
                    + 0.3 * (2.0 * PI * 360.0 * t).sin()
                    + 0.18 * (2.0 * PI * 540.0 * t).sin()
            })
            .collect()
    }

    /// A mix-like input: vocal plus dense broadband instrumental content.
    fn mix_like(n: usize) -> Vec<f32> {
        let v = clean_vocal(n);
        let bed = noise(0.5, n, 0xABCD);
        v.iter().zip(&bed).map(|(a, b)| a + b).collect()
    }

    #[test]
    fn clean_stem_passes_silently() {
        let d = DeBleed::default();
        let v = d.assess(&buf(clean_vocal(48_000)));
        assert_eq!(v, BleedVerdict::Clean, "a tonal vocal must read as clean");
        assert!(v.message().is_none());
    }

    #[test]
    fn mix_is_blocked_with_yesstems_message() {
        let d = DeBleed::default();
        let v = d.assess(&buf(mix_like(48_000)));
        assert_eq!(v, BleedVerdict::Block, "a broadband mix must be blocked");
        assert!(v.message().unwrap().to_lowercase().contains("yesstems"));
    }

    #[test]
    fn borderline_warns() {
        // Light broadband bleed over the vocal → between the thresholds.
        let v = clean_vocal(48_000);
        let bed = noise(0.12, 48_000, 0x1357);
        let borderline: Vec<f32> = v.iter().zip(&bed).map(|(a, b)| a + b).collect();
        let verdict = DeBleed::default().assess(&buf(borderline));
        assert_eq!(verdict, BleedVerdict::Warn, "light bleed should warn");
        assert!(verdict.message().is_some());
    }

    #[test]
    fn thresholds_are_tunable() {
        let mix = buf(mix_like(48_000));
        let mut d = DeBleed::default();
        // Raise the block threshold above any score → no longer blocked.
        d.set_thresholds(0.95, 0.99);
        assert_ne!(d.assess(&mix), BleedVerdict::Block);
        // Lower it to zero → even a clean stem blocks.
        d.set_thresholds(0.0, 0.0);
        assert_eq!(d.assess(&buf(clean_vocal(48_000))), BleedVerdict::Block);
    }

    #[test]
    fn native_take_is_off_imported_is_on() {
        assert!(DeBleed::for_source(true).enabled());
        assert!(!DeBleed::for_source(false).enabled());
    }

    #[test]
    fn cleans_residual_on_a_clean_stem() {
        // Clean vocal with low-level broadband residual → de-bleed reduces the
        // residual (measured in a non-harmonic band) while staying Clean.
        let n = 48_000;
        let v = clean_vocal(n);
        let bed = noise(0.04, n, 0x2468);
        let stem: Vec<f32> = v.iter().zip(&bed).map(|(a, b)| a + b).collect();

        let mut d = DeBleed::default();
        // Keep it classed Clean so it actually cleans.
        d.set_thresholds(0.5, 0.9);
        let mut b = buf(stem.clone());
        d.process(&mut b);

        let band = |x: &[f32]| {
            10.0 * crate::dsp::goertzel_power(&x[2048..n - 2048], SR as f32, 5000.0)
                .max(1e-12)
                .log10()
        };
        assert!(
            band(b.channel(0)) < band(&stem) - 3.0,
            "residual leakage should be reduced"
        );
    }

    #[test]
    fn bypass_is_exact_identity() {
        let mut d = DeBleed::default();
        d.set_enabled(false);
        let x = clean_vocal(10_000);
        let mut b = buf(x.clone());
        d.process(&mut b);
        for (a, c) in b.channel(0).iter().zip(&x) {
            assert!((a - c).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }
}
