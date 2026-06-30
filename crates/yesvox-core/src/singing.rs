//! Mechanical singing-quality metrics (plan §6.2) — the objective, code-only
//! replacement for the blind-listening release gate.
//!
//! The §6 gate originally required human ears on isolated singing clips. Instead,
//! YesVox validates sung-vocal **character preservation** with metrics that need
//! no human and no curated reference set: generate a synthetic singing fixture
//! with *known* ground truth (a vibrato note, a sustained tail, a breath), run it
//! through the singing chain, and assert the chain preserved its character within
//! the §6.2 tolerances. These are preservation metrics — they compare the
//! processed output against the input — so they are reproducible and CI-friendly.
//!
//! This module is the metric library; [`crate::singing_gate`] wires it into the
//! synthetic-fixture gate.

/// A frame-wise F0 deviation summary between a reference and a processed signal,
/// in cents (1200·log2(f_proc/f_ref)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CentsDeviation {
    /// Median absolute deviation over commonly-voiced frames, cents.
    pub median: f32,
    /// 95th-percentile absolute deviation, cents.
    pub p95: f32,
    /// How many frames were voiced in both signals (the comparison support).
    pub voiced_frames: usize,
}

/// Pitch-tracker frame geometry and search range.
const HOP_S: f32 = 0.010; // 10 ms
const WIN_S: f32 = 0.040; // 40 ms (≥2 periods at the low end)
const F_MIN: f32 = 70.0;
const F_MAX: f32 = 1000.0;
/// Minimum autocorrelation clarity (peak / energy) for a frame to count voiced.
const VOICED_CLARITY: f32 = 0.55;

/// Per-frame fundamental frequency (Hz), or `None` for unvoiced/low-clarity
/// frames. Frames are `HOP_S`-spaced; each is autocorrelated over `WIN_S` with
/// parabolic interpolation of the peak lag for sub-sample (sub-cent) precision.
pub fn pitch_contour(signal: &[f32], sr: f32) -> Vec<Option<f32>> {
    let hop = ((HOP_S * sr) as usize).max(1);
    let win = ((WIN_S * sr) as usize).max(hop * 2);
    let lag_lo = (sr / F_MAX) as usize;
    let lag_hi = (sr / F_MIN) as usize;
    let mut out = Vec::new();
    let mut start = 0;
    while start + win <= signal.len() {
        out.push(frame_pitch(&signal[start..start + win], sr, lag_lo, lag_hi));
        start += hop;
    }
    out
}

/// Interpolated F0 (Hz) of one frame, or `None` if not clearly voiced.
fn frame_pitch(seg: &[f32], sr: f32, lag_lo: usize, lag_hi: usize) -> Option<f32> {
    let energy: f32 = seg.iter().map(|x| x * x).sum();
    if energy <= 1.0e-9 || lag_hi <= lag_lo || lag_hi + 1 >= seg.len() {
        return None;
    }
    let autocorr = |lag: usize| -> f32 {
        seg.iter()
            .skip(lag)
            .zip(seg.iter())
            .map(|(a, b)| a * b)
            .sum::<f32>()
    };
    let mut best_lag = lag_lo;
    let mut best = f32::MIN;
    for lag in lag_lo..=lag_hi {
        let r = autocorr(lag);
        if r > best {
            best = r;
            best_lag = lag;
        }
    }
    let clarity = (best / energy).clamp(0.0, 1.0);
    if clarity < VOICED_CLARITY || best_lag <= lag_lo || best_lag >= lag_hi {
        return None;
    }
    // Parabolic interpolation around the integer peak for sub-sample precision.
    let r0 = autocorr(best_lag - 1);
    let r1 = best;
    let r2 = autocorr(best_lag + 1);
    let denom = r0 - 2.0 * r1 + r2;
    let delta = if denom.abs() > 1.0e-12 {
        0.5 * (r0 - r2) / denom
    } else {
        0.0
    };
    let lag = best_lag as f32 + delta.clamp(-1.0, 1.0);
    Some(sr / lag)
}

/// Interval in cents from `f_ref` to `f_proc`.
pub fn cents(f_ref: f32, f_proc: f32) -> f32 {
    1200.0 * (f_proc / f_ref).log2()
}

/// F0 deviation (cents) between a reference and a processed signal over the
/// frames both call voiced. Pass threshold (plan §6.2): median ≤ 3 cents, p95 ≤ 5.
pub fn f0_cents_deviation(reference: &[f32], processed: &[f32], sr: f32) -> CentsDeviation {
    let a = pitch_contour(reference, sr);
    let b = pitch_contour(processed, sr);
    let mut devs: Vec<f32> = a
        .iter()
        .zip(&b)
        .filter_map(|(ra, rb)| match (ra, rb) {
            (Some(fa), Some(fb)) => Some(cents(*fa, *fb).abs()),
            _ => None,
        })
        .collect();
    if devs.is_empty() {
        return CentsDeviation {
            median: 0.0,
            p95: 0.0,
            voiced_frames: 0,
        };
    }
    devs.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let pct = |p: f32| devs[((devs.len() as f32 * p) as usize).min(devs.len() - 1)];
    CentsDeviation {
        median: pct(0.50),
        p95: pct(0.95),
        voiced_frames: devs.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;

    /// A steady sung vowel: F0 plus a few harmonics (a glottal-ish source).
    fn vowel(f0: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                0.5 * (2.0 * PI * f0 * t).sin()
                    + 0.3 * (2.0 * PI * 2.0 * f0 * t).sin()
                    + 0.18 * (2.0 * PI * 3.0 * f0 * t).sin()
            })
            .collect()
    }

    #[test]
    fn pitch_tracks_a_steady_tone() {
        let x = vowel(220.0, 48_000);
        let contour = pitch_contour(&x, SR);
        let voiced: Vec<f32> = contour.iter().flatten().copied().collect();
        assert!(!voiced.is_empty(), "a sung vowel should be voiced");
        let mean = voiced.iter().sum::<f32>() / voiced.len() as f32;
        assert!(
            (mean - 220.0).abs() < 2.0,
            "tracked pitch {mean} should be ~220 Hz"
        );
    }

    #[test]
    fn cents_deviation_is_near_zero_for_identical_signals() {
        let x = vowel(196.0, 48_000);
        let d = f0_cents_deviation(&x, &x, SR);
        assert!(d.voiced_frames > 10, "should have voiced support");
        assert!(
            d.median < 0.5,
            "identical signals: ~0 cents, got {}",
            d.median
        );
        assert!(d.p95 < 1.0, "identical signals p95 {}", d.p95);
    }

    #[test]
    fn cents_deviation_detects_a_pitch_shift() {
        let reference = vowel(196.0, 48_000);
        // 20 cents sharp.
        let shifted = 196.0 * 2.0_f32.powf(20.0 / 1200.0);
        let processed = vowel(shifted, 48_000);
        let d = f0_cents_deviation(&reference, &processed, SR);
        assert!(
            (d.median - 20.0).abs() < 3.0,
            "should measure ~20 cents, got {}",
            d.median
        );
    }
}
