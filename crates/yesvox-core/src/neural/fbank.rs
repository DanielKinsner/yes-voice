//! Kaldi-compatible log-mel filter-bank front-end for MossFormer2_SE_48K.
//!
//! Builds the exact 180-dim feature the ONNX net consumes: 60 log-mel
//! filter-banks plus their first and second time-deltas (`[fbank | Δ | ΔΔ]`).
//! It reproduces `torchaudio.compliance.kaldi.fbank` with the model's settings
//! (`frame_length 40 ms`, `frame_shift 8 ms`, `num_mel_bins 60`,
//! `window_type "hamming"`, 48 kHz) followed by `torchaudio.functional`'s
//! `compute_deltas` twice — the stack clearvoice's `compute_fbank` +
//! `decode_one_audio_mossformer2_se_48k` run host-side.
//!
//! Determinism: Kaldi's `dither` is forced to **0** here. The real pipeline
//! dithers at 1.0, but the net is robust to it and the offline engine must be
//! byte-reproducible, so the golden (and this port) pin the dither-free path.
//! Pinned to torchaudio by `matches_torchaudio_fbank_golden` (values from
//! `spikes/mossformer2-onnx/fbank_golden.py`).

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

const WIN_LEN: usize = 1920; // 40 ms @ 48 kHz (args.win_len)
const HOP: usize = 384; // 8 ms @ 48 kHz (args.win_inc)
const N_FFT: usize = 2048; // next_power_of_2(1920) — Kaldi round_to_power_of_two
const N_MELS: usize = 60; // args.num_mels
const SR: f32 = 48_000.0;
const LOW_FREQ: f32 = 20.0; // kaldi.fbank default
const PREEMPH: f32 = 0.97; // kaldi.fbank default

/// One triangular mel filter as a contiguous nonzero band over the FFT bins.
struct MelBin {
    offset: usize,
    weights: Vec<f32>,
}

/// The MossFormer2 fbank front-end: window, mel filter-bank, and FFT plan.
pub struct MossformerFbank {
    window: Vec<f32>,
    mel: Vec<MelBin>,
    fft: Arc<dyn Fft<f32>>,
}

impl Default for MossformerFbank {
    fn default() -> Self {
        MossformerFbank::new()
    }
}

/// Kaldi mel scale (the "slaney"-free 1127·ln form Kaldi/torchaudio use).
fn mel_scale(freq: f32) -> f32 {
    1127.0 * (1.0 + freq / 700.0).ln()
}

impl MossformerFbank {
    /// Build the front-end for the fixed MossFormer2_SE_48K configuration.
    pub fn new() -> Self {
        // torch.hamming_window(WIN_LEN, periodic=False, alpha=0.54, beta=0.46).
        let window: Vec<f32> = (0..WIN_LEN)
            .map(|n| {
                0.54 - 0.46 * (2.0 * std::f32::consts::PI * n as f32 / (WIN_LEN as f32 - 1.0)).cos()
            })
            .collect();

        let mut planner = FftPlanner::new();
        MossformerFbank {
            window,
            mel: build_mel_banks(),
            fft: planner.plan_fft_forward(N_FFT),
        }
    }

    /// Number of frames for `len` samples (Kaldi `snip_edges=true`).
    pub fn frames(&self, len: usize) -> usize {
        if len < WIN_LEN {
            0
        } else {
            (len - WIN_LEN) / HOP + 1
        }
    }

    /// Feature dimension per frame (60 fbank + 60 Δ + 60 ΔΔ).
    pub fn feat_dim(&self) -> usize {
        N_MELS * 3
    }

    /// Compute `frames × 180` features for a mono signal.
    pub fn features(&self, signal: &[f32]) -> Vec<Vec<f32>> {
        let m = self.frames(signal.len());
        if m == 0 {
            return Vec::new();
        }
        let fbank = self.log_mel(signal, m);
        let d1 = compute_deltas(&fbank);
        let d2 = compute_deltas(&d1);

        (0..m)
            .map(|f| {
                let mut row = Vec::with_capacity(N_MELS * 3);
                row.extend_from_slice(&fbank[f]);
                row.extend_from_slice(&d1[f]);
                row.extend_from_slice(&d2[f]);
                row
            })
            .collect()
    }

    /// The 60-bin log-mel filter-bank (no deltas), `m` frames.
    fn log_mel(&self, signal: &[f32], m: usize) -> Vec<Vec<f32>> {
        let eps = f32::EPSILON;
        let mut frame = vec![0.0f32; WIN_LEN];
        let mut buf = vec![Complex32::new(0.0, 0.0); N_FFT];
        let mut out = Vec::with_capacity(m);

        for f in 0..m {
            let start = f * HOP;
            frame.copy_from_slice(&signal[start..start + WIN_LEN]);

            // remove_dc_offset: subtract the per-frame mean.
            let mean = frame.iter().sum::<f32>() / WIN_LEN as f32;
            for s in frame.iter_mut() {
                *s -= mean;
            }

            // preemphasis: x[j] -= 0.97 * x[j-1] (j=0 uses replicate, x[-1]=x[0]).
            // Descend so each read of x[j-1] is still the pre-emphasis input.
            for j in (1..WIN_LEN).rev() {
                frame[j] -= PREEMPH * frame[j - 1];
            }
            frame[0] -= PREEMPH * frame[0];

            // window, then zero-pad into the N_FFT buffer.
            for (slot, (&x, &w)) in buf.iter_mut().zip(frame.iter().zip(&self.window)) {
                *slot = Complex32::new(x * w, 0.0);
            }
            for slot in buf.iter_mut().skip(WIN_LEN) {
                *slot = Complex32::new(0.0, 0.0);
            }
            self.fft.process(&mut buf);

            // power spectrum (|rfft|²) → mel sum → log(max(·, eps)).
            let power: Vec<f32> = buf[..N_FFT / 2 + 1].iter().map(|c| c.norm_sqr()).collect();
            let mel: Vec<f32> = self
                .mel
                .iter()
                .map(|b| {
                    let e: f32 = b
                        .weights
                        .iter()
                        .enumerate()
                        .map(|(k, &w)| w * power[b.offset + k])
                        .sum();
                    e.max(eps).ln()
                })
                .collect();
            out.push(mel);
        }
        out
    }
}

/// Build the 60 triangular mel filters over the `N_FFT/2` (=1024) FFT bins,
/// matching `torchaudio.compliance.kaldi.get_mel_banks` (no VTLN). The Nyquist
/// bin (index 1024) carries zero weight, so bands index `power[0..1024]`.
fn build_mel_banks() -> Vec<MelBin> {
    let num_fft_bins = N_FFT / 2; // 1024
    let nyquist = 0.5 * SR;
    let high_freq = nyquist; // kaldi high_freq=0.0 → += nyquist
    let fft_bin_width = SR / N_FFT as f32;
    let mel_low = mel_scale(LOW_FREQ);
    let mel_high = mel_scale(high_freq);
    let mel_delta = (mel_high - mel_low) / (N_MELS as f32 + 1.0);

    // Mel value of each FFT bin centre, precomputed once.
    let bin_mel: Vec<f32> = (0..num_fft_bins)
        .map(|j| mel_scale(fft_bin_width * j as f32))
        .collect();

    (0..N_MELS)
        .map(|c| {
            let left = mel_low + c as f32 * mel_delta;
            let center = mel_low + (c as f32 + 1.0) * mel_delta;
            let right = mel_low + (c as f32 + 2.0) * mel_delta;
            let full: Vec<f32> = bin_mel
                .iter()
                .map(|&mj| {
                    let up = (mj - left) / (center - left);
                    let down = (right - mj) / (right - center);
                    up.min(down).max(0.0)
                })
                .collect();
            // Trim to the contiguous nonzero band.
            let offset = full.iter().position(|&w| w > 0.0).unwrap_or(0);
            let end = full.iter().rposition(|&w| w > 0.0).map_or(0, |e| e + 1);
            MelBin {
                offset,
                weights: full[offset..end.max(offset)].to_vec(),
            }
        })
        .collect()
}

/// `torchaudio.functional.compute_deltas` (win_length=5, replicate pad): the
/// 5-tap linear regression Δ over the frame (time) axis, denominator 10.
fn compute_deltas(seq: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let m = seq.len();
    let cols = seq.first().map_or(0, |r| r.len());
    const DENOM: f32 = 10.0; // n(n+1)(2n+1)/3 with n=2
    (0..m)
        .map(|t| {
            (0..cols)
                .map(|c| {
                    let mut acc = 0.0f32;
                    for k in -2i32..=2 {
                        let idx = (t as i32 + k).clamp(0, m as i32 - 1) as usize;
                        acc += k as f32 * seq[idx][c];
                    }
                    acc / DENOM
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn speech_like(n: usize) -> Vec<f32> {
        (0..n)
            .map(|k| {
                let t = k as f32;
                0.4 * (2.0 * PI * 140.0 * t / SR).sin()
                    + 0.2 * (2.0 * PI * 700.0 * t / SR).sin()
                    + 0.15 * (2.0 * PI * 1700.0 * t / SR).sin()
            })
            .collect()
    }

    #[test]
    fn frames_and_dims_match_config() {
        let fb = MossformerFbank::new();
        assert_eq!(fb.feat_dim(), 180);
        // 1920 + 6*384 = 4224 → 7 frames.
        assert_eq!(fb.frames(WIN_LEN + 6 * HOP), 7);
        assert_eq!(fb.frames(100), 0);
    }

    #[test]
    fn mel_banks_are_triangular_and_bounded() {
        let mel = build_mel_banks();
        assert_eq!(mel.len(), 60);
        for b in &mel {
            // Every weight in [0, 1]; the band is contiguous and within 1024 bins.
            assert!(b.offset + b.weights.len() <= N_FFT / 2);
            assert!(b.weights.iter().all(|&w| (0.0..=1.0001).contains(&w)));
            assert!(b.weights.iter().any(|&w| w > 0.0));
        }
    }

    #[test]
    fn matches_torchaudio_fbank_golden() {
        // Goldens from spikes/mossformer2-onnx/fbank_golden.py (dither=0) on a
        // deterministic 3-formant signal. Pins the Rust front-end byte-faithful
        // to torchaudio.compliance.kaldi.fbank + compute_deltas×2.
        let fb = MossformerFbank::new();
        let x = speech_like(WIN_LEN + 6 * HOP);
        let feats = fb.features(&x);
        assert_eq!(feats.len(), 7);
        assert_eq!(feats[0].len(), 180);

        let approx = |a: f32, b: f32, tol: f32, what: &str| {
            assert!((a - b).abs() < tol, "{what}: got {a}, want {b}");
        };
        // Point samples (frame, col). Literals trimmed to f32-clean digits
        // well inside the 2e-3 tolerance.
        approx(feats[0][0], -3.9634, 2e-3, "f0 c0");
        approx(feats[0][30], -6.1833, 2e-3, "f0 c30");
        approx(feats[0][59], -7.7328, 2e-3, "f0 c59");
        approx(feats[3][0], -3.9118, 2e-3, "f3 c0");
        approx(feats[3][30], -7.7857, 2e-3, "f3 c30");
        approx(feats[3][70], 0.0, 2e-3, "f3 c70 (Δ)");
        approx(feats[3][130], 0.0, 2e-3, "f3 c130 (ΔΔ)");
        approx(feats[6][59], -8.366, 2e-3, "f6 c59");
        approx(feats[6][119], -0.1624, 2e-3, "f6 c119 (Δ)");
        approx(feats[6][179], -0.2266, 2e-3, "f6 c179 (ΔΔ)");

        // Block checksums (whole-array pins).
        let sum = |lo: usize, hi: usize| -> f32 {
            feats.iter().map(|r| r[lo..hi].iter().sum::<f32>()).sum()
        };
        approx(sum(0, 60), -2326.54, 0.5, "Σ fbank");
        approx(sum(60, 120), -1.9625, 0.05, "Σ Δ");
        approx(sum(120, 180), 14.8883, 0.05, "Σ ΔΔ");
        approx(sum(0, 180), -2313.61, 0.5, "Σ all");
    }

    #[test]
    fn deltas_of_constant_are_zero() {
        // Δ and ΔΔ of a time-constant fbank must vanish (replicate-pad edges).
        let constant: Vec<Vec<f32>> = (0..5).map(|_| vec![1.0, 2.0, 3.0]).collect();
        let d = compute_deltas(&constant);
        assert!(d.iter().flatten().all(|&v| v.abs() < 1e-7));
    }
}
