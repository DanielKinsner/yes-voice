//! STFT/iSTFT matching MossFormer2_SE_48K's host transform (plan §2.2).
//!
//! Mirrors the model's `stft`/`istft` exactly so a mask predicted in that
//! domain can be applied and inverted: `torch.stft(n_fft=1920, hop=384,
//! win_length=1920, window=hamming(symmetric), center=false, onesided=true)`.
//! The forward returns 961 complex bins per frame; the inverse is
//! window-weighted overlap-add with the same per-sample normalization
//! `torch.istft` uses (divide by Σ w²), so a round-trip with an all-ones mask
//! reconstructs the input on the fully-overlapped interior.

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// The MossFormer STFT parameters and FFT plans.
pub struct MossformerStft {
    n_fft: usize,
    hop: usize,
    win_len: usize,
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
}

impl Default for MossformerStft {
    fn default() -> Self {
        MossformerStft::new(1920, 384, 1920)
    }
}

impl MossformerStft {
    /// Build with explicit FFT size, hop, and window length (`win == n_fft` for
    /// the MossFormer config).
    pub fn new(n_fft: usize, hop: usize, win_len: usize) -> Self {
        // torch.hamming_window(win_len, periodic=False): symmetric.
        let window: Vec<f32> = (0..win_len)
            .map(|n| {
                0.54 - 0.46 * (2.0 * std::f32::consts::PI * n as f32 / (win_len as f32 - 1.0)).cos()
            })
            .collect();
        let mut planner = FftPlanner::new();
        MossformerStft {
            n_fft,
            hop,
            win_len,
            window,
            fft: planner.plan_fft_forward(n_fft),
            ifft: planner.plan_fft_inverse(n_fft),
        }
    }

    /// Number of one-sided frequency bins (`n_fft/2 + 1`, = 961 by default).
    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Number of frames for a signal of `len` samples (center=false framing).
    pub fn frames(&self, len: usize) -> usize {
        if len < self.win_len {
            0
        } else {
            (len - self.win_len) / self.hop + 1
        }
    }

    /// Forward STFT → `frames × bins` one-sided complex spectrum.
    pub fn forward(&self, signal: &[f32]) -> Vec<Vec<Complex32>> {
        let nbins = self.bins();
        let nframes = self.frames(signal.len());
        let mut out = Vec::with_capacity(nframes);
        let mut buf = vec![Complex32::new(0.0, 0.0); self.n_fft];
        for f in 0..nframes {
            let start = f * self.hop;
            for (i, slot) in buf.iter_mut().enumerate() {
                let s = if i < self.win_len {
                    signal[start + i] * self.window[i]
                } else {
                    0.0
                };
                *slot = Complex32::new(s, 0.0);
            }
            self.fft.process(&mut buf);
            out.push(buf[..nbins].to_vec());
        }
        out
    }

    /// Inverse STFT (window-weighted overlap-add with Σw² normalization) to a
    /// signal of `len` samples.
    pub fn inverse(&self, spectrum: &[Vec<Complex32>], len: usize) -> Vec<f32> {
        let nbins = self.bins();
        let mut out = vec![0.0f32; len];
        let mut norm = vec![0.0f32; len];
        let mut buf = vec![Complex32::new(0.0, 0.0); self.n_fft];
        for (f, frame) in spectrum.iter().enumerate() {
            let start = f * self.hop;
            // Rebuild the full conjugate-symmetric spectrum from the one-sided half.
            buf[..nbins].copy_from_slice(&frame[..nbins]);
            for b in 1..nbins - 1 {
                buf[self.n_fft - b] = frame[b].conj();
            }
            self.ifft.process(&mut buf);
            let scale = 1.0 / self.n_fft as f32;
            for (i, (&w, b)) in self.window.iter().zip(buf.iter()).enumerate() {
                let idx = start + i;
                if idx < len {
                    out[idx] += b.re * scale * w;
                    norm[idx] += w * w;
                }
            }
        }
        for (o, &nrm) in out.iter_mut().zip(norm.iter()) {
            if nrm > 1.0e-8 {
                *o /= nrm;
            }
        }
        out
    }

    /// Apply a complex ratio mask (same shape as the spectrum) in place.
    pub fn apply_mask(spectrum: &mut [Vec<Complex32>], mask: &[Vec<Complex32>]) {
        for (frame, m) in spectrum.iter_mut().zip(mask) {
            for (s, g) in frame.iter_mut().zip(m) {
                *s *= *g;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn tone(freq: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| 0.5 * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect()
    }

    #[test]
    fn bins_and_frames_match_config() {
        let s = MossformerStft::default();
        assert_eq!(s.bins(), 961);
        // 1920 + 5*384 = 3840 → 6 frames.
        assert_eq!(s.frames(1920 + 5 * 384), 6);
        assert_eq!(s.frames(1000), 0);
    }

    #[test]
    fn round_trip_reconstructs_interior() {
        // istft(stft(x)) ≈ x on the fully-overlapped interior (identity mask).
        let sr = 48_000.0;
        let n = 1920 * 8;
        let x = tone(220.0, sr, n);
        let s = MossformerStft::default();
        let spec = s.forward(&x);
        let y = s.inverse(&spec, n);
        // Skip the first/last window where the overlap isn't yet full.
        let lo = 1920;
        let hi = n - 1920;
        let err: f32 = x[lo..hi]
            .iter()
            .zip(&y[lo..hi])
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            / (hi - lo) as f32;
        assert!(err < 1.0e-6, "reconstruction error too high: {err}");
    }

    #[test]
    fn tone_lands_in_the_expected_bin() {
        let sr = 48_000.0;
        let freq = 1000.0;
        let x = tone(freq, sr, 1920 * 6);
        let s = MossformerStft::default();
        let spec = s.forward(&x);
        // Bin spacing = sr / n_fft; nearest bin to 1 kHz.
        let bin = (freq / (sr / 1920.0)).round() as usize;
        // Use a settled middle frame.
        let frame = &spec[3];
        let peak = frame
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm().partial_cmp(&b.1.norm()).unwrap())
            .unwrap()
            .0;
        assert_eq!(peak, bin, "tone should peak at bin {bin}, got {peak}");
    }

    #[test]
    fn matches_torch_stft_golden() {
        // Golden values from torch.stft on a deterministic 1 kHz+60 Hz signal
        // (n_fft 1920, hop 384, hamming symmetric, center=false, onesided),
        // generated by spikes/mossformer2-onnx (see PR notes). This pins that
        // the Rust transform is byte-faithful to the model's host STFT.
        let n = 1920 + 5 * 384;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32;
                0.5 * (2.0 * PI * 1000.0 * t / 48_000.0).sin()
                    + 0.3 * (2.0 * PI * 60.0 * t / 48_000.0).sin()
            })
            .collect();
        let s = MossformerStft::default();
        let spec = s.forward(&x);
        assert_eq!(spec.len(), 6);
        assert_eq!(spec[0].len(), 961);

        let peak = spec[2]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm().partial_cmp(&b.1.norm()).unwrap())
            .unwrap()
            .0;
        assert_eq!(peak, 40, "1 kHz peak bin");
        assert!(
            (spec[2][40].norm() - 258.898_38).abs() < 0.1,
            "{}",
            spec[2][40].norm()
        );
        assert!(
            (spec[3][20].norm() - 0.450_779).abs() < 2.0e-3,
            "{}",
            spec[3][20].norm()
        );
        assert!(
            (spec[1][100].norm() - 0.060_253).abs() < 2.0e-3,
            "{}",
            spec[1][100].norm()
        );

        let energy: f32 = spec.iter().flatten().map(|c| c.norm_sqr()).sum();
        assert!((energy - 746_797.75).abs() < 50.0, "energy {energy}");
    }

    #[test]
    fn identity_mask_is_transparent_but_zero_mask_silences() {
        let sr = 48_000.0;
        let n = 1920 * 6;
        let x = tone(330.0, sr, n);
        let s = MossformerStft::default();
        let mut spec = s.forward(&x);
        let zero: Vec<Vec<Complex32>> = spec
            .iter()
            .map(|f| vec![Complex32::new(0.0, 0.0); f.len()])
            .collect();
        MossformerStft::apply_mask(&mut spec, &zero);
        let y = s.inverse(&spec, n);
        let energy: f32 = y[1920..n - 1920].iter().map(|v| v * v).sum();
        assert!(energy < 1.0e-6, "zero mask should silence, got {energy}");
    }
}
