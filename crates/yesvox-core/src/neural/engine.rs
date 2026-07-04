//! The MossFormer2_SE_48K host-side enhancement chain (plan §2.2, slice #10).
//!
//! Ties the front-end ([`MossformerFbank`]) and back-end ([`MossformerStft`])
//! around a [`MaskModel`] — the (later, `ort`-backed) neural net — reproducing
//! clearvoice's `decode_one_audio_mossformer2_se_48k` exactly:
//!
//! 1. scale the 48 kHz mono signal by `MAX_WAV_VALUE` (32768),
//! 2. compute the 180-dim Kaldi fbank+Δ+ΔΔ features,
//! 3. run the net → a 961-bin **real** ratio mask per frame,
//! 4. STFT the (scaled) signal, multiply each complex bin by its real gain
//!    (phase-preserving — the mask scales magnitude only),
//! 5. iSTFT back to the time domain and un-scale by `1/32768`.
//!
//! The scale/un-scale is a linear no-op for the reconstruction math (it only
//! sets the level the *features* — hence the net — see), so the chain is exact
//! for any ratio mask. The whole host chain is pinned to clearvoice's own
//! `stft`/`istft` by `matches_clearvoice_decode_golden`, which independently
//! validates the iSTFT (including partial-overlap edges) against `torch.istft`.
//!
//! The mask source is abstracted behind [`MaskModel`] so the entire chain is
//! golden-tested in pure Rust with deterministic stand-in masks; only the real
//! ONNX inference sits behind the off-by-default `neural` feature.

use super::{from_48k, to_48k, MossformerFbank, MossformerStft};
use crate::denoise::Denoiser;

/// int16 full-scale — clearvoice scales the waveform by this before feature
/// extraction and the STFT, then divides the reconstruction back down.
const MAX_WAV_VALUE: f32 = 32768.0;

/// The neural mask predictor: 180-dim features per frame → a 961-bin real
/// ratio mask per frame (`gains[t][bin] ∈ [0, 1]`, applied to magnitude).
///
/// Implementors must be deterministic and offline so the engine inherits the
/// [`Denoiser`](crate::denoise::Denoiser) contract. The production
/// implementation (ONNX Runtime) lands behind the `neural` feature; tests use
/// closed-form stand-ins.
pub trait MaskModel: Send + Sync {
    /// Predict gains for `features` (`frames × 180`) → `frames × 961`.
    fn run(&self, features: &[Vec<f32>]) -> Vec<Vec<f32>>;
}

/// The host-side enhancement chain around a [`MaskModel`]. Operates at the
/// model's native 48 kHz; resampling to/from other rates is the caller's job
/// (the resample island).
pub struct MossformerEngine {
    fbank: MossformerFbank,
    stft: MossformerStft,
    model: Box<dyn MaskModel>,
}

impl MossformerEngine {
    /// Build the engine around a mask predictor.
    pub fn new(model: Box<dyn MaskModel>) -> Self {
        MossformerEngine {
            fbank: MossformerFbank::new(),
            stft: MossformerStft::default(),
            model,
        }
    }

    /// Enhance a 48 kHz mono signal, returning a same-length result. Signals
    /// shorter than one analysis frame pass through unchanged.
    pub fn enhance(&self, signal: &[f32]) -> Vec<f32> {
        let n = signal.len();
        if self.stft.frames(n) == 0 {
            return signal.to_vec();
        }
        // Scale into the int16 domain the net was trained on.
        let scaled: Vec<f32> = signal.iter().map(|&x| x * MAX_WAV_VALUE).collect();

        let features = self.fbank.features(&scaled);
        let gains = self.model.run(&features);

        let mut spectrum = self.stft.forward(&scaled);
        apply_gains(&mut spectrum, &gains);
        let recon = self.stft.inverse(&spectrum, n);

        recon.iter().map(|&x| x / MAX_WAV_VALUE).collect()
    }

    /// Enhance at an arbitrary `sample_rate`, returning a same-length result.
    /// Non-48 kHz input is taken through the resample island for the neural
    /// pass and carried back to the source rate fitted to the input length.
    pub fn enhance_at(&self, signal: &[f32], sample_rate: u32) -> Vec<f32> {
        if sample_rate == 48_000 {
            return self.enhance(signal);
        }
        let up = to_48k(signal, sample_rate);
        let processed = self.enhance(&up);
        from_48k(&processed, sample_rate, signal.len())
    }
}

impl Denoiser for MossformerEngine {
    fn denoise(&self, input: &[f32], sample_rate: f32) -> Vec<f32> {
        self.enhance_at(input, sample_rate.round() as u32)
    }
}

/// Multiply each complex STFT bin by its real gain (magnitude-only mask;
/// phase preserved). Frames/bins beyond the shorter operand are left untouched.
fn apply_gains(spectrum: &mut [Vec<rustfft::num_complex::Complex32>], gains: &[Vec<f32>]) {
    for (frame, gain_row) in spectrum.iter_mut().zip(gains) {
        for (bin, &g) in frame.iter_mut().zip(gain_row) {
            *bin *= g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;
    const BINS: usize = 961;

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

    /// Returns a fixed gain for every frame/bin, ignoring features.
    struct ConstMask(f32);
    impl MaskModel for ConstMask {
        fn run(&self, features: &[Vec<f32>]) -> Vec<Vec<f32>> {
            vec![vec![self.0; BINS]; features.len()]
        }
    }

    /// The synthetic frequency ramp used by the clearvoice decode golden:
    /// `gains[t][b] = b / 960`, independent of frame.
    struct RampMask;
    impl MaskModel for RampMask {
        fn run(&self, features: &[Vec<f32>]) -> Vec<Vec<f32>> {
            let row: Vec<f32> = (0..BINS).map(|b| b as f32 / (BINS as f32 - 1.0)).collect();
            vec![row; features.len()]
        }
    }

    #[test]
    fn identity_mask_reconstructs_interior() {
        // Gain 1.0 everywhere ⇒ iSTFT∘STFT ≈ identity on the overlapped interior.
        let x = speech_like(1920 * 8);
        let eng = MossformerEngine::new(Box::new(ConstMask(1.0)));
        let y = eng.enhance(&x);
        assert_eq!(y.len(), x.len());
        let (lo, hi) = (1920, x.len() - 1920);
        let err: f32 = x[lo..hi]
            .iter()
            .zip(&y[lo..hi])
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            / (hi - lo) as f32;
        assert!(err < 1e-6, "identity reconstruction error {err}");
    }

    #[test]
    fn zero_mask_silences() {
        let x = speech_like(1920 * 6);
        let eng = MossformerEngine::new(Box::new(ConstMask(0.0)));
        let y = eng.enhance(&x);
        let energy: f32 = y[1920..x.len() - 1920].iter().map(|v| v * v).sum();
        assert!(energy < 1e-9, "zero mask should silence, got {energy}");
    }

    #[test]
    fn short_signal_passes_through() {
        let x = speech_like(100); // < one 1920-sample frame
        let eng = MossformerEngine::new(Box::new(ConstMask(0.0)));
        assert_eq!(eng.enhance(&x), x);
    }

    #[test]
    fn enhance_at_non_48k_is_same_length_and_preserves_tone() {
        // Identity mask + 44.1 kHz input: the Denoiser path resamples to 48k,
        // runs an iSTFT∘STFT round-trip, and resamples back — same length out,
        // and a passband tone survives the island round-trip.
        let sr = 44_100.0;
        let x = tone_at(1000.0, sr, 44_100);
        let eng = MossformerEngine::new(Box::new(ConstMask(1.0)));
        let y = eng.denoise(&x, sr);
        assert_eq!(y.len(), x.len(), "Denoiser must preserve length");
        let (lo, hi) = (3000, x.len() - 3000);
        let p_in = crate::dsp::goertzel_power(&x[lo..hi], sr, 1000.0);
        let p_out = crate::dsp::goertzel_power(&y[lo..hi], sr, 1000.0);
        let d_db = 10.0 * (p_out.max(1e-12) / p_in.max(1e-12)).log10();
        assert!(
            d_db.abs() < 0.8,
            "tone drifted {d_db} dB through the island"
        );
    }

    fn tone_at(freq: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| 0.4 * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect()
    }

    #[test]
    fn matches_clearvoice_decode_golden() {
        // Golden from spikes/mossformer2-onnx/decode_chain_golden.py: the exact
        // clearvoice STFT → ramp mask → iSTFT path. Pins our host chain (and so
        // torch.istft on a *masked* spectrum, edges included) byte-faithful.
        let n = 1920 + 11 * 384; // 12 frames, matches the golden
        let x = speech_like(n);
        let eng = MossformerEngine::new(Box::new(RampMask));
        let y = eng.enhance(&x);
        assert_eq!(y.len(), 6144);

        // Reference samples/energy/sum live in
        // tests/goldens/decode_chain_clearvoice.golden.
        use crate::golden::{check, golden};
        let energy: f32 = y.iter().map(|v| v * v).sum();
        let sum: f32 = y.iter().sum();
        check(
            "decode_chain_clearvoice",
            &[
                golden("out_0", y[0], 1.5e-3),
                golden("out_1", y[1], 1.5e-3),
                golden("out_1000", y[1000], 1.5e-3),
                golden("out_3000", y[3000], 1.5e-3),
                golden("out_3072", y[3072], 1.5e-3),
                golden("out_5000", y[5000], 1.5e-3),
                golden("out_6143", y[6143], 1.5e-3),
                golden("energy", energy, 1.5e-3),
                golden("sum", sum, 1.5e-3),
            ],
        );
    }
}
