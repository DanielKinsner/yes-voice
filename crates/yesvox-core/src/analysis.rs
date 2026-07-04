//! Whole-file analysis pass + reject paths (plan §5.1).
//!
//! On open, YesVox runs one analysis pass and emits an [`AnalysisReport`] the
//! rest of the app reads instead of re-measuring. Unusable inputs are rejected
//! up front with a specific, actionable message (silence, mostly-clipped, or a
//! header the decoder can't read). The report carries everything the presets,
//! the difference monitor, and de-bleed need: loudness, true peak, noise floor,
//! clip count, click rate, the dominant hum + harmonic count, an RT60 estimate,
//! a voiceover-vs-singing guess with confidence, the channel mode, and the
//! residual non-vocal energy fraction.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::dsp::{goertzel_power, Biquad};
use crate::io::{read_audio, IoError};
use crate::loudness::{integrated_lufs, true_peak_dbtp};
use crate::settings::ContentMode;
use std::path::Path;

/// Mains-hum estimate: the detected fundamental, how many harmonics are present,
/// and the fundamental's strength over the broadband level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HumEstimate {
    /// Detected fundamental in Hz (≈50 or ≈60); `0.0` if no hum found.
    pub frequency: f32,
    /// Count of harmonics (including the fundamental) found above the floor.
    pub harmonic_count: u32,
    /// Fundamental strength relative to the broadband RMS, in dB.
    pub strength_db: f32,
}

/// Voiceover-vs-singing guess with a 0–1 confidence (plan §5.2). The mode is a
/// *guess*; the user's explicit toggle is authoritative.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentGuess {
    /// The guessed content mode.
    pub mode: ContentMode,
    /// Confidence in `[0, 1]`.
    pub confidence: f32,
}

/// The whole-file analysis payload (plan §1.1 `ProcessReport`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnalysisReport {
    /// Mono or dual-mono.
    pub channel_mode: ChannelLayout,
    /// Integrated loudness, LUFS (BS.1770-4).
    pub integrated_lufs: f32,
    /// True peak, dBTP.
    pub true_peak_dbtp: f32,
    /// Noise floor estimate, dBFS (10th-percentile short-time RMS).
    pub noise_floor_db: f32,
    /// Count of samples judged clipped (flat-topped near full scale).
    pub clip_sample_count: u64,
    /// Clipped fraction of all samples, `[0, 1]`.
    pub clip_fraction: f32,
    /// Impulsive-defect rate, clicks per second.
    pub click_rate_hz: f32,
    /// Mains-hum estimate.
    pub hum: HumEstimate,
    /// Reverberation-time estimate (RT60-like), seconds.
    pub reverb_time_s: f32,
    /// Content-type guess with confidence.
    pub content: ContentGuess,
    /// Residual non-vocal energy fraction `[0, 1]` (used later by de-bleed #15).
    pub residual_nonvocal: f32,
    /// Whether selecting the batch (HQ) engine will engage its internal 48 kHz
    /// resample for this input — true whenever the source is not already 48 kHz
    /// (plan §2.2; #10). The UI surfaces this so the internal rate conversion is
    /// transparent; export still matches the source rate.
    pub resampled_for_batch: bool,
    /// Count of non-finite (NaN/±Inf) samples flushed at decode by
    /// [`crate::audio::AudioBuffer::sanitize`] (#28) — a "your file had corrupt
    /// samples, we fixed them" defect the UI can surface. 0 for clean input.
    pub non_finite_samples: u64,
}

/// Why an input was rejected — each variant renders an actionable message.
#[derive(Debug)]
pub enum AnalysisError {
    /// The decoder could not read the file (corrupt/unsupported header).
    Decode(IoError),
    /// The file is effectively silent.
    Silence,
    /// More than half the file is clipped — restoration can't recover it.
    ExcessiveClipping {
        /// Clipped fraction that triggered the reject.
        fraction: f32,
    },
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalysisError::Decode(e) => write!(
                f,
                "couldn't read this file ({e}). Re-export it as a standard WAV or FLAC and try again."
            ),
            AnalysisError::Silence => write!(
                f,
                "this file is silent — there's no vocal to restore. Check you exported the right track."
            ),
            AnalysisError::ExcessiveClipping { fraction } => write!(
                f,
                "{:.0}% of this file is clipped — that's past what declip can repair. Re-record or re-export with more headroom.",
                fraction * 100.0
            ),
        }
    }
}

impl std::error::Error for AnalysisError {}

/// Samples at or above this magnitude are clipping candidates (≈ −0.1 dBFS).
const CLIP_LEVEL: f32 = 0.989;
/// A silent file: nothing exceeds this magnitude (≈ −80 dBFS).
const SILENCE_PEAK: f32 = 1.0e-4;

/// Read a file and analyze it, surfacing a corrupt header as a reject.
pub fn analyze_file(path: &Path) -> Result<AnalysisReport, AnalysisError> {
    let buffer = read_audio(path).map_err(AnalysisError::Decode)?;
    analyze(&buffer)
}

/// Analyze an already-decoded buffer (the whole-file pass).
pub fn analyze(buffer: &AudioBuffer) -> Result<AnalysisReport, AnalysisError> {
    let sr = buffer.sample_rate() as f32;

    // --- Reject paths first (cheap, decisive) -----------------------------
    let peak = buffer
        .channels()
        .iter()
        .flat_map(|c| c.iter())
        .fold(0.0f32, |m, &v| m.max(v.abs()));
    if peak < SILENCE_PEAK {
        return Err(AnalysisError::Silence);
    }

    let (clip_count, total) = count_clipped(buffer);
    let clip_fraction = if total > 0 {
        clip_count as f32 / total as f32
    } else {
        0.0
    };
    if clip_fraction > 0.5 {
        return Err(AnalysisError::ExcessiveClipping {
            fraction: clip_fraction,
        });
    }

    // --- Measurements ------------------------------------------------------
    let integrated = integrated_lufs(buffer).unwrap_or(-70.0);
    let tp = true_peak_dbtp(buffer);
    let mono = downmix(buffer);
    let noise_floor_db = noise_floor_db(&mono, sr);
    let click_rate_hz = click_rate(&mono, sr);
    let hum = detect_hum(&mono, sr);
    let reverb_time_s = reverb_time(&mono, sr);
    let content = guess_content(&mono, sr);
    let residual_nonvocal = residual_nonvocal(&mono, sr);

    Ok(AnalysisReport {
        channel_mode: buffer.layout(),
        integrated_lufs: integrated,
        true_peak_dbtp: tp,
        noise_floor_db,
        clip_sample_count: clip_count,
        clip_fraction,
        click_rate_hz,
        hum,
        reverb_time_s,
        content,
        residual_nonvocal,
        // The 48 kHz-only batch engine resamples any non-48 kHz source.
        resampled_for_batch: buffer.sample_rate() != 48_000,
        // Decode-time numeric-hygiene defect count (#28).
        non_finite_samples: buffer.non_finite_samples(),
    })
}

/// Count flat-topped near-full-scale samples (true clipping), not just loud
/// peaks: a sample counts only inside a run of ≥3 samples that barely move.
fn count_clipped(buffer: &AudioBuffer) -> (u64, u64) {
    let mut clipped = 0u64;
    let mut total = 0u64;
    for ch in buffer.channels() {
        total += ch.len() as u64;
        let mut run = 0usize;
        for w in ch.windows(2) {
            let a = w[0].abs();
            let flat = (w[0] - w[1]).abs() < 1.0e-3;
            if a >= CLIP_LEVEL && flat {
                run += 1;
            } else {
                if run >= 2 {
                    clipped += (run + 1) as u64; // the run plus its first sample
                }
                run = 0;
            }
        }
        if run >= 2 {
            clipped += (run + 1) as u64;
        }
    }
    (clipped, total)
}

/// Sum channels to a working mono lane for the single-channel measurements.
fn downmix(buffer: &AudioBuffer) -> Vec<f32> {
    let n = buffer.frames();
    let nch = buffer.channel_count() as f32;
    let mut out = vec![0.0f32; n];
    for ch in buffer.channels() {
        for (o, &s) in out.iter_mut().zip(ch.iter()) {
            *o += s / nch;
        }
    }
    out
}

/// Short-time RMS in dBFS at the 10th percentile — a robust noise-floor proxy.
fn noise_floor_db(mono: &[f32], sr: f32) -> f32 {
    let win = ((0.020 * sr) as usize).max(1);
    let mut rms: Vec<f32> = mono
        .chunks(win)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    if rms.is_empty() {
        return -120.0;
    }
    rms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p10 = rms[(rms.len() as f32 * 0.10) as usize];
    20.0 * p10.max(1.0e-9).log10()
}

/// Clicks per second from an impulsive-residual detector: a sample is a click
/// when its high-passed magnitude spikes far above the local median.
fn click_rate(mono: &[f32], sr: f32) -> f32 {
    if mono.len() < 4 {
        return 0.0;
    }
    // Simple HF emphasis (first difference) exposes impulsive energy.
    let mut hp = vec![0.0f32; mono.len()];
    for i in 1..mono.len() {
        hp[i] = mono[i] - mono[i - 1];
    }
    let med = median_abs(&hp).max(1.0e-6);
    let thresh = (med * 12.0).max(0.05);
    let mut count = 0u64;
    let mut i = 1;
    while i < hp.len() {
        if hp[i].abs() > thresh {
            count += 1;
            i += (sr * 0.002) as usize + 1; // refractory ~2 ms, one event per click
        } else {
            i += 1;
        }
    }
    let secs = mono.len() as f32 / sr;
    count as f32 / secs.max(1.0e-3)
}

/// Median of the absolute values (used for a robust threshold).
fn median_abs(xs: &[f32]) -> f32 {
    let mut v: Vec<f32> = xs.iter().map(|x| x.abs()).collect();
    let mid = v.len() / 2;
    if v.is_empty() {
        return 0.0;
    }
    let (_, median, _) = v.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
    *median
}

/// Detect the dominant mains hum (50 vs 60 Hz) and count its harmonics.
fn detect_hum(mono: &[f32], sr: f32) -> HumEstimate {
    let broadband = (mono.iter().map(|x| x * x).sum::<f32>() / mono.len().max(1) as f32).sqrt();
    let broadband = broadband.max(1.0e-9);

    let candidates = [50.0f32, 60.0];
    let mut best = HumEstimate {
        frequency: 0.0,
        harmonic_count: 0,
        strength_db: -120.0,
    };
    for &f0 in &candidates {
        // Fundamental strength (Goertzel power → amplitude).
        let amp = goertzel_power(mono, sr, f0).sqrt();
        let strength_db = 20.0 * (amp / broadband).max(1.0e-9).log10();
        // Count harmonics standing clearly above the broadband floor.
        let mut harmonics = 0u32;
        for h in 1..=8 {
            let f = f0 * h as f32;
            if f >= sr / 2.0 {
                break;
            }
            let a = goertzel_power(mono, sr, f).sqrt();
            if 20.0 * (a / broadband).max(1.0e-9).log10() > -30.0 {
                harmonics += 1;
            }
        }
        if strength_db > best.strength_db {
            best = HumEstimate {
                frequency: f0,
                harmonic_count: harmonics,
                strength_db,
            };
        }
    }
    // Below a clear floor, call it "no hum".
    if best.strength_db < -40.0 {
        best.frequency = 0.0;
        best.harmonic_count = 0;
    }
    best
}

/// RT60-like estimate from the median exponential decay of the smoothed energy
/// envelope after local peaks. Dry signals read low; reverberant ones read high.
fn reverb_time(mono: &[f32], sr: f32) -> f32 {
    let hop = ((0.010 * sr) as usize).max(1);
    let env: Vec<f32> = mono
        .chunks(hop)
        .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    if env.len() < 8 {
        return 0.0;
    }
    let env_db: Vec<f32> = env.iter().map(|&e| 20.0 * e.max(1.0e-9).log10()).collect();
    let frame_s = hop as f32 / sr;

    // Collect decay slopes (dB/s) over falling stretches that start near a peak.
    let mut slopes: Vec<f32> = Vec::new();
    let mut i = 1;
    while i + 1 < env_db.len() {
        let rising = env_db[i] > env_db[i - 1];
        let local_peak = rising && env_db[i] >= env_db[i + 1];
        if local_peak && env_db[i] > -50.0 {
            // Measure the fall from this peak until it stops dropping.
            let start = i;
            let mut j = i + 1;
            while j < env_db.len() && env_db[j] < env_db[j - 1] {
                j += 1;
            }
            let drop = env_db[start] - env_db[j - 1];
            let dt = (j - 1 - start) as f32 * frame_s;
            if drop > 6.0 && dt > 0.0 {
                slopes.push(drop / dt); // dB per second
            }
            i = j;
        } else {
            i += 1;
        }
    }
    if slopes.is_empty() {
        return 0.0;
    }
    slopes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_slope = slopes[slopes.len() / 2].max(1.0);
    (60.0 / median_slope).clamp(0.0, 3.0)
}

/// Decimate to ~6 kHz with an anti-alias low-pass — pitch is sub-1 kHz, so this
/// keeps the autocorrelation search cheap enough for the ≤5 s budget.
fn decimate_for_pitch(mono: &[f32], sr: f32) -> (Vec<f32>, f32) {
    let factor = (sr / 6000.0).floor().max(1.0) as usize;
    if factor <= 1 {
        return (mono.to_vec(), sr);
    }
    let mut lp = Biquad::low_pass(sr, sr / (2.5 * factor as f32), 0.707);
    let filtered: Vec<f32> = mono.iter().map(|&x| lp.process(x)).collect();
    let out: Vec<f32> = filtered.iter().step_by(factor).copied().collect();
    (out, sr / factor as f32)
}

/// Heuristic voiceover-vs-singing guess. Singing tends to hold stable pitch over
/// long sustained notes; speech is more broken up. Confidence in `[0, 1]`.
fn guess_content(mono_full: &[f32], sr_full: f32) -> ContentGuess {
    let (mono, sr) = decimate_for_pitch(mono_full, sr_full);
    let mono = mono.as_slice();
    let hop = ((0.020 * sr) as usize).max(1);
    let win = hop * 2;
    let lag_lo = (sr / 1000.0) as usize; // ~1000 Hz
    let lag_hi = (sr / 70.0) as usize; // ~70 Hz
    let mut voiced = 0u32;
    let mut sustained = 0u32;
    let mut frames = 0u32;
    let mut prev_lag: Option<usize> = None;

    let mut start = 0;
    while start + win <= mono.len() {
        let seg = &mono[start..start + win];
        let energy = seg.iter().map(|x| x * x).sum::<f32>();
        if energy > 1.0e-5 {
            if let Some((lag, clarity)) = best_lag(seg, lag_lo, lag_hi) {
                if clarity > 0.6 {
                    voiced += 1;
                    if let Some(p) = prev_lag {
                        // Within ~3% pitch → a held note.
                        if (lag as f32 - p as f32).abs() / p as f32 <= 0.03 {
                            sustained += 1;
                        }
                    }
                    prev_lag = Some(lag);
                } else {
                    prev_lag = None;
                }
            }
        }
        frames += 1;
        start += hop;
    }

    let voiced_ratio = voiced as f32 / frames.max(1) as f32;
    let sustain_ratio = sustained as f32 / voiced.max(1) as f32;
    // Singing score rises with how much of the voiced material is sustained.
    let singing_score = sustain_ratio * voiced_ratio.sqrt();
    if singing_score > 0.35 {
        ContentGuess {
            mode: ContentMode::Singing,
            confidence: (singing_score).min(1.0),
        }
    } else {
        ContentGuess {
            mode: ContentMode::Voiceover,
            confidence: (1.0 - singing_score).clamp(0.0, 1.0),
        }
    }
}

/// Best autocorrelation lag in `[lo, hi]` and a 0–1 clarity (peak/energy).
fn best_lag(seg: &[f32], lo: usize, hi: usize) -> Option<(usize, f32)> {
    let energy: f32 = seg.iter().map(|x| x * x).sum();
    if energy <= 0.0 || hi <= lo || hi >= seg.len() {
        return None;
    }
    let mut best = (0usize, f32::MIN);
    for lag in lo..=hi {
        let mut acc = 0.0f32;
        for i in lag..seg.len() {
            acc += seg[i] * seg[i - lag];
        }
        if acc > best.1 {
            best = (lag, acc);
        }
    }
    Some((best.0, (best.1 / energy).clamp(0.0, 1.0)))
}

/// Fraction of energy outside the core vocal band (~80 Hz–8 kHz) — a rough
/// "non-vocal residual" proxy that de-bleed (#15) refines.
fn residual_nonvocal(mono: &[f32], sr: f32) -> f32 {
    let total: f32 = mono.iter().map(|x| x * x).sum::<f32>().max(1.0e-12);
    // Band-limit a copy to the vocal band and compare residual energy.
    let mut hp = Biquad::high_pass(sr, 80.0, 0.707);
    let mut lp = Biquad::low_pass(sr, 8000.0, 0.707);
    let band: f32 = mono
        .iter()
        .map(|&x| {
            let y = lp.process(hp.process(x));
            y * y
        })
        .sum();
    (1.0 - (band / total)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use std::f32::consts::PI;

    fn buf(ch: Vec<f32>, sr: u32) -> AudioBuffer {
        AudioBuffer::new(sr, 16, SampleEncoding::Int, vec![ch])
    }

    fn sine(amp: f32, freq: f32, sr: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / sr as f32).sin())
            .collect()
    }

    #[test]
    fn silence_is_rejected() {
        let b = buf(vec![0.0; 48_000], 48_000);
        assert!(matches!(analyze(&b), Err(AnalysisError::Silence)));
    }

    #[test]
    fn mostly_clipped_is_rejected() {
        // A loud square wave: almost every sample sits flat at ±full scale.
        let mut ch = sine(1.0, 200.0, 48_000, 48_000);
        for s in &mut ch {
            *s = if *s >= 0.0 { 1.0 } else { -1.0 };
        }
        match analyze(&buf(ch, 48_000)) {
            Err(AnalysisError::ExcessiveClipping { fraction }) => {
                assert!(fraction > 0.5, "fraction {fraction}");
            }
            other => panic!("expected clipping reject, got {other:?}"),
        }
    }

    #[test]
    fn clean_tone_reports_low_clip_and_emits_full_report() {
        let r = analyze(&buf(sine(0.5, 220.0, 48_000, 48_000), 48_000)).expect("report");
        assert_eq!(r.channel_mode, ChannelLayout::Mono);
        assert!(r.clip_fraction < 0.01, "clean tone shouldn't clip");
        assert!(r.true_peak_dbtp < 0.5);
        assert!(r.content.confidence >= 0.0 && r.content.confidence <= 1.0);
        assert!(r.residual_nonvocal >= 0.0 && r.residual_nonvocal <= 1.0);
    }

    #[test]
    fn batch_resample_flag_tracks_sample_rate() {
        // 48 kHz source: the batch engine needs no internal resample.
        let r48 = analyze(&buf(sine(0.5, 220.0, 48_000, 48_000), 48_000)).expect("report");
        assert!(!r48.resampled_for_batch, "48 kHz needs no resample");
        // Any other rate flags the internal 48 kHz conversion.
        let r44 = analyze(&buf(sine(0.5, 220.0, 44_100, 44_100), 44_100)).expect("report");
        assert!(r44.resampled_for_batch, "44.1 kHz flags batch resample");
    }

    #[test]
    fn hum_is_detected_at_sixty_hz() {
        let mut ch = sine(0.3, 220.0, 48_000, 48_000);
        for (i, s) in ch.iter_mut().enumerate() {
            *s += 0.2 * (2.0 * PI * 60.0 * i as f32 / 48_000.0).sin();
            *s += 0.08 * (2.0 * PI * 120.0 * i as f32 / 48_000.0).sin();
        }
        let r = analyze(&buf(ch, 48_000)).expect("report");
        assert_eq!(r.hum.frequency, 60.0, "should pick 60 Hz");
        assert!(r.hum.harmonic_count >= 2, "fundamental + a harmonic");
    }

    #[test]
    fn no_hum_on_clean_tone() {
        let r = analyze(&buf(sine(0.5, 440.0, 48_000, 48_000), 48_000)).expect("report");
        assert_eq!(r.hum.frequency, 0.0, "clean tone has no mains hum");
    }

    #[test]
    fn reverberant_reads_longer_than_dry() {
        // Dry: a few decaying impulses. Reverberant: the same with a long tail.
        let n = 48_000;
        let mut dry = vec![0.0f32; n];
        for k in 0..6 {
            dry[k * 7000] = 0.9;
        }
        let mut wet = dry.clone();
        // Add an exponential tail after each impulse.
        for k in 0..6 {
            let start = k * 7000;
            for t in 0..6000 {
                if start + t < n {
                    wet[start + t] += 0.9 * (-(t as f32) / 4000.0).exp();
                }
            }
        }
        let dry_rt = reverb_time(&dry, 48_000.0);
        let wet_rt = reverb_time(&wet, 48_000.0);
        assert!(
            wet_rt > dry_rt,
            "reverberant {wet_rt} should exceed dry {dry_rt}"
        );
    }

    #[test]
    fn clicks_raise_the_click_rate() {
        let n = 48_000;
        let mut clean = sine(0.3, 300.0, 48_000, n);
        let clean_rate = click_rate(&clean, 48_000.0);
        // Inject 10 clicks.
        for k in 0..10 {
            clean[k * 4000 + 100] += 0.9;
        }
        let clicky_rate = click_rate(&clean, 48_000.0);
        assert!(
            clicky_rate > clean_rate + 5.0,
            "clicks should raise the rate: {clean_rate} -> {clicky_rate}"
        );
    }
}
