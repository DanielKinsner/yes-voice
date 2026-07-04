//! Shared deterministic-DSP primitives (plan §3).
//!
//! These are the building blocks the repair rack is assembled from — biquad
//! filters (RBJ cookbook designs) and a Goertzel single-bin power detector.
//! The *primitives in this module* are alloc-free in their hot path. That is
//! **not** a claim about the render as a whole (#32): v1's memory model is
//! **whole-buffer** — every stage processes the entire f32 buffer and several
//! make full-length copies, so peak memory scales with file length. Only the
//! denoiser streams in overlapping chunks; a truly alloc-free, whole-chain
//! streaming preview is post-v1. The enforcing RAM preflight
//! ([`crate::denoise::ChunkPolicy::preflight`]) bounds that whole-buffer peak
//! in the meantime.

use std::f32::consts::PI;

/// Flush a subnormal (denormal) magnitude to zero (#28). Recursive filter state
/// that decays toward silence drifts into the denormal range, where every
/// arithmetic op is ~100× slower; pinning tiny magnitudes to exact zero avoids
/// that stall. The threshold sits far above f32's smallest normal yet ~600 dB
/// below full scale, so it never touches an audible value.
#[inline]
pub(crate) fn flush_denormal(v: f32) -> f32 {
    if v.abs() < 1.0e-30 {
        0.0
    } else {
        v
    }
}

/// A transposed-Direct-Form-II biquad. Coefficients are stored already
/// normalized by `a0`.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// Identity (pass-through) biquad.
    pub fn identity() -> Self {
        Biquad {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Build directly from un-normalized coefficients.
    pub fn from_coeffs(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// RBJ notch at `freq` Hz with quality `q`.
    pub fn notch(sample_rate: f32, freq: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * freq / sample_rate;
        let (sin, cos) = (w0.sin(), w0.cos());
        let alpha = sin / (2.0 * q);
        Biquad::from_coeffs(1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// RBJ peaking EQ, `gain_db` cut/boost at `freq` with quality `q`.
    pub fn peaking(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sample_rate;
        let (sin, cos) = (w0.sin(), w0.cos());
        let alpha = sin / (2.0 * q);
        Biquad::from_coeffs(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        )
    }

    /// RBJ high-shelf, `gain_db` above `freq`.
    pub fn high_shelf(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sample_rate;
        let (sin, cos) = (w0.sin(), w0.cos());
        let alpha = sin / (2.0 * q);
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        Biquad::from_coeffs(
            a * ((a + 1.0) + (a - 1.0) * cos + two_sqrt_a_alpha),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - two_sqrt_a_alpha),
            (a + 1.0) - (a - 1.0) * cos + two_sqrt_a_alpha,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - two_sqrt_a_alpha,
        )
    }

    /// RBJ low-shelf, `gain_db` below `freq`.
    pub fn low_shelf(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sample_rate;
        let (sin, cos) = (w0.sin(), w0.cos());
        let alpha = sin / (2.0 * q);
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        Biquad::from_coeffs(
            a * ((a + 1.0) - (a - 1.0) * cos + two_sqrt_a_alpha),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - two_sqrt_a_alpha),
            (a + 1.0) + (a - 1.0) * cos + two_sqrt_a_alpha,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - two_sqrt_a_alpha,
        )
    }

    /// RBJ second-order high-pass at `freq` with quality `q`.
    pub fn high_pass(sample_rate: f32, freq: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * freq / sample_rate;
        let (sin, cos) = (w0.sin(), w0.cos());
        let alpha = sin / (2.0 * q);
        Biquad::from_coeffs(
            (1.0 + cos) / 2.0,
            -(1.0 + cos),
            (1.0 + cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// RBJ second-order low-pass at `freq` with quality `q`.
    pub fn low_pass(sample_rate: f32, freq: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * freq / sample_rate;
        let (sin, cos) = (w0.sin(), w0.cos());
        let alpha = sin / (2.0 * q);
        Biquad::from_coeffs(
            (1.0 - cos) / 2.0,
            1.0 - cos,
            (1.0 - cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// Process one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        // Flush denormal state so decaying tails don't stall the recursion (#28).
        self.z1 = flush_denormal(self.b1 * x - self.a1 * y + self.z2);
        self.z2 = flush_denormal(self.b2 * x - self.a2 * y);
        y
    }

    /// Clear the delay line.
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Replace this filter's coefficients with another's while keeping the
    /// current delay-line state — a click-free retune (plan §3.1 tracker).
    pub fn copy_coeffs_from(&mut self, other: &Biquad) {
        self.b0 = other.b0;
        self.b1 = other.b1;
        self.b2 = other.b2;
        self.a1 = other.a1;
        self.a2 = other.a2;
    }

    /// Magnitude response at `freq` Hz (for tests / analysis), in linear units.
    pub fn magnitude(&self, sample_rate: f32, freq: f32) -> f32 {
        // H(e^jw) = (b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)
        let w = 2.0 * PI * freq / sample_rate;
        let (c1, s1) = ((-w).cos(), (-w).sin());
        let (c2, s2) = ((-2.0 * w).cos(), (-2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = self.b1 * s1 + self.b2 * s2;
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = self.a1 * s1 + self.a2 * s2;
        let num = (num_re * num_re + num_im * num_im).sqrt();
        let den = (den_re * den_re + den_im * den_im).sqrt();
        num / den
    }
}

/// Goertzel single-frequency power: the squared magnitude of the DFT bin at
/// `freq`, normalized to mean power per sample. Cheap way to probe one tone
/// (used by the hum detector, plan §3.1).
pub fn goertzel_power(samples: &[f32], sample_rate: f32, freq: f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let n = samples.len() as f32;
    let w = 2.0 * PI * freq / sample_rate;
    let coeff = 2.0 * w.cos();
    let (mut s_prev, mut s_prev2) = (0.0f32, 0.0f32);
    for &x in samples {
        let s = x + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    let power = s_prev * s_prev + s_prev2 * s_prev2 - coeff * s_prev * s_prev2;
    // Normalize so a unit-amplitude sine at `freq` reads ~0.25 (its mean
    // power), comparable across window lengths.
    power / (n * n / 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process_buf(mut f: Biquad, xs: &[f32]) -> Vec<f32> {
        xs.iter().map(|&x| f.process(x)).collect()
    }

    #[test]
    fn flush_denormal_zeros_subnormals_and_keeps_normals() {
        assert_eq!(flush_denormal(1.0e-40), 0.0, "subnormal flushed");
        assert_eq!(flush_denormal(-1.0e-35), 0.0, "below threshold flushed");
        assert_eq!(flush_denormal(0.0), 0.0);
        assert_eq!(flush_denormal(0.5), 0.5, "normal kept");
        assert_eq!(flush_denormal(-1.0e-3), -1.0e-3, "audible-range kept");
    }

    #[test]
    fn biquad_impulse_then_silence_settles_to_exact_zero() {
        // A stable shelf fed an impulse then long silence: the recursive state
        // decays and, with the denormal flush, settles to *exact* zero and stays
        // finite — no denormal tail (#28).
        let mut f = Biquad::high_shelf(48_000.0, 6_000.0, 0.7, 6.0);
        f.process(1.0);
        let mut y = 1.0;
        for _ in 0..20_000 {
            y = f.process(0.0);
            assert!(y.is_finite(), "output stayed finite");
        }
        assert_eq!(y, 0.0, "state flushed to exact zero, no denormal tail");
    }

    fn sine(freq: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect()
    }

    fn rms(xs: &[f32]) -> f32 {
        (xs.iter().map(|x| x * x).sum::<f32>() / xs.len() as f32).sqrt()
    }

    #[test]
    fn notch_kills_target_keeps_far_tone() {
        let sr = 48_000.0;
        let notch = Biquad::notch(sr, 1000.0, 30.0);
        // Settle past the transient by skipping the first samples.
        let on = process_buf(notch, &sine(1000.0, sr, 8000))[4000..].to_vec();
        let off = process_buf(notch, &sine(200.0, sr, 8000))[4000..].to_vec();
        assert!(rms(&on) < 0.1, "notch should crush the target tone");
        assert!(rms(&off) > 0.7, "notch should pass a far tone");
    }

    #[test]
    fn magnitude_matches_processing_gain() {
        let sr = 48_000.0;
        let f = Biquad::peaking(sr, 4000.0, 1.0, -12.0);
        let m = f.magnitude(sr, 4000.0);
        let out = process_buf(f, &sine(4000.0, sr, 16000))[8000..].to_vec();
        let measured = rms(&out) / rms(&sine(4000.0, sr, 8000));
        assert!(
            (m - measured).abs() < 0.05,
            "analytic magnitude {m} vs measured {measured}"
        );
    }

    #[test]
    fn goertzel_finds_the_tone() {
        let sr = 48_000.0;
        let present = goertzel_power(&sine(60.0, sr, 9600), sr, 60.0);
        let absent = goertzel_power(&sine(60.0, sr, 9600), sr, 400.0);
        assert!(present > 0.2, "tone power should be ~0.25, got {present}");
        assert!(absent < 0.01, "off-bin power should be tiny, got {absent}");
    }
}
