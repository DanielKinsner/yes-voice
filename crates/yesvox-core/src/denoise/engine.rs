//! The denoiser engine seam (plan §1.6, §2.1).
//!
//! v1's responsive/Natural tier denoiser plugs in behind the [`Denoiser`]
//! trait. The trait is the extensibility seam the plan calls for: the neural
//! DeepFilterNet3 engine (whose ONNX export + license audit are the separate
//! #9 spike / §7.5 work) drops in here without touching the worker pool,
//! streaming, or preflight built on top. The shipped v1 engine is
//! [`SpectralDenoiser`] — a deterministic STFT spectral-gating denoiser that is
//! fully offline, preserves voiced energy, and is the reference the streaming
//! path null-tests against.

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// A whole-chunk denoiser. Implementations must be **deterministic** (same input
/// → same output) and **offline** (no I/O) so the streaming path can null-test
/// chunked output against a single-shot reference.
pub trait Denoiser: Send + Sync {
    /// Denoise a contiguous block, returning a same-length result.
    fn denoise(&self, input: &[f32], sample_rate: f32) -> Vec<f32>;
}

/// Deterministic STFT spectral-gating denoiser (the v1 CPU engine).
#[derive(Clone)]
pub struct SpectralDenoiser {
    frame: usize,
    hop: usize,
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    /// Over-subtraction factor on the estimated noise floor.
    over_sub: f32,
    /// Spectral floor (minimum gain) — keeps character, avoids musical noise.
    floor: f32,
}

impl Default for SpectralDenoiser {
    fn default() -> Self {
        SpectralDenoiser::new(1024, 256)
    }
}

impl SpectralDenoiser {
    /// New denoiser with the given STFT `frame` size (power of two) and `hop`.
    pub fn new(frame: usize, hop: usize) -> Self {
        let window: Vec<f32> = (0..frame)
            .map(|n| {
                let w = 2.0 * std::f32::consts::PI * n as f32 / frame as f32;
                0.5 - 0.5 * w.cos() // Hann
            })
            .collect();
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(frame);
        let ifft = planner.plan_fft_inverse(frame);
        SpectralDenoiser {
            frame,
            hop,
            window,
            fft,
            ifft,
            over_sub: 1.5,
            floor: 0.12,
        }
    }
}

impl Denoiser for SpectralDenoiser {
    fn denoise(&self, input: &[f32], _sample_rate: f32) -> Vec<f32> {
        let n = input.len();
        if n < self.frame {
            return input.to_vec();
        }
        let frame = self.frame;
        let hop = self.hop;
        let nbins = frame / 2 + 1;

        // --- Analysis: windowed STFT magnitudes + complex frames -----------
        let mut frames: Vec<Vec<Complex32>> = Vec::new();
        let mut starts: Vec<usize> = Vec::new();
        let mut pos = 0;
        while pos + frame <= n {
            let mut buf: Vec<Complex32> = (0..frame)
                .map(|i| Complex32::new(input[pos + i] * self.window[i], 0.0))
                .collect();
            self.fft.process(&mut buf);
            frames.push(buf);
            starts.push(pos);
            pos += hop;
        }
        if frames.is_empty() {
            return input.to_vec();
        }

        // --- Per-bin noise estimate via a broadband floor -----------------
        // Average power per bin, then take the median across bins as the
        // broadband noise level: most bins carry only noise, while the few
        // tonal/voiced bins sit far above it. Capping each bin's noise near
        // that floor preserves stationary tonal energy (which minimum
        // statistics would mistake for noise) while still removing the floor.
        let inv_frames = 1.0 / frames.len() as f32;
        let mut avg = vec![0.0f32; nbins];
        for f in &frames {
            for (b, a) in avg.iter_mut().enumerate().take(nbins) {
                *a += f[b].norm_sqr() * inv_frames;
            }
        }
        let mut sorted = avg.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[nbins / 2];
        let noise_cap = 2.0 * median;
        let noise: Vec<f32> = avg
            .iter()
            .map(|&a| self.over_sub * a.min(noise_cap))
            .collect();

        // --- Wiener-style gain + synthesis --------------------------------
        let mut out = vec![0.0f32; n];
        let mut norm = vec![0.0f32; n];
        let mut scratch: Vec<Complex32> = vec![Complex32::new(0.0, 0.0); frame];
        for (f, &start) in frames.iter().zip(&starts) {
            scratch.copy_from_slice(f);
            // Apply the Wiener gain to the unique half...
            for (b, sb) in scratch.iter_mut().enumerate().take(nbins) {
                let p = sb.norm_sqr();
                let gain = if p > 0.0 {
                    (1.0 - noise[b] / p).max(self.floor)
                } else {
                    self.floor
                };
                *sb *= gain;
            }
            // ...then restore conjugate symmetry so the inverse FFT is real.
            for b in 1..nbins - 1 {
                scratch[frame - b] = scratch[b].conj();
            }
            self.ifft.process(&mut scratch);
            let scale = 1.0 / frame as f32;
            for i in 0..frame {
                let s = scratch[i].re * scale * self.window[i];
                out[start + i] += s;
                norm[start + i] += self.window[i] * self.window[i];
            }
        }

        // Normalize the overlap-add; outside covered range, keep the input.
        for i in 0..n {
            if norm[i] > 1.0e-6 {
                out[i] /= norm[i];
            } else {
                out[i] = input[i];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;

    fn tone(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / SR).sin())
            .collect()
    }

    /// Deterministic pseudo-noise in [-amp, amp].
    fn noise(amp: f32, n: usize) -> Vec<f32> {
        let mut seed = 0x9E37_79B9u32;
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 2.0 * amp
            })
            .collect()
    }

    #[test]
    fn gain_one_path_reconstructs_input() {
        // With no noise to remove, the STFT round-trip should reproduce the
        // input closely (overlap-add reconstruction).
        let clean = tone(300.0, 0.4, 8192);
        let den = SpectralDenoiser::default();
        let out = den.denoise(&clean, SR);
        // Compare on the fully-overlapped interior.
        let a = &out[1024..7168];
        let b = &clean[1024..7168];
        let err: f32 = a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f32>() / a.len() as f32;
        assert!(err < 1.0e-3, "reconstruction error too high: {err}");
    }

    #[test]
    fn reduces_noise_while_keeping_the_tone() {
        let n = 48_000;
        let clean = tone(300.0, 0.5, n);
        let noisy: Vec<f32> = clean
            .iter()
            .zip(noise(0.15, n))
            .map(|(c, e)| c + e)
            .collect();
        let den = SpectralDenoiser::default();
        let out = den.denoise(&noisy, SR);

        // Tone (300 Hz) preserved within ~2 dB.
        let tone_db = |x: &[f32]| {
            10.0 * crate::dsp::goertzel_power(&x[2048..n - 2048], SR, 300.0)
                .max(1e-12)
                .log10()
        };
        assert!(
            (tone_db(&out) - tone_db(&clean)).abs() < 2.0,
            "voice tone must survive"
        );

        // Noise (energy at an empty band, 9 kHz) reduced.
        let band_db = |x: &[f32]| {
            10.0 * crate::dsp::goertzel_power(&x[2048..n - 2048], SR, 9000.0)
                .max(1e-12)
                .log10()
        };
        assert!(
            band_db(&out) < band_db(&noisy) - 4.0,
            "broadband noise should drop ≥4 dB: {} -> {}",
            band_db(&noisy),
            band_db(&out)
        );
    }

    #[test]
    fn is_deterministic() {
        let x = tone(440.0, 0.3, 16_384);
        let den = SpectralDenoiser::default();
        assert_eq!(den.denoise(&x, SR), den.denoise(&x, SR));
    }
}
