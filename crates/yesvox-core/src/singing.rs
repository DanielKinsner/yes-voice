//! Mechanical singing-quality metrics (plan §6.2) — the objective, code-only
//! replacement for the former blind-listening release plan.
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

/// A vibrato estimate: oscillation rate (Hz) and depth (cents, peak amplitude).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vibrato {
    /// Vibrato rate, Hz (typical sung vibrato is ~4–7 Hz).
    pub rate_hz: f32,
    /// Vibrato depth, cents (peak excursion of the F0 contour).
    pub depth_cents: f32,
}

/// Frame rate of [`pitch_contour`] (frames per second).
const FRAME_RATE: f32 = 1.0 / HOP_S;

/// Estimate vibrato from a signal's F0 contour: detrend the voiced contour to
/// cents about its own median, take the peak amplitude as depth, and find the
/// dominant oscillation rate by autocorrelation over the 3–9 Hz band. Returns
/// `None` if there isn't enough voiced support.
pub fn vibrato(signal: &[f32], sr: f32) -> Option<Vibrato> {
    let voiced: Vec<f32> = pitch_contour(signal, sr).into_iter().flatten().collect();
    if voiced.len() < 24 {
        return None;
    }
    let mut sorted = voiced.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    // Contour in cents about the median, then remove any residual DC.
    let mut c: Vec<f32> = voiced.iter().map(|&f| cents(median, f)).collect();
    let mean = c.iter().sum::<f32>() / c.len() as f32;
    for v in &mut c {
        *v -= mean;
    }
    let rms = (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt();
    let depth_cents = rms * std::f32::consts::SQRT_2; // peak of a sinusoid

    let lag_lo = (FRAME_RATE / 9.0) as usize;
    let lag_hi = (FRAME_RATE / 3.0) as usize;
    let energy: f32 = c.iter().map(|v| v * v).sum();
    let mut best_lag = 0usize;
    let mut best = 0.0f32;
    if energy > 0.0 && lag_hi < c.len() {
        for lag in lag_lo..=lag_hi {
            let r: f32 = c.iter().skip(lag).zip(&c).map(|(a, b)| a * b).sum::<f32>() / energy;
            if r > best {
                best = r;
                best_lag = lag;
            }
        }
    }
    let rate_hz = if best_lag > 0 {
        FRAME_RATE / best_lag as f32
    } else {
        0.0
    };
    Some(Vibrato {
        rate_hz,
        depth_cents,
    })
}

/// Normalized autocorrelation peak (clarity, `[0,1]`) of one frame at the pitch
/// lag — the periodicity ratio HNR is derived from. Uses the lag-normalized
/// cross-correlation (each lag normalized by the energy of its own overlap), so
/// a clean periodic frame reads ≈1 instead of decaying with lag.
fn frame_clarity(seg: &[f32], lag_lo: usize, lag_hi: usize) -> Option<f32> {
    let energy: f32 = seg.iter().map(|x| x * x).sum();
    if energy <= 1.0e-9 || lag_hi <= lag_lo || lag_hi >= seg.len() {
        return None;
    }
    let n = seg.len();
    let mut best = 0.0f32;
    for lag in lag_lo..=lag_hi {
        let mut num = 0.0f32;
        let mut e0 = 0.0f32;
        let mut e1 = 0.0f32;
        for i in 0..(n - lag) {
            num += seg[i] * seg[i + lag];
            e0 += seg[i] * seg[i];
            e1 += seg[i + lag] * seg[i + lag];
        }
        let denom = (e0 * e1).sqrt();
        if denom > 1.0e-12 {
            let r = num / denom;
            if r > best {
                best = r;
            }
        }
    }
    Some(best.clamp(0.0, 1.0))
}

/// Median harmonic-to-noise ratio (dB) over voiced frames, via the Boersma
/// autocorrelation method: `HNR = 10·log10(r / (1 − r))` where `r` is the
/// normalized autocorrelation peak at the pitch lag. Higher = more periodic
/// (cleaner harmonics). `None` if nothing is voiced.
pub fn hnr_db(signal: &[f32], sr: f32) -> Option<f32> {
    let hop = ((HOP_S * sr) as usize).max(1);
    let win = ((WIN_S * sr) as usize).max(hop * 2);
    let lag_lo = (sr / F_MAX) as usize;
    let lag_hi = (sr / F_MIN) as usize;
    let mut hnrs = Vec::new();
    let mut start = 0;
    while start + win <= signal.len() {
        if let Some(c) = frame_clarity(&signal[start..start + win], lag_lo, lag_hi) {
            if c >= VOICED_CLARITY {
                let c = c.min(0.999_999);
                hnrs.push(10.0 * (c / (1.0 - c)).log10());
            }
        }
        start += hop;
    }
    if hnrs.is_empty() {
        return None;
    }
    hnrs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(hnrs[hnrs.len() / 2])
}

/// Cosine similarity (`[0,1]`) of the average log-magnitude spectra of two
/// signals — a timbre/formant-preservation proxy (plan §6.4: spectral-envelope
/// cosine ≥ 0.95). `1.0` = identical spectral shape; drops flag musical-noise
/// holes or harmonic shearing. Uses 2048-pt Hann-windowed frames.
pub fn spectral_envelope_cosine(reference: &[f32], processed: &[f32], _sr: f32) -> f32 {
    let a = avg_log_spectrum(reference);
    let b = avg_log_spectrum(processed);
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na <= 1.0e-12 || nb <= 1.0e-12 {
        return 0.0;
    }
    (dot / (na * nb)).clamp(0.0, 1.0)
}

/// Average log-magnitude spectrum (DC-removed, mean-subtracted) over Hann frames.
fn avg_log_spectrum(signal: &[f32]) -> Vec<f32> {
    use rustfft::{num_complex::Complex, FftPlanner};
    const N: usize = 2048;
    if signal.len() < N {
        return Vec::new();
    }
    let hop = N / 2;
    let hann: Vec<f32> = (0..N)
        .map(|i| {
            let w = (std::f32::consts::PI * i as f32 / (N - 1) as f32).sin();
            w * w
        })
        .collect();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let mut acc = vec![0.0f32; N / 2 + 1];
    let mut frames = 0u32;
    let mut start = 0;
    while start + N <= signal.len() {
        let mut buf: Vec<Complex<f32>> = (0..N)
            .map(|i| Complex::new(signal[start + i] * hann[i], 0.0))
            .collect();
        fft.process(&mut buf);
        for (a, c) in acc.iter_mut().zip(buf.iter().take(N / 2 + 1)) {
            *a += (c.norm() + 1.0e-9).ln();
        }
        frames += 1;
        start += hop;
    }
    if frames == 0 {
        return Vec::new();
    }
    for a in &mut acc {
        *a /= frames as f32;
    }
    // Remove the mean so cosine compares spectral *shape*, not overall level.
    let mean = acc.iter().sum::<f32>() / acc.len() as f32;
    for a in &mut acc {
        *a -= mean;
    }
    acc
}

/// Tail-survival ratio in dB: energy the processed signal kept in the reference's
/// decay region (samples whose smoothed envelope sits −60…−20 dB below the
/// reference peak) versus the reference's energy there (plan §6.2). `~0 dB` =
/// the sustain/tail survived; strongly negative = the tail was truncated.
pub fn tail_survival_db(reference: &[f32], processed: &[f32], sr: f32) -> f32 {
    let n = reference.len().min(processed.len());
    if n == 0 {
        return 0.0;
    }
    // Smoothed energy envelope of the reference (50 ms one-pole on |x|).
    let coeff = (-1.0 / (0.050 * sr)).exp();
    let mut env = 0.0f32;
    let mut envelope = vec![0.0f32; n];
    for i in 0..n {
        env = coeff * env + (1.0 - coeff) * reference[i].abs();
        envelope[i] = env;
    }
    // The tail is the decay *after* the envelope peak — exclude the attack ramp,
    // which also passes through the low-level band but isn't a tail.
    let mut peak = 1.0e-9f32;
    let mut peak_idx = 0usize;
    for (i, &e) in envelope.iter().enumerate() {
        if e > peak {
            peak = e;
            peak_idx = i;
        }
    }
    let lo = peak * 10f32.powf(-60.0 / 20.0);
    let hi = peak * 10f32.powf(-20.0 / 20.0);
    let mut e_ref = 0.0f64;
    let mut e_proc = 0.0f64;
    for i in (peak_idx + 1)..n {
        if envelope[i] >= lo && envelope[i] <= hi {
            e_ref += (reference[i] * reference[i]) as f64;
            e_proc += (processed[i] * processed[i]) as f64;
        }
    }
    if e_ref <= 1.0e-12 {
        return 0.0;
    }
    10.0 * (e_proc / e_ref).max(1.0e-12).log10() as f32
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

    /// A sung vowel with sinusoidal vibrato: F0 swings ±`depth_cents` at
    /// `rate_hz`. Phase is accumulated so the harmonics track the vibrato.
    fn vibrato_note(f0_center: f32, rate_hz: f32, depth_cents: f32, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n);
        let mut phase = 0.0f32;
        for i in 0..n {
            let t = i as f32 / SR;
            let f0 =
                f0_center * 2.0_f32.powf((depth_cents / 1200.0) * (2.0 * PI * rate_hz * t).sin());
            phase += 2.0 * PI * f0 / SR;
            out.push(0.5 * phase.sin() + 0.3 * (2.0 * phase).sin() + 0.18 * (3.0 * phase).sin());
        }
        out
    }

    #[test]
    fn vibrato_recovers_known_rate_and_depth() {
        let x = vibrato_note(220.0, 5.5, 60.0, 96_000); // 2 s
        let v = vibrato(&x, SR).expect("vibrato present");
        assert!(
            (v.rate_hz - 5.5).abs() < 0.7,
            "rate {} should be ~5.5 Hz",
            v.rate_hz
        );
        assert!(
            (v.depth_cents - 60.0).abs() < 15.0,
            "depth {} should be ~60 cents",
            v.depth_cents
        );
    }

    #[test]
    fn steady_tone_has_negligible_vibrato_depth() {
        let x = vowel(220.0, 96_000);
        let v = vibrato(&x, SR).expect("voiced");
        assert!(
            v.depth_cents < 10.0,
            "a steady tone should read shallow, got {} cents",
            v.depth_cents
        );
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

    /// Deterministic broadband noise in [-amp, amp).
    fn noise(amp: f32, n: usize) -> Vec<f32> {
        let mut s: u32 = 0x9e37_79b9;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                amp * (((s >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0)
            })
            .collect()
    }

    #[test]
    fn hnr_is_high_for_a_clean_vowel_and_drops_with_noise() {
        let clean = vowel(220.0, 48_000);
        let mut noisy = clean.clone();
        for (s, ns) in noisy.iter_mut().zip(noise(0.15, 48_000)) {
            *s += ns;
        }
        let h_clean = hnr_db(&clean, SR).expect("voiced");
        let h_noisy = hnr_db(&noisy, SR).expect("voiced");
        assert!(h_clean > 15.0, "clean vowel HNR {h_clean} should be high");
        assert!(
            h_noisy < h_clean - 3.0,
            "noise should drop HNR: {h_clean} -> {h_noisy}"
        );
    }

    #[test]
    fn spectral_cosine_is_one_for_identical_and_drops_for_different_timbre() {
        let a = vowel(196.0, 48_000);
        let same = spectral_envelope_cosine(&a, &a, SR);
        assert!(same > 0.999, "identical timbre should be ~1.0, got {same}");
        // A very different spectral shape (a single high partial) → low cosine.
        let b: Vec<f32> = (0..48_000)
            .map(|i| 0.5 * (2.0 * PI * 6000.0 * i as f32 / SR).sin())
            .collect();
        let diff = spectral_envelope_cosine(&a, &b, SR);
        assert!(diff < 0.5, "different timbre should be low, got {diff}");
    }

    /// A sustained vowel that decays to a tail over its last third.
    fn note_with_tail(f0: f32, n: usize) -> Vec<f32> {
        let mut x = vowel(f0, n);
        let decay_start = n * 2 / 3;
        for (i, s) in x.iter_mut().enumerate().skip(decay_start) {
            let t = (i - decay_start) as f32 / SR;
            *s *= (-t / 0.05).exp(); // ~50 ms time constant: decays through -60 dB
        }
        x
    }

    #[test]
    fn tail_survives_when_unchanged_and_reads_negative_when_truncated() {
        let reference = note_with_tail(220.0, 48_000);
        // Identity: the decay region energy matches → ~0 dB.
        let keep = tail_survival_db(&reference, &reference, SR);
        assert!(
            keep.abs() < 0.5,
            "unchanged tail should be ~0 dB, got {keep}"
        );
        // Truncate: zero the last third → the tail energy collapses.
        let mut truncated = reference.clone();
        for s in truncated.iter_mut().skip(48_000 * 2 / 3) {
            *s = 0.0;
        }
        let cut = tail_survival_db(&reference, &truncated, SR);
        assert!(
            cut < -6.0,
            "a truncated tail should read well negative, got {cut}"
        );
    }
}
